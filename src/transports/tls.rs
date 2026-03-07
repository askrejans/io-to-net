use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::TlsTransportConfig;
use crate::error::{IoToNetError, Result};

use super::ClientHandle;

/// Build a rustls `ServerConfig` from PEM cert and key files.
pub fn build_tls_config(cert_path: &str, key_path: &str) -> Result<Arc<ServerConfig>> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls_pemfile::{certs, pkcs8_private_keys};

    let cert_pem = std::fs::read(cert_path)
        .map_err(|e| IoToNetError::Tls(format!("Cannot read cert {}: {}", cert_path, e)))?;
    let key_pem = std::fs::read(key_path)
        .map_err(|e| IoToNetError::Tls(format!("Cannot read key {}: {}", key_path, e)))?;

    let certs: Vec<CertificateDer<'static>> = certs(&mut cert_pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| IoToNetError::Tls(format!("Invalid cert PEM: {}", e)))?;

    let mut keys: Vec<_> = pkcs8_private_keys(&mut key_pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| IoToNetError::Tls(format!("Invalid key PEM: {}", e)))?;

    if keys.is_empty() {
        return Err(IoToNetError::Tls("No PKCS8 private key found in key file".to_string()));
    }

    let key = PrivateKeyDer::Pkcs8(keys.remove(0));

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| IoToNetError::Tls(format!("TLS config error: {}", e)))?;

    Ok(Arc::new(config))
}

/// Accept loop for TLS transport.
pub async fn run_tls_listener(
    cfg: TlsTransportConfig,
    new_clients_tx: mpsc::Sender<ClientHandle>,
    max_clients: Option<usize>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    let tls_config = build_tls_config(&cfg.cert, &cfg.key)?;
    let acceptor = TlsAcceptor::from(tls_config);

    let listener = TcpListener::bind(&cfg.listen)
        .await
        .map_err(|e| IoToNetError::Transport(format!("TLS bind {} failed: {}", cfg.listen, e)))?;

    info!(listen = %cfg.listen, "TLS transport listening");

    let mut client_count = 0usize;

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Err(e) => { warn!(error = %e, "TLS accept error"); continue; }
                    Ok((stream, addr)) => {
                        if let Some(max) = max_clients {
                            if client_count >= max {
                                warn!(addr = %addr, "Max clients reached, refusing TLS connection");
                                drop(stream);
                                continue;
                            }
                        }
                        let acc = acceptor.clone();
                        let timeout = Duration::from_millis(cfg.write_timeout_ms);
                        let clients_tx = new_clients_tx.clone();
                        tokio::spawn(async move {
                            match acc.accept(stream).await {
                                Err(e) => { warn!(addr = %addr, error = %e, "TLS handshake failed"); }
                                Ok(tls_stream) => {
                                    info!(addr = %addr, "TLS client connected");
                                    let handle = spawn_tls_client_task(tls_stream, addr, timeout).await;
                                    let _ = clients_tx.send(handle).await;
                                }
                            }
                        });
                        client_count += 1;
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("TLS transport shutting down");
                break;
            }
        }
    }
    Ok(())
}

async fn spawn_tls_client_task(
    stream: tokio_rustls::server::TlsStream<TcpStream>,
    addr: SocketAddr,
    write_timeout: Duration,
) -> ClientHandle {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let id = Uuid::new_v4();
    let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(256);
    let (in_tx, in_rx) = mpsc::channel::<Bytes>(256);

    tokio::spawn(async move {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let mut buf = [0u8; 4096];

        loop {
            tokio::select! {
                data = out_rx.recv() => {
                    match data {
                        None => break,
                        Some(bytes) => {
                            let res = tokio::time::timeout(write_timeout, writer.write_all(&bytes)).await;
                            if res.is_err() || res.unwrap().is_err() {
                                warn!(addr = %addr, "TLS write failed");
                                break;
                            }
                        }
                    }
                }
                read = reader.read(&mut buf) => {
                    match read {
                        Err(e) => { debug!(addr = %addr, error = %e, "TLS read error"); break; }
                        Ok(0) => { debug!(addr = %addr, "TLS client disconnected"); break; }
                        Ok(n) => {
                            let data = Bytes::copy_from_slice(&buf[..n]);
                            if in_tx.send(data).await.is_err() { break; }
                        }
                    }
                }
            }
        }
    });

    ClientHandle { id, addr, tx: out_tx, rx: in_rx }
}
