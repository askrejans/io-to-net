//! Serial ports: a reader and a writer thread on cloned handles of one port.

use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use serialport::{DataBits, FlowControl, Parity, SerialPortType, StopBits};
use tracing::{debug, warn};

use super::{Link, LinkEvent};
use crate::config::{SerialConfig, SerialFlowControl, SerialParity};

/// Consecutive BrokenPipe/Interrupted reads tolerated before reopening: Linux
/// UARTs report them on modem-line glitches and they usually clear.
const MAX_TRANSIENT: u32 = 30;

/// `usb:VID:PID[:SERIAL]` or `usb:name=<text>`, matched against attached ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Ids {
        vid: u16,
        pid: u16,
        serial: Option<String>,
    },
    Name(String),
}

impl Selector {
    pub fn parse(spec: &str) -> Result<Self> {
        let rest = spec
            .strip_prefix("usb:")
            .context("USB selectors start with usb:")?;
        if let Some(name) = rest.strip_prefix("name=") {
            if name.is_empty() {
                bail!("usb:name= needs some text to match");
            }
            return Ok(Selector::Name(name.to_lowercase()));
        }
        let mut parts = rest.splitn(3, ':');
        let hex = |p: Option<&str>, what: &str| -> Result<u16> {
            let p = p
                .filter(|p| !p.is_empty())
                .with_context(|| format!("{spec}: missing {what}"))?;
            u16::from_str_radix(p, 16)
                .with_context(|| format!("{spec}: {what} must be 4 hex digits"))
        };
        let vid = hex(parts.next(), "VID")?;
        let pid = hex(parts.next(), "PID")?;
        let serial = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Selector::Ids { vid, pid, serial })
    }

    fn matches(&self, info: &serialport::UsbPortInfo) -> bool {
        match self {
            Selector::Ids { vid, pid, serial } => {
                info.vid == *vid
                    && info.pid == *pid
                    && serial
                        .as_ref()
                        .is_none_or(|s| info.serial_number.as_deref() == Some(s.as_str()))
            }
            Selector::Name(n) => [&info.product, &info.manufacturer]
                .iter()
                .any(|f| f.as_deref().is_some_and(|v| v.to_lowercase().contains(n))),
        }
    }
}

/// Turn a configured port into a device path, re-resolved on every open so a
/// re-plugged adapter is found under its new name.
pub fn resolve(port: &str) -> Result<String> {
    if !port.starts_with("usb:") {
        return Ok(port.to_string());
    }
    let sel = Selector::parse(port)?;
    let mut found: Vec<String> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| matches!(&p.port_type, SerialPortType::UsbPort(info) if sel.matches(info)))
        .map(|p| p.port_name)
        // macOS lists each device twice; tty.* blocks on open waiting for DCD.
        .filter(|n| !n.starts_with("/dev/tty."))
        .collect();
    found.sort();
    match found.len() {
        0 => bail!("no attached serial port matches {port}"),
        1 => Ok(found.remove(0)),
        _ => bail!(
            "{port} matches {} ports ({}); add the serial number",
            found.len(),
            found.join(", ")
        ),
    }
}

pub async fn open(cfg: SerialConfig) -> Result<Link> {
    // Opening a port can block (USB enumeration, DCD); keep it off the runtime.
    tokio::task::spawn_blocking(move || open_blocking(&cfg)).await?
}

fn open_blocking(cfg: &SerialConfig) -> Result<Link> {
    let path = resolve(&cfg.port)?;
    let mut port = serialport::new(&path, cfg.baud)
        .data_bits(match cfg.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })
        .parity(match cfg.parity {
            SerialParity::None => Parity::None,
            SerialParity::Odd => Parity::Odd,
            SerialParity::Even => Parity::Even,
        })
        .stop_bits(if cfg.stop_bits == 2 {
            StopBits::Two
        } else {
            StopBits::One
        })
        .flow_control(match cfg.flow_control {
            SerialFlowControl::None => FlowControl::None,
            SerialFlowControl::Software => FlowControl::Software,
            SerialFlowControl::Hardware => FlowControl::Hardware,
        })
        .timeout(Duration::from_millis(cfg.read_timeout_ms.max(1)))
        .open()
        .with_context(|| format!("cannot open serial port {path}"))?;
    if let Some(dtr) = cfg.dtr {
        port.write_data_terminal_ready(dtr).context("setting DTR")?;
    }
    if let Some(rts) = cfg.rts {
        port.write_request_to_send(rts).context("setting RTS")?;
    }
    let mut writer = port
        .try_clone()
        .context("cloning the serial handle for writing")?;
    let description = format!("serial://{path}@{}", cfg.baud);
    debug!(port = %path, baud = cfg.baud, "serial port opened");

    let (ev_tx, ev_rx, w_tx, mut w_rx) = Link::channels();
    let stop = Arc::new(AtomicBool::new(false));

    let reader_stop = stop.clone();
    let reader_tx = ev_tx.clone();
    std::thread::Builder::new()
        .name(format!("serial-rx {path}"))
        .spawn(move || {
            let mut buf = [0u8; 4096];
            let mut transient = 0u32;
            while !reader_stop.load(Ordering::Relaxed) && !reader_tx.is_closed() {
                match port.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        transient = 0;
                        if reader_tx
                            .blocking_send(LinkEvent::Data(Bytes::copy_from_slice(&buf[..n])))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                    Err(e)
                        if matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::Interrupted)
                            && transient < MAX_TRANSIENT =>
                    {
                        transient += 1;
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        let _ = reader_tx.blocking_send(LinkEvent::Closed(format!("read: {e}")));
                        break;
                    }
                }
            }
        })?;

    std::thread::Builder::new()
        .name(format!("serial-tx {path}"))
        .spawn(move || {
            while let Some(chunk) = w_rx.blocking_recv() {
                if let Err(e) = writer.write_all(&chunk).and_then(|_| writer.flush()) {
                    warn!(error = %e, "serial write failed");
                    let _ = ev_tx.blocking_send(LinkEvent::Closed(format!("write: {e}")));
                    break;
                }
            }
            // The link was dropped (or the write failed): stop the reader too.
            stop.store(true, Ordering::Relaxed);
        })?;

    Ok(Link {
        rx: ev_rx,
        tx: w_tx,
        description,
        _guard: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse() {
        assert_eq!(
            Selector::parse("usb:1546:01a8").unwrap(),
            Selector::Ids {
                vid: 0x1546,
                pid: 0x01a8,
                serial: None
            }
        );
        assert_eq!(
            Selector::parse("usb:303A:1001:68:B6:B3:33:C6:AC").unwrap(),
            Selector::Ids {
                vid: 0x303a,
                pid: 0x1001,
                serial: Some("68:B6:B3:33:C6:AC".into())
            }
        );
        assert_eq!(
            Selector::parse("usb:name=u-blox").unwrap(),
            Selector::Name("u-blox".into())
        );
        for bad in ["usb:", "usb:zz:1", "usb:1546", "usb:name=", "/dev/ttyUSB0"] {
            assert!(Selector::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn selector_matches_usb_info() {
        let info = serialport::UsbPortInfo {
            vid: 0x1546,
            pid: 0x01a8,
            serial_number: Some("ABC".into()),
            manufacturer: Some("u-blox AG".into()),
            product: Some("u-blox GNSS receiver".into()),
        };
        assert!(Selector::parse("usb:1546:01a8").unwrap().matches(&info));
        assert!(Selector::parse("usb:1546:01a8:ABC").unwrap().matches(&info));
        assert!(!Selector::parse("usb:1546:01a8:XYZ").unwrap().matches(&info));
        assert!(Selector::parse("usb:name=GNSS").unwrap().matches(&info));
        assert!(!Selector::parse("usb:name=ftdi").unwrap().matches(&info));
    }

    #[test]
    fn plain_paths_pass_through_and_missing_usb_devices_explain() {
        assert_eq!(resolve("/dev/ttyUSB7").unwrap(), "/dev/ttyUSB7");
        let err = resolve("usb:dead:beef").unwrap_err().to_string();
        assert!(err.contains("no attached serial port"), "{err}");
    }

    #[tokio::test]
    async fn opening_a_missing_port_fails_with_its_path() {
        let cfg = SerialConfig {
            port: "/dev/nonexistent_io_to_net".into(),
            ..SerialConfig::default()
        };
        let err = format!("{:#}", open(cfg).await.err().unwrap());
        assert!(err.contains("/dev/nonexistent_io_to_net"), "{err}");
    }
}
