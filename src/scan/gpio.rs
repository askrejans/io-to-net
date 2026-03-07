/// Enumerate GPIO chips (Linux only via sysfs).
#[cfg(target_os = "linux")]
pub fn scan_gpio() -> Vec<String> {
    use std::path::Path;

    let mut chips = Vec::new();
    let dev = Path::new("/dev");
    if let Ok(rd) = std::fs::read_dir(dev) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("gpiochip") {
                chips.push(format!("/dev/{}", name.to_string_lossy()));
            }
        }
    }
    chips.sort();
    chips
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn scan_gpio() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_sorted_vec() {
        let chips = scan_gpio();
        let mut sorted = chips.clone();
        sorted.sort();
        assert_eq!(chips, sorted);
    }

    #[test]
    fn entries_start_with_dev_prefix() {
        for entry in scan_gpio() {
            assert!(entry.starts_with("/dev/"), "unexpected entry: {}", entry);
        }
    }
}
