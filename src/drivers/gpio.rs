//! GPIO edge events (Linux character device): one byte per event,
//! `0x01` rising, `0x02` falling.

#[cfg(target_os = "linux")]
pub use linux::GpioDriver;

#[cfg(target_os = "linux")]
mod linux {
    use crate::config::{GpioConfig, GpioEdge};
    use crate::drivers::IoDriver;
    use crate::error::{IoToNetError, Result};
    use gpio_cdev::{Chip, EventRequestFlags, EventType, LineRequestFlags};

    pub struct GpioDriver {
        cfg: GpioConfig,
        events: Option<tokio::sync::mpsc::Receiver<u8>>,
    }

    impl GpioDriver {
        pub fn new(cfg: GpioConfig) -> Self {
            GpioDriver { cfg, events: None }
        }
    }

    impl IoDriver for GpioDriver {
        async fn open(&mut self) -> Result<()> {
            let flags = match self.cfg.edge {
                GpioEdge::Rising => EventRequestFlags::RISING_EDGE,
                GpioEdge::Falling => EventRequestFlags::FALLING_EDGE,
                GpioEdge::Both => EventRequestFlags::BOTH_EDGES,
            };
            let mut chip = Chip::new(&self.cfg.chip)
                .map_err(|e| IoToNetError::Driver(format!("GPIO {}: {e}", self.cfg.chip)))?;
            let events = chip
                .get_line(self.cfg.line)
                .and_then(|l| l.events(LineRequestFlags::INPUT, flags, "io-to-net"))
                .map_err(|e| IoToNetError::Driver(format!("GPIO line {}: {e}", self.cfg.line)))?;
            let (tx, rx) = tokio::sync::mpsc::channel::<u8>(512);
            std::thread::spawn(move || {
                for ev in events {
                    let Ok(ev) = ev else { break };
                    let b = if ev.event_type() == EventType::RisingEdge {
                        0x01
                    } else {
                        0x02
                    };
                    if tx.blocking_send(b).is_err() {
                        break;
                    }
                }
            });
            self.events = Some(rx);
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let rx = self
                .events
                .as_mut()
                .ok_or_else(|| IoToNetError::Driver("GPIO not open".into()))?;
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Some(b)) => {
                    buf[0] = b;
                    Ok(1)
                }
                Ok(None) => Err(IoToNetError::Driver("GPIO event stream ended".into())),
                Err(_) => Ok(0),
            }
        }

        async fn write(&mut self, _data: &[u8]) -> Result<usize> {
            Err(IoToNetError::Driver("GPIO sources are input-only".into()))
        }

        fn description(&self) -> String {
            format!("gpio://{}:{}", self.cfg.chip, self.cfg.line)
        }
    }
}
