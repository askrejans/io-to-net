//! I²C sensor polling (Linux): optional init writes, then every `poll_ms`
//! an optional register select and a `read_len`-byte read.

#[cfg(target_os = "linux")]
pub use linux::I2cDriver;

#[cfg(target_os = "linux")]
mod linux {
    use crate::config::I2cConfig;
    use crate::drivers::IoDriver;
    use crate::error::{IoToNetError, Result};
    use i2cdev::core::I2CDevice;
    use i2cdev::linux::LinuxI2CDevice;
    use std::sync::{Arc, Mutex};

    pub struct I2cDriver {
        cfg: I2cConfig,
        device: Option<Arc<Mutex<LinuxI2CDevice>>>,
    }

    impl I2cDriver {
        pub fn new(cfg: I2cConfig) -> Self {
            I2cDriver { cfg, device: None }
        }
        fn device(&self) -> Result<Arc<Mutex<LinuxI2CDevice>>> {
            self.device
                .clone()
                .ok_or_else(|| IoToNetError::Driver("I2C not open".into()))
        }
    }

    impl IoDriver for I2cDriver {
        async fn open(&mut self) -> Result<()> {
            let (bus, addr, init) = (
                self.cfg.bus.clone(),
                self.cfg.address,
                self.cfg.init_bytes.clone(),
            );
            let dev = tokio::task::spawn_blocking(
                move || -> std::result::Result<LinuxI2CDevice, String> {
                    let mut dev = LinuxI2CDevice::new(&bus, addr)
                        .map_err(|e| format!("I2C {bus}@0x{addr:02X}: {e}"))?;
                    for (i, seq) in init.iter().enumerate() {
                        dev.write(seq).map_err(|e| format!("I2C init[{i}]: {e}"))?;
                    }
                    Ok(dev)
                },
            )
            .await
            .map_err(|e| IoToNetError::Driver(e.to_string()))?
            .map_err(IoToNetError::Driver)?;
            self.device = Some(Arc::new(Mutex::new(dev)));
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            tokio::time::sleep(std::time::Duration::from_millis(self.cfg.poll_ms)).await;
            let dev = self.device()?;
            let (reg, len) = (self.cfg.read_reg, self.cfg.read_len.min(buf.len()));
            let data =
                tokio::task::spawn_blocking(move || -> std::result::Result<Vec<u8>, String> {
                    let mut d = dev
                        .lock()
                        .map_err(|_| "I2C device lock poisoned".to_string())?;
                    if let Some(r) = reg {
                        d.write(&[r])
                            .map_err(|e| format!("I2C select 0x{r:02X}: {e}"))?;
                    }
                    let mut out = vec![0u8; len];
                    d.read(&mut out).map_err(|e| format!("I2C read: {e}"))?;
                    Ok(out)
                })
                .await
                .map_err(|e| IoToNetError::Driver(e.to_string()))?
                .map_err(IoToNetError::Driver)?;
            buf[..data.len()].copy_from_slice(&data);
            Ok(data.len())
        }

        async fn write(&mut self, data: &[u8]) -> Result<usize> {
            let dev = self.device()?;
            let owned = data.to_vec();
            tokio::task::spawn_blocking(move || -> std::result::Result<(), String> {
                dev.lock()
                    .map_err(|_| "I2C device lock poisoned".to_string())?
                    .write(&owned)
                    .map_err(|e| format!("I2C write: {e}"))
            })
            .await
            .map_err(|e| IoToNetError::Driver(e.to_string()))?
            .map_err(IoToNetError::Driver)?;
            Ok(data.len())
        }

        fn description(&self) -> String {
            let reg = self
                .cfg
                .read_reg
                .map(|r| format!(" reg 0x{r:02X}"))
                .unwrap_or_default();
            format!("i2c://{}@0x{:02X}{reg}", self.cfg.bus, self.cfg.address)
        }
    }
}
