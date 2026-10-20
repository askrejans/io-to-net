//! Full-duplex connection to a source.
//!
//! Every source is opened into a [`Link`]: bytes from the device arrive on
//! `rx`, bytes for the device go into `tx`. Reading and writing are
//! independent, so a client's request reaches the device the moment it
//! arrives, not after the next read timeout (request–reply protocols such as
//! Speeduino, MegaSquirt or MODBUS depend on it). Dropping the link closes the
//! source and stops its threads or tasks.

pub mod polled;
pub mod pty;
pub mod serial;
pub mod stream;

use std::any::Any;

use anyhow::Result;
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::config::SourceConfig;

/// Channel depth in chunks; a chunk is one read (≤ 4 KiB).
pub const DEPTH: usize = 256;

#[derive(Debug)]
pub enum LinkEvent {
    Data(Bytes),
    /// The source failed or closed; the bridge reopens it.
    Closed(String),
}

pub struct Link {
    pub rx: mpsc::Receiver<LinkEvent>,
    pub tx: mpsc::Sender<Bytes>,
    /// The resolved source, e.g. `serial:///dev/cu.usbmodem1101@9600`.
    pub description: String,
    /// Whatever must stay alive for the link (PTY slave, BLE session …).
    pub _guard: Option<Box<dyn Any + Send>>,
}

impl Link {
    pub fn channels() -> (
        mpsc::Sender<LinkEvent>,
        mpsc::Receiver<LinkEvent>,
        mpsc::Sender<Bytes>,
        mpsc::Receiver<Bytes>,
    ) {
        let (ev_tx, ev_rx) = mpsc::channel(DEPTH);
        let (w_tx, w_rx) = mpsc::channel(DEPTH);
        (ev_tx, ev_rx, w_tx, w_rx)
    }
}

pub async fn open(source: &SourceConfig) -> Result<Link> {
    match source {
        SourceConfig::Serial(cfg) => serial::open(cfg.clone()).await,
        SourceConfig::TcpClient(cfg) => stream::open_tcp(cfg).await,
        #[cfg(unix)]
        SourceConfig::UnixSocket(cfg) => stream::open_unix(cfg).await,
        #[cfg(unix)]
        SourceConfig::Pty(cfg) => pty::open(cfg),
        #[cfg(not(unix))]
        SourceConfig::UnixSocket(_) | SourceConfig::Pty(_) => {
            anyhow::bail!("{} sources need a Unix system", source.kind())
        }
        other => polled::open(other).await,
    }
}
