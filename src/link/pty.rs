//! Pseudo-terminal source: programs open the slave end (or its symlink) as if
//! it were a serial port, and io-to-net exposes what they write.

#![cfg(unix)]

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use bytes::Bytes;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::pty::openpty;
use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};

use super::{Link, LinkEvent};
use crate::config::PtyConfig;

/// Keeps the slave end open (so the master does not fail between programs)
/// and removes the symlink when the link closes.
struct Guard {
    _slave: OwnedFd,
    symlink: Option<String>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(link) = &self.symlink {
            let _ = std::fs::remove_file(link);
        }
    }
}

pub fn open(cfg: &PtyConfig) -> Result<Link> {
    let pair = openpty(None, None).context("openpty")?;
    let mut attrs = tcgetattr(&pair.slave).context("tcgetattr")?;
    cfmakeraw(&mut attrs);
    tcsetattr(&pair.slave, SetArg::TCSANOW, &attrs).context("tcsetattr")?;
    let slave_path = nix::unistd::ttyname(&pair.slave)
        .context("ttyname")?
        .display()
        .to_string();
    if let Some(link) = &cfg.symlink {
        let _ = std::fs::remove_file(link);
        std::os::unix::fs::symlink(&slave_path, link).with_context(|| format!("symlink {link}"))?;
    }
    let description = match &cfg.symlink {
        Some(l) => format!("pty://{l} → {slave_path}"),
        None => format!("pty://{slave_path}"),
    };

    let master = File::from(pair.master);
    let mut reader = master.try_clone()?;
    let mut writer = master;
    let (ev_tx, ev_rx, w_tx, mut w_rx) = Link::channels();
    let stop = Arc::new(AtomicBool::new(false));

    let reader_stop = stop.clone();
    let reader_tx = ev_tx.clone();
    std::thread::Builder::new()
        .name("pty-rx".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while !reader_stop.load(Ordering::Relaxed) && !reader_tx.is_closed() {
                // Wait at most 50 ms so a dropped link is noticed.
                let ready = {
                    let mut fds = [PollFd::new(reader.as_fd(), PollFlags::POLLIN)];
                    poll(&mut fds, PollTimeout::from(50u16)).unwrap_or(0)
                };
                if ready == 0 {
                    continue;
                }
                match reader.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        if reader_tx
                            .blocking_send(LinkEvent::Data(Bytes::copy_from_slice(&buf[..n])))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e)
                        if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {}
                    Err(e) => {
                        let _ =
                            reader_tx.blocking_send(LinkEvent::Closed(format!("pty read: {e}")));
                        break;
                    }
                }
            }
        })?;
    std::thread::Builder::new()
        .name("pty-tx".into())
        .spawn(move || {
            while let Some(chunk) = w_rx.blocking_recv() {
                if writer.write_all(&chunk).is_err() {
                    break;
                }
            }
            stop.store(true, Ordering::Relaxed);
        })?;

    Ok(Link {
        rx: ev_rx,
        tx: w_tx,
        description,
        _guard: Some(Box::new(Guard {
            _slave: pair.slave,
            symlink: cfg.symlink.clone(),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bytes_cross_the_pty_both_ways() {
        let symlink = format!("/tmp/io-to-net-test-{}", std::process::id());
        let mut link = open(&PtyConfig {
            symlink: Some(symlink.clone()),
        })
        .unwrap();
        // A program opens the slave by its symlink, like a serial port.
        let mut dev = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&symlink)
            .unwrap();
        dev.write_all(b"$GPRMC\r\n").unwrap();
        let mut got = Vec::new();
        while got.len() < 8 {
            match tokio::time::timeout(std::time::Duration::from_secs(2), link.rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                LinkEvent::Data(b) => got.extend_from_slice(&b),
                LinkEvent::Closed(e) => panic!("{e}"),
            }
        }
        assert_eq!(got, b"$GPRMC\r\n");
        link.tx.send(Bytes::from_static(b"OK")).await.unwrap();
        let mut back = [0u8; 2];
        dev.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"OK");
        drop(link);
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            std::fs::symlink_metadata(&symlink).is_err(),
            "symlink removed with the link"
        );
    }
}
