use async_trait::async_trait;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::{debug, warn};

use crate::config::{SerialConfig, SerialFlowControl, SerialParity};
use crate::error::{IoToNetError, Result};

use super::IoDriver;

pub struct SerialDriver {
    cfg: SerialConfig,
    port: Option<SerialStream>,
}

impl SerialDriver {
    pub fn new(cfg: SerialConfig) -> Self {
        SerialDriver { cfg, port: None }
    }
}

#[async_trait]
impl IoDriver for SerialDriver {
    async fn open(&mut self) -> Result<()> {
        use serialport::FlowControl;
        use tokio_serial::DataBits;
        use tokio_serial::Parity;
        use tokio_serial::StopBits;

        let data_bits = match self.cfg.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        };

        let parity = match self.cfg.parity {
            SerialParity::None => Parity::None,
            SerialParity::Odd => Parity::Odd,
            SerialParity::Even => Parity::Even,
        };

        let stop_bits = match self.cfg.stop_bits {
            2 => StopBits::Two,
            _ => StopBits::One,
        };

        let flow_control = match self.cfg.flow_control {
            SerialFlowControl::None => FlowControl::None,
            SerialFlowControl::Software => FlowControl::Software,
            SerialFlowControl::Hardware => FlowControl::Hardware,
        };

        let builder = tokio_serial::new(&self.cfg.port, self.cfg.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(flow_control)
            .timeout(std::time::Duration::from_millis(self.cfg.read_timeout_ms));

        let stream = builder
            .open_native_async()
            .map_err(|e| IoToNetError::Driver(format!("Cannot open serial port {}: {}", self.cfg.port, e)))?;

        self.port = Some(stream);
        debug!(port = %self.cfg.port, baud = self.cfg.baud, "Serial port opened");
        Ok(())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        use tokio::io::AsyncReadExt;
        match &mut self.port {
            None => Err(IoToNetError::Driver("Serial port not opened".to_string())),
            Some(port) => {
                let timeout = std::time::Duration::from_millis(self.cfg.read_timeout_ms);
                match tokio::time::timeout(timeout, port.read(buf)).await {
                    Err(_) => Ok(0), // timeout — no data within the window
                    Ok(Ok(0)) => Ok(0), // EOF / no data
                    Ok(Ok(n)) => Ok(n),
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
                    Ok(Err(e)) => {
                        warn!(error = %e, "Serial read error");
                        Err(IoToNetError::Network(e))
                    }
                }
            }
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        use tokio::io::AsyncWriteExt;
        match &mut self.port {
            None => Err(IoToNetError::Driver("Serial port not opened".to_string())),
            Some(port) => port
                .write(data)
                .await
                .map_err(IoToNetError::Network),
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
        d.close().await.unwrap(); // second close should not panic
    }
}
