use async_trait::async_trait;
use tracing::{debug, warn};

use crate::config::BluetoothConfig;
use crate::error::{IoToNetError, Result};

use super::IoDriver;

/// Bluetooth / BLE driver.
/// Subscribes to a BLE GATT characteristic and converts notifications into a byte stream.
/// Falls back to polling if no characteristic UUID is configured.
pub struct BluetoothDriver {
    cfg: BluetoothConfig,
    rx: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    // BLE objects kept alive to hold the subscription
    _peripheral_guard: Option<BleGuard>,
}

/// Holds the btleplug objects keeping the BLE connection alive.
struct BleGuard {
    #[allow(dead_code)]
    manager: btleplug::platform::Manager,
}

impl BluetoothDriver {
    pub fn new(cfg: BluetoothConfig) -> Self {
        BluetoothDriver { cfg, rx: None, _peripheral_guard: None }
    }
}

#[async_trait]
impl IoDriver for BluetoothDriver {
    async fn open(&mut self) -> Result<()> {
        use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
        use btleplug::platform::{Manager, Peripheral};
        use std::time::Duration;
        use tokio::time::sleep;

        let manager = Manager::new()
            .await
            .map_err(|e| IoToNetError::Driver(format!("BLE manager init failed: {}", e)))?;

        let adapters = manager
            .adapters()
            .await
            .map_err(|e| IoToNetError::Driver(format!("No BLE adapters found: {}", e)))?;

        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| IoToNetError::Driver("No Bluetooth adapter available".to_string()))?;

        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|e| IoToNetError::Driver(format!("BLE scan start failed: {}", e)))?;

        // Allow some discovery time
        sleep(Duration::from_secs(3)).await;
        adapter
            .stop_scan()
            .await
            .map_err(|e| IoToNetError::Driver(format!("BLE scan stop failed: {}", e)))?;

        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|e| IoToNetError::Driver(format!("Peripheral list failed: {}", e)))?;

        // Find peripheral by MAC address
        let target_mac = self.cfg.device.to_uppercase();
        let mut found: Option<Peripheral> = None;
        for p in peripherals {
            let props = p.properties()
                .await
                .map_err(|e| IoToNetError::Driver(format!("Properties failed: {}", e)))?;
            if let Some(props) = props {
                let mac = props.address.to_string().to_uppercase();
                if mac == target_mac {
                    found = Some(p);
                    break;
                }
            }
        }

        let peripheral = found.ok_or_else(|| {
            IoToNetError::Driver(format!("BLE device {} not found", self.cfg.device))
        })?;

        peripheral
            .connect()
            .await
            .map_err(|e| IoToNetError::Driver(format!("BLE connect failed: {}", e)))?;

        peripheral
            .discover_services()
            .await
            .map_err(|e| IoToNetError::Driver(format!("Service discovery failed: {}", e)))?;

        // Find matching characteristic
        let characteristics = peripheral.characteristics();
        let target_char = if let Some(ref char_uuid_str) = self.cfg.characteristic_uuid {
            let target: uuid::Uuid = char_uuid_str
                .parse()
                .map_err(|_| IoToNetError::Driver(format!("Invalid characteristic UUID: {}", char_uuid_str)))?;
            characteristics
                .iter()
                .find(|c| c.uuid == target)
                .cloned()
        } else {
            // Subscribe to first notify characteristic found
            use btleplug::api::CharPropFlags;
            characteristics
                .iter()
                .find(|c| c.properties.contains(CharPropFlags::NOTIFY))
                .cloned()
        };

        let characteristic = target_char.ok_or_else(|| {
            IoToNetError::Driver("No subscribable BLE characteristic found".to_string())
        })?;

        peripheral
            .subscribe(&characteristic)
            .await
            .map_err(|e| IoToNetError::Driver(format!("BLE subscribe failed: {}", e)))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);

        // Notification forwarding task
        let mut notif_stream = peripheral
            .notifications()
            .await
            .map_err(|e| IoToNetError::Driver(format!("Notification stream failed: {}", e)))?;

        tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(data) = notif_stream.next().await {
                if tx.send(data.value).await.is_err() {
                    break;
                }
            }
        });

        self.rx = Some(rx);
        self._peripheral_guard = Some(BleGuard { manager });
        debug!(device = %self.cfg.device, "BLE connected and subscribed");
        Ok(())
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match &mut self.rx {
            None => Err(IoToNetError::Driver("BLE device not connected".to_string())),
            Some(rx) => {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    rx.recv(),
                )
                .await
                {
                    Ok(Some(data)) => {
                        let len = data.len().min(buf.len());
                        buf[..len].copy_from_slice(&data[..len]);
                        Ok(len)
                    }
                    Ok(None) => {
                        warn!("BLE notification channel closed");
                        Err(IoToNetError::Driver("BLE channel closed".to_string()))
                    }
                    Err(_) => Ok(0), // timeout — no data
                }
            }
        }
    }

    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        // BLE write could be implemented via write_characteristic; for now stream is RO
        Err(IoToNetError::Driver(
            "BLE write not implemented; use read_only mode for BLE bridges".to_string(),
        ))
    }

    async fn close(&mut self) -> Result<()> {
        self.rx = None;
        self._peripheral_guard = None;
        Ok(())
    }

    fn description(&self) -> String {
        format!("bluetooth://{}", self.cfg.device)
    }
}
