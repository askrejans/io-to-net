//! SocketCAN source (Linux). Frames are encoded per `format`; writes in the
//! same format are transmitted.
// The codec is portable (and tested everywhere); only Linux has SocketCAN.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::config::{CanFormat, CanbusConfig};

/// A received or to-be-sent frame, independent of SocketCAN types (testable anywhere).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: u32,
    pub extended: bool,
    pub data: Vec<u8>,
}

/// `id & mask == filter_id & mask` (mask defaults to all bits of the ID).
pub fn accepted(cfg: &CanbusConfig, id: u32) -> bool {
    match cfg.filter_id {
        None => true,
        Some(want) => {
            let mask = cfg.filter_mask.unwrap_or(0x1FFF_FFFF);
            id & mask == want & mask
        }
    }
}

pub fn encode(format: CanFormat, f: &Frame, timestamp_ms: u64) -> Vec<u8> {
    match format {
        CanFormat::Json => {
            let mut s = serde_json::json!({
                "id": f.id, "extended": f.extended, "timestampMs": timestamp_ms, "data": f.data,
            })
            .to_string()
            .into_bytes();
            s.push(b'\n');
            s
        }
        CanFormat::Slcan => {
            let mut s = if f.extended {
                format!("T{:08X}", f.id)
            } else {
                format!("t{:03X}", f.id)
            };
            s.push_str(&format!("{}", f.data.len()));
            for b in &f.data {
                s.push_str(&format!("{b:02X}"));
            }
            s.push('\r');
            s.into_bytes()
        }
        CanFormat::Binary => {
            let mut b = Vec::with_capacity(5 + f.data.len());
            b.extend_from_slice(&f.id.to_le_bytes());
            b.push(f.data.len() as u8);
            b.extend_from_slice(&f.data);
            b
        }
    }
}

/// Parse frames written by a client. Incomplete trailing input stays in `pending`.
pub fn decode(format: CanFormat, pending: &mut Vec<u8>) -> Vec<Frame> {
    let mut out = Vec::new();
    match format {
        CanFormat::Json | CanFormat::Slcan => {
            let term = if format == CanFormat::Json {
                b'\n'
            } else {
                b'\r'
            };
            while let Some(pos) = pending.iter().position(|&b| b == term || b == b'\n') {
                let line: Vec<u8> = pending.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line).trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let frame = if format == CanFormat::Json {
                    parse_json(&line)
                } else {
                    parse_slcan(&line)
                };
                if let Some(f) = frame {
                    out.push(f);
                }
            }
        }
        CanFormat::Binary => {
            while pending.len() >= 5 {
                let len = pending[4] as usize;
                if len > 8 {
                    pending.clear();
                    break;
                }
                if pending.len() < 5 + len {
                    break;
                }
                let id = u32::from_le_bytes([pending[0], pending[1], pending[2], pending[3]]);
                let data = pending[5..5 + len].to_vec();
                pending.drain(..5 + len);
                out.push(Frame {
                    id,
                    extended: id > 0x7FF,
                    data,
                });
            }
        }
    }
    out
}

fn parse_json(line: &str) -> Option<Frame> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = v.get("id")?.as_u64()? as u32;
    let data: Vec<u8> = v
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|b| b.as_u64().map(|b| b as u8))
        .collect();
    let extended = v
        .get("extended")
        .and_then(|e| e.as_bool())
        .unwrap_or(id > 0x7FF);
    (data.len() <= 8).then_some(Frame { id, extended, data })
}

fn parse_slcan(line: &str) -> Option<Frame> {
    let (extended, id_len) = match line.as_bytes().first()? {
        b't' => (false, 3),
        b'T' => (true, 8),
        _ => return None,
    };
    let id = u32::from_str_radix(line.get(1..1 + id_len)?, 16).ok()?;
    let len = line.get(1 + id_len..2 + id_len)?.parse::<usize>().ok()?;
    let hex = line.get(2 + id_len..2 + id_len + len * 2)?;
    let data = (0..len)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (len <= 8).then_some(Frame { id, extended, data })
}

#[cfg(target_os = "linux")]
pub use linux::CanbusDriver;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::drivers::IoDriver;
    use crate::error::{IoToNetError, Result};
    use socketcan::{
        CanFrame, CanSocket, EmbeddedFrame, ExtendedId, Frame as _, Socket, StandardId,
    };
    use std::sync::Arc;

    pub struct CanbusDriver {
        cfg: CanbusConfig,
        socket: Option<Arc<CanSocket>>,
        frames: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
        pending: Vec<u8>,
    }

    impl CanbusDriver {
        pub fn new(cfg: CanbusConfig) -> Self {
            CanbusDriver {
                cfg,
                socket: None,
                frames: None,
                pending: Vec::new(),
            }
        }
    }

    impl IoDriver for CanbusDriver {
        async fn open(&mut self) -> Result<()> {
            let socket =
                Arc::new(CanSocket::open(&self.cfg.interface).map_err(|e| {
                    IoToNetError::Driver(format!("CAN {}: {e}", self.cfg.interface))
                })?);
            socket
                .set_read_timeout(std::time::Duration::from_millis(100))
                .map_err(|e| IoToNetError::Driver(format!("CAN timeout: {e}")))?;
            let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
            let cfg = self.cfg.clone();
            let reader = socket.clone();
            std::thread::spawn(move || {
                while !tx.is_closed() {
                    match reader.read_frame() {
                        Ok(frame) => {
                            if frame.is_error_frame() || frame.is_remote_frame() {
                                continue;
                            }
                            let id = frame.raw_id();
                            if !accepted(&cfg, id) {
                                continue;
                            }
                            let f = Frame {
                                id,
                                extended: frame.is_extended(),
                                data: frame.data().to_vec(),
                            };
                            let now = chrono::Utc::now().timestamp_millis() as u64;
                            if tx.blocking_send(encode(cfg.format, &f, now)).is_err() {
                                break;
                            }
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(_) => break,
                    }
                }
            });
            self.socket = Some(socket);
            self.frames = Some(rx);
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let rx = self
                .frames
                .as_mut()
                .ok_or_else(|| IoToNetError::Driver("CAN not open".into()))?;
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Some(chunk)) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    Ok(n)
                }
                Ok(None) => Err(IoToNetError::Driver(format!(
                    "CAN {} closed",
                    self.cfg.interface
                ))),
                Err(_) => Ok(0),
            }
        }

        async fn write(&mut self, data: &[u8]) -> Result<usize> {
            let socket = self
                .socket
                .clone()
                .ok_or_else(|| IoToNetError::Driver("CAN not open".into()))?;
            self.pending.extend_from_slice(data);
            for f in decode(self.cfg.format, &mut self.pending) {
                let frame = if f.extended {
                    ExtendedId::new(f.id).and_then(|id| CanFrame::new(id, &f.data))
                } else {
                    StandardId::new(f.id as u16).and_then(|id| CanFrame::new(id, &f.data))
                };
                if let Some(frame) = frame {
                    socket
                        .write_frame(&frame)
                        .map_err(|e| IoToNetError::Driver(format!("CAN write: {e}")))?;
                }
            }
            Ok(data.len())
        }

        fn description(&self) -> String {
            format!(
                "canbus://{} ({})",
                self.cfg.interface,
                self.cfg.format.name()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn haltech() -> Frame {
        Frame {
            id: 0x360,
            extended: false,
            data: vec![23, 112, 3, 245, 2, 238, 0, 0],
        }
    }

    #[test]
    fn json_matches_the_ecu_to_mqtt_gateway_format() {
        let line =
            String::from_utf8(encode(CanFormat::Json, &haltech(), 1_750_000_000_000)).unwrap();
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"id": 864, "extended": false, "timestampMs": 1_750_000_000_000u64, "data": [23,112,3,245,2,238,0,0]})
        );
    }

    #[test]
    fn slcan_standard_and_extended() {
        assert_eq!(
            encode(CanFormat::Slcan, &haltech(), 0),
            b"t3608177003F502EE0000\r"
        );
        let ext = Frame {
            id: 0x01F0A000,
            extended: true,
            data: vec![1, 2],
        };
        assert_eq!(encode(CanFormat::Slcan, &ext, 0), b"T01F0A00020102\r");
    }

    #[test]
    fn every_format_round_trips_and_keeps_partial_input() {
        for format in [CanFormat::Json, CanFormat::Slcan, CanFormat::Binary] {
            let bytes = encode(format, &haltech(), 5);
            let (head, tail) = bytes.split_at(bytes.len() / 2);
            let mut pending = head.to_vec();
            assert!(
                decode(format, &mut pending).is_empty(),
                "{format:?}: half a frame decodes to nothing"
            );
            pending.extend_from_slice(tail);
            assert_eq!(decode(format, &mut pending), vec![haltech()], "{format:?}");
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn filter_uses_the_mask() {
        let mut cfg = CanbusConfig::default();
        assert!(accepted(&cfg, 0x123));
        cfg.filter_id = Some(0x360);
        cfg.filter_mask = Some(0x7F0);
        assert!(accepted(&cfg, 0x360) && accepted(&cfg, 0x36F));
        assert!(!accepted(&cfg, 0x370));
        cfg.filter_mask = None;
        assert!(accepted(&cfg, 0x360) && !accepted(&cfg, 0x361));
    }
}
