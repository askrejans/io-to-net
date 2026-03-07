pub mod tcp;
pub mod tls;
pub mod websocket;

use std::net::SocketAddr;

/// A connected client handle returned by the transport accept loop.
pub struct ClientHandle {
    pub id: uuid::Uuid,
    pub addr: SocketAddr,
    /// Channel to send data to the client
    pub tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
    /// Channel to receive data from the client (client → driver)
    pub rx: tokio::sync::mpsc::Receiver<bytes::Bytes>,
}
