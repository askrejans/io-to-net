use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, trace, warn};

use crate::config::{SerialConfig, SerialFlowControl, SerialParity};
use crate::error::{IoToNetError, Result};

use super::IoDriver;

/// After this many *consecutive* transient errors (BrokenPipe / Interrupted)
/// without a successful read, the driver gives up and forces a port reopen.
/// At 100 ms timeout per read that is ~3 seconds of silence before reconnecting.
const MAX_TRANSIENT_ERRORS: u32 = 30;

/// Wraps `Box<dyn serialport::SerialPort>` to make it `Send`.
///
/// All serialport concrete implementations (`TTYPort` on Unix, `COMPort` on Windows)
/// are `Send`. The `dyn SerialPort` trait object erases that bound, so we restore it
/// here to allow moving the port into `spawn_blocking` closures.
struct SerialPortHandle(Box<dyn serialport::SerialPort>);

// SAFETY: All concrete serialport implementations (TTYPort/COMPort) are Send.
// Sync is also safe here because SerialDriver only accesses the port through
// &mut self, so no concurrent shared references are ever possible.
unsafe impl Send for SerialPortHandle {}
unsafe impl Sync for SerialPortHandle {}

pub struct SerialDriver {
    cfg: SerialConfig,
    port: Option<SerialPortHandle>,
    /// Counts consecutive transient errors (BrokenPipe / Interrupted).
    /// Reset to 0 on any successful byte read.
    transient_error_count: u32,
}

impl SerialDriver {
    pub fn new(cfg: SerialConfig) -> Self {
        SerialDriver { cfg, port: None, transient_error_count: 0 }
    }
}

/// Outcome returned from the `spawn_blocking` read closure.
enum ReadOutcome {
    Data(SerialPortHandle, Vec<u8>, usize),
    Timeout(SerialPortHandle),
    Transient(SerialPortHandle, String), // BrokenPipe / Interrupted — may self-heal
    Fatal(SerialPortHandle, String),
}

#[async_trait]
impl IoDriver for SerialDriver {
    async fn open(&mut self) -> Result<()> {
        use serialport::{DataBits, FlowControl, Parity, StopBits};

        let data_bits = match self.cfg.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        };

        let parity = match self.cfg.parity {
            SerialParity::None => Parity::None,
            SerialParity::Odd  => Parity::Odd,
            SerialParity::Even => Parity::Even,
        };

        let stop_bits = match self.cfg.stop_bits {
            2 => StopBits::Two,
            _ => StopBits::One,
        };

        let flow_control = match self.cfg.flow_control {
            SerialFlowControl::None     => FlowControl::None,
            SerialFlowControl::Software => FlowControl::Software,
            SerialFlowControl::Hardware => FlowControl::Hardware,
        };

        let port = serialport::new(&self.cfg.port, self.cfg.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(flow_control)
            .timeout(Duration::from_millis(self.cfg.read_timeout_ms))
            .open()
            .map_err(|e| IoToNetError::Driver(
                format!("Cannot open serial port {}: {}", self.cfg.port, e)))?;

        self.port = Some(SerialPortHandle(port));
        self.transient_error_count = 0;
        debug!(port = %self.cfg.port, baud = self.cfg.baud, "Serial port opened");
        Ok(())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut handle = self.port.take()
            .ok_or_else(|| IoToNetError::Driver("Serial port not opened".to_string()))?;

        let read_len = buf.len();

        // Run the blocking serial read on a dedicated thread so the tokio executor
        // is not stalled. Mirrors the I2C driver pattern which is known to work.
        let outcome = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut tmp = vec![0u8; read_len];
            match handle.0.read(&mut tmp) {
                Ok(0) => ReadOutcome::Timeout(handle),
                Ok(n) => ReadOutcome::Data(handle, tmp, n),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut  => ReadOutcome::Timeout(handle),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => ReadOutcome::Timeout(handle),
                // BrokenPipe / Interrupted are transient on Linux UARTs (modem signal
                // glitches, GPS module resets). Retry without reopening the port.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe   => ReadOutcome::Transient(handle, format!("{e}")),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted  => ReadOutcome::Transient(handle, format!("{e}")),
                Err(e) => ReadOutcome::Fatal(handle, format!("{e}")),
            }
        })
        .await
        .map_err(|e| IoToNetError::Driver(format!("Serial read spawn_blocking: {e}")))?;

        match outcome {
            ReadOutcome::Data(handle, data, n) => {
                self.port = Some(handle);
                self.transient_error_count = 0;
                buf[..n].copy_from_slice(&data[..n]);
                Ok(n)
            }
            ReadOutcome::Timeout(handle) => {
                self.port = Some(handle);
                Ok(0)
            }
            ReadOutcome::Transient(handle, msg) => {
                self.port = Some(handle);
                self.transient_error_count += 1;
                if self.transient_error_count < MAX_TRANSIENT_ERRORS {
                    trace!(
                        error = %msg,
                        count = self.transient_error_count,
                        max = MAX_TRANSIENT_ERRORS,
                        "Serial transient error — retrying"
                    );
                    Ok(0) // caller loops and retries; no port reopen needed
                } else {
                    // Too many consecutive transient errors — something is genuinely wrong.
                    warn!(
                        error = %msg,
                        count = self.transient_error_count,
                        "Serial transient errors exceeded threshold — forcing port reopen"
                    );
                    self.transient_error_count = 0;
                    Err(IoToNetError::Driver(msg))
                }
            }
            ReadOutcome::Fatal(handle, msg) => {
                self.port = Some(handle);
                warn!(error = %msg, "Serial read error");
                Err(IoToNetError::Driver(msg))
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        let mut handle = self.port.take()
            .ok_or_else(|| IoToNetError::Driver("Serial port not opened".to_string()))?;

        let data_owned = data.to_vec();

        let result = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            match handle.0.write(&data_owned) {
                Ok(n)  => Ok((handle, n)),
                Err(e) => Err((handle, format!("{e}"))),
            }
        })
        .await
        .map_err(|e| IoToNetError::Driver(format!("Serial write spawn_blocking: {e}")))?;

        match result {
            Ok((handle, n)) => {
                self.port = Some(handle);
                Ok(n)
            }
            Err((handle, msg)) => {
                self.port = Some(handle);
                Err(IoToNetError::Driver(msg))
            }
        }
    }

    async fn close(&mut self) -> Result<()> {
        self.port = None;
        Ok(())
    }

    fn description(&self) -> String {
        format!("serial://{}@{}", self.cfg.port, self.cfg.baud)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SerialConfig;

    #[tokio::test]
    async fn open_nonexistent_port_returns_driver_error() {
        let cfg = SerialConfig {
            port: "/dev/nonexistent_io_to_net".into(),
            ..SerialConfig::default()
        };
        let mut d = SerialDriver::new(cfg);
        let err = d.open().await.unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("Cannot open serial port"));
    }

    #[tokio::test]
    async fn read_before_open_returns_error() {
        let mut d = SerialDriver::new(SerialConfig::default());
        let mut buf = [0u8; 8];
        assert!(d.read(&mut buf).await.is_err());
    }

    #[tokio::test]
    async fn write_before_open_returns_error() {
        let mut d = SerialDriver::new(SerialConfig::default());
        assert!(d.write(b"test").await.is_err());
    }

    #[test]
    fn description_includes_port_and_baud() {
        let d = SerialDriver::new(SerialConfig {
            port: "/dev/ttyUSB0".into(),
            baud: 9600,
            ..SerialConfig::default()
        });
        let desc = d.description();
        assert!(desc.contains("/dev/ttyUSB0"));
        assert!(desc.contains("9600"));
    }

    #[tokio::test]
    async fn close_is_idempotent() {
        let mut d = SerialDriver::new(SerialConfig::default());
        d.close().await.unwrap();
        d.close().await.unwrap();
    }
}
