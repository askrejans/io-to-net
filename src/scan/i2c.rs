/// Enumerate I2C buses (Linux only via sysfs).
#[cfg(target_os = "linux")]
pub fn scan_i2c() -> Vec<String> {
    use std::path::Path;

    let mut buses = Vec::new();
    let dev = Path::new("/dev");
    if let Ok(rd) = std::fs::read_dir(dev) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("i2c-") {
                buses.push(format!("/dev/{}", name.to_string_lossy()));
            }
        }
    }
    buses.sort();
    buses
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn scan_i2c() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_sorted_vec() {
        let buses = scan_i2c();
        let mut sorted = buses.clone();
        sorted.sort();
        assert_eq!(buses, sorted);
    }

    #[test]
    fn entries_start_with_dev_prefix() {
        for entry in scan_i2c() {
            assert!(entry.starts_with("/dev/"), "unexpected entry: {}", entry);
        }
    }
}
