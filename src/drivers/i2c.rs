#![allow(dead_code)]

use async_trait::async_trait;
#[cfg(target_os = "linux")]
use tracing::debug;

use crate::config::I2cConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

#[cfg(target_os = "linux")]
use i2cdev::core::I2CDevice;
#[cfg(target_os = "linux")]
use i2cdev::linux::LinuxI2CDevice;

pub struct I2cDriver {
    cfg: I2cConfig,
    #[cfg(target_os = "linux")]
    device: Option<LinuxI2CDevice>,
}

impl I2cDriver {
    pub fn new(cfg: I2cConfig) -> Self {
        I2cDriver {
            cfg,
            #[cfg(target_os = "linux")]
            device: None,
        }
    }
}

#[async_trait]
impl IoDriver for I2cDriver {
    async fn open(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let dev = LinuxI2CDevice::new(&self.cfg.bus, self.cfg.address)
                .map_err(|e| IoToNetError::Driver(format!("I2C open {}: {}", self.cfg.bus, e)))?;
            self.device = Some(dev);
            debug!(bus = %self.cfg.bus, addr = self.cfg.address, "I2C opened");
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("I2C requires Linux".to_string()))
    }

    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            use i2cdev::core::I2CDevice;
            match &mut self.device {
                None => Err(IoToNetError::Driver("I2C not opened".to_string())),
                Some(dev) => {
                    let read_len = self.cfg.read_len.min(_buf.len());
                    // Blocking I2C read — run on blocking thread
                    let mut tmp = vec![0u8; read_len];
                    dev.read(&mut tmp)
                        .map_err(|e| IoToNetError::Driver(format!("I2C read: {}", e)))?;
                    _buf[..read_len].copy_from_slice(&tmp);
                    // Insert poll delay
                    tokio::time::sleep(std::time::Duration::from_millis(self.cfg.poll_ms)).await;
                    Ok(read_len)
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("I2C requires Linux".to_string()))
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            use i2cdev::core::I2CDevice;
            match &mut self.device {
                None => Err(IoToNetError::Driver("I2C not opened".to_string())),
                Some(dev) => {
                    dev.write(_data)
                        .map_err(|e| IoToNetError::Driver(format!("I2C write: {}", e)))?;
                    Ok(_data.len())
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("I2C requires Linux".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.device = None;
        }
        Ok(())
    }

    fn description(&self) -> String {
        format!("i2c://{}@0x{:02X}", self.cfg.bus, self.cfg.address)
    }
}
