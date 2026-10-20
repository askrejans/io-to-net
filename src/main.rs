use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::sync::watch;
use tracing::{error, info, warn};

mod api;
mod certgen;
mod config;
mod core;
mod drivers;
mod error;
mod link;
mod protocol;
mod scan;
mod transports;
mod tui;

use crate::config::loader::{load_config, resolve_path};
use crate::core::manager::{self, Manager};
use crate::core::stats::Registry;

#[derive(Parser)]
#[command(
    name = "io-to-net",
    version,
    about = "Transparent bridge from serial, CAN, I2C, GPIO, BLE and more to TCP, TLS and WebSocket clients"
)]
struct Cli {
    /// Config file (default: ./config.toml, then /etc/io-to-net/config.toml)
    #[arg(short, long, env = "IO_TO_NET_CONFIG")]
    config: Option<PathBuf>,

    /// No terminal UI; log to stdout (also when stdout is not a terminal)
    #[arg(long, visible_alias = "daemon", short = 'd')]
    headless: bool,

    /// Log as JSON lines (headless only)
    #[arg(long)]
    log_json: bool,

    /// trace, debug, info, warn or error (overrides log_level in the config)
    #[arg(short, long, env = "IO_TO_NET_LOG_LEVEL")]
    log_level: Option<String>,

    /// List serial ports (with USB selectors), I2C buses, GPIO chips and BLE devices
    #[arg(short, long)]
    scan: bool,

    /// With --scan: probe I2C buses and discover BLE services (slower)
    #[arg(long, requires = "scan")]
    deep: bool,

    /// With --scan: print JSON
    #[arg(long, requires = "scan")]
    json: bool,

    /// Validate the configuration and exit
    #[arg(long)]
    check_config: bool,

    /// Query the running instance's /health and exit 0 when every source is open
    #[arg(long)]
    healthcheck: bool,

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
        /// Common name (host name) for the certificate
        #[arg(long, default_value = "io-to-net")]
        cn: String,
        /// Validity in days
        #[arg(long, default_value_t = 365)]
        days: u32,
    },
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("io-to-net: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    if let Some(Commands::GenerateCert { out, cn, days }) = cli.command {
        return certgen::generate_cert(&out, &cn, days);
    }
    if cli.scan {
        init_logging(
            None,
            cli.log_level.as_deref().unwrap_or("warn"),
            true,
            false,
        );
        scan::run_scan(cli.deep, cli.json).await;
        return Ok(());
    }

    let path = resolve_path(cli.config.as_deref())?;
    let cfg = load_config(&path)?;
    let api_cfg = cfg.http_api.clone().unwrap_or_default();

    if cli.check_config {
        println!("{}: OK, {} bridge(s)", path.display(), cfg.bridges.len());
        for b in &cfg.bridges {
            println!(
                "  {:<16} {} → {} {}",
                b.name,
                b.source.describe(),
                b.transport.kind(),
                b.transport.listen()
            );
        }
        return Ok(());
    }
    if cli.healthcheck {
        return healthcheck(&api_cfg.listen).await;
    }

    let tui = !cli.headless && std::io::stdout().is_terminal();
    let reg = Registry::new();
    init_logging(
        Some(reg.clone()),
        cli.log_level.as_deref().unwrap_or(&cfg.log_level),
        !tui,
        cli.log_json,
    );
    *reg.config_path.lock().unwrap() = Some(path.display().to_string());
    *reg.config.lock().unwrap() = config::masked(&cfg);

    info!(version = env!("CARGO_PKG_VERSION"), config = %path.display(), "io-to-net starting");
    if cfg.bridges.is_empty() {
        warn!("no bridges configured; add [[bridges]] and reload (SIGHUP or POST /api/v1/reload)");
    }

    let mut mgr = Manager::new(reg.clone());
    mgr.apply(&cfg.bridges).await;

    let (stop_tx, stop_rx) = watch::channel(false);
    let stopped = |mut rx: watch::Receiver<bool>| async move {
        let _ = rx.wait_for(|v| *v).await;
    };
    let (control, mgr_task) = manager::spawn(mgr, path, stopped(stop_rx.clone()));

    if api_cfg.enabled {
        let (reg, control, stop) = (reg.clone(), control.clone(), stopped(stop_rx.clone()));
        tokio::spawn(async move {
            if let Err(e) = api::serve(api_cfg, reg, control, stop).await {
                error!(error = %format!("{e:#}"), "HTTP API stopped");
            }
        });
    }
    if let Some(m) = cfg.metrics.clone().filter(|m| m.enabled) {
        let (reg, stop) = (reg.clone(), stopped(stop_rx.clone()));
        tokio::spawn(async move {
            if let Err(e) = api::serve_metrics(m.listen, reg, stop).await {
                error!(error = %format!("{e:#}"), "metrics listener stopped");
            }
        });
    }

    #[cfg(unix)]
    {
        let control = control.clone();
        tokio::spawn(async move {
            let Ok(mut hup) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            else {
                return;
            };
            while hup.recv().await.is_some() {
                info!("SIGHUP: reloading configuration");
                let _ = control.reload().await;
            }
        });
    }

    if tui {
        let (reg, control, rt) = (
            reg.clone(),
            control.clone(),
            tokio::runtime::Handle::current(),
        );
        let ui = tokio::task::spawn_blocking(move || tui::app::run(reg, control, rt));
        tokio::select! {
            r = ui => match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "terminal UI failed"),
                Err(e) => warn!(error = %e, "terminal UI panicked"),
            },
            _ = terminate() => {}
        }
    } else {
        terminate().await;
    }

    info!("shutting down");
    let _ = stop_tx.send(true);
    let _ = mgr_task.await;
    info!("io-to-net stopped");
    Ok(())
}

async fn terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = term.recv() => info!("SIGTERM received"),
            _ = int.recv() => info!("SIGINT received"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        info!("Ctrl-C received");
    }
}

/// GET /health on the configured API address (wildcards become loopback).
async fn healthcheck(listen: &str) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut addr: std::net::SocketAddr = listen
        .parse()
        .with_context(|| format!("http_api.listen {listen}"))?;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let fut = async {
        let mut s = tokio::net::TcpStream::connect(addr).await?;
        s.write_all(format!("GET /health HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes())
            .await?;
        let mut out = String::new();
        s.read_to_string(&mut out).await?;
        Ok::<_, std::io::Error>(out)
    };
    let resp = tokio::time::timeout(std::time::Duration::from_secs(5), fut)
        .await
        .context("health check timed out")?
        .with_context(|| format!("cannot reach {addr}"))?;
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    println!("{body}");
    if resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        anyhow::bail!("unhealthy")
    }
}

/// Logs always go to the registry ring (TUI, API); headless also to stdout.
fn init_logging(ring: Option<Arc<Registry>>, level: &str, stdout: bool, json: bool) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let ring_layer = ring.map(|reg| {
        fmt::layer()
            .with_ansi(false)
            .with_target(false)
            .compact()
            .with_writer(RingWriter(reg))
    });
    let (text, json_layer) = match (stdout, json) {
        (false, _) => (None, None),
        (true, false) => (
            Some(
                fmt::layer()
                    .with_target(false)
                    .with_ansi(std::io::stdout().is_terminal())
                    .compact(),
            ),
            None,
        ),
        (true, true) => (None, Some(fmt::layer().json())),
    };
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(ring_layer)
        .with(text)
        .with(json_layer)
        .try_init();
}

#[derive(Clone)]
struct RingWriter(Arc<Registry>);

impl std::io::Write for RingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buf);
        let line = line.trim_end();
        if !line.is_empty() {
            self.0.logs.push(line.to_string());
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RingWriter {
    type Writer = RingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
