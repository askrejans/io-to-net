//! Byte-stream sources: a TCP device (Wi-Fi serial bridge, ESP32 node,
//! another io-to-net) or a Unix domain socket.

use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{Link, LinkEvent};
use crate::config::TcpClientConfig;
#[cfg(unix)]
use crate::config::UnixSocketConfig;

pub async fn open_tcp(cfg: &TcpClientConfig) -> Result<Link> {
    let stream = tokio::time::timeout(
        Duration::from_millis(cfg.connect_timeout_ms.max(1)),
        tokio::net::TcpStream::connect(&cfg.address),
    )
    .await
    .with_context(|| format!("connecting to {} timed out", cfg.address))?
    .with_context(|| format!("cannot connect to {}", cfg.address))?;
    stream.set_nodelay(true).ok();
    Ok(spawn(stream, format!("tcp://{}", cfg.address)))
}

#[cfg(unix)]
pub async fn open_unix(cfg: &UnixSocketConfig) -> Result<Link> {
    let stream = tokio::net::UnixStream::connect(&cfg.path)
        .await
        .with_context(|| format!("cannot connect to Unix socket {}", cfg.path))?;
    Ok(spawn(stream, format!("unix://{}", cfg.path)))
}

/// Read and write halves on their own tasks; both end when the link drops.
pub fn spawn<S>(stream: S, description: String) -> Link
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (ev_tx, ev_rx, w_tx, mut w_rx) = Link::channels();

    let read_tx = ev_tx.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            tokio::select! {
                r = reader.read(&mut buf) => match r {
                    Ok(0) => { let _ = read_tx.send(LinkEvent::Closed("closed by the device".into())).await; break; }
                    Ok(n) => {
                        if read_tx.send(LinkEvent::Data(Bytes::copy_from_slice(&buf[..n]))).await.is_err() { break; }
                    }
                    Err(e) => { let _ = read_tx.send(LinkEvent::Closed(format!("read: {e}"))).await; break; }
                },
                _ = read_tx.closed() => break,
            }
        }
    });
    tokio::spawn(async move {
        while let Some(chunk) = w_rx.recv().await {
            if let Err(e) = writer.write_all(&chunk).await {
                let _ = ev_tx.send(LinkEvent::Closed(format!("write: {e}"))).await;
                break;
            }
        }
        let _ = writer.shutdown().await;
    });
    Link {
        rx: ev_rx,
        tx: w_tx,
        description,
        _guard: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_source_is_full_duplex() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let device = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8];
            let n = s.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"Q");
            s.write_all(b"speeduino 202501").await.unwrap();
        });
        let mut link = open_tcp(&TcpClientConfig {
            address: addr.to_string(),
            connect_timeout_ms: 1000,
        })
        .await
        .unwrap();
        link.tx.send(Bytes::from_static(b"Q")).await.unwrap();
        let mut got = Vec::new();
        while got.len() < 16 {
            match link.rx.recv().await.unwrap() {
                LinkEvent::Data(b) => got.extend_from_slice(&b),
                LinkEvent::Closed(e) => panic!("{e}"),
            }
        }
        assert_eq!(got, b"speeduino 202501");
        device.await.unwrap();
        // The device closing is reported, so the bridge can reopen.
        assert!(matches!(link.rx.recv().await, Some(LinkEvent::Closed(_))));
    }

    #[tokio::test]
    async fn unreachable_tcp_device_fails_fast() {
        let err = open_tcp(&TcpClientConfig {
            address: "127.0.0.1:1".into(),
            connect_timeout_ms: 500,
        })
        .await;
        assert!(err.is_err());
    }
}
