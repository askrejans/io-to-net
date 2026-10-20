//! 1-Wire sensors through the Linux w1 sysfs interface: the `w1_slave` text
//! every `poll_ms`.

#[cfg(target_os = "linux")]
pub use linux::OnewireDriver;

#[cfg(target_os = "linux")]
mod linux {
    use crate::config::OnewireConfig;
    use crate::drivers::IoDriver;
    use crate::error::{IoToNetError, Result};
    use std::path::PathBuf;

    pub struct OnewireDriver {
        cfg: OnewireConfig,
    }

    impl OnewireDriver {
        pub fn new(cfg: OnewireConfig) -> Self {
            OnewireDriver { cfg }
        }
        fn path(&self) -> PathBuf {
            PathBuf::from(&self.cfg.device).join("w1_slave")
        }
    }

    impl IoDriver for OnewireDriver {
        async fn open(&mut self) -> Result<()> {
            if !self.path().exists() {
                return Err(IoToNetError::Driver(format!(
                    "1-Wire device not found: {}",
                    self.path().display()
                )));
            }
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            tokio::time::sleep(std::time::Duration::from_millis(self.cfg.poll_ms)).await;
            let text = tokio::fs::read(self.path()).await.map_err(|e| {
                IoToNetError::Driver(format!("1-Wire {}: {e}", self.path().display()))
            })?;
            let n = text.len().min(buf.len());
            buf[..n].copy_from_slice(&text[..n]);
            Ok(n)
        }

        async fn write(&mut self, _data: &[u8]) -> Result<usize> {
            Err(IoToNetError::Driver("1-Wire sources are read-only".into()))
        }

        fn description(&self) -> String {
            format!("onewire://{}", self.cfg.device)
        }
    }
}
