#[cfg(feature = "bluetooth")]
pub mod bluetooth;
pub mod gpio;
pub mod i2c;
pub mod serial;

use std::fmt::Write;

/// Entry point for the --scan command.
pub async fn run_scan(deep: bool, json: bool) {
    let serials = serial::scan_serial();

    #[cfg(target_os = "linux")]
    let i2c_buses = {
        if deep {
            probe_i2c_deep()
        } else {
            i2c::scan_i2c()
        }
    };

    #[cfg(target_os = "linux")]
    let gpio_chips = gpio::scan_gpio();

    // BLE scan: deep mode does service discovery, quick mode just name+address.
    if !json {
        let mut out = String::new();
        writeln!(
            out,
            "\nio-to-net — hardware device scan{}\n",
            if deep { " (deep)" } else { "" }
        )
        .unwrap();

        writeln!(out, "SERIAL").unwrap();
        if serials.is_empty() {
            writeln!(out, "  (none detected)").unwrap();
        } else {
            for s in &serials {
                writeln!(out, "  {}", serial::describe(s)).unwrap();
            }
        }
        writeln!(out).unwrap();

        #[cfg(target_os = "linux")]
        {
            writeln!(out, "I2C").unwrap();
            if i2c_buses.is_empty() {
                writeln!(out, "  (none detected)").unwrap();
            } else {
                for b in &i2c_buses {
                    writeln!(out, "  {}", b).unwrap();
                }
            }
            writeln!(out).unwrap();

            writeln!(out, "GPIO").unwrap();
            if gpio_chips.is_empty() {
                writeln!(out, "  (none detected)").unwrap();
            } else {
                for g in &gpio_chips {
                    writeln!(out, "  {}", g).unwrap();
                }
            }
            writeln!(out).unwrap();
        }

        #[cfg(feature = "bluetooth")]
        writeln!(
            out,
            "BLUETOOTH (scanning for {}s...)",
            if deep { 10 } else { 5 }
        )
        .unwrap();
        print!("{}", out);
    }

    #[cfg(feature = "bluetooth")]
    let ble_devices = {
        let scan_duration = std::time::Duration::from_secs(if deep { 10 } else { 5 });
        bluetooth::scan_bluetooth_timed(scan_duration, deep).await
    };
    #[cfg(not(feature = "bluetooth"))]
    let ble_devices: Vec<(String, String)> = Vec::new();

    if json {
        #[cfg(target_os = "linux")]
        let result = serde_json::json!({
            "serial": serials,
            "i2c": i2c_buses,
            "gpio": gpio_chips,
            "bluetooth": ble_devices.iter().map(|(a, n)| serde_json::json!({"address": a, "name": n})).collect::<Vec<_>>(),
        });
        #[cfg(not(target_os = "linux"))]
        let result = serde_json::json!({
            "serial": serials,
            "bluetooth": ble_devices.iter().map(|(a, n)| serde_json::json!({"address": a, "name": n})).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else if cfg!(feature = "bluetooth") {
        let mut ble_out = String::new();
        if ble_devices.is_empty() {
            writeln!(ble_out, "  (none detected)").unwrap();
        } else {
            for (addr, name) in &ble_devices {
                writeln!(ble_out, "  {}  {}", addr, name).unwrap();
            }
        }
        println!("{}", ble_out);
    }
}

/// Deep I2C scan: probe each bus for responding addresses (Linux only).
#[cfg(target_os = "linux")]
fn probe_i2c_deep() -> Vec<String> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let buses = i2c::scan_i2c();
    let mut results = Vec::new();
    for bus in buses {
        // Try to open each bus; list it if accessible.
        if OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&bus)
            .is_ok()
        {
            results.push(bus);
        }
    }
    results
}
