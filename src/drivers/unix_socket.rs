use async_trait::async_trait;
use tracing::debug;

use crate::config::UnixSocketConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

/// Unix domain socket driver.
/// Connects to an existing socket or binds and accepts one client.
pub struct UnixSocketDriver {
    cfg: UnixSocketConfig,
    #[cfg(unix)]
    stream: Option<tokio::net::UnixStream>,
}

impl UnixSocketDriver {
    pub fn new(cfg: UnixSocketConfig) -> Self {
        UnixSocketDriver {
            cfg,
            #[cfg(unix)]
            stream: None,
        }
    }
}

#[async_trait]
impl IoDriver for UnixSocketDriver {
    async fn open(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            let stream = tokio::net::UnixStream::connect(&self.cfg.path)
                .await
                .map_err(|e| IoToNetError::Driver(format!("Cannot connect to Unix socket {}: {}", self.cfg.path, e)))?;

            self.stream = Some(stream);
            debug!(path = %self.cfg.path, "Unix socket connected");
            Ok(())
        }
        #[cfg(not(unix))]
        Err(IoToNetError::PlatformNotSupported("Unix sockets require Unix".to_string()))
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        #[cfg(unix)]
        {
            use tokio::io::AsyncReadExt;
            match &mut self.stream {
                None => Err(IoToNetError::Driver("Socket not connected".to_string())),
                Some(s) => s.read(buf).await.map_err(IoToNetError::Network),
            }
        }
        #[cfg(not(unix))]
        Err(IoToNetError::PlatformNotSupported("Unix sockets require Unix".to_string()))
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        #[cfg(unix)]
        {
            use tokio::io::AsyncWriteExt;
            match &mut self.stream {
                None => Err(IoToNetError::Driver("Socket not connected".to_string())),
                Some(s) => s.write(data).await.map_err(IoToNetError::Network),
            }
        }
        #[cfg(not(unix))]
        Err(IoToNetError::PlatformNotSupported("Unix sockets require Unix".to_string()))
    }

    async fn close(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            self.stream = None;
        }
        Ok(())
    }

    fn description(&self) -> String {
        format!("unix://{}", self.cfg.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UnixSocketConfig;

    #[test]
    fn description_format() {
        let d = UnixSocketDriver::new(UnixSocketConfig { path: "/tmp/test.sock".into() });
        assert_eq!(d.description(), "unix:///tmp/test.sock");
    }

    #[tokio::test]
    async fn open_nonexistent_path_returns_error() {
        let mut d = UnixSocketDriver::new(UnixSocketConfig {
            path: "/nonexistent/sock_io_to_net_test".into(),
        });
        assert!(d.open().await.is_err());
    }

    #[tokio::test]
    async fn read_before_open_returns_error() {
        let mut d = UnixSocketDriver::new(UnixSocketConfig::default());
        let mut buf = [0u8; 8];
        assert!(d.read(&mut buf).await.is_err());
    }

    #[tokio::test]
    async fn write_before_open_returns_error() {
        let mut d = UnixSocketDriver::new(UnixSocketConfig::default());
        assert!(d.write(b"data").await.is_err());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn read_write_round_trip_via_unix_socket() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixListener;

        let path = format!("/tmp/io_to_net_rw_{}.sock", std::process::id());
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let server = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 32];
            let n = conn.read(&mut buf).await.unwrap();
            conn.write_all(&buf[..n]).await.unwrap(); // echo
        });

        let mut driver = UnixSocketDriver::new(UnixSocketConfig { path: path.clone() });
        driver.open().await.unwrap();
        assert_eq!(driver.write(b"ping").await.unwrap(), 4);

        let mut buf = [0u8; 32];
        let n = driver.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ping");

        driver.close().await.unwrap();
        let _ = server.await;
        let _ = std::fs::remove_file(&path);
    }
}
