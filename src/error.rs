use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum IoToNetError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Driver error: {0}")]
    Driver(String),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Network error: {0}")]
    Network(#[from] std::io::Error),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("Not supported on this platform: {0}")]
    PlatformNotSupported(String),

    #[error("Bridge not found: {0}")]
    BridgeNotFound(String),

    #[error("Serialisation error: {0}")]
    Serialisation(String),

    #[error("Authentication failure")]
    AuthFailure,
}

pub type Result<T> = std::result::Result<T, IoToNetError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_driver() {
        let e = IoToNetError::Driver("port not found".into());
        assert_eq!(format!("{}", e), "Driver error: port not found");
    }

    #[test]
    fn error_display_transport() {
        let e = IoToNetError::Transport("bind failed".into());
        assert_eq!(format!("{}", e), "Transport error: bind failed");
    }

    #[test]
    fn error_display_platform() {
        let e = IoToNetError::PlatformNotSupported("I2C requires Linux".into());
        assert_eq!(format!("{}", e), "Not supported on this platform: I2C requires Linux");
    }

    #[test]
    fn error_display_auth_failure() {
        let e = IoToNetError::AuthFailure;
        assert_eq!(format!("{}", e), "Authentication failure");
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        let e: IoToNetError = io_err.into();
        assert!(matches!(e, IoToNetError::Network(_)));
    }

    #[test]
    fn error_debug_format() {
        let e = IoToNetError::Config("bad cfg".into());
        let dbg = format!("{:?}", e);
        assert!(dbg.contains("Config"));
    }
}
