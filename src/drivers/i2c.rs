#![allow(dead_code)]

use async_trait::async_trait;
#[cfg(target_os = "linux")]
use tracing::{debug, info, warn};

use crate::config::I2cConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

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
            use i2cdev::core::I2CDevice;

            let mut dev = LinuxI2CDevice::new(&self.cfg.bus, self.cfg.address)
                .map_err(|e| IoToNetError::Driver(format!("I2C open {}: {}", self.cfg.bus, e)))?;

            debug!(bus = %self.cfg.bus, addr = %format!("0x{:02X}", self.cfg.address), "I2C opened");

            // Send any device-specific initialisation sequences.
            for (i, seq) in self.cfg.init_bytes.iter().enumerate() {
                dev.write(seq)
                    .map_err(|e| IoToNetError::Driver(
                        format!("I2C init[{}] write: {}", i, e)))?;
                info!(
                    bus = %self.cfg.bus,
                    addr = %format!("0x{:02X}", self.cfg.address),
                    seq = ?seq,
                    "I2C init sequence sent"
                );
            }

            self.device = Some(dev);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("I2C requires Linux".to_string()))
    }

    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            use i2cdev::core::I2CDevice;
            let buf = _buf;

            // Take device out so we can move it into spawn_blocking.
            let mut dev = self.device.take()
                .ok_or_else(|| IoToNetError::Driver("I2C not opened".to_string()))?;

            let read_len  = self.cfg.read_len.min(buf.len());
            let read_reg  = self.cfg.read_reg;
            let poll_ms   = self.cfg.poll_ms;

            // Run blocking I2C operations on a dedicated thread so the tokio
            // executor is not stalled.
            let result = tokio::task::spawn_blocking(move || -> std::result::Result<(LinuxI2CDevice, Vec<u8>), String> {
                // Write register address to point the device's internal pointer.
                if let Some(reg) = read_reg {
                    dev.write(&[reg])
                        .map_err(|e| format!("I2C reg write 0x{:02X}: {}", reg, e))?;
                }
                let mut tmp = vec![0u8; read_len];
                dev.read(&mut tmp)
                    .map_err(|e| format!("I2C read: {}", e))?;
                Ok((dev, tmp))
            }).await
            .map_err(|e| IoToNetError::Driver(format!("I2C spawn_blocking: {}", e)))?
            .map_err(|e| IoToNetError::Driver(e))?;

            let (dev, data) = result;
            self.device = Some(dev); // return device
            buf[..read_len].copy_from_slice(&data);

            // Poll delay — async, does not block the executor.
            tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
            Ok(read_len)
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("I2C requires Linux".to_string()))
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            use i2cdev::core::I2CDevice;
            let data = _data;

            let mut dev = self.device.take()
                .ok_or_else(|| IoToNetError::Driver("I2C not opened".to_string()))?;

            let data_owned = data.to_vec();
            let result = tokio::task::spawn_blocking(move || -> std::result::Result<LinuxI2CDevice, String> {
                dev.write(&data_owned)
                    .map_err(|e| format!("I2C write: {}", e))?;
                Ok(dev)
            }).await
            .map_err(|e| IoToNetError::Driver(format!("I2C write spawn_blocking: {}", e)))?
            .map_err(|e| IoToNetError::Driver(e))?;

            self.device = Some(result);
            Ok(data.len())
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("I2C requires Linux".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            if self.device.take().is_some() {
                warn!(bus = %self.cfg.bus, "I2C device closed");
            }
        }
        Ok(())
    }

    fn description(&self) -> String {
        let reg = self.cfg.read_reg
            .map(|r| format!(", reg=0x{:02X}", r))
            .unwrap_or_default();
        format!("i2c://{}@0x{:02X}{}", self.cfg.bus, self.cfg.address, reg)
    }
}
