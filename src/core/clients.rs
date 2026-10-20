//! Network clients of one bridge: admission (token, client limit, exclusive
//! takeover), fan-out of device data, and forwarding client writes.
//!
//! Every client has its own bounded queue. A client that falls a full queue
//! behind is disconnected instead of slowing the device or other clients.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::stats::{BridgeStats, ClientStats, Counters, now_ms};
use crate::config::BridgeMode;
use crate::transports::{ClientReader, Upgrader};

/// Chunks queued per client before it counts as lagging.
pub const CLIENT_QUEUE: usize = 1024;
/// Time to send the token line.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TOKEN_LINE: usize = 512;

struct Slot {
    id: Uuid,
    addr: SocketAddr,
    tx: mpsc::Sender<Bytes>,
}

pub struct Hub {
    stats: Arc<BridgeStats>,
    slots: Mutex<Vec<Slot>>,
    closed: AtomicBool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Full,
    Closed,
}

impl Hub {
    pub fn new(stats: Arc<BridgeStats>) -> Arc<Self> {
        Arc::new(Hub {
            stats,
            slots: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        })
    }

    pub fn admit(&self, client: Arc<ClientStats>) -> Result<mpsc::Receiver<Bytes>, Refused> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(Refused::Closed);
        }
        let mut slots = self.slots.lock().unwrap();
        if self.stats.mode == BridgeMode::Exclusive {
            for old in slots.drain(..) {
                info!(bridge = %self.stats.name, client = %old.addr, by = %client.addr, "client replaced by a new connection");
                Counters::add(&self.stats.counters.replaced, 1);
            }
        } else if self.stats.max_clients.is_some_and(|max| slots.len() >= max) {
            Counters::add(&self.stats.counters.refused, 1);
            return Err(Refused::Full);
        }
        let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
        slots.push(Slot {
            id: client.id,
            addr: client.addr,
            tx,
        });
        self.stats.add_client(client);
        Ok(rx)
    }

    pub fn broadcast(&self, frame: &Bytes) {
        let mut slots = self.slots.lock().unwrap();
        slots.retain(|s| match s.tx.try_send(frame.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(bridge = %self.stats.name, client = %s.addr, "client cannot keep up, disconnecting");
                Counters::add(&self.stats.counters.lagged, 1);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    #[cfg(test)]
    fn has_clients(&self) -> bool {
        !self.slots.lock().unwrap().is_empty()
    }

    fn remove(&self, id: Uuid) {
        self.slots.lock().unwrap().retain(|s| s.id != id);
    }

    /// Disconnect everyone and refuse new clients (bridge stopping).
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.slots.lock().unwrap().clear();
    }
}

pub struct ClientCtx {
    pub hub: Arc<Hub>,
    pub stats: Arc<BridgeStats>,
    pub upgrader: Upgrader,
    pub to_device: mpsc::Sender<Bytes>,
    pub token: Option<String>,
    pub write_timeout: Duration,
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Read the token line; returns whatever followed it.
async fn authenticate(reader: &mut ClientReader, token: &str) -> Option<Bytes> {
    let mut line = Vec::new();
    let read = async {
        loop {
            let chunk = reader.next().await.ok()??;
            line.extend_from_slice(&chunk);
            if let Some(i) = line.iter().position(|&b| b == b'\n') {
                return Some(i);
            }
            if reader.message_based() {
                return Some(line.len());
            }
            if line.len() > MAX_TOKEN_LINE {
                return None;
            }
        }
    };
    let end = tokio::time::timeout(AUTH_TIMEOUT, read).await.ok()??;
    let given = line[..end].strip_suffix(b"\r").unwrap_or(&line[..end]);
    same(given, token.as_bytes())
        .then(|| Bytes::copy_from_slice(line.get(end + 1..).unwrap_or_default()))
}

pub async fn serve(ctx: Arc<ClientCtx>, stream: TcpStream, addr: SocketAddr) {
    let bridge = ctx.stats.name.clone();
    let io = match ctx.upgrader.upgrade(stream).await {
        Ok(io) => io,
        Err(e) => {
            debug!(%bridge, client = %addr, error = %format!("{e:#}"), "handshake failed");
            return;
        }
    };
    let (mut reader, mut writer) = io.split();

    let mut first = Bytes::new();
    if let Some(token) = &ctx.token {
        match authenticate(&mut reader, token).await {
            Some(rest) => first = rest,
            None => {
                warn!(%bridge, client = %addr, "authentication failed");
                Counters::add(&ctx.stats.counters.auth_failures, 1);
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    writer.send(Bytes::from_static(b"auth failed\n")),
                )
                .await;
                writer.close().await;
                return;
            }
        }
    }

    let client = Arc::new(ClientStats {
        id: Uuid::new_v4(),
        addr,
        since: Instant::now(),
        connected_at_ms: now_ms(),
        sent: Default::default(),
        received: Default::default(),
    });
    let mut out = match ctx.hub.admit(client.clone()) {
        Ok(rx) => rx,
        Err(r) => {
            if r == Refused::Full {
                warn!(%bridge, client = %addr, "client limit reached, refusing");
            }
            writer.close().await;
            return;
        }
    };
    info!(%bridge, client = %addr, "client connected");

    let forward = |data: Bytes| {
        let ctx = &ctx;
        let client = &client;
        async move {
            Counters::add(&client.received, data.len() as u64);
            if ctx.stats.read_only || data.is_empty() {
                return true;
            }
            ctx.to_device.send(data).await.is_ok()
        }
    };

    let mut open = first.is_empty() || forward(first).await;
    let reason = loop {
        if !open {
            break "bridge stopped";
        }
        tokio::select! {
            frame = out.recv() => match frame {
                None => break "disconnected by the bridge",
                Some(f) => {
                    let n = f.len() as u64;
                    match tokio::time::timeout(ctx.write_timeout, writer.send(f)).await {
                        Ok(Ok(())) => Counters::add(&client.sent, n),
                        Ok(Err(_)) => break "write failed",
                        Err(_) => break "write timed out",
                    }
                }
            },
            data = reader.next() => match data {
                Ok(Some(d)) => open = forward(d).await,
                Ok(None) => break "closed by client",
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break "reset by client",
                Err(_) => break "read failed",
            },
        }
    };
    writer.close().await;
    ctx.hub.remove(client.id);
    ctx.stats.remove_client(client.id);
    info!(%bridge, client = %addr, reason, "client disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BridgeConfig;

    fn stats(extra: &str) -> Arc<BridgeStats> {
        let cfg: BridgeConfig = toml::from_str(&format!(
            "name = \"t\"\n{extra}\n[source]\ntype = \"serial\"\nport = \"/dev/null\"\n[transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:0\"\n"
        ))
        .unwrap();
        Arc::new(BridgeStats::new(&cfg))
    }

    fn client() -> Arc<ClientStats> {
        Arc::new(ClientStats {
            id: Uuid::new_v4(),
            addr: "127.0.0.1:1".parse().unwrap(),
            since: Instant::now(),
            connected_at_ms: 0,
            sent: Default::default(),
            received: Default::default(),
        })
    }

    #[test]
    fn limit_refuses_and_counts() {
        let s = stats("max_clients = 1");
        let hub = Hub::new(s.clone());
        let _a = hub.admit(client()).unwrap();
        assert_eq!(hub.admit(client()).unwrap_err(), Refused::Full);
        assert_eq!(Counters::get(&s.counters.refused), 1);
    }

    #[tokio::test]
    async fn exclusive_replaces_previous_client() {
        let s = stats("mode = \"exclusive\"");
        let hub = Hub::new(s.clone());
        let mut a = hub.admit(client()).unwrap();
        let mut b = hub.admit(client()).unwrap();
        hub.broadcast(&Bytes::from_static(b"x"));
        assert_eq!(a.recv().await, None, "old client's queue is closed");
        assert_eq!(b.recv().await, Some(Bytes::from_static(b"x")));
        assert_eq!(Counters::get(&s.counters.replaced), 1);
    }

    #[test]
    fn lagging_client_is_dropped_others_keep_receiving() {
        let s = stats("");
        let hub = Hub::new(s.clone());
        let _slow = hub.admit(client()).unwrap();
        let mut fast = hub.admit(client()).unwrap();
        for _ in 0..=CLIENT_QUEUE {
            hub.broadcast(&Bytes::from_static(b"d"));
            while fast.try_recv().is_ok() {}
        }
        assert_eq!(Counters::get(&s.counters.lagged), 1);
        assert!(hub.has_clients());
        hub.broadcast(&Bytes::from_static(b"e"));
        assert_eq!(fast.try_recv().unwrap(), Bytes::from_static(b"e"));
    }

    #[test]
    fn closed_hub_refuses() {
        let hub = Hub::new(stats(""));
        hub.close();
        assert_eq!(hub.admit(client()).unwrap_err(), Refused::Closed);
    }

    #[test]
    fn constant_time_compare() {
        assert!(same(b"secret", b"secret"));
        assert!(!same(b"secret", b"secreT"));
        assert!(!same(b"secret", b"secret2"));
    }
}
