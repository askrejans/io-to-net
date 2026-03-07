pub mod bluetooth;
pub mod canbus;
pub mod gpio;
pub mod i2c;
pub mod onewire;
pub mod pty;
pub mod serial;
pub mod unix_socket;

use async_trait::async_trait;

use crate::config::SourceConfig;
use crate::error::{IoToNetError, Result};

/// Unified async trait implemented by every hardware IO driver.
#[async_trait]
pub trait IoDriver: Send + Sync {
    /// Open / connect to the hardware interface.
    async fn open(&mut self) -> Result<()>;

    /// Read raw bytes from the hardware interface into `buf`.
    /// Returns the number of bytes read (may be 0 on timeout, > 0 on data).
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Write raw bytes to the hardware interface.
    #[allow(dead_code)]
    async fn write(&mut self, data: &[u8]) -> Result<usize>;

    /// Close the hardware interface cleanly.
    #[allow(dead_code)]
    async fn close(&mut self) -> Result<()>;

    /// Human-readable description for logs/TUI.
    #[allow(dead_code)]
    fn description(&self) -> String;
}

/// Create the correct driver implementation for the given source configuration.
pub fn create_driver(source: &SourceConfig) -> Result<Box<dyn IoDriver>> {
    match source {
        SourceConfig::Serial(cfg) => Ok(Box::new(serial::SerialDriver::new(cfg.clone()))),
        SourceConfig::Pty(cfg) => Ok(Box::new(pty::PtyDriver::new(cfg.clone()))),
        SourceConfig::UnixSocket(cfg) => Ok(Box::new(unix_socket::UnixSocketDriver::new(cfg.clone()))),
        SourceConfig::Bluetooth(cfg) => Ok(Box::new(bluetooth::BluetoothDriver::new(cfg.clone()))),

        #[cfg(target_os = "linux")]
        SourceConfig::I2c(cfg) => Ok(Box::new(i2c::I2cDriver::new(cfg.clone()))),
        #[cfg(target_os = "linux")]
        SourceConfig::Gpio(cfg) => Ok(Box::new(gpio::GpioDriver::new(cfg.clone()))),
        #[cfg(target_os = "linux")]
        SourceConfig::Canbus(cfg) => Ok(Box::new(canbus::CanbusDriver::new(cfg.clone()))),
        #[cfg(target_os = "linux")]
        SourceConfig::Onewire(cfg) => Ok(Box::new(onewire::OnewireDriver::new(cfg.clone()))),
        #[cfg(target_os = "linux")]
        SourceConfig::Spi(_cfg) => Err(IoToNetError::PlatformNotSupported(
            "SPI driver not yet implemented".to_string(),
        )),

        #[cfg(not(target_os = "linux"))]
        SourceConfig::I2c(_) => Err(IoToNetError::PlatformNotSupported(
            "I2C driver requires Linux".to_string(),
        )),
        #[cfg(not(target_os = "linux"))]
        SourceConfig::Gpio(_) => Err(IoToNetError::PlatformNotSupported(
            "GPIO driver requires Linux".to_string(),
        )),
        #[cfg(not(target_os = "linux"))]
        SourceConfig::Canbus(_) => Err(IoToNetError::PlatformNotSupported(
            "CAN bus driver requires Linux".to_string(),
        )),
        #[cfg(not(target_os = "linux"))]
        SourceConfig::Onewire(_) => Err(IoToNetError::PlatformNotSupported(
            "1-Wire driver requires Linux".to_string(),
        )),
        #[cfg(not(target_os = "linux"))]
        SourceConfig::Spi(_) => Err(IoToNetError::PlatformNotSupported(
            "SPI driver requires Linux".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    #[test]
    fn create_serial_driver() {
        let src = SourceConfig::Serial(SerialConfig::default());
        let d = create_driver(&src);
        assert!(d.is_ok());
    }

    #[test]
    fn create_pty_driver() {
        let src = SourceConfig::Pty(PtyConfig::default());
        assert!(create_driver(&src).is_ok());
    }

    #[test]
    fn create_unix_socket_driver() {
        let src = SourceConfig::UnixSocket(UnixSocketConfig::default());
        assert!(create_driver(&src).is_ok());
    }

    #[test]
    fn create_bluetooth_driver() {
        let src = SourceConfig::Bluetooth(BluetoothConfig::default());
        assert!(create_driver(&src).is_ok());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn linux_drivers_error_on_non_linux() {
        use crate::error::IoToNetError;
        let cases = vec![
            SourceConfig::I2c(I2cConfig::default()),
            SourceConfig::Gpio(GpioConfig::default()),
            SourceConfig::Canbus(CanbusConfig::default()),
            SourceConfig::Onewire(OnewireConfig::default()),
            SourceConfig::Spi(SpiConfig::default()),
        ];
        for src in cases {
            match create_driver(&src) {
                Err(IoToNetError::PlatformNotSupported(_)) => {}
                Err(e) => panic!("expected PlatformNotSupported, got {}", e),
                Ok(_) => panic!("expected Err, got Ok"),
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_drivers_created_on_linux() {
        let cases = vec![
            SourceConfig::I2c(I2cConfig::default()),
            SourceConfig::Gpio(GpioConfig::default()),
            SourceConfig::Canbus(CanbusConfig::default()),
            SourceConfig::Onewire(OnewireConfig::default()),
        ];
        for src in cases {
            assert!(create_driver(&src).is_ok());
        }
    }
}
