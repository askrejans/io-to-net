//! Serial port enumeration with the USB selector to put in a config.

use serde::Serialize;
use serialport::SerialPortType;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PortInfo {
    pub path: String,
    /// usb | bluetooth | pci | other
    pub kind: &'static str,
    pub vid: Option<String>,
    pub pid: Option<String>,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    /// `usb:VID:PID[:SERIAL]`: finds the device again after re-plugging.
    pub selector: Option<String>,
}

pub fn scan_serial() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        // macOS lists each device twice; the tty.* node blocks on open.
        .filter(|p| !p.port_name.starts_with("/dev/tty."))
        .map(|p| {
            let mut info = PortInfo {
                path: p.port_name,
                kind: "other",
                vid: None,
                pid: None,
                serial: None,
                manufacturer: None,
                product: None,
                selector: None,
            };
            match p.port_type {
                SerialPortType::UsbPort(u) => {
                    let (vid, pid) = (format!("{:04X}", u.vid), format!("{:04X}", u.pid));
                    info.selector = Some(match &u.serial_number {
                        Some(s) if !s.contains(':') && !s.is_empty() => {
                            format!("usb:{vid}:{pid}:{s}")
                        }
                        _ => format!("usb:{vid}:{pid}"),
                    });
                    info.kind = "usb";
                    info.vid = Some(vid);
                    info.pid = Some(pid);
                    info.serial = u.serial_number;
                    info.manufacturer = u.manufacturer;
                    info.product = u.product;
                }
                SerialPortType::BluetoothPort => info.kind = "bluetooth",
                SerialPortType::PciPort => info.kind = "pci",
                SerialPortType::Unknown => {}
            }
            info
        })
        .collect();
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    ports
}

pub fn describe(p: &PortInfo) -> String {
    let what = [p.manufacturer.as_deref(), p.product.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" / ");
    match &p.selector {
        Some(sel) => format!("{:<28} {:<10} {sel}  {what}", p.path, p.kind),
        None => format!("{:<28} {}", p.path, p.kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_parse_back() {
        for p in scan_serial() {
            assert!(!p.path.starts_with("/dev/tty."));
            if let Some(sel) = &p.selector {
                crate::link::serial::Selector::parse(sel).unwrap();
            }
        }
    }
}
