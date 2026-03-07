use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::TcpTransportConfig;
use crate::error::{IoToNetError, Result};

use super::ClientHandle;

/// Accept loop for plain TCP transport.
/// Yields `ClientHandle`s via the `new_clients_tx` channel.
pub async fn run_tcp_listener(
    cfg: TcpTransportConfig,
    new_clients_tx: mpsc::Sender<ClientHandle>,
    max_clients: Option<usize>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen)
        .await
        .map_err(|e| IoToNetError::Transport(format!("TCP bind {} failed: {}", cfg.listen, e)))?;

    info!(listen = %cfg.listen, "TCP transport listening");

    let mut client_count: usize = 0;

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Err(e) => {
                        warn!(error = %e, "TCP accept error");
                        continue;
                    }
                    Ok((stream, addr)) => {
                        if let Some(max) = max_clients {
                            if client_count >= max {
                                warn!(addr = %addr, "Max clients reached, refusing connection");
                                drop(stream);
                                continue;
                            }
                        }
                        let write_timeout = Duration::from_millis(cfg.write_timeout_ms);
                        let handle = spawn_client_task(stream, addr, write_timeout).await;
                        client_count += 1;
                        info!(addr = %addr, "TCP client connected");
                        if new_clients_tx.send(handle).await.is_err() {
                            break;
                        }
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("TCP transport shutting down");
                break;
            }
        }
    }
    Ok(())
}

/// Spawn per-client IO tasks. Returns a `ClientHandle` for the bridge to use.
async fn spawn_client_task(
    stream: TcpStream,
    addr: SocketAddr,
    write_timeout: Duration,
) -> ClientHandle {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let id = Uuid::new_v4();
    // Channel from bridge→client (data going out to network)
    let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(256);
    // Channel from client→bridge (data coming in from network)
    let (in_tx, in_rx) = mpsc::channel::<Bytes>(256);

    tokio::spawn(async move {
        let (mut reader, mut writer) = stream.into_split();
        let mut buf = [0u8; 4096];

        loop {
            tokio::select! {
                // Data from bridge → client
                data = out_rx.recv() => {
                    match data {
                        None => { debug!(addr = %addr, "Client outbound channel closed"); break; }
                        Some(bytes) => {
                            let write_result = tokio::time::timeout(
                                write_timeout,
                                writer.write_all(&bytes),
                            ).await;
                            match write_result {
                                Err(_) => { warn!(addr = %addr, "TCP write timeout, disconnecting"); break; }
                                Ok(Err(e)) => { debug!(addr = %addr, error = %e, "TCP write error"); break; }
                                Ok(Ok(_)) => {}
                            }
                        }
                    }
                }
                // Data from client → bridge
                read = reader.read(&mut buf) => {
                    match read {
                        Err(e) => { debug!(addr = %addr, error = %e, "TCP read error"); break; }
                        Ok(0) => { debug!(addr = %addr, "TCP client disconnected"); break; }
                        Ok(n) => {
                            let data = Bytes::copy_from_slice(&buf[..n]);
                            if in_tx.send(data).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        debug!(addr = %addr, "TCP client task ended");
    });

    ClientHandle { id, addr, tx: out_tx, rx: in_rx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn ephemeral_port() -> u16 {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    // ── TCP bind / accept ───────────────────────────────────────────────────

    #[tokio::test]
    async fn listener_binds_and_accepts_connection() {
        let port = ephemeral_port().await;
        let cfg = TcpTransportConfig {
            listen: format!("127.0.0.1:{}", port),
            write_timeout_ms: 1000,
        };

        let (tx, mut rx) = mpsc::channel::<ClientHandle>(4);
        let (sd_tx, sd_rx) = tokio::sync::broadcast::channel::<()>(1);

        tokio::spawn(async move {
            let _ = run_tcp_listener(cfg, tx, None, sd_rx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect a client
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let handle = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            rx.recv(),
        ).await.expect("timeout").expect("recv");

        assert_eq!(handle.addr.port(), client.local_addr().unwrap().port());

        // Clean shutdown
        let _ = sd_tx.send(());
        let _ = client.shutdown().await;
    }

    #[tokio::test]
    async fn max_clients_enforced() {
        let port = ephemeral_port().await;
        let cfg = TcpTransportConfig {
            listen: format!("127.0.0.1:{}", port),
            write_timeout_ms: 1000,
        };

        let (tx, mut rx) = mpsc::channel::<ClientHandle>(8);
        let (sd_tx, sd_rx) = tokio::sync::broadcast::channel::<()>(1);

        tokio::spawn(async move {
            let _ = run_tcp_listener(cfg, tx, Some(1), sd_rx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let addr = format!("127.0.0.1:{}", port);
        let _c1 = tokio::net::TcpStream::connect(&addr).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // First client should land in channel
        let handle = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            rx.recv(),
        ).await.expect("timeout").expect("recv1");
        drop(handle);

        // Second client should be refused (server drops it) — connect itself succeeds
        // at OS level but the accepted connection is immediately dropped by the listener
        let _c2 = tokio::net::TcpStream::connect(&addr).await.unwrap();
        // Listener should NOT emit a second ClientHandle for max=1
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(150),
            rx.recv(),
        ).await;
        assert!(second.is_err(), "second client should not get a handle when max=1 is reached");

        let _ = sd_tx.send(());
    }

    // ── Data flow via client handle channels ───────────────────────────────────

    #[tokio::test]
    async fn data_sent_to_handle_tx_reaches_tcp_socket() {
        let port = ephemeral_port().await;
        let cfg = TcpTransportConfig {
            listen: format!("127.0.0.1:{}", port),
            write_timeout_ms: 1000,
        };

        let (new_tx, mut new_rx) = mpsc::channel::<ClientHandle>(4);
        let (_sd_tx, sd_rx) = tokio::sync::broadcast::channel::<()>(1);

        tokio::spawn(async move {
            let _ = run_tcp_listener(cfg, new_tx, None, sd_rx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut socket = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await.unwrap();

        let handle = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            new_rx.recv(),
        ).await.unwrap().unwrap();

        // Push bytes from bridge side → client socket
        handle.tx.send(Bytes::from_static(b"bridge_data")).await.unwrap();

        let mut buf = vec![0u8; 32];
        let n = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            socket.read(&mut buf),
        ).await.unwrap().unwrap();

        assert_eq!(&buf[..n], b"bridge_data");
    }

    #[tokio::test]
    async fn data_written_to_tcp_socket_arrives_on_handle_rx() {
        let port = ephemeral_port().await;
        let cfg = TcpTransportConfig {
            listen: format!("127.0.0.1:{}", port),
            write_timeout_ms: 1000,
        };

        let (new_tx, mut new_rx) = mpsc::channel::<ClientHandle>(4);
        let (_sd_tx, sd_rx) = tokio::sync::broadcast::channel::<()>(1);

        tokio::spawn(async move {
            let _ = run_tcp_listener(cfg, new_tx, None, sd_rx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut socket = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await.unwrap();

        let mut handle = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            new_rx.recv(),
        ).await.unwrap().unwrap();

        socket.write_all(b"client_msg").await.unwrap();

        let received = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            handle.rx.recv(),
        ).await.unwrap().unwrap();

        assert_eq!(received, Bytes::from_static(b"client_msg"));
    }

    #[tokio::test]
    async fn shutdown_signal_stops_listener() {
        let port = ephemeral_port().await;
        let cfg = TcpTransportConfig {
            listen: format!("127.0.0.1:{}", port),
            write_timeout_ms: 1000,
        };

        let (tx, _rx) = mpsc::channel::<ClientHandle>(4);
        let (sd_tx, sd_rx) = tokio::sync::broadcast::channel::<()>(1);

        let task = tokio::spawn(async move {
            run_tcp_listener(cfg, tx, None, sd_rx).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = sd_tx.send(());

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            task,
        ).await;
        assert!(result.is_ok(), "listener should stop after shutdown signal");
    }
}
