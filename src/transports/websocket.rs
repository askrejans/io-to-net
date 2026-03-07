use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::WebSocketTransportConfig;
use crate::error::{IoToNetError, Result};

use super::ClientHandle;

/// Accept loop for WebSocket transport.
pub async fn run_ws_listener(
    cfg: WebSocketTransportConfig,
    new_clients_tx: mpsc::Sender<ClientHandle>,
    max_clients: Option<usize>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen)
        .await
        .map_err(|e| IoToNetError::Transport(format!("WebSocket bind {} failed: {}", cfg.listen, e)))?;

    info!(listen = %cfg.listen, "WebSocket transport listening");

    let mut client_count = 0usize;

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Err(e) => { warn!(error = %e, "WebSocket accept error"); continue; }
                    Ok((stream, addr)) => {
                        if let Some(max) = max_clients {
                            if client_count >= max {
                                warn!(addr = %addr, "Max WebSocket clients reached");
                                drop(stream);
                                continue;
                            }
                        }
                        let clients_tx = new_clients_tx.clone();
                        let timeout = Duration::from_millis(cfg.write_timeout_ms);
                        tokio::spawn(async move {
                            match accept_async(stream).await {
                                Err(e) => { warn!(addr = %addr, error = %e, "WebSocket upgrade failed"); }
                                Ok(ws) => {
                                    info!(addr = %addr, "WebSocket client connected");
                                    let handle = spawn_ws_client_task(ws, addr, timeout).await;
                                    let _ = clients_tx.send(handle).await;
                                }
                            }
                        });
                        client_count += 1;
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("WebSocket transport shutting down");
                break;
            }
        }
    }
    Ok(())
}

async fn spawn_ws_client_task(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    addr: SocketAddr,
    write_timeout: Duration,
) -> ClientHandle {
    let id = Uuid::new_v4();
    let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(256);
    let (in_tx, in_rx) = mpsc::channel::<Bytes>(256);

    tokio::spawn(async move {
        let (mut ws_sink, mut ws_stream) = ws.split();

        loop {
            tokio::select! {
                data = out_rx.recv() => {
                    match data {
                        None => break,
                        Some(bytes) => {
                            let msg = Message::Binary(bytes.to_vec().into());
                            let res = tokio::time::timeout(
                                write_timeout,
                                ws_sink.send(msg),
                            ).await;
                            if res.is_err() {
                                warn!(addr = %addr, "WebSocket write timeout");
                                break;
                            }
                            if let Ok(Err(e)) = res {
                                debug!(addr = %addr, error = %e, "WebSocket write error");
                                break;
                            }
                        }
                    }
                }
                msg = ws_stream.next() => {
                    match msg {
                        None => { debug!(addr = %addr, "WebSocket client closed stream"); break; }
                        Some(Err(e)) => { debug!(addr = %addr, error = %e, "WebSocket recv error"); break; }
                        Some(Ok(Message::Binary(data))) => {
                            if in_tx.send(Bytes::from(data)).await.is_err() { break; }
                        }
                        Some(Ok(Message::Text(text))) => {
                            if in_tx.send(Bytes::copy_from_slice(text.as_bytes())).await.is_err() { break; }
                        }
                        Some(Ok(Message::Close(_))) => { debug!(addr = %addr, "WebSocket close frame"); break; }
                        Some(Ok(_)) => {} // ping/pong handled by tungstenite
                    }
                }
            }
        }
    });

    ClientHandle { id, addr, tx: out_tx, rx: in_rx }
}
