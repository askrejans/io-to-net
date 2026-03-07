use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use parking_lot::RwLock;
use tokio::signal;
use tracing::{error, info, warn};

mod api;
mod certgen;
mod config;
mod core;
mod drivers;
mod error;
mod protocol;
mod scan;
mod transports;
mod tui;

use config::loader::load_config;
use core::bridge_manager::BridgeManager;
use core::metrics::GlobalMetrics;

/// Resolve the config file path.
///
/// If the user supplied an explicit `--config` value (i.e. it differs from the
/// compiled-in default), use it directly.  Otherwise try, in order:
///   1. `./config.toml`          — current working directory
///   2. `/etc/io-to-net/config.toml` — system-wide default
fn resolve_config_path(cli_path: &PathBuf) -> anyhow::Result<PathBuf> {
    const DEFAULT: &str = "/etc/io-to-net/config.toml";

    // Explicit path supplied by the user — honour it exactly.
    if cli_path != &PathBuf::from(DEFAULT) {
        return Ok(cli_path.clone());
    }

    let candidates: &[&str] = &["config.toml", DEFAULT];
    for p in candidates {
        if std::path::Path::new(p).exists() {
            return Ok(PathBuf::from(p));
        }
    }

    anyhow::bail!(
        "No config file found. Tried:\n  {}\n\nCreate one or pass --config <path>.",
        candidates.join("\n  ")
    )
}

#[derive(Parser)]
#[command(
    name = "io-to-net",
    about = "Universal hardware IO bridge — exposes local hardware interfaces over TCP/IP",
    version
)]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "/etc/io-to-net/config.toml", env = "IO_TO_NET_CONFIG")]
    config: PathBuf,

    /// Run as a background daemon (no TUI)
    #[arg(short, long)]
    daemon: bool,

    /// Scan system for available hardware devices and exit
    #[arg(short, long)]
    scan: bool,

    /// Deep scan: BLE service discovery, I2C address probing (slower)
    #[arg(long, requires = "scan")]
    deep: bool,

    /// Output scan results as JSON
    #[arg(long, requires = "scan")]
    json: bool,

    /// Override log level (trace, debug, info, warn, error)
    #[arg(short, long, env = "IO_TO_NET_LOG_LEVEL")]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a self-signed TLS certificate and private key
    GenerateCert {
        /// Output directory for cert.pem and key.pem
        #[arg(short, long, default_value = ".")]
        out: PathBuf,

        /// Common name (domain / hostname) for the certificate
        #[arg(long, default_value = "io-to-net")]
        cn: String,

        /// Validity in days
        #[arg(long, default_value_t = 365)]
        days: u32,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Handle subcommands before any config loading
    if let Some(Commands::GenerateCert { out, cn, days }) = cli.command {
        certgen::generate_cert(&out, &cn, days)?;
        return Ok(());
    }

    // Initialise logging
    let log_level = cli
        .log_level
        .clone()
        .unwrap_or_else(|| "info".to_string());

    // Device scan mode: plain stdout logging, no ring buffer needed.
    if cli.scan {
        init_logging(&log_level, true, None);
        scan::run_scan(cli.deep, cli.json).await;
        return Ok(());
    }

    // Shared log ring-buffer — created before init_logging so TUI mode can
    // route all tracing output into it instead of writing to stdout.
    let log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>> =
        Arc::new(parking_lot::Mutex::new(std::collections::VecDeque::with_capacity(1000)));

    // In TUI mode pass the ring-buffer so tracing writes there, not stdout.
    init_logging(
        &log_level,
        cli.daemon,
        if cli.daemon { None } else { Some(log_buffer.clone()) },
    );

    // Load configuration
    let config_path = resolve_config_path(&cli.config)?;
    let cfg = load_config(&config_path)
        .with_context(|| format!("Failed to load config from {}", config_path.display()))?;

    info!(version = env!("CARGO_PKG_VERSION"), "io-to-net starting");

    // Shared metrics
    let metrics = Arc::new(RwLock::new(GlobalMetrics::new()));

    // Bridge manager
    let mut bridge_manager = BridgeManager::new(cfg.clone(), metrics.clone(), log_buffer.clone());

    // Start all bridges
    bridge_manager.start_all().await?;

    // Start HTTP admin API if enabled
    if cfg.http_api.as_ref().map_or(false, |a| a.enabled) {
        let api_cfg = cfg.http_api.clone().unwrap();
        let metrics_clone = metrics.clone();
        let log_buffer_clone = log_buffer.clone();
        let bridge_manager_handle = bridge_manager.get_handle();
        tokio::spawn(async move {
            if let Err(e) = api::http::serve(api_cfg, metrics_clone, log_buffer_clone, bridge_manager_handle).await {
                error!(error = %e, "HTTP API server failed");
            }
        });
    }

    // Start Prometheus metrics endpoint if enabled
    if cfg.metrics.as_ref().map_or(false, |m| m.enabled) {
        let metrics_cfg = cfg.metrics.clone().unwrap();
        let metrics_clone = metrics.clone();
        tokio::spawn(async move {
            if let Err(e) = api::prometheus::serve(metrics_cfg, metrics_clone).await {
                error!(error = %e, "Prometheus metrics server failed");
            }
        });
    }

    if cli.daemon {
        // Daemon mode: wait for shutdown signal
        info!("Running in daemon mode");
        wait_for_shutdown().await;
    } else {
        // Interactive TUI mode
        let tui_result = tui::app::run(metrics.clone(), log_buffer.clone(), bridge_manager.get_handle()).await;
        // After TUI exits, clean up
        match tui_result {
            Ok(_) => info!("TUI exited normally"),
            Err(e) => warn!(error = %e, "TUI exited with error"),
        }
    }

    info!("Shutting down bridges");
    bridge_manager.stop_all().await;
    info!("io-to-net stopped");
    Ok(())
}

// ── Ring-buffer tracing writer ────────────────────────────────────────────────

/// `MakeWriter` implementation that appends each formatted log line to the
/// shared ring buffer instead of writing to stdout/stderr.
#[derive(Clone)]
struct RingMakeWriter {
    buffer: Arc<parking_lot::Mutex<VecDeque<String>>>,
}

struct RingWriter {
    buffer: Arc<parking_lot::Mutex<VecDeque<String>>>,
}

impl Write for RingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(s) = std::str::from_utf8(buf) {
            let line = s.trim_end_matches('\n').to_string();
            if !line.is_empty() {
                let mut b = self.buffer.lock();
                if b.len() >= 1000 {
                    b.pop_front();
                }
                b.push_back(line);
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RingMakeWriter {
    type Writer = RingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        RingWriter { buffer: self.buffer.clone() }
    }
}

// ── Logging init ─────────────────────────────────────────────────────────────

fn init_logging(
    level: &str,
    daemon: bool,
    ring: Option<Arc<parking_lot::Mutex<VecDeque<String>>>>,
) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    if daemon {
        // Structured JSON logging for daemon / systemd
        fmt::Subscriber::builder()
            .with_env_filter(filter)
            .json()
            .init();
    } else if let Some(buffer) = ring {
        // TUI mode: write into the ring buffer; nothing reaches stdout.
        fmt::Subscriber::builder()
            .with_env_filter(filter)
            .with_target(false)
            .with_ansi(false)
            .compact()
            .with_writer(RingMakeWriter { buffer })
            .init();
    } else {
        // Plain stdout (scan mode, tests, etc.)
        fmt::Subscriber::builder()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
    }
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => { info!("Received SIGTERM"); }
            _ = sigint.recv() => { info!("Received SIGINT"); }
        }
    }
    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.expect("failed to install Ctrl-C handler");
        info!("Received Ctrl-C");
    }
}
