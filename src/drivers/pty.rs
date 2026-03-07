use async_trait::async_trait;
use tracing::debug;

use crate::config::PtyConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

/// Pseudo-terminal (PTY) driver.
/// Opens a master PTY and exposes its slave end for external programs.
pub struct PtyDriver {
    cfg: PtyConfig,
    #[cfg(unix)]
    master_fd: Option<tokio::fs::File>,
    #[cfg(unix)]
    slave_path: Option<String>,
}

impl PtyDriver {
    pub fn new(cfg: PtyConfig) -> Self {
        PtyDriver {
            cfg,
            #[cfg(unix)]
            master_fd: None,
            #[cfg(unix)]
            slave_path: None,
        }
    }
}

#[async_trait]
impl IoDriver for PtyDriver {
    async fn open(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            use nix::fcntl::OFlag;
            use nix::pty::{grantpt, posix_openpt, ptsname, unlockpt};
            use std::os::unix::io::{FromRawFd, IntoRawFd};

            // Open master PTY device (works on both Linux and macOS)
            let master = posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY)
                .map_err(|e| IoToNetError::Driver(format!("posix_openpt failed: {}", e)))?;

            grantpt(&master)
                .map_err(|e| IoToNetError::Driver(format!("grantpt: {}", e)))?;
            unlockpt(&master)
                .map_err(|e| IoToNetError::Driver(format!("unlockpt: {}", e)))?;

            // ptsname is unsafe (not thread-safe) but correctness is fine here
            let slave_name = unsafe {
                ptsname(&master)
                    .map_err(|e| IoToNetError::Driver(format!("ptsname: {}", e)))?
            };

            // Create symlink if configured
            if let Some(ref symlink) = self.cfg.symlink {
                let _ = std::fs::remove_file(symlink);
                std::os::unix::fs::symlink(&slave_name, symlink)
                    .map_err(|e| IoToNetError::Driver(format!("Cannot create PTY symlink: {}", e)))?;
                debug!(link = %symlink, target = %slave_name, "PTY symlink created");
            }

            let raw_fd = master.into_raw_fd();
            let master_file = unsafe { std::fs::File::from_raw_fd(raw_fd) };

            self.master_fd = Some(tokio::fs::File::from_std(master_file));
            self.slave_path = Some(slave_name.clone());
            debug!(slave = %slave_name, "PTY opened");
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(IoToNetError::PlatformNotSupported("PTY requires Unix".to_string()))
        }
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        #[cfg(unix)]
        {
            use tokio::io::AsyncReadExt;
            match &mut self.master_fd {
                None => Err(IoToNetError::Driver("PTY not opened".to_string())),
                Some(f) => f.read(buf).await.map_err(IoToNetError::Network),
            }
        }
        #[cfg(not(unix))]
        Err(IoToNetError::PlatformNotSupported("PTY requires Unix".to_string()))
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        #[cfg(unix)]
        {
            use tokio::io::AsyncWriteExt;
            match &mut self.master_fd {
                None => Err(IoToNetError::Driver("PTY not opened".to_string())),
                Some(f) => f.write(data).await.map_err(IoToNetError::Network),
            }
        }
        #[cfg(not(unix))]
        Err(IoToNetError::PlatformNotSupported("PTY requires Unix".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            self.master_fd = None;
            if let Some(ref link) = self.cfg.symlink {
                let _ = std::fs::remove_file(link);
            }
        }
        Ok(())
    }

    fn description(&self) -> String {
        #[cfg(unix)]
        {
            self.slave_path
                .as_deref()
                .map(|p| format!("pty://{}", p))
                .unwrap_or_else(|| "pty://(not opened)".to_string())
        }
        #[cfg(not(unix))]
        "pty://(unavailable)".to_string()
    }
}


