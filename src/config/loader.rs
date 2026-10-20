use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::schema::{AppConfig, SourceConfig, TransportConfig};

/// Config locations tried when no path is given, in order.
pub const SEARCH_PATHS: &[&str] = &["config.toml", "/etc/io-to-net/config.toml"];

pub fn resolve_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    for p in SEARCH_PATHS {
        if Path::new(p).exists() {
            return Ok(PathBuf::from(p));
        }
    }
    bail!(
        "no config file found (tried {}); pass --config <path>",
        SEARCH_PATHS.join(", ")
    )
}

pub fn load_config(path: &Path) -> Result<AppConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config file {}", path.display()))?;
    let cfg: AppConfig =
        toml::from_str(&content).with_context(|| format!("invalid config {}", path.display()))?;
    validate(&cfg)?;
    Ok(cfg)
}

pub fn validate(cfg: &AppConfig) -> Result<()> {
    let mut names = HashSet::new();
    let mut listens = HashSet::new();
    for b in &cfg.bridges {
        if b.name.trim().is_empty() {
            bail!("bridge name must not be empty");
        }
        if !b
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            bail!(
                "bridge name {:?}: use letters, digits, '-', '_' or '.'",
                b.name
            );
        }
        if !names.insert(b.name.as_str()) {
            bail!("duplicate bridge name {:?}", b.name);
        }
        if !listens.insert(b.transport.listen().to_string()) {
            bail!("bridges share the listen address {}", b.transport.listen());
        }
        if b.max_clients == Some(0) {
            bail!("bridge {:?}: max_clients must be at least 1", b.name);
        }
        if b.auth.as_ref().is_some_and(|a| a.token.trim().is_empty()) {
            bail!("bridge {:?}: auth.token must not be empty", b.name);
        }
        if let SourceConfig::Serial(s) = &b.source {
            if s.baud == 0 {
                bail!("bridge {:?}: baud must be greater than 0", b.name);
            }
            if !(5..=8).contains(&s.data_bits) || !(1..=2).contains(&s.stop_bits) {
                bail!("bridge {:?}: data_bits 5–8 and stop_bits 1–2", b.name);
            }
            if s.port.starts_with("usb:") {
                crate::link::serial::Selector::parse(&s.port)
                    .with_context(|| format!("bridge {:?}", b.name))?;
            }
        }
        if let TransportConfig::Tls(t) = &b.transport
            && (t.cert.is_empty() || t.key.is_empty())
        {
            bail!("bridge {:?}: tls needs cert and key", b.name);
        }
    }
    if let Some(api) = &cfg.http_api
        && api.enabled
        && api.auth.as_ref().is_some_and(|a| a.token.trim().is_empty())
    {
        bail!("http_api.auth.token must not be empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn load(src: &str) -> Result<AppConfig> {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(src.as_bytes()).unwrap();
        load_config(f.path())
    }

    const EXAMPLE: &str = include_str!("../../config/example.toml");

    #[test]
    fn shipped_example_is_valid() {
        let cfg = load(EXAMPLE).unwrap();
        assert_eq!(cfg.bridges.len(), 1);
    }

    /// Every commented-out example block, uncommented, is valid too.
    #[test]
    fn every_example_block_is_valid() {
        let all: String = EXAMPLE
            .lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(rest)
                    if rest.starts_with('[')
                        || rest.split_once(" = ").is_some_and(|(k, _)| {
                            k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                        }) =>
                {
                    rest
                }
                _ => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = load(&all).unwrap();
        let kinds: Vec<_> = cfg.bridges.iter().map(|b| b.source.kind()).collect();
        assert_eq!(
            kinds,
            ["serial", "serial", "tcp_client", "tcp_client", "canbus", "i2c", "gpio", "onewire", "bluetooth", "pty", "unix_socket"]
        );
        assert!(cfg.metrics.is_some() && cfg.http_api.unwrap().auth.is_some());
    }

    const ECU: &str = "[[bridges]]\nname = \"ecu\"\n[bridges.source]\ntype = \"serial\"\nport = \"/dev/ttyUSB0\"\n[bridges.transport]\ntype = \"tcp\"\nlisten = \"[::]:9000\"\n";

    #[test]
    fn loads_a_minimal_bridge() {
        let cfg = load(ECU).unwrap();
        assert_eq!(cfg.bridges[0].name, "ecu");
    }

    #[test]
    fn empty_file_is_valid() {
        assert!(load("").unwrap().bridges.is_empty());
    }

    #[test]
    fn rejects_bad_files_with_a_reason() {
        let cases = [
            ("not toml ===", "invalid config"),
            (
                &ECU.replace("name = \"ecu\"", "name = \"\""),
                "must not be empty",
            ),
            (
                &format!(
                    "{ECU}{}",
                    ECU.replace("listen = \"[::]:9000\"", "listen = \"[::]:9001\"")
                ),
                "duplicate bridge name",
            ),
            (
                &format!("{ECU}{}", ECU.replace("\"ecu\"", "\"ecu2\"")),
                "share the listen address",
            ),
            (&ECU.replace("/dev/ttyUSB0", "usb:zz:1"), "VID"),
            (
                &ECU.replace("name = \"ecu\"", "name = \"ecu\"\nmax_clients = 0"),
                "max_clients",
            ),
        ];
        for (src, want) in cases {
            let err = format!("{:#}", load(src).unwrap_err());
            assert!(err.contains(want), "{want:?} not in {err:?}");
        }
    }

    #[test]
    fn missing_file_names_the_path() {
        let err = load_config(Path::new("/nonexistent/io.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("/nonexistent/io.toml"));
    }
}
