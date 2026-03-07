#![allow(dead_code)]

use async_trait::async_trait;
#[cfg(target_os = "linux")]
use tracing::debug;

use crate::config::GpioConfig;
#[cfg(target_os = "linux")]
use crate::config::GpioEdge;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

pub struct GpioDriver {
    cfg: GpioConfig,
    #[cfg(target_os = "linux")]
    events: Option<tokio::sync::mpsc::Receiver<u8>>,
}

impl GpioDriver {
    pub fn new(cfg: GpioConfig) -> Self {
        GpioDriver {
            cfg,
            #[cfg(target_os = "linux")]
            events: None,
        }
    }
}

#[async_trait]
impl IoDriver for GpioDriver {
    async fn open(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use gpio_cdev::{Chip, EventRequestFlags, LineRequestFlags};

            let edge_flags = match self.cfg.edge {
                GpioEdge::Rising => EventRequestFlags::RISING_EDGE,
                GpioEdge::Falling => EventRequestFlags::FALLING_EDGE,
                GpioEdge::Both => EventRequestFlags::BOTH_EDGES,
            };

            let mut chip = Chip::new(&self.cfg.chip)
                .map_err(|e| IoToNetError::Driver(format!("GPIO chip {}: {}", self.cfg.chip, e)))?;

            let event_handle = chip
                .get_line(self.cfg.line)
                .map_err(|e| IoToNetError::Driver(format!("GPIO line {}: {}", self.cfg.line, e)))?
                .events(
                    LineRequestFlags::INPUT,
                    edge_flags,
                    "io-to-net",
                )
                .map_err(|e| IoToNetError::Driver(format!("GPIO event setup: {}", e)))?;

            let (tx, rx) = tokio::sync::mpsc::channel::<u8>(512);

            // Blocking event loop on dedicated thread
            std::thread::spawn(move || {
                for event in event_handle {
                    match event {
                        Ok(evt) => {
                            // Encode event type as single byte: 0x01 rising, 0x02 falling
                            let byte: u8 = if evt.event_type() == gpio_cdev::EventType::RisingEdge { 0x01 } else { 0x02 };
                            if tx.blocking_send(byte).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });

            self.events = Some(rx);
            debug!(chip = %self.cfg.chip, line = self.cfg.line, "GPIO opened");
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("GPIO requires Linux".to_string()))
    }

    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            match &mut self.events {
                None => Err(IoToNetError::Driver("GPIO not opened".to_string())),
                Some(rx) => match tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    rx.recv(),
                )
                .await
                {
                    Ok(Some(b)) => { _buf[0] = b; Ok(1) }
                    Ok(None) => Err(IoToNetError::Driver("GPIO channel closed".to_string())),
                    Err(_) => Ok(0),
                },
            }
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("GPIO requires Linux".to_string()))
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        // GPIO output not implemented (event input only)
        Err(IoToNetError::Driver("GPIO write not supported (event input only)".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.events = None;
        }
        Ok(())
    }

    fn description(&self) -> String {
        format!("gpio://{}:line{}", self.cfg.chip, self.cfg.line)
    }
}
