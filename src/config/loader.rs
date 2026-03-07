use std::path::Path;

use anyhow::{Context, Result};
use tracing::debug;

use super::schema::AppConfig;

/// Load application configuration from a TOML file.
pub fn load_config(path: &Path) -> Result<AppConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config file: {}", path.display()))?;

    let cfg: AppConfig = toml::from_str(&content)
        .with_context(|| format!("Invalid TOML in config file: {}", path.display()))?;

    validate_config(&cfg)?;
    debug!(bridges = cfg.bridges.len(), "Configuration loaded");
    Ok(cfg)
}

/// Reload configuration from the same path (used for live-reload via SIGHUP).
#[allow(dead_code)]
pub fn reload_config(path: &Path) -> Result<AppConfig> {
    load_config(path)
}

fn validate_config(cfg: &AppConfig) -> Result<()> {
    if cfg.bridges.is_empty() {
        tracing::warn!("No bridges configured — io-to-net will run but forward no data");
    }

    for bridge in &cfg.bridges {
        if bridge.name.is_empty() {
            anyhow::bail!("Bridge name must not be empty");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_minimal_config() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
log_level = "debug"

[[bridges]]
name = "ecu"

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 115200

[bridges.transport]
type = "tcp"
listen = "[::]:9000"
"#).unwrap();

        let cfg = load_config(f.path()).unwrap();
        assert_eq!(cfg.bridges.len(), 1);
        assert_eq!(cfg.bridges[0].name, "ecu");
    }

    #[test]
    fn test_load_empty_bridges() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"log_level = "info""#).unwrap();
        let cfg = load_config(f.path()).unwrap();
        assert!(cfg.bridges.is_empty());
    }

    #[test]
    fn test_file_not_found() {
        let err = load_config(std::path::Path::new("/nonexistent/path/config.toml"));
        assert!(err.is_err());
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("Cannot read config file"));
    }

    #[test]
    fn test_invalid_toml_returns_error() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not valid toml ====").unwrap();
        let err = load_config(f.path());
        assert!(err.is_err());
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("Invalid TOML"));
    }

    #[test]
    fn test_bridge_empty_name_rejected() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
[[bridges]]
name = ""

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"

[bridges.transport]
type = "tcp"
listen = "[::]:9000"
"#).unwrap();
        let err = load_config(f.path());
        assert!(err.is_err());
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("Bridge name must not be empty"));
    }

    #[test]
    fn test_reload_returns_same_config() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
log_level = "debug"

[[bridges]]
name = "ecu"

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 115200

[bridges.transport]
type = "tcp"
listen = "[::]:9000"
"#).unwrap();
        let c1 = load_config(f.path()).unwrap();
        let c2 = reload_config(f.path()).unwrap();
        assert_eq!(c1.bridges[0].name, c2.bridges[0].name);
        assert_eq!(c1.log_level, c2.log_level);
    }

    #[test]
    fn test_multiple_bridges() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
[[bridges]]
name = "a"
[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
[bridges.transport]
type = "tcp"
listen = "[::]:9000"

[[bridges]]
name = "b"
[bridges.source]
type = "pty"
[bridges.transport]
type = "tcp"
listen = "[::]:9001"
"#).unwrap();
        let cfg = load_config(f.path()).unwrap();
        assert_eq!(cfg.bridges.len(), 2);
        assert_eq!(cfg.bridges[0].name, "a");
        assert_eq!(cfg.bridges[1].name, "b");
    }

    #[test]
    fn test_http_api_config_loaded() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
[http_api]
enabled = true
listen = "127.0.0.1:9106"

[http_api.auth]
token = "mysecret"
"#).unwrap();
        let cfg = load_config(f.path()).unwrap();
        let api = cfg.http_api.unwrap();
        assert!(api.enabled);
        assert_eq!(api.listen, "127.0.0.1:9106");
        assert_eq!(api.auth.unwrap().token, "mysecret");
    }
}
