use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

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

/// Core bridge task: drives the IO source and forwards data to all clients.
async fn run_bridge(
    cfg: BridgeConfig,
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
    shutdown: broadcast::Receiver<()>,
) -> Result<()> {
    use crate::transports::ClientHandle;

    let mut driver = create_driver(&cfg.source)?;
    driver.open().await?;

    // Channel: new clients arrive from the transport
    let (new_client_tx, mut new_client_rx) = mpsc::channel::<ClientHandle>(32);
    // Channel: device write requests from clients
    let (device_write_tx, mut device_write_rx) = mpsc::channel::<Bytes>(1024);

    let mut conn_mgr = ConnectionManager::new(
        cfg.name.clone(),
        metrics.clone(),
        log_buffer.clone(),
    );

    let max_clients = cfg.max_clients;
    let read_only = cfg.read_only;

    // Spawn transport listener in a separate task, propagate bind errors immediately
    let transport_cfg = cfg.transport.clone();
    let tx_clone = new_client_tx.clone();
    let _shutdown_rx = shutdown;
    let (sd_tx, _) = broadcast::channel::<()>(1);
    let sd_rx = sd_tx.subscribe();

    // Try binding transport listener, propagate errors
    let bind_result = match &transport_cfg {
        TransportConfig::Tcp(tcp_cfg) => {
            tokio::net::TcpListener::bind(&tcp_cfg.listen).await.map_err(|e| {
                error!(bridge = %cfg.name, error = %e, "TCP bind failed");
                crate::error::IoToNetError::Transport(format!("TCP bind {} failed: {}", tcp_cfg.listen, e))
            })
        }
        TransportConfig::Tls(tls_cfg) => {
            tokio::net::TcpListener::bind(&tls_cfg.listen).await.map_err(|e| {
                error!(bridge = %cfg.name, error = %e, "TLS bind failed");
                crate::error::IoToNetError::Transport(format!("TLS bind {} failed: {}", tls_cfg.listen, e))
            })
        }
        TransportConfig::WebSocket(ws_cfg) => {
            tokio::net::TcpListener::bind(&ws_cfg.listen).await.map_err(|e| {
                error!(bridge = %cfg.name, error = %e, "WebSocket bind failed");
                crate::error::IoToNetError::Transport(format!("WebSocket bind {} failed: {}", ws_cfg.listen, e))
            })
        }
    };
    if let Err(e) = bind_result {
        return Err(e);
    }
    // Spawn actual transport listener
    tokio::spawn(async move {
        match transport_cfg {
            TransportConfig::Tcp(tcp_cfg) => {
                let _ = tcp::run_tcp_listener(tcp_cfg, tx_clone, max_clients, sd_rx).await;
            }
            TransportConfig::Tls(tls_cfg) => {
                let _ = tls::run_tls_listener(tls_cfg, tx_clone, max_clients, sd_rx).await;
            }
            TransportConfig::WebSocket(ws_cfg) => {
                let _ = websocket::run_ws_listener(ws_cfg, tx_clone, max_clients, sd_rx).await;
            }
        }
    });
    // Hold sd_tx alive so listener gets shutdown when we drop it on bridge end
    let _ = sd_tx; // keep alive; will drop at task end

    let mut buf = [0u8; 4096];
    let mut detector = ProtocolDetector::new();
    let mut failure_count = 0;
    let mut backoff_ms = 500;
    const FAILURE_LIMIT: u32 = 8;

    loop {
        // Try to open hardware driver, retry with backoff if fails
        let mut driver = match create_driver(&cfg.source) {
            Ok(d) => d,
            Err(e) => {
                error!(bridge = %cfg.name, error = %e, "Driver creation failed");
                failure_count += 1;
                if failure_count >= FAILURE_LIMIT {
                    warn!(bridge = %cfg.name, failures = failure_count, "Too many consecutive driver failures, increasing backoff");
                    tokio::time::sleep(std::time::Duration::from_millis(30000)).await;
                    failure_count = 0;
                    backoff_ms = 500;
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(10000);
                }
                continue;
            }
        };
        match driver.open().await {
            Ok(_) => {
                failure_count = 0;
                backoff_ms = 500;
            }
            Err(e) => {
                error!(bridge = %cfg.name, error = %e, "Driver open failed");
                failure_count += 1;
                if failure_count >= FAILURE_LIMIT {
                    warn!(bridge = %cfg.name, failures = failure_count, "Too many consecutive driver failures, increasing backoff");
                    tokio::time::sleep(std::time::Duration::from_millis(30000)).await;
                    failure_count = 0;
                    backoff_ms = 500;
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(10000);
                }
                continue;
            }
        }

        // Main bridge loop: process clients and hardware reads
        loop {
            tokio::select! {
                // New client connected via transport
                client = new_client_rx.recv() => {
                    if let Some(client) = client {
                        let dw_tx = device_write_tx.clone();
                        conn_mgr.add_client(
                            client.id,
                            client.addr,
                            client.tx,
                            client.rx,
                            dw_tx,
                            read_only,
                        );
                    }
                }

                // Data from driver (hardware → broadcast to clients)
                read_result = driver.read(&mut buf) => {
                    match read_result {
                        Err(e) => {
                            error!(bridge = %cfg.name, error = %e, "Driver read error");
                            failure_count += 1;
                            if failure_count >= FAILURE_LIMIT {
                                warn!(bridge = %cfg.name, failures = failure_count, "Too many consecutive driver failures, increasing backoff");
                                tokio::time::sleep(std::time::Duration::from_millis(30000)).await;
                                failure_count = 0;
                                backoff_ms = 500;
                            } else {
                                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                                backoff_ms = (backoff_ms * 2).min(10000);
                            }
                            break; // Restart driver
                        }
                        Ok(0) => {}  // timeout, no data
                        Ok(n) => {
                            let data = Bytes::copy_from_slice(&buf[..n]);
                            // Side-channel protocol observer — never modifies data.
                            detector.feed(&buf[..n]);
                            // Publish updated hint to metrics.
                            {
                                let stats = detector.stats();
                                let mut m = metrics.write();
                                if let Some(bm) = m.bridges.get_mut(&cfg.name) {
                                    bm.add_rx(n as u64);
                                    bm.protocol_hint = detector.hint().clone();
                                    bm.message_rate = stats.message_rate;
                                    bm.avg_frame_size = stats.avg_frame_size;
                                }
                            }
                            conn_mgr.broadcast(data);
                        }
                    }
                }

                // Data from client (client → hardware device)
                write_data = device_write_rx.recv() => {
                    if let Some(data) = write_data {
                        if !read_only {
                            if let Err(e) = driver.write(&data).await {
                                warn!(bridge = %cfg.name, error = %e, "Driver write error");
                            }
                        }
                    }
                }
            }
        }
    }
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
