#![allow(dead_code)]

use async_trait::async_trait;
#[cfg(target_os = "linux")]
use tracing::debug;

use crate::config::CanbusConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

/// CAN bus driver using socketcan (Linux only).
/// Each CAN frame is serialised as: [ID:4 bytes LE][DLC:1 byte][data:0-8 bytes]
pub struct CanbusDriver {
    cfg: CanbusConfig,
    #[cfg(target_os = "linux")]
    frames: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
}

impl CanbusDriver {
    pub fn new(cfg: CanbusConfig) -> Self {
        CanbusDriver {
            cfg,
            #[cfg(target_os = "linux")]
            frames: None,
        }
    }
}

#[async_trait]
impl IoDriver for CanbusDriver {
    async fn open(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use socketcan::CanSocket;
            use socketcan::Socket;

            let iface = self.cfg.interface.clone();
            let filter = self.cfg.filter;
            let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);

            std::thread::spawn(move || {
                let socket = match CanSocket::open(&iface) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, iface = %iface, "Cannot open CAN socket");
                        return;
                    }
                };

                loop {
                    match socket.read_frame() {
                        Ok(frame) => {
                            use socketcan::Frame;
                            use socketcan::EmbeddedFrame;
                            // Apply optional ID filter
                            if let Some(mask) = filter {
                                if frame.raw_id() & mask != frame.raw_id() & mask {
                                    continue;
                                }
                            }
                            // Serialize frame: [raw_id:4][dlc:1][data:dlc]
                            let mut buf = Vec::with_capacity(5 + frame.data().len());
                            buf.extend_from_slice(&frame.raw_id().to_le_bytes());
                            buf.push(frame.data().len() as u8);
                            buf.extend_from_slice(frame.data());
                            if tx.blocking_send(buf).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });

            self.frames = Some(rx);
            debug!(iface = %self.cfg.interface, "CAN bus opened");
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("CAN bus requires Linux".to_string()))
    }

    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            match &mut self.frames {
                None => Err(IoToNetError::Driver("CAN not opened".to_string())),
                Some(rx) => match tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    rx.recv(),
                )
                .await
                {
                    Ok(Some(frame)) => {
                        let len = frame.len().min(_buf.len());
                        _buf[..len].copy_from_slice(&frame[..len]);
                        Ok(len)
                    }
                    Ok(None) => Err(IoToNetError::Driver("CAN channel closed".to_string())),
                    Err(_) => Ok(0),
                },
            }
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("CAN bus requires Linux".to_string()))
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        Err(IoToNetError::Driver("CAN write not implemented".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.frames = None;
        }
        Ok(())
    }

    fn description(&self) -> String {
        format!("canbus://{}", self.cfg.interface)
    }
}
