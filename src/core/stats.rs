//! Shared state read by the TUI, HTTP API and Prometheus: one `BridgeStats`
//! per bridge with atomic counters (no lock on the data path), a short
//! traffic monitor and the client list.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde::Serialize;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::config::BridgeConfig;
use crate::protocol::ProtocolHint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Opening the source.
    Opening,
    /// Source open, data flowing.
    Open,
    /// Waiting to retry after a failure.
    Waiting,
    /// Stopped (auto_restart = false) until restarted.
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Device → clients.
    Rx,
    /// Clients → device.
    Tx,
}

#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    /// Increases by one per chunk; `monitor_since` takes the last one seen.
    pub seq: u64,
    /// Time of the first byte.
    pub ms: u64,
    pub dir: Direction,
    /// Real length; `bytes` keeps at most MONITOR_CHUNK_BYTES.
    pub len: usize,
    pub bytes: Vec<u8>,
    #[serde(skip)]
    updated: u64,
}

#[derive(Debug, Default)]
pub struct Counters {
    pub rx_bytes: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub rx_chunks: AtomicU64,
    pub tx_chunks: AtomicU64,
    pub opens: AtomicU64,
    pub open_failures: AtomicU64,
    pub source_errors: AtomicU64,
    /// Client writes dropped because the source was not open (or its queue full).
    pub tx_dropped: AtomicU64,
    /// Clients disconnected because they could not keep up.
    pub lagged: AtomicU64,
    /// Connections refused by max_clients.
    pub refused: AtomicU64,
    pub auth_failures: AtomicU64,
    /// Clients replaced by a newer connection (exclusive mode).
    pub replaced: AtomicU64,
    pub clients_total: AtomicU64,
}

impl Counters {
    pub fn add(c: &AtomicU64, n: u64) {
        c.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct ClientStats {
    pub id: Uuid,
    pub addr: SocketAddr,
    pub since: Instant,
    pub connected_at_ms: u64,
    /// Bytes sent to this client.
    pub sent: AtomicU64,
    /// Bytes received from this client.
    pub received: AtomicU64,
}

#[derive(Debug)]
struct State {
    status: Status,
    detail: String,
    since: Instant,
    protocol: ProtocolHint,
    frame_rate: f64,
    avg_frame: f64,
    rate: (u64, u64, Instant, f64, f64),
}

const MONITOR_CHUNKS: usize = 200;
const MONITOR_BYTES: usize = 16 * 1024;
const MONITOR_CHUNK_BYTES: usize = 1024;
/// Reads in the same direction this close together are shown as one chunk:
/// USB CDC devices deliver a 130-byte reply in dozens of tiny reads.
const MONITOR_MERGE_MS: u64 = 3;

pub struct BridgeStats {
    pub name: String,
    pub source_kind: &'static str,
    pub source: String,
    pub transport: &'static str,
    pub listen: String,
    pub mode: crate::config::BridgeMode,
    pub read_only: bool,
    pub frame_mode: crate::config::FrameMode,
    pub auth: bool,
    pub max_clients: Option<usize>,
    pub counters: Counters,
    pub restart: Notify,
    state: Mutex<State>,
    clients: Mutex<Vec<Arc<ClientStats>>>,
    /// (chunks, bytes kept, next seq)
    monitor: Mutex<(VecDeque<Chunk>, usize, u64)>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl BridgeStats {
    pub fn new(cfg: &BridgeConfig) -> Self {
        let now = Instant::now();
        BridgeStats {
            name: cfg.name.clone(),
            source_kind: cfg.source.kind(),
            source: cfg.source.describe(),
            transport: cfg.transport.kind(),
            listen: cfg.transport.listen().to_string(),
            mode: cfg.mode,
            read_only: cfg.read_only,
            frame_mode: cfg.frame_mode,
            auth: cfg.auth.is_some(),
            max_clients: cfg.max_clients,
            counters: Counters::default(),
            restart: Notify::new(),
            state: Mutex::new(State {
                status: Status::Opening,
                detail: String::new(),
                since: now,
                protocol: ProtocolHint::Unknown,
                frame_rate: 0.0,
                avg_frame: 0.0,
                rate: (0, 0, now, 0.0, 0.0),
            }),
            clients: Mutex::new(Vec::new()),
            monitor: Mutex::new((VecDeque::new(), 0, 1)),
        }
    }

    pub fn set_status(&self, status: Status, detail: impl Into<String>) {
        let mut s = self.state.lock().unwrap();
        if s.status != status {
            s.since = Instant::now();
        }
        s.status = status;
        s.detail = detail.into();
    }

    pub fn status(&self) -> Status {
        self.state.lock().unwrap().status
    }

    pub fn set_protocol(&self, hint: ProtocolHint, frame_rate: f64, avg_frame: f64) {
        let mut s = self.state.lock().unwrap();
        s.protocol = hint;
        s.frame_rate = frame_rate;
        s.avg_frame = avg_frame;
    }

    /// Recompute byte rates; called about once a second by the bridge.
    pub fn tick(&self) {
        let rx = Counters::get(&self.counters.rx_bytes);
        let tx = Counters::get(&self.counters.tx_bytes);
        let mut s = self.state.lock().unwrap();
        let (prx, ptx, at, _, _) = s.rate;
        let dt = at.elapsed().as_secs_f64();
        if dt >= 0.5 {
            s.rate = (
                rx,
                tx,
                Instant::now(),
                (rx - prx) as f64 / dt,
                (tx - ptx) as f64 / dt,
            );
        }
    }

    pub fn record(&self, dir: Direction, bytes: &Bytes) {
        let now = now_ms();
        let mut guard = self.monitor.lock().unwrap();
        let m = &mut *guard;
        if let Some(last) = m.0.back_mut()
            && last.dir == dir
            && now.saturating_sub(last.updated) <= MONITOR_MERGE_MS
            && last.len + bytes.len() <= MONITOR_CHUNK_BYTES
        {
            last.bytes.extend_from_slice(bytes);
            last.len += bytes.len();
            last.updated = now;
            m.1 += bytes.len();
            return;
        }
        let kept = bytes[..bytes.len().min(MONITOR_CHUNK_BYTES)].to_vec();
        m.1 += kept.len();
        m.0.push_back(Chunk {
            seq: m.2,
            ms: now,
            dir,
            len: bytes.len(),
            bytes: kept,
            updated: now,
        });
        m.2 += 1;
        while m.0.len() > MONITOR_CHUNKS || m.1 > MONITOR_BYTES {
            match m.0.pop_front() {
                Some(old) => m.1 -= old.bytes.len().min(m.1),
                None => break,
            }
        }
    }

    /// Settled monitor chunks after `after_seq` (the newest chunk is held back
    /// while reads may still merge into it).
    pub fn monitor_since(&self, after_seq: u64) -> Vec<Chunk> {
        let now = now_ms();
        self.monitor
            .lock()
            .unwrap()
            .0
            .iter()
            .filter(|c| c.seq > after_seq && now.saturating_sub(c.updated) > MONITOR_MERGE_MS)
            .cloned()
            .collect()
    }

    pub fn add_client(&self, c: Arc<ClientStats>) {
        Counters::add(&self.counters.clients_total, 1);
        self.clients.lock().unwrap().push(c);
    }

    pub fn remove_client(&self, id: Uuid) {
        self.clients.lock().unwrap().retain(|c| c.id != id);
    }

    pub fn snapshot(&self) -> BridgeSnapshot {
        let s = self.state.lock().unwrap();
        let c = &self.counters;
        let clients = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .map(|c| ClientSnapshot {
                id: c.id.to_string(),
                addr: c.addr.to_string(),
                connected_s: c.since.elapsed().as_secs(),
                connected_at_ms: c.connected_at_ms,
                sent_bytes: Counters::get(&c.sent),
                received_bytes: Counters::get(&c.received),
            })
            .collect();
        BridgeSnapshot {
            name: self.name.clone(),
            source_kind: self.source_kind,
            source: self.source.clone(),
            transport: self.transport,
            listen: self.listen.clone(),
            mode: self.mode,
            read_only: self.read_only,
            frame_mode: self.frame_mode,
            auth: self.auth,
            max_clients: self.max_clients,
            status: s.status,
            detail: s.detail.clone(),
            status_for_s: s.since.elapsed().as_secs(),
            protocol: s.protocol.as_str(),
            frame_rate: s.frame_rate,
            avg_frame_bytes: s.avg_frame,
            rx_bytes: Counters::get(&c.rx_bytes),
            tx_bytes: Counters::get(&c.tx_bytes),
            rx_bps: s.rate.3,
            tx_bps: s.rate.4,
            rx_chunks: Counters::get(&c.rx_chunks),
            tx_chunks: Counters::get(&c.tx_chunks),
            opens: Counters::get(&c.opens),
            open_failures: Counters::get(&c.open_failures),
            source_errors: Counters::get(&c.source_errors),
            tx_dropped: Counters::get(&c.tx_dropped),
            lagged: Counters::get(&c.lagged),
            refused: Counters::get(&c.refused),
            auth_failures: Counters::get(&c.auth_failures),
            replaced: Counters::get(&c.replaced),
            clients_total: Counters::get(&c.clients_total),
            clients,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientSnapshot {
    pub id: String,
    pub addr: String,
    pub connected_s: u64,
    pub connected_at_ms: u64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeSnapshot {
    pub name: String,
    pub source_kind: &'static str,
    pub source: String,
    pub transport: &'static str,
    pub listen: String,
    pub mode: crate::config::BridgeMode,
    pub read_only: bool,
    pub frame_mode: crate::config::FrameMode,
    pub auth: bool,
    pub max_clients: Option<usize>,
    pub status: Status,
    /// Resolved source when open, the error while waiting.
    pub detail: String,
    pub status_for_s: u64,
    pub protocol: &'static str,
    pub frame_rate: f64,
    pub avg_frame_bytes: f64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_chunks: u64,
    pub tx_chunks: u64,
    pub opens: u64,
    pub open_failures: u64,
    pub source_errors: u64,
    pub tx_dropped: u64,
    pub lagged: u64,
    pub refused: u64,
    pub auth_failures: u64,
    pub replaced: u64,
    pub clients_total: u64,
    pub clients: Vec<ClientSnapshot>,
}

/// Last log lines for the TUI and API.
pub struct LogRing {
    lines: Mutex<VecDeque<String>>,
    cap: usize,
}

impl LogRing {
    pub fn new(cap: usize) -> Self {
        LogRing {
            lines: Mutex::new(VecDeque::with_capacity(cap)),
            cap,
        }
    }
    pub fn push(&self, line: String) {
        let mut l = self.lines.lock().unwrap();
        if l.len() >= self.cap {
            l.pop_front();
        }
        l.push_back(line);
    }
    /// Newest last.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let l = self.lines.lock().unwrap();
        l.iter().skip(l.len().saturating_sub(n)).cloned().collect()
    }
}

pub struct Registry {
    pub started: Instant,
    pub config_path: Mutex<Option<String>>,
    /// Effective configuration with secrets masked (API and dashboard).
    pub config: Mutex<serde_json::Value>,
    pub logs: LogRing,
    bridges: RwLock<Vec<Arc<BridgeStats>>>,
}

impl Registry {
    pub fn new() -> Arc<Self> {
        Arc::new(Registry {
            started: Instant::now(),
            config_path: Mutex::new(None),
            config: Mutex::new(serde_json::Value::Null),
            logs: LogRing::new(1000),
            bridges: RwLock::new(Vec::new()),
        })
    }
    pub fn set_bridges(&self, b: Vec<Arc<BridgeStats>>) {
        *self.bridges.write().unwrap() = b;
    }
    pub fn bridges(&self) -> Vec<Arc<BridgeStats>> {
        self.bridges.read().unwrap().clone()
    }
    pub fn bridge(&self, name: &str) -> Option<Arc<BridgeStats>> {
        self.bridges
            .read()
            .unwrap()
            .iter()
            .find(|b| b.name == name)
            .cloned()
    }
    pub fn uptime_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BridgeConfig {
        toml::from_str(
            "name = \"ecu\"\n[source]\ntype = \"serial\"\nport = \"/dev/ttyUSB0\"\n[transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap()
    }

    #[test]
    fn snapshot_reflects_counters_and_status() {
        let b = BridgeStats::new(&cfg());
        Counters::add(&b.counters.rx_bytes, 130);
        b.set_status(Status::Open, "serial:///dev/ttyUSB0@115200");
        let s = b.snapshot();
        assert_eq!(
            (s.rx_bytes, s.status, s.source_kind, s.transport),
            (130, Status::Open, "serial", "tcp")
        );
        assert_eq!(s.detail, "serial:///dev/ttyUSB0@115200");
    }

    #[test]
    fn monitor_merges_fragmented_reads() {
        let b = BridgeStats::new(&cfg());
        b.record(Direction::Tx, &Bytes::from_static(b"A"));
        for _ in 0..65 {
            b.record(Direction::Rx, &Bytes::from_static(b"\x01\x02"));
        }
        assert!(
            b.monitor_since(0).len() <= 1,
            "newest chunk is held back while it may grow"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
        let all = b.monitor_since(0);
        assert_eq!(all.len(), 2);
        assert_eq!((all[1].dir, all[1].len), (Direction::Rx, 130));
        assert!(b.monitor_since(all[1].seq).is_empty());
    }

    #[test]
    fn monitor_keeps_recent_traffic_within_bounds() {
        let b = BridgeStats::new(&cfg());
        for i in 0..500u32 {
            // alternate directions so reads do not merge
            let dir = if i % 2 == 0 {
                Direction::Rx
            } else {
                Direction::Tx
            };
            b.record(dir, &Bytes::from(i.to_le_bytes().to_vec()));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        let all = b.monitor_since(0);
        assert_eq!(all.len(), MONITOR_CHUNKS);
        assert_eq!(all.last().unwrap().bytes, 499u32.to_le_bytes());
    }

    #[test]
    fn log_ring_is_bounded() {
        let r = LogRing::new(3);
        for i in 0..5 {
            r.push(i.to_string());
        }
        assert_eq!(r.tail(10), vec!["2", "3", "4"]);
    }
}
