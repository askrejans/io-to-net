//! BLE GATT source (btleplug): notifications of one characteristic become the
//! byte stream; client writes go to `write_characteristic_uuid` if set.

#[cfg(feature = "bluetooth")]
pub use ble::BluetoothDriver;

#[cfg(feature = "bluetooth")]
mod ble {
    use crate::config::BluetoothConfig;
    use crate::drivers::IoDriver;
    use crate::error::{IoToNetError, Result};
    use btleplug::api::{
        Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
        WriteType,
    };
    use btleplug::platform::{Manager, Peripheral};
    use futures_util::StreamExt;

    pub struct BluetoothDriver {
        cfg: BluetoothConfig,
        peripheral: Option<Peripheral>,
        write_char: Option<Characteristic>,
        rx: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    }

    impl BluetoothDriver {
        pub fn new(cfg: BluetoothConfig) -> Self {
            BluetoothDriver {
                cfg,
                peripheral: None,
                write_char: None,
                rx: None,
            }
        }
    }

    fn err(what: &str, e: impl std::fmt::Display) -> IoToNetError {
        IoToNetError::Driver(format!("BLE {what}: {e}"))
    }

    fn uuid(s: &str) -> Result<uuid::Uuid> {
        s.parse()
            .map_err(|_| IoToNetError::Driver(format!("invalid UUID {s}")))
    }

    impl IoDriver for BluetoothDriver {
        async fn open(&mut self) -> Result<()> {
            let manager = Manager::new().await.map_err(|e| err("manager", e))?;
            let adapter = manager
                .adapters()
                .await
                .map_err(|e| err("adapters", e))?
                .into_iter()
                .next()
                .ok_or_else(|| IoToNetError::Driver("no Bluetooth adapter".into()))?;
            adapter
                .start_scan(ScanFilter::default())
                .await
                .map_err(|e| err("scan", e))?;
            tokio::time::sleep(std::time::Duration::from_secs(self.cfg.scan_secs.max(1))).await;
            let _ = adapter.stop_scan().await;

            let want = self.cfg.device.to_uppercase();
            let mut found = None;
            for p in adapter
                .peripherals()
                .await
                .map_err(|e| err("peripherals", e))?
            {
                let address = p
                    .properties()
                    .await
                    .ok()
                    .flatten()
                    .map(|pr| pr.address.to_string().to_uppercase());
                // macOS hides MAC addresses; it identifies peripherals by UUID.
                if address.as_deref() == Some(want.as_str())
                    || p.id().to_string().to_uppercase() == want
                {
                    found = Some(p);
                    break;
                }
            }
            let p = found.ok_or_else(|| {
                IoToNetError::Driver(format!("BLE device {} not found", self.cfg.device))
            })?;
            p.connect().await.map_err(|e| err("connect", e))?;
            p.discover_services()
                .await
                .map_err(|e| err("services", e))?;
            let chars = p.characteristics();
            let notify = match &self.cfg.characteristic_uuid {
                Some(u) => {
                    let u = uuid(u)?;
                    chars.iter().find(|c| c.uuid == u).cloned()
                }
                None => chars
                    .iter()
                    .find(|c| {
                        c.properties.contains(CharPropFlags::NOTIFY)
                            && self.cfg.service_uuid.as_deref().is_none_or(|s| {
                                s.parse::<uuid::Uuid>().ok() == Some(c.service_uuid)
                            })
                    })
                    .cloned(),
            }
            .ok_or_else(|| {
                IoToNetError::Driver("no matching notifying BLE characteristic".into())
            })?;
            if let Some(w) = &self.cfg.write_characteristic_uuid {
                let w = uuid(w)?;
                self.write_char =
                    Some(chars.iter().find(|c| c.uuid == w).cloned().ok_or_else(|| {
                        IoToNetError::Driver(format!("BLE write characteristic {w} not found"))
                    })?);
            }
            p.subscribe(&notify)
                .await
                .map_err(|e| err("subscribe", e))?;
            let mut notifications = p
                .notifications()
                .await
                .map_err(|e| err("notifications", e))?;
            let (tx, rx) = tokio::sync::mpsc::channel(256);
            tokio::spawn(async move {
                while let Some(n) = notifications.next().await {
                    if tx.send(n.value).await.is_err() {
                        break;
                    }
                }
            });
            self.peripheral = Some(p);
            self.rx = Some(rx);
            Ok(())
        }

        async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let rx = self
                .rx
                .as_mut()
                .ok_or_else(|| IoToNetError::Driver("BLE not connected".into()))?;
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Some(d)) => {
                    let n = d.len().min(buf.len());
                    buf[..n].copy_from_slice(&d[..n]);
                    Ok(n)
                }
                Ok(None) => Err(IoToNetError::Driver("BLE notifications ended".into())),
                Err(_) => Ok(0),
            }
        }

        async fn write(&mut self, data: &[u8]) -> Result<usize> {
            let (Some(p), Some(c)) = (&self.peripheral, &self.write_char) else {
                return Err(IoToNetError::Driver(
                    "set write_characteristic_uuid to write to this BLE device".into(),
                ));
            };
            p.write(c, data, WriteType::WithoutResponse)
                .await
                .map_err(|e| err("write", e))?;
            Ok(data.len())
        }

        fn description(&self) -> String {
            format!("bluetooth://{}", self.cfg.device)
        }
    }

    impl Drop for BluetoothDriver {
        fn drop(&mut self) {
            if let Some(p) = self.peripheral.take() {
                tokio::spawn(async move {
                    let _ = p.disconnect().await;
                });
            }
        }
    }
}
