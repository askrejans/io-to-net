#![allow(dead_code)]

use async_trait::async_trait;
#[cfg(target_os = "linux")]
use tracing::debug;

use crate::config::OnewireConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

/// 1-Wire bus driver — reads the Linux sysfs w1_slave file periodically.
/// Typically used for DS18B20 temperature sensors.
pub struct OnewireDriver {
    cfg: OnewireConfig,
}

impl OnewireDriver {
    pub fn new(cfg: OnewireConfig) -> Self {
        OnewireDriver { cfg }
    }

    fn sysfs_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.cfg.device).join("w1_slave")
    }
}

#[async_trait]
impl IoDriver for OnewireDriver {
    async fn open(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let path = self.sysfs_path();
            if !path.exists() {
                return Err(IoToNetError::Driver(format!(
                    "1-Wire device not found: {}",
                    path.display()
                )));
            }
            debug!(device = %self.cfg.device, "1-Wire device opened");
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("1-Wire requires Linux".to_string()))
    }

    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            let path = self.sysfs_path();
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| IoToNetError::Driver(format!("1-Wire read {}: {}", path.display(), e)))?;

            let bytes = content.as_bytes();
            let len = bytes.len().min(_buf.len());
            _buf[..len].copy_from_slice(&bytes[..len]);

            tokio::time::sleep(std::time::Duration::from_millis(self.cfg.poll_ms)).await;
            Ok(len)
        }
        #[cfg(not(target_os = "linux"))]
        Err(IoToNetError::PlatformNotSupported("1-Wire requires Linux".to_string()))
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        Err(IoToNetError::Driver("1-Wire is read-only".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> String {
        format!("onewire://{}", self.cfg.device)
    }
}
