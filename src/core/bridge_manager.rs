use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, trace, warn};

/// Initial retry delay.
const BACKOFF_BASE_SECS: u64 = 5;
/// Maximum retry delay cap.
const BACKOFF_MAX_SECS: u64 = 300;
/// Stop retrying after this many consecutive failures.
const MAX_CONSECUTIVE_FAILURES: u32 = 8;
/// A run is considered healthy (resets backoff) if it lasted at least this long.
const HEALTHY_RUN_SECS: u64 = 30;

use crate::config::{AppConfig, BridgeConfig, TransportConfig};
use crate::core::connection_manager::{BridgeHandle, ConnectionManager};
use crate::core::metrics::GlobalMetrics;
use crate::drivers::create_driver;
use crate::protocol::ProtocolDetector;
use crate::transports::{tcp, tls, websocket};
use crate::error::Result;

/// Manages the lifecycle of all configured bridges.
pub struct BridgeManager {
    cfg: AppConfig,
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
    handles: HashMap<String, BridgeHandle>,
    shutdown_txs: Vec<broadcast::Sender<()>>,
}

/// A cloneable reference used by the HTTP API to request bridge restarts.
#[derive(Clone)]
pub struct BridgeManagerHandle {
    pub handles: Arc<parking_lot::Mutex<HashMap<String, BridgeHandle>>>,
}

impl BridgeManager {
    pub fn new(
        cfg: AppConfig,
        metrics: Arc<RwLock<GlobalMetrics>>,
        log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
    ) -> Self {
        BridgeManager {
            cfg,
            metrics,
            log_buffer,
            handles: HashMap::new(),
            shutdown_txs: Vec::new(),
        }
    }

    /// Returns an external handle for use by the HTTP API.
    pub fn get_handle(&self) -> BridgeManagerHandle {
        let map = self
            .handles
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        BridgeManagerHandle {
            handles: Arc::new(parking_lot::Mutex::new(map)),
        }
    }

    /// Start all configured bridges as async tasks.
    pub async fn start_all(&mut self) -> anyhow::Result<()> {
        for bridge_cfg in self.cfg.bridges.clone() {
            self.start_bridge(bridge_cfg).await?;
        }
        Ok(())
    }

    async fn start_bridge(&mut self, bridge_cfg: BridgeConfig) -> anyhow::Result<()> {
        let name = bridge_cfg.name.clone();
        let metrics = self.metrics.clone();
        let log_buffer = self.log_buffer.clone();

        // Determine source description and listen address for metrics
        let source_desc = source_description(&bridge_cfg);
        let listen_addr = transport_listen(&bridge_cfg.transport);

        {
            let mut m = metrics.write();
            m.register_bridge(name.clone(), source_desc, listen_addr);
        }

        let (shutdown_tx, _) = broadcast::channel::<()>(1);
        let (restart_tx, mut restart_rx) = mpsc::channel::<()>(4);
        self.shutdown_txs.push(shutdown_tx.clone());

        let handle = BridgeHandle { name: name.clone(), restart_tx };
        self.handles.insert(name.clone(), handle);

        let metrics_clone = metrics.clone();
        let log_buffer_clone = log_buffer.clone();

        // Outer loop: restart bridge on failure if auto_restart = true
        tokio::spawn(async move {
            let mut consecutive_failures: u32 = 0;
            let mut backoff_secs = BACKOFF_BASE_SECS;

            loop {
                info!(bridge = %name, "Bridge starting");
                {
                    let mut m = metrics_clone.write();
                    if let Some(bm) = m.bridges.get_mut(&name) {
                        bm.running = true;
                    }
                }

                let run_started = Instant::now();
                let result = run_bridge(
                    bridge_cfg.clone(),
                    metrics_clone.clone(),
                    log_buffer_clone.clone(),
                    shutdown_tx.subscribe(),
                )
                .await;
                let run_duration = run_started.elapsed();

                {
                    let mut m = metrics_clone.write();
                    if let Some(bm) = m.bridges.get_mut(&name) {
                        bm.running = false;
                        if result.is_err() {
                            bm.increment_errors();
                        }
                    }
                }

                match &result {
                    Err(e) => error!(bridge = %name, error = %e, "Bridge task failed"),
                    Ok(_) => info!(bridge = %name, "Bridge task ended"),
                }

                if !bridge_cfg.auto_restart {
                    break;
                }

                if result.is_err() {
                    if run_duration.as_secs() >= HEALTHY_RUN_SECS {
                        // Was healthy for a while before failing — reset backoff.
                        consecutive_failures = 0;
                        backoff_secs = BACKOFF_BASE_SECS;
                    } else {
                        consecutive_failures += 1;
                    }

                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        error!(
                            bridge = %name,
                            attempts = consecutive_failures,
                            "Bridge disabled for this session after too many consecutive failures"
                        );
                        let mut m = metrics_clone.write();
                        if let Some(bm) = m.bridges.get_mut(&name) {
                            bm.set_offline();
                        }
                        break;
                    }

                    warn!(
                        bridge = %name,
                        attempt = consecutive_failures,
                        delay_secs = backoff_secs,
                        "Bridge restarting after failure"
                    );

                    // Wait for restart signal or backoff delay
                    tokio::select! {
                        _ = restart_rx.recv() => {
                            info!(bridge = %name, "Bridge restart requested — resetting backoff");
                            consecutive_failures = 0;
                            backoff_secs = BACKOFF_BASE_SECS;
                        }
                        _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                    }

                    // Exponential backoff, capped at max
                    backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
                } else {
                    // Clean exit (e.g. shutdown signal) — reset state and wait for restart signal
                    consecutive_failures = 0;
                    backoff_secs = BACKOFF_BASE_SECS;
                    tokio::select! {
                        _ = restart_rx.recv() => {
                            info!(bridge = %name, "Bridge restart requested");
                        }
                        _ = tokio::time::sleep(Duration::from_secs(BACKOFF_BASE_SECS)) => {
                            warn!(bridge = %name, "Bridge restarting");
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn stop_all(&mut self) {
        for tx in &self.shutdown_txs {
            let _ = tx.send(());
        }
        // Brief wait for graceful shutdown
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Inner driver retry backoff: increments failure count, returns wait duration in ms.
fn driver_backoff(failure_count: &mut u32, backoff_ms: &mut u64) -> u64 {
    *failure_count += 1;
    if *failure_count >= MAX_CONSECUTIVE_FAILURES {
        *failure_count = 0;
        *backoff_ms = 1_000;
        30_000
    } else {
        let w = *backoff_ms;
        *backoff_ms = (*backoff_ms * 2).min(30_000);
        w
    }
}

/// Core bridge task: drives the IO source and forwards data to all clients.
///
/// The transport (client accept), client intake, and hardware driver are fully
/// independent. Clients can always connect regardless of driver state, and
/// existing connections survive driver restarts.
async fn run_bridge(
    cfg: BridgeConfig,
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<()> {
    use crate::transports::ClientHandle;

    let bridge_name = cfg.name.clone();
    let read_only = cfg.read_only;
    let max_clients = cfg.max_clients;

    // Channel: transport → bridge (new client connections)
    let (new_client_tx, new_client_rx) = mpsc::channel::<ClientHandle>(32);
    // Shared channel: all client writes are funnelled here; driver loop drains & forwards
    let (device_write_tx, mut device_write_rx) = mpsc::channel::<Bytes>(1024);

    // Connection manager shared between client intake task and driver loop.
    // Arc<Mutex> so both can access it independently.
    let conn_mgr = Arc::new(parking_lot::Mutex::new(ConnectionManager::new(
        bridge_name.clone(),
        metrics.clone(),
        log_buffer,
    )));

    // ── Transport ────────────────────────────────────────────────────────────
    // One shutdown channel for the transport listener — dropped when run_bridge ends.
    let (transport_sd_tx, _) = broadcast::channel::<()>(1);
    let transport_cfg = cfg.transport.clone();

    // Probe-bind to catch misconfiguration early (listener is dropped immediately).
    {
        let probe = match &transport_cfg {
            TransportConfig::Tcp(c) => tokio::net::TcpListener::bind(&c.listen).await
                .map_err(|e| crate::error::IoToNetError::Transport(
                    format!("TCP bind {} failed: {}", c.listen, e))),
            TransportConfig::Tls(c) => tokio::net::TcpListener::bind(&c.listen).await
                .map_err(|e| crate::error::IoToNetError::Transport(
                    format!("TLS bind {} failed: {}", c.listen, e))),
            TransportConfig::WebSocket(c) => tokio::net::TcpListener::bind(&c.listen).await
                .map_err(|e| crate::error::IoToNetError::Transport(
                    format!("WS bind {} failed: {}", c.listen, e))),
        };
        if let Err(e) = probe {
            error!(bridge = %bridge_name, error = %e, "Transport bind failed");
            return Err(e);
        }
    } // probe listener dropped here — port is free for the real listener

    let sd_rx = transport_sd_tx.subscribe();
    let tx_clone = new_client_tx.clone();
    tokio::spawn(async move {
        match transport_cfg {
            TransportConfig::Tcp(c)        => { let _ = tcp::run_tcp_listener(c, tx_clone, max_clients, sd_rx).await; }
            TransportConfig::Tls(c)        => { let _ = tls::run_tls_listener(c, tx_clone, max_clients, sd_rx).await; }
            TransportConfig::WebSocket(c)  => { let _ = websocket::run_ws_listener(c, tx_clone, max_clients, sd_rx).await; }
        }
    });
    // transport_sd_tx kept alive until run_bridge returns — dropping it stops the listener.

    // ── Client intake task ───────────────────────────────────────────────────
    // Completely independent of driver state. Clients are accepted at all times.
    {
        let conn_mgr = Arc::clone(&conn_mgr);
        let dw_tx = device_write_tx.clone();
        let name = bridge_name.clone();
        tokio::spawn(async move {
            let mut new_client_rx = new_client_rx;
            while let Some(client) = new_client_rx.recv().await {
                info!(
                    bridge = %name,
                    addr  = %client.addr,
                    id    = %client.id,
                    "Client connected"
                );
                conn_mgr.lock().add_client(
                    client.id,
                    client.addr,
                    client.tx,
                    client.rx,
                    dw_tx.clone(),
                    read_only,
                );
            }
            debug!(bridge = %name, "Client intake task ended");
        });
    }

    // ── Driver loop ──────────────────────────────────────────────────────────
    // Retries hardware open with exponential backoff. During backoff the transport
    // and client intake keep running so clients can connect and stay connected.
    let mut buf = [0u8; 4096];
    let mut detector = ProtocolDetector::new();
    let mut last_hint = crate::protocol::ProtocolHint::Unknown;
    let mut failure_count: u32 = 0;
    let mut backoff_ms: u64 = 1_000;

    'outer: loop {
        // Respect shutdown signal
        if shutdown.try_recv().is_ok() {
            info!(bridge = %bridge_name, "Shutdown received");
            break;
        }

        // Create driver
        let mut driver = match create_driver(&cfg.source) {
            Ok(d) => d,
            Err(e) => {
                let wait = driver_backoff(&mut failure_count, &mut backoff_ms);
                error!(bridge = %bridge_name, error = %e, wait_ms = wait, "Driver creation failed");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                    _ = shutdown.recv() => { break 'outer; }
                }
                continue;
            }
        };

        // Open driver
        match driver.open().await {
            Ok(_) => {
                info!(bridge = %bridge_name, "Hardware driver opened");
                failure_count = 0;
                backoff_ms = 1_000;
            }
            Err(e) => {
                let wait = driver_backoff(&mut failure_count, &mut backoff_ms);
                error!(bridge = %bridge_name, error = %e, wait_ms = wait, "Driver open failed");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                    _ = shutdown.recv() => { break 'outer; }
                }
                continue;
            }
        }

        // Hardware read loop
        info!(bridge = %bridge_name, "Hardware data loop started");
        loop {
            if shutdown.try_recv().is_ok() {
                info!(bridge = %bridge_name, "Shutdown received during read loop");
                break 'outer;
            }

            // Forward any pending client writes to hardware before reading.
            if !read_only {
                while let Ok(data) = device_write_rx.try_recv() {
                    debug!(bridge = %bridge_name, bytes = data.len(), "Forwarding client write to hardware");
                    if let Err(e) = driver.write(&data).await {
                        warn!(bridge = %bridge_name, error = %e, "Hardware write failed");
                    }
                }
            } else {
                // Drain and discard — keeps the channel from filling up.
                while device_write_rx.try_recv().is_ok() {}
            }

            match driver.read(&mut buf).await {
                Err(e) => {
                    let wait = driver_backoff(&mut failure_count, &mut backoff_ms);
                    error!(bridge = %bridge_name, error = %e, wait_ms = wait, "Hardware read error — going offline");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                        _ = shutdown.recv() => { break 'outer; }
                    }
                    break; // retry driver open
                }
                Ok(0) => {
                    trace!(bridge = %bridge_name, "Hardware read: no data (timeout)");
                }
                Ok(n) => {
                    trace!(bridge = %bridge_name, bytes = n, "Hardware data received");

                    // Protocol detection (locks metrics only when hint changes).
                    detector.feed(&buf[..n]);
                    let hint = detector.hint().clone();
                    if hint != last_hint {
                        last_hint = hint.clone();
                        let stats = detector.stats();
                        info!(bridge = %bridge_name, protocol = %hint, "Protocol detected");
                        let mut m = metrics.write();
                        if let Some(bm) = m.bridges.get_mut(&bridge_name) {
                            bm.protocol_hint = hint;
                            bm.message_rate = stats.message_rate;
                            bm.avg_frame_size = stats.avg_frame_size;
                        }
                    }

                    // Broadcast to all subscribed client tasks.
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    conn_mgr.lock().broadcast(data);
                    trace!(bridge = %bridge_name, bytes = n, "Broadcast sent");
                }
            }
        }
    }

    // Dropping transport_sd_tx here signals the transport listener to stop.
    drop(transport_sd_tx);
    Ok(())
}

fn source_description(cfg: &BridgeConfig) -> String {
    use crate::config::SourceConfig;
    match &cfg.source {
        SourceConfig::Serial(s) => format!("serial://{}@{}", s.port, s.baud),
        SourceConfig::I2c(s) => format!("i2c://{}@0x{:02X}", s.bus, s.address),
        SourceConfig::Spi(s) => format!("spi://{}", s.device),
        SourceConfig::Gpio(s) => format!("gpio://{}:{}", s.chip, s.line),
        SourceConfig::Bluetooth(s) => format!("bluetooth://{}", s.device),
        SourceConfig::Canbus(s) => format!("canbus://{}", s.interface),
        SourceConfig::Onewire(s) => format!("onewire://{}", s.device),
        SourceConfig::Pty(_) => "pty://master".to_string(),
        SourceConfig::UnixSocket(s) => format!("unix://{}", s.path),
    }
}

fn transport_listen(cfg: &TransportConfig) -> String {
    match cfg {
        TransportConfig::Tcp(c) => c.listen.clone(),
        TransportConfig::Tls(c) => c.listen.clone(),
        TransportConfig::WebSocket(c) => c.listen.clone(),
    }
}
