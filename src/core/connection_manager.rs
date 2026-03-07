use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::core::metrics::{ConnectionMetrics, GlobalMetrics};

const BROADCAST_CAPACITY: usize = 1024;

/// Manages all clients connected to a single bridge.
/// Uses a tokio broadcast channel to fan-out device→client data.
pub struct ConnectionManager {
    bridge_name: String,
    /// Broadcast sender — bridge pushes device data here
    bcast_tx: broadcast::Sender<Bytes>,
    /// Map of connected client IDs → their reverse channel (client→device)
    reverse_channels: HashMap<Uuid, mpsc::Sender<Bytes>>,
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
}

impl ConnectionManager {
    pub fn new(
        bridge_name: String,
        metrics: Arc<RwLock<GlobalMetrics>>,
        log_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<String>>>,
    ) -> Self {
        let (bcast_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        ConnectionManager {
            bridge_name,
            bcast_tx,
            reverse_channels: HashMap::new(),
            metrics,
            log_buffer,
        }
    }

    /// Add a new connected client. Spawns its data relay task.
    pub fn add_client(
        &mut self,
        id: Uuid,
        addr: SocketAddr,
        client_tx: mpsc::Sender<Bytes>,
        client_rx: mpsc::Receiver<Bytes>,
        device_write_tx: mpsc::Sender<Bytes>,
        read_only: bool,
    ) {
        let bridge_name = self.bridge_name.clone();
        let bcast_rx = self.bcast_tx.subscribe();
        let metrics = self.metrics.clone();
        let _log_buffer = self.log_buffer.clone();

        // Store reverse channel so bridge can query it
        self.reverse_channels.insert(id, device_write_tx.clone());

        {
            let mut m = metrics.write();
            if let Some(bm) = m.bridges.get_mut(&bridge_name) {
                bm.clients_connected += 1;
            }
            m.connections.insert(
                id.to_string(),
                ConnectionMetrics::new(id, addr.to_string(), bridge_name.clone()),
            );
        }

        self.log(format!("client {} connected from {}", id, addr));
        info!(bridge = %bridge_name, addr = %addr, id = %id, "Client connected");

        let bridge_name_clone = bridge_name.clone();

        // Spawn the outbound task: broadcast → client socket
        tokio::spawn(async move {
            let mut bcast_rx = bcast_rx;
            let mut client_rx = client_rx;
            let id_str = id.to_string();

            loop {
                tokio::select! {
                    result = bcast_rx.recv() => {
                        match result {
                            Err(broadcast::error::RecvError::Closed) => break,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(bridge = %bridge_name_clone, client = %id_str, lagged = n, "Client lagged");
                                continue;
                            }
                            Ok(data) => {
                                let len = data.len() as u64;
                                if client_tx.send(data).await.is_err() {
                                    debug!(client = %id_str, "Client send channel closed");
                                    break;
                                }
                                let mut m = metrics.write();
                                if let Some(cm) = m.connections.get_mut(&id_str) {
                                    cm.bytes_rx += len;
                                    cm.refresh_uptime();
                                }
                                if let Some(bm) = m.bridges.get_mut(&bridge_name_clone) {
                                    bm.add_rx(len);
                                }
                            }
                        }
                    }
                    data = client_rx.recv() => {
                        match data {
                            None => { debug!(client = %id_str, "Client inbound channel closed"); break; }
                            Some(bytes) => {
                                if !read_only {
                                    let len = bytes.len() as u64;
                                    if device_write_tx.send(bytes).await.is_err() {
                                        debug!(client = %id_str, "Device write channel closed");
                                        break;
                                    }
                                    let mut m = metrics.write();
                                    if let Some(cm) = m.connections.get_mut(&id_str) {
                                        cm.bytes_tx += len;
                                    }
                                    if let Some(bm) = m.bridges.get_mut(&bridge_name_clone) {
                                        bm.add_tx(len);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Cleanup after client disconnects
            {
                let mut m = metrics.write();
                m.connections.remove(&id_str);
                if let Some(bm) = m.bridges.get_mut(&bridge_name_clone) {
                    bm.clients_connected = bm.clients_connected.saturating_sub(1);
                }
            }
            info!(bridge = %bridge_name_clone, client = %id_str, "Client disconnected");
        });
    }

    /// Broadcast a chunk of device data to all connected clients.
    pub fn broadcast(&self, data: Bytes) {
        // send() only fails when there are 0 receivers — that's fine
        let _ = self.bcast_tx.send(data);
    }

    /// Number of active receivers (clients) on the broadcast channel.
    #[allow(dead_code)]
    pub fn client_count(&self) -> usize {
        self.bcast_tx.receiver_count()
    }

    #[allow(dead_code)]
    fn log(&self, msg: String) {
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let entry = format!("{} {}", ts, msg);
        let mut buf = self.log_buffer.lock();
        if buf.len() >= 1000 {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

/// Handle returned by `BridgeManager` for external control.
#[derive(Clone)]
pub struct BridgeHandle {
    #[allow(dead_code)]
    pub name: String,
    pub restart_tx: mpsc::Sender<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn make_manager() -> (ConnectionManager, Arc<RwLock<GlobalMetrics>>) {
        let metrics = Arc::new(RwLock::new(GlobalMetrics::new()));
        metrics.write().register_bridge("test".into(), "s".into(), "t".into());
        let log_buf = Arc::new(parking_lot::Mutex::new(VecDeque::<String>::new()));
        let mgr = ConnectionManager::new("test".into(), metrics.clone(), log_buf);
        (mgr, metrics)
    }

    #[test]
    fn new_manager_has_no_clients() {
        let (mgr, _) = make_manager();
        assert_eq!(mgr.client_count(), 0);
    }

    #[tokio::test]
    async fn broadcast_fans_out_to_subscribers() {
        let (mgr, _) = make_manager();

        let mut rx1 = mgr.bcast_tx.subscribe();
        let mut rx2 = mgr.bcast_tx.subscribe();

        let payload = Bytes::from_static(b"hello");
        mgr.broadcast(payload.clone());

        let got1 = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx1.recv(),
        ).await.expect("timeout").expect("recv");
        let got2 = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx2.recv(),
        ).await.expect("timeout").expect("recv");

        assert_eq!(got1, payload);
        assert_eq!(got2, payload);
    }

    #[tokio::test]
    async fn broadcast_empty_does_not_panic() {
        let (mgr, _) = make_manager();
        // No subscribers — broadcast.send() returns Err(NoReceivers) which is ignored
        mgr.broadcast(Bytes::from_static(b"ping"));
    }

    #[tokio::test]
    async fn add_client_increments_connected_count() {
        let (mut mgr, metrics) = make_manager();

        let id = Uuid::new_v4();
        let addr: SocketAddr = "127.0.0.1:60000".parse().unwrap();
        let (client_tx, _client_rx) = mpsc::channel::<Bytes>(16);
        let (_data_tx, client_data_rx) = mpsc::channel::<Bytes>(16);
        let (device_write_tx, _dev_rx) = mpsc::channel::<Bytes>(16);

        mgr.add_client(id, addr, client_tx, client_data_rx, device_write_tx, false);

        // give spawned task a moment to register
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let m = metrics.read();
        let bm = m.bridges.get("test").unwrap();
        assert_eq!(bm.clients_connected, 1);
        assert!(m.connections.contains_key(&id.to_string()));
    }

    #[tokio::test]
    async fn client_disconnect_cleans_up_metrics() {
        let (mut mgr, metrics) = make_manager();

        let id = Uuid::new_v4();
        let addr: SocketAddr = "127.0.0.1:60001".parse().unwrap();
        let (client_tx, _client_rx) = mpsc::channel::<Bytes>(16);
        let (_data_tx, client_data_rx) = mpsc::channel::<Bytes>(16);
        let (device_write_tx, _dev_rx) = mpsc::channel::<Bytes>(16);

        mgr.add_client(id, addr, client_tx, client_data_rx, device_write_tx, false);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Drop reverse channel reference tracked in manager
        drop(mgr);
        // Wait for cleanup task to notice the sender side is gone
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connection should eventually be removed
        let m = metrics.read();
        // Bridge record persists; connected count should have decremented to 0
        let bm = m.bridges.get("test").unwrap();
        assert_eq!(bm.clients_connected, 0);
    }

    #[tokio::test]
    async fn data_flows_from_broadcast_to_client() {
        let (mut mgr, _metrics) = make_manager();

        let id = Uuid::new_v4();
        let addr: SocketAddr = "127.0.0.1:60002".parse().unwrap();
        let (client_tx, mut client_out_rx) = mpsc::channel::<Bytes>(16);
        let (_data_tx, client_data_rx) = mpsc::channel::<Bytes>(16);
        let (device_write_tx, _dev_rx) = mpsc::channel::<Bytes>(16);

        mgr.add_client(id, addr, client_tx, client_data_rx, device_write_tx, false);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        mgr.broadcast(Bytes::from_static(b"from_device"));

        let received = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            client_out_rx.recv(),
        ).await.expect("timeout").expect("recv");

        assert_eq!(received, Bytes::from_static(b"from_device"));
    }

    #[tokio::test]
    async fn read_only_blocks_client_to_device_writes() {
        let (mut mgr, _metrics) = make_manager();

        let id = Uuid::new_v4();
        let addr: SocketAddr = "127.0.0.1:60003".parse().unwrap();
        let (client_tx, _) = mpsc::channel::<Bytes>(16);
        let (data_tx, client_data_rx) = mpsc::channel::<Bytes>(16);
        let (device_write_tx, mut device_rx) = mpsc::channel::<Bytes>(16);

        mgr.add_client(id, addr, client_tx, client_data_rx, device_write_tx, true /* read_only */);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Client sends data to bridge
        data_tx.send(Bytes::from_static(b"client_data")).await.unwrap();

        // In read_only mode nothing should reach device_write channel
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(80),
            device_rx.recv(),
        ).await;
        assert!(result.is_err(), "read_only: data should not reach device channel");
    }
}
