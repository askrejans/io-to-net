//! One bridge: listens for clients for its whole life and keeps its source
//! open, reopening it with backoff after failures. Clients stay connected
//! while the source is down (a re-plugged USB adapter or a rebooting ECU),
//! and data flows again as soon as it is back.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use super::clients::{ClientCtx, Hub, serve};
use super::framer::Framer;
use super::stats::{BridgeStats, Counters, Direction, Status};
use crate::config::BridgeConfig;
use crate::link::{self, LinkEvent};
use crate::protocol::{ProtocolDetector, ProtocolHint};
use crate::transports::Listener;

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A source that stayed open this long starts the next retry from the minimum.
const HEALTHY_RUN: Duration = Duration::from_secs(30);
const BIND_RETRY: Duration = Duration::from_secs(5);

struct Backoff(Duration);

impl Backoff {
    fn next(&mut self) -> Duration {
        let d = self.0;
        self.0 = (self.0 * 2).min(BACKOFF_MAX);
        d
    }
    fn reset(&mut self) {
        self.0 = BACKOFF_MIN;
    }
}

/// What "frame rate" counts: lines for line protocols, polls (client
/// requests) for request–reply ECUs, reads for everything else.
enum Unit {
    Lines,
    Polls,
    Reads,
}

fn unit(h: ProtocolHint) -> Unit {
    if matches!(
        h,
        ProtocolHint::Speeduino | ProtocolHint::Megasquirt | ProtocolHint::ModbusRtu
    ) {
        Unit::Polls
    } else if line_based(h) {
        Unit::Lines
    } else {
        Unit::Reads
    }
}

fn line_based(h: ProtocolHint) -> bool {
    matches!(
        h,
        ProtocolHint::Nmea
            | ProtocolHint::AsciiStream
            | ProtocolHint::AtCommands
            | ProtocolHint::Scpi
            | ProtocolHint::CanOverSerial
            | ProtocolHint::Slip
            | ProtocolHint::Cobs
    )
}

struct Rates {
    at: Instant,
    frames: u64,
    chunks: u64,
    polls: u64,
    bytes: u64,
}

impl Rates {
    fn update(&mut self, stats: &BridgeStats, det: &ProtocolDetector) {
        let dt = self.at.elapsed().as_secs_f64();
        if dt < 0.5 {
            return;
        }
        let c = &stats.counters;
        let (frames, chunks, polls, bytes) = (
            det.frames(),
            Counters::get(&c.rx_chunks),
            Counters::get(&c.tx_chunks),
            Counters::get(&c.rx_bytes),
        );
        let hint = *det.hint();
        let n = match unit(hint) {
            Unit::Lines => frames - self.frames,
            Unit::Polls => polls - self.polls,
            Unit::Reads => chunks - self.chunks,
        };
        let avg = if n > 0 {
            (bytes - self.bytes) as f64 / n as f64
        } else {
            0.0
        };
        stats.set_protocol(hint, n as f64 / dt, avg);
        stats.tick();
        *self = Rates {
            at: Instant::now(),
            frames,
            chunks,
            polls,
            bytes,
        };
    }
}

pub async fn run(cfg: BridgeConfig, stats: Arc<BridgeStats>, mut stop: watch::Receiver<bool>) {
    let name = cfg.name.clone();
    let listener = loop {
        match Listener::bind(&cfg.transport).await {
            Ok(l) => break l,
            Err(e) => {
                let msg = format!("{e:#}");
                error!(bridge = %name, error = %msg, "cannot start listener, retrying");
                stats.set_status(Status::Waiting, msg);
                tokio::select! {
                    _ = tokio::time::sleep(BIND_RETRY) => {}
                    _ = stop.changed() => return stats.set_status(Status::Stopped, "stopped"),
                }
            }
        }
    };
    info!(bridge = %name, transport = cfg.transport.kind(), listen = %cfg.transport.listen(), source = %cfg.source.describe(), "bridge listening");

    let hub = Hub::new(stats.clone());
    let (to_device, mut from_clients) = mpsc::channel::<Bytes>(link::DEPTH);
    let ctx = Arc::new(ClientCtx {
        hub: hub.clone(),
        stats: stats.clone(),
        upgrader: listener.upgrader.clone(),
        to_device,
        token: cfg.auth.as_ref().map(|a| a.token.clone()),
        write_timeout: Duration::from_millis(cfg.transport.write_timeout_ms().max(1)),
    });
    let accept = tokio::spawn({
        let ctx = ctx.clone();
        let name = name.clone();
        async move {
            loop {
                match listener.accept().await {
                    Ok((s, addr)) => {
                        tokio::spawn(serve(ctx.clone(), s, addr));
                    }
                    Err(e) => {
                        // EMFILE and friends: back off instead of spinning.
                        warn!(bridge = %name, error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    });
    drop(ctx);

    let c = &stats.counters;
    let mut detector = ProtocolDetector::new();
    let mut framer = Framer::new(cfg.frame_mode);
    let mut frames = Vec::new();
    let mut backoff = Backoff(BACKOFF_MIN);
    let mut rates = Rates {
        at: Instant::now(),
        frames: 0,
        chunks: 0,
        polls: 0,
        bytes: 0,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    'run: loop {
        stats.set_status(Status::Opening, cfg.source.describe());
        let opened = tokio::select! {
            r = link::open(&cfg.source) => r,
            _ = stop.changed() => break 'run,
        };
        let pause = match opened {
            Err(e) => {
                let msg = format!("{e:#}");
                Counters::add(&c.open_failures, 1);
                warn!(bridge = %name, error = %msg, "cannot open source");
                stats.set_status(
                    if cfg.auto_restart {
                        Status::Waiting
                    } else {
                        Status::Stopped
                    },
                    msg,
                );
                cfg.auto_restart.then(|| backoff.next())
            }
            Ok(mut link) => {
                Counters::add(&c.opens, 1);
                info!(bridge = %name, source = %link.description, "source open");
                stats.set_status(Status::Open, link.description.clone());
                let opened_at = Instant::now();
                let reason: String = loop {
                    tokio::select! {
                        ev = link.rx.recv() => match ev {
                            Some(LinkEvent::Data(b)) => {
                                Counters::add(&c.rx_bytes, b.len() as u64);
                                Counters::add(&c.rx_chunks, 1);
                                detector.feed(&b);
                                stats.record(Direction::Rx, &b);
                                framer.push(b, &mut frames);
                                for f in frames.drain(..) {
                                    hub.broadcast(&f);
                                }
                            }
                            Some(LinkEvent::Closed(r)) => break r,
                            None => break "source closed".into(),
                        },
                        Some(d) = from_clients.recv() => {
                            detector.feed_tx(&d);
                            stats.record(Direction::Tx, &d);
                            let n = d.len() as u64;
                            match link.tx.try_send(d) {
                                Ok(()) => {
                                    Counters::add(&c.tx_bytes, n);
                                    Counters::add(&c.tx_chunks, 1);
                                }
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    Counters::add(&c.tx_dropped, 1);
                                    debug!(bridge = %name, "device write queue full, dropping");
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => break "source closed".into(),
                            }
                        }
                        _ = stats.restart.notified() => break "restart requested".into(),
                        _ = stop.changed() => {
                            if let Some(rest) = framer.flush() {
                                hub.broadcast(&rest);
                            }
                            break 'run;
                        }
                        _ = tick.tick() => rates.update(&stats, &detector),
                    }
                };
                drop(link);
                if let Some(rest) = framer.flush() {
                    hub.broadcast(&rest);
                }
                if opened_at.elapsed() >= HEALTHY_RUN {
                    backoff.reset();
                }
                if reason == "restart requested" {
                    info!(bridge = %name, "restarting source");
                    backoff.reset();
                    continue 'run;
                }
                Counters::add(&c.source_errors, 1);
                warn!(bridge = %name, %reason, "source lost");
                stats.set_status(
                    if cfg.auto_restart {
                        Status::Waiting
                    } else {
                        Status::Stopped
                    },
                    reason,
                );
                cfg.auto_restart.then(|| backoff.next())
            }
        };

        // Wait for the retry (or, with auto_restart off, for a restart request).
        // Client writes meanwhile have nowhere to go.
        let sleep = tokio::time::sleep(pause.unwrap_or(Duration::MAX / 4));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                _ = stats.restart.notified() => {
                    backoff.reset();
                    break;
                }
                _ = stop.changed() => break 'run,
                Some(_) = from_clients.recv() => Counters::add(&c.tx_dropped, 1),
                _ = tick.tick() => rates.update(&stats, &detector),
            }
        }
    }

    hub.close();
    accept.abort();
    stats.set_status(Status::Stopped, "stopped");
    info!(bridge = %name, "bridge stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// A fake request–reply device behind a tcp_client source: answers `A`
    /// with 130 bytes, echoes anything else.
    async fn device() -> (String, tokio::task::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let h = tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 256];
                    while let Ok(n @ 1..) = s.read(&mut buf).await {
                        if &buf[..n] == b"A" {
                            s.write_all(&[0x41; 130]).await.unwrap();
                        } else {
                            s.write_all(&buf[..n]).await.unwrap();
                        }
                    }
                });
            }
        });
        (addr, h)
    }

    async fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn config(device: &str, port: u16, extra: &str) -> BridgeConfig {
        toml::from_str(&format!(
            "name = \"ecu\"\n{extra}\n[source]\ntype = \"tcp_client\"\naddress = \"{device}\"\n[transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:{port}\"\n"
        ))
        .unwrap()
    }

    async fn connect(port: u16) -> TcpStream {
        for _ in 0..100 {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)).await {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("bridge did not listen");
    }

    async fn wait_open(stats: &BridgeStats) {
        for _ in 0..200 {
            if stats.status() == Status::Open {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("source did not open: {:?}", stats.snapshot().detail);
    }

    async fn read_n(s: &mut TcpStream, n: usize) -> Vec<u8> {
        let mut out = vec![0u8; n];
        tokio::time::timeout(Duration::from_secs(2), s.read_exact(&mut out))
            .await
            .unwrap()
            .unwrap();
        out
    }

    async fn start(
        cfg: BridgeConfig,
    ) -> (
        Arc<BridgeStats>,
        watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let stats = Arc::new(BridgeStats::new(&cfg));
        let (stop_tx, stop_rx) = watch::channel(false);
        let h = tokio::spawn(run(cfg, stats.clone(), stop_rx));
        wait_open(&stats).await;
        (stats, stop_tx, h)
    }

    #[tokio::test]
    async fn request_reply_is_immediate_and_counted_once() {
        let (dev, _d) = device().await;
        let port = free_port().await;
        let (stats, stop, h) = start(config(&dev, port, "")).await;
        let mut a = connect(port).await;
        let mut b = connect(port).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let t = Instant::now();
        for _ in 0..20 {
            a.write_all(b"A").await.unwrap();
            assert_eq!(read_n(&mut a, 130).await, vec![0x41; 130]);
        }
        // 20 round trips without waiting on read timeouts
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "took {:?}",
            t.elapsed()
        );
        // the second client sees the same stream (mirror)
        assert_eq!(read_n(&mut b, 130 * 20).await.len(), 2600);
        let s = stats.snapshot();
        assert_eq!(s.rx_bytes, 2600, "RX counted once, not per client");
        assert_eq!(s.tx_bytes, 20);
        assert_eq!(s.clients.len(), 2);
        stop.send(true).unwrap();
        h.await.unwrap();
        assert_eq!(stats.status(), Status::Stopped);
    }

    #[tokio::test]
    async fn read_only_bridge_ignores_client_writes() {
        let (dev, _d) = device().await;
        let port = free_port().await;
        let (stats, _stop, _h) = start(config(&dev, port, "read_only = true")).await;
        let mut a = connect(port).await;
        a.write_all(b"A").await.unwrap();
        let mut buf = [0u8; 8];
        assert!(
            tokio::time::timeout(Duration::from_millis(200), a.read(&mut buf))
                .await
                .is_err()
        );
        assert_eq!(stats.snapshot().tx_bytes, 0);
    }

    #[tokio::test]
    async fn token_auth_and_leftover_bytes() {
        let (dev, _d) = device().await;
        let port = free_port().await;
        let (stats, _stop, _h) = start(config(&dev, port, "[auth]\ntoken = \"s3cret\"")).await;

        let mut bad = connect(port).await;
        bad.write_all(b"wrong\n").await.unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), bad.read_to_end(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(buf, b"auth failed\n");

        let mut good = connect(port).await;
        good.write_all(b"s3cret\r\nA").await.unwrap();
        assert_eq!(read_n(&mut good, 130).await.len(), 130);
        assert_eq!(Counters::get(&stats.counters.auth_failures), 1);
    }

    #[tokio::test]
    async fn exclusive_mode_hands_over_to_newest_client() {
        let (dev, _d) = device().await;
        let port = free_port().await;
        let (stats, _stop, _h) = start(config(&dev, port, "mode = \"exclusive\"")).await;
        let mut old = connect(port).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut new = connect(port).await;
        new.write_all(b"A").await.unwrap();
        assert_eq!(read_n(&mut new, 130).await.len(), 130);
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), old.read_to_end(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert!(buf.is_empty(), "old client closed without data");
        assert_eq!(stats.snapshot().clients.len(), 1);
    }

    #[tokio::test]
    async fn clients_survive_source_loss_and_reconnect() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dev = l.local_addr().unwrap().to_string();
        let port = free_port().await;
        let cfg = config(&dev, port, "");
        let stats = Arc::new(BridgeStats::new(&cfg));
        let (_stop, stop_rx) = watch::channel(false);
        tokio::spawn(run(cfg, stats.clone(), stop_rx));

        let (mut d1, _) = l.accept().await.unwrap();
        wait_open(&stats).await;
        let mut client = connect(port).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        d1.write_all(b"one").await.unwrap();
        assert_eq!(read_n(&mut client, 3).await, b"one");
        drop(d1); // device goes away

        let (mut d2, _) = tokio::time::timeout(Duration::from_secs(5), l.accept())
            .await
            .unwrap()
            .unwrap();
        wait_open(&stats).await;
        d2.write_all(b"two").await.unwrap();
        assert_eq!(read_n(&mut client, 3).await, b"two");
        let s = stats.snapshot();
        assert_eq!((s.opens, s.source_errors), (2, 1));
    }

    #[tokio::test]
    async fn restart_request_reopens_source() {
        let (dev, _d) = device().await;
        let port = free_port().await;
        let (stats, _stop, _h) = start(config(&dev, port, "")).await;
        stats.restart.notify_one();
        tokio::time::sleep(Duration::from_millis(200)).await;
        wait_open(&stats).await;
        let s = stats.snapshot();
        assert_eq!((s.opens, s.source_errors), (2, 0));
    }

    #[tokio::test]
    async fn unreachable_source_waits_and_listener_stays_up() {
        let port = free_port().await;
        let dead = free_port().await;
        let cfg = config(&format!("127.0.0.1:{dead}"), port, "");
        let stats = Arc::new(BridgeStats::new(&cfg));
        let (_stop, stop_rx) = watch::channel(false);
        tokio::spawn(run(cfg, stats.clone(), stop_rx));
        let _c = connect(port).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let s = stats.snapshot();
        assert_eq!(s.status, Status::Waiting);
        assert!(s.open_failures >= 1);
    }

    #[test]
    fn backoff_doubles_to_cap() {
        let mut b = Backoff(BACKOFF_MIN);
        let seq: Vec<u64> = (0..7).map(|_| b.next().as_secs()).collect();
        assert_eq!(seq, vec![1, 2, 4, 8, 16, 30, 30]);
        b.reset();
        assert_eq!(b.next(), BACKOFF_MIN);
    }
}
