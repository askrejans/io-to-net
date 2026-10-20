//! Link adapter for polled drivers: one task alternates between forwarding
//! pending writes and the driver's next (time-bounded) read. Reads are not
//! cancelled mid-flight, because several drivers move the device into a
//! blocking thread for the duration of a read.
// Only Linux drivers and BLE use the polled path.
#![cfg_attr(not(any(target_os = "linux", feature = "bluetooth")), allow(dead_code))]

use anyhow::{Result, bail};
use bytes::Bytes;

use super::{Link, LinkEvent};
use crate::config::SourceConfig;
use crate::drivers::IoDriver;

pub async fn open(source: &SourceConfig) -> Result<Link> {
    match source {
        #[cfg(target_os = "linux")]
        SourceConfig::I2c(c) => start(crate::drivers::i2c::I2cDriver::new(c.clone())).await,
        #[cfg(target_os = "linux")]
        SourceConfig::Gpio(c) => start(crate::drivers::gpio::GpioDriver::new(c.clone())).await,
        #[cfg(target_os = "linux")]
        SourceConfig::Canbus(c) => {
            start(crate::drivers::canbus::CanbusDriver::new(c.clone())).await
        }
        #[cfg(target_os = "linux")]
        SourceConfig::Onewire(c) => {
            start(crate::drivers::onewire::OnewireDriver::new(c.clone())).await
        }
        #[cfg(feature = "bluetooth")]
        SourceConfig::Bluetooth(c) => {
            start(crate::drivers::bluetooth::BluetoothDriver::new(c.clone())).await
        }
        #[cfg(not(feature = "bluetooth"))]
        SourceConfig::Bluetooth(_) => {
            bail!("this build has no Bluetooth support (feature \"bluetooth\")")
        }
        SourceConfig::Spi(_) => bail!("SPI sources are not implemented yet"),
        other => bail!("{} sources need Linux", other.kind()),
    }
}

pub async fn start<D: IoDriver + 'static>(mut driver: D) -> Result<Link> {
    driver.open().await?;
    let description = driver.description();
    let (ev_tx, ev_rx, w_tx, mut w_rx) = Link::channels();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            loop {
                match w_rx.try_recv() {
                    Ok(chunk) => {
                        if let Err(e) = driver.write(&chunk).await {
                            tracing::warn!(error = %e, "device write failed");
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
                }
            }
            if ev_tx.is_closed() {
                return;
            }
            match driver.read(&mut buf).await {
                Ok(0) => {}
                Ok(n) => {
                    if ev_tx
                        .send(LinkEvent::Data(Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(e) => {
                    let _ = ev_tx.send(LinkEvent::Closed(e.to_string())).await;
                    return;
                }
            }
        }
    });
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
    use crate::error::IoToNetError;
    use std::sync::{Arc, Mutex};

    /// A sensor that produces one 3-byte reading per poll and records writes.
    struct FakeSensor {
        reads: u8,
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl IoDriver for FakeSensor {
        async fn open(&mut self) -> crate::error::Result<()> {
            Ok(())
        }
        async fn read(&mut self, buf: &mut [u8]) -> crate::error::Result<usize> {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            self.reads += 1;
            if self.reads > 3 {
                return Err(IoToNetError::Driver("sensor gone".into()));
            }
            buf[..3].copy_from_slice(&[self.reads, 0xAA, 0x55]);
            Ok(3)
        }
        async fn write(&mut self, data: &[u8]) -> crate::error::Result<usize> {
            self.written.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn description(&self) -> String {
            "fake://sensor".into()
        }
    }

    #[tokio::test]
    async fn polled_driver_reads_writes_and_reports_failure() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut link = start(FakeSensor {
            reads: 0,
            written: written.clone(),
        })
        .await
        .unwrap();
        assert_eq!(link.description, "fake://sensor");
        link.tx
            .send(Bytes::from_static(&[0x6B, 0x00]))
            .await
            .unwrap();
        let mut readings = 0;
        loop {
            match link.rx.recv().await.unwrap() {
                LinkEvent::Data(d) => {
                    assert_eq!(&d[1..], &[0xAA, 0x55]);
                    readings += 1;
                }
                LinkEvent::Closed(e) => {
                    assert!(e.contains("sensor gone"));
                    break;
                }
            }
        }
        assert_eq!(readings, 3);
        assert_eq!(*written.lock().unwrap(), vec![0x6B, 0x00]);
    }
}
