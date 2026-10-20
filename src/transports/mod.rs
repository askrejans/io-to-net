//! Network side of a bridge: a listener that accepts TCP connections and
//! upgrades each one to the configured transport (plain TCP, TLS or
//! WebSocket). The upgrade runs in the client's own task, so a slow or
//! hostile handshake never holds up the accept loop.

pub mod tls;

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::config::TransportConfig;

/// Time a client has to finish the TLS or WebSocket handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub enum Upgrader {
    Tcp,
    Tls(TlsAcceptor),
    WebSocket,
}

pub struct Listener {
    tcp: TcpListener,
    pub upgrader: Upgrader,
}

impl Listener {
    pub async fn bind(cfg: &TransportConfig) -> Result<Self> {
        let upgrader = match cfg {
            TransportConfig::Tcp(_) => Upgrader::Tcp,
            TransportConfig::Tls(t) => Upgrader::Tls(TlsAcceptor::from(tls::server_config(t)?)),
            TransportConfig::WebSocket(_) => Upgrader::WebSocket,
        };
        let tcp = TcpListener::bind(cfg.listen())
            .await
            .with_context(|| format!("cannot listen on {}", cfg.listen()))?;
        Ok(Listener { tcp, upgrader })
    }

    #[cfg(test)]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (s, addr) = self.tcp.accept().await?;
        let _ = s.set_nodelay(true);
        Ok((s, addr))
    }
}

pub trait Duplex: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Duplex for T {}

pub enum ClientIo {
    Stream(Box<dyn Duplex>),
    Ws(Box<WebSocketStream<TcpStream>>),
}

impl Upgrader {
    pub async fn upgrade(&self, stream: TcpStream) -> Result<ClientIo> {
        let fut = async {
            Ok::<_, anyhow::Error>(match self {
                Upgrader::Tcp => ClientIo::Stream(Box::new(stream)),
                Upgrader::Tls(acceptor) => ClientIo::Stream(Box::new(
                    acceptor.accept(stream).await.context("TLS handshake")?,
                )),
                Upgrader::WebSocket => ClientIo::Ws(Box::new(
                    tokio_tungstenite::accept_async(stream)
                        .await
                        .context("WebSocket handshake")?,
                )),
            })
        };
        tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
            .await
            .context("handshake timed out")?
    }
}

impl ClientIo {
    pub fn split(self) -> (ClientReader, ClientWriter) {
        match self {
            ClientIo::Stream(s) => {
                let (r, w) = tokio::io::split(s);
                (
                    ClientReader::Stream(r, BytesMut::with_capacity(4096)),
                    ClientWriter::Stream(w),
                )
            }
            ClientIo::Ws(ws) => {
                let (sink, stream) = (*ws).split();
                (ClientReader::Ws(stream), ClientWriter::Ws(sink))
            }
        }
    }
}

pub enum ClientReader {
    Stream(ReadHalf<Box<dyn Duplex>>, BytesMut),
    Ws(SplitStream<WebSocketStream<TcpStream>>),
}

impl ClientReader {
    /// Next chunk from the client; `None` when it disconnected.
    /// Cancel-safe, so it can sit in a `select!`.
    pub async fn next(&mut self) -> io::Result<Option<Bytes>> {
        match self {
            ClientReader::Stream(r, buf) => {
                buf.reserve(4096);
                match r.read_buf(buf).await? {
                    0 => Ok(None),
                    _ => Ok(Some(buf.split().freeze())),
                }
            }
            ClientReader::Ws(s) => loop {
                match s.next().await {
                    None | Some(Ok(Message::Close(_))) => return Ok(None),
                    Some(Err(e)) => return Err(io::Error::other(e)),
                    Some(Ok(Message::Binary(b))) => return Ok(Some(b)),
                    Some(Ok(Message::Text(t))) => {
                        return Ok(Some(Bytes::copy_from_slice(t.as_bytes())));
                    }
                    Some(Ok(_)) => {} // ping/pong are answered by tungstenite
                }
            },
        }
    }

    /// WebSocket clients send whole messages; a stream client's chunks are arbitrary.
    pub fn message_based(&self) -> bool {
        matches!(self, ClientReader::Ws(_))
    }
}

pub enum ClientWriter {
    Stream(WriteHalf<Box<dyn Duplex>>),
    Ws(SplitSink<WebSocketStream<TcpStream>, Message>),
}

impl ClientWriter {
    pub async fn send(&mut self, data: Bytes) -> io::Result<()> {
        match self {
            ClientWriter::Stream(w) => {
                w.write_all(&data).await?;
                w.flush().await
            }
            ClientWriter::Ws(s) => s
                .send(Message::Binary(data))
                .await
                .map_err(io::Error::other),
        }
    }

    pub async fn close(&mut self) {
        match self {
            ClientWriter::Stream(w) => {
                let _ = w.shutdown().await;
            }
            ClientWriter::Ws(s) => {
                let _ = s.close().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TcpTransportConfig, WebSocketTransportConfig};

    async fn pair(cfg: TransportConfig) -> (ClientIo, SocketAddr) {
        let l = Listener::bind(&cfg).await.unwrap();
        let addr = l.local_addr().unwrap();
        let up = l.upgrader.clone();
        let accept = tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            up.upgrade(s).await.unwrap()
        });
        (accept_with_client(accept, addr, &cfg).await, addr)
    }

    async fn accept_with_client(
        accept: tokio::task::JoinHandle<ClientIo>,
        addr: SocketAddr,
        cfg: &TransportConfig,
    ) -> ClientIo {
        match cfg {
            TransportConfig::WebSocket(_) => {
                let (mut ws, _) = tokio_tungstenite::client_async(
                    format!("ws://{addr}/"),
                    TcpStream::connect(addr).await.unwrap(),
                )
                .await
                .unwrap();
                let io = accept.await.unwrap();
                ws.send(Message::Text("hello".into())).await.unwrap();
                tokio::spawn(async move {
                    while let Some(Ok(m)) = ws.next().await {
                        if let Message::Binary(b) = m {
                            ws.send(Message::Binary(b)).await.unwrap();
                        }
                    }
                });
                io
            }
            _ => {
                let mut c = TcpStream::connect(addr).await.unwrap();
                let io = accept.await.unwrap();
                c.write_all(b"hello").await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    while let Ok(n @ 1..) = c.read(&mut buf).await {
                        c.write_all(&buf[..n]).await.unwrap();
                    }
                });
                io
            }
        }
    }

    async fn roundtrip(cfg: TransportConfig) {
        let (io, _) = pair(cfg).await;
        let (mut r, mut w) = io.split();
        assert_eq!(
            r.next().await.unwrap().unwrap(),
            Bytes::from_static(b"hello")
        );
        w.send(Bytes::from_static(b"\x01\x02echo")).await.unwrap();
        assert_eq!(
            r.next().await.unwrap().unwrap(),
            Bytes::from_static(b"\x01\x02echo")
        );
    }

    #[tokio::test]
    async fn tcp_roundtrip() {
        roundtrip(TransportConfig::Tcp(TcpTransportConfig {
            listen: "127.0.0.1:0".into(),
            ..Default::default()
        }))
        .await;
    }

    #[tokio::test]
    async fn websocket_roundtrip_text_and_binary() {
        roundtrip(TransportConfig::WebSocket(WebSocketTransportConfig {
            listen: "127.0.0.1:0".into(),
            ..Default::default()
        }))
        .await;
    }

    #[tokio::test]
    async fn websocket_handshake_from_plain_tcp_fails() {
        let cfg = TransportConfig::WebSocket(WebSocketTransportConfig {
            listen: "127.0.0.1:0".into(),
            ..Default::default()
        });
        let l = Listener::bind(&cfg).await.unwrap();
        let addr = l.local_addr().unwrap();
        let mut c = TcpStream::connect(addr).await.unwrap();
        c.write_all(b"not http\r\n\r\n").await.unwrap();
        let (s, _) = l.accept().await.unwrap();
        assert!(l.upgrader.upgrade(s).await.is_err());
    }
}
