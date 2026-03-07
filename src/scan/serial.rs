/// Enumerate available serial ports.
pub fn scan_serial() -> Vec<String> {
    match serialport::available_ports() {
        Err(_) => Vec::new(),
        Ok(ports) => ports
            .into_iter()
            .map(|p| {
                let extra = match &p.port_type {
                    serialport::SerialPortType::UsbPort(info) => {
                        let manufacturer = info.manufacturer.as_deref().unwrap_or("unknown");
                        let product = info.product.as_deref().unwrap_or("unknown");
                        format!("  [USB] {} / {}", manufacturer, product)
                    }
                    serialport::SerialPortType::BluetoothPort => "  [Bluetooth]".to_string(),
                    serialport::SerialPortType::PciPort => "  [PCI]".to_string(),
                    serialport::SerialPortType::Unknown => String::new(),
                };
                format!("{}{}", p.port_name, extra)
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_returns_vec() {
        // Returns a Vec on any platform; empty on CI without hardware is fine.
        let ports = scan_serial();
        for p in &ports {
            assert!(!p.is_empty());
        }
    }

    #[test]
    fn scan_serial_is_idempotent() {
        let a = scan_serial();
        let b = scan_serial();
        assert_eq!(a.len(), b.len());
    }
}
