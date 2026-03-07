use std::time::Duration;

/// Perform a BLE scan for the given duration.
///
/// If `deep` is true, services are discovered for each peripheral (slower).
/// Returns a sorted list of `(MAC address, display name)` pairs.
pub async fn scan_bluetooth_timed(duration: Duration, deep: bool) -> Vec<(String, String)> {
    use btleplug::api::{Central, Manager as _, ScanFilter};
    use btleplug::platform::Manager;

    let manager = match Manager::new().await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("  Bluetooth unavailable: {}", e);
            return Vec::new();
        }
    };

    let adapters = match manager.adapters().await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("  No Bluetooth adapters: {}", e);
            return Vec::new();
        }
    };

    let adapter = match adapters.into_iter().next() {
        Some(a) => a,
        None => {
            eprintln!("  No Bluetooth adapter found");
            return Vec::new();
        }
    };

    if let Err(e) = adapter.start_scan(ScanFilter::default()).await {
        eprintln!("  BLE scan failed: {}", e);
        return Vec::new();
    }

    tokio::time::sleep(duration).await;
    let _ = adapter.stop_scan().await;

    let peripherals = match adapter.peripherals().await {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    for peripheral in peripherals {
        use btleplug::api::Peripheral;

        // Deep scan: connect and discover services.
        if deep {
            let _ = peripheral.connect().await;
            let _ = peripheral.discover_services().await;
            // Services are attached to the peripheral; we just note the count.
        }

        if let Ok(Some(props)) = peripheral.properties().await {
            let addr = props.address.to_string();
            let name = props.local_name.unwrap_or_else(|| "(unnamed)".to_string());
            let entry = if deep {
                let services = peripheral.services();
                if services.is_empty() {
                    name
                } else {
                    format!("{} [{} services]", name, services.len())
                }
            } else {
                name
            };
            results.push((addr, entry));
            if deep {
                let _ = peripheral.disconnect().await;
            }
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// Convenience wrapper used by existing code paths (5-second quick scan).
#[allow(dead_code)]
pub async fn scan_bluetooth() -> Vec<(String, String)> {
    scan_bluetooth_timed(Duration::from_secs(5), false).await
}

