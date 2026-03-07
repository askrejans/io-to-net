use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;

use crate::protocol::ProtocolHint;

/// Per-bridge runtime metrics.
#[derive(Debug, Clone, Serialize)]
pub struct BridgeMetrics {
    pub name: String,
    pub source_description: String,
    pub transport_listen: String,
    pub clients_connected: usize,
    pub bytes_rx_total: u64,   // from hardware → network
    pub bytes_tx_total: u64,   // from network → hardware
    pub errors: u64,
    #[serde(skip)]
    pub last_byte_time: Option<Instant>,
    /// Bytes/s measured over last interval
    pub rx_rate_bps: f64,
    pub tx_rate_bps: f64,
    #[serde(skip)]
    rate_window_rx: u64,
    #[serde(skip)]
    rate_window_tx: u64,
    #[serde(skip)]
    rate_last_snapshot: Instant,
    pub running: bool,
    /// Best-effort protocol classification from passive observation.
    pub protocol_hint: ProtocolHint,
    /// Detected message/frame rate (messages per second).
    pub message_rate: f64,
    /// Average frame size in bytes.
    pub avg_frame_size: f64,
}

impl BridgeMetrics {
    pub fn new(name: String, source: String, listen: String) -> Self {
        BridgeMetrics {
            name,
            source_description: source,
            transport_listen: listen,
            clients_connected: 0,
            bytes_rx_total: 0,
            bytes_tx_total: 0,
            errors: 0,
            last_byte_time: None,
            rx_rate_bps: 0.0,
            tx_rate_bps: 0.0,
            rate_window_rx: 0,
            rate_window_tx: 0,
            rate_last_snapshot: Instant::now(),
            running: false,
            protocol_hint: ProtocolHint::Unknown,
            message_rate: 0.0,
            avg_frame_size: 0.0,
        }
    }

    pub fn add_rx(&mut self, n: u64) {
        self.bytes_rx_total += n;
        self.rate_window_rx += n;
        self.last_byte_time = Some(Instant::now());
    }

    pub fn add_tx(&mut self, n: u64) {
        self.bytes_tx_total += n;
        self.rate_window_tx += n;
    }

    pub fn increment_errors(&mut self) {
        self.errors += 1;
    }

    /// Update rate calculations — call this periodically (~1s interval).
    pub fn update_rates(&mut self) {
        let elapsed = self.rate_last_snapshot.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            self.rx_rate_bps = self.rate_window_rx as f64 / elapsed;
            self.tx_rate_bps = self.rate_window_tx as f64 / elapsed;
            self.rate_window_rx = 0;
            self.rate_window_tx = 0;
            self.rate_last_snapshot = Instant::now();
        }
    }
}

/// Per-client connection metrics.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionMetrics {
    pub id: String,
    pub client_ip: String,
    pub bridge_name: String,
    pub bytes_rx: u64,
    pub bytes_tx: u64,
    #[serde(skip)]
    pub connected_at: Instant,
    pub connected_seconds: u64,
}

impl ConnectionMetrics {
    pub fn new(id: uuid::Uuid, client_ip: String, bridge_name: String) -> Self {
        ConnectionMetrics {
            id: id.to_string(),
            client_ip,
            bridge_name,
            bytes_rx: 0,
            bytes_tx: 0,
            connected_at: Instant::now(),
            connected_seconds: 0,
        }
    }

    pub fn refresh_uptime(&mut self) {
        self.connected_seconds = self.connected_at.elapsed().as_secs();
    }
}

/// Global metrics aggregator shared across all components.
#[derive(Debug, Default)]
pub struct GlobalMetrics {
    pub bridges: HashMap<String, BridgeMetrics>,
    pub connections: HashMap<String, ConnectionMetrics>,
    pub start_time: Option<Instant>,
}

impl GlobalMetrics {
    pub fn new() -> Self {
        GlobalMetrics {
            bridges: HashMap::new(),
            connections: HashMap::new(),
            start_time: Some(Instant::now()),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.start_time.map_or(0, |t| t.elapsed().as_secs())
    }

    pub fn total_clients(&self) -> usize {
        self.bridges.values().map(|b| b.clients_connected).sum()
    }

    pub fn total_rx_bytes(&self) -> u64 {
        self.bridges.values().map(|b| b.bytes_rx_total).sum()
    }

    pub fn total_tx_bytes(&self) -> u64 {
        self.bridges.values().map(|b| b.bytes_tx_total).sum()
    }

    pub fn total_rx_rate(&self) -> f64 {
        self.bridges.values().map(|b| b.rx_rate_bps).sum()
    }

    pub fn register_bridge(&mut self, name: String, source: String, listen: String) {
        self.bridges.insert(name.clone(), BridgeMetrics::new(name, source, listen));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BridgeMetrics ─────────────────────────────────────────────────────

    #[test]
    fn test_bridge_metrics_accumulation() {
        let mut m = BridgeMetrics::new("test".into(), "serial://x".into(), "[::]:9000".into());
        m.add_rx(100);
        m.add_rx(50);
        assert_eq!(m.bytes_rx_total, 150);
        m.add_tx(30);
        assert_eq!(m.bytes_tx_total, 30);
    }

    #[test]
    fn test_bridge_metrics_error_counter() {
        let mut m = BridgeMetrics::new("e".into(), "s".into(), "t".into());
        assert_eq!(m.errors, 0);
        m.increment_errors();
        m.increment_errors();
        assert_eq!(m.errors, 2);
    }

    #[test]
    fn test_bridge_metrics_new_has_zero_rates() {
        let m = BridgeMetrics::new("b".into(), "s".into(), "t".into());
        assert_eq!(m.rx_rate_bps, 0.0);
        assert_eq!(m.tx_rate_bps, 0.0);
        assert!(!m.running);
        assert!(m.last_byte_time.is_none());
    }

    #[test]
    fn test_bridge_metrics_rate_update() {
        let mut m = BridgeMetrics::new("r".into(), "s".into(), "t".into());
        // Force rate_last_snapshot to be old enough by directly manipulating state via sleep
        std::thread::sleep(std::time::Duration::from_millis(600));
        m.add_rx(1024);
        m.update_rates();
        // Rate should be non-zero after accumulating bytes and waiting ≥0.5s
        assert!(m.rx_rate_bps > 0.0);
        // Rate window resets after update
        m.update_rates(); // elapsed since last snapshot < 0.5s, so no update
        assert!(m.rx_rate_bps > 0.0); // still holds previous value
    }

    #[test]
    fn test_bridge_metrics_last_byte_time_set_on_rx() {
        let mut m = BridgeMetrics::new("b".into(), "s".into(), "t".into());
        assert!(m.last_byte_time.is_none());
        m.add_rx(1);
        assert!(m.last_byte_time.is_some());
        // add_tx does NOT set last_byte_time
        let mut m2 = BridgeMetrics::new("b2".into(), "s".into(), "t".into());
        m2.add_tx(10);
        assert!(m2.last_byte_time.is_none());
    }

    // ── ConnectionMetrics ─────────────────────────────────────────────────

    #[test]
    fn test_connection_metrics_refresh_uptime() {
        let id = uuid::Uuid::new_v4();
        let mut cm = ConnectionMetrics::new(id, "1.2.3.4".into(), "bridge0".into());
        assert_eq!(cm.connected_seconds, 0);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        cm.refresh_uptime();
        assert!(cm.connected_seconds >= 1);
    }

    #[test]
    fn test_connection_metrics_fields() {
        let id = uuid::Uuid::new_v4();
        let cm = ConnectionMetrics::new(id, "10.0.0.1".into(), "ecu".into());
        assert_eq!(cm.client_ip, "10.0.0.1");
        assert_eq!(cm.bridge_name, "ecu");
        assert_eq!(cm.id, id.to_string());
        assert_eq!(cm.bytes_rx, 0);
        assert_eq!(cm.bytes_tx, 0);
    }

    // ── GlobalMetrics ─────────────────────────────────────────────────────

    #[test]
    fn test_global_metrics_uptime() {
        let m = GlobalMetrics::new();
        assert!(m.uptime_seconds() < 5);
    }

    #[test]
    fn test_global_metrics_default_uptime_zero() {
        let m = GlobalMetrics::default();
        assert_eq!(m.uptime_seconds(), 0);
    }

    #[test]
    fn test_global_metrics_register_bridge() {
        let mut m = GlobalMetrics::new();
        m.register_bridge("uart".into(), "serial:///dev/ttyUSB0".into(), "[::]:9000".into());
        assert!(m.bridges.contains_key("uart"));
        assert_eq!(m.total_clients(), 0);
    }

    #[test]
    fn test_global_metrics_aggregation() {
        let mut m = GlobalMetrics::new();
        m.register_bridge("a".into(), "s".into(), "t".into());
        m.register_bridge("b".into(), "s".into(), "t".into());

        m.bridges.get_mut("a").unwrap().add_rx(500);
        m.bridges.get_mut("a").unwrap().clients_connected = 2;
        m.bridges.get_mut("b").unwrap().add_rx(300);
        m.bridges.get_mut("b").unwrap().add_tx(100);
        m.bridges.get_mut("b").unwrap().clients_connected = 1;

        assert_eq!(m.total_rx_bytes(), 800);
        assert_eq!(m.total_tx_bytes(), 100);
        assert_eq!(m.total_clients(), 3);
    }

    #[test]
    fn test_global_metrics_total_rx_rate() {
        let mut m = GlobalMetrics::new();
        m.register_bridge("x".into(), "s".into(), "t".into());
        m.bridges.get_mut("x").unwrap().rx_rate_bps = 1000.0;
        assert_eq!(m.total_rx_rate(), 1000.0);
    }
}
