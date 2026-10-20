//! Configuration schema (TOML). Every field has a default, so a file only
//! needs what differs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// trace | debug | info | warn | error
    pub log_level: String,
    pub bridges: Vec<BridgeConfig>,
    pub http_api: Option<HttpApiConfig>,
    /// Separate Prometheus listener (the HTTP API also serves /metrics).
    pub metrics: Option<MetricsConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            log_level: "info".into(),
            bridges: Vec::new(),
            http_api: None,
            metrics: None,
        }
    }
}

/// One source mapped to one network transport.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfig {
    pub name: String,
    pub source: SourceConfig,
    pub transport: TransportConfig,
    /// Clients must send this token as their first line before any data flows.
    pub auth: Option<AuthConfig>,
    /// Maximum simultaneous clients (default: unlimited).
    pub max_clients: Option<usize>,
    #[serde(default)]
    pub mode: BridgeMode,
    /// Clients cannot write to the device.
    #[serde(default)]
    pub read_only: bool,
    /// Reopen the source after a failure (default: true).
    #[serde(default = "default_true")]
    pub auto_restart: bool,
    #[serde(default)]
    pub frame_mode: FrameMode,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BridgeMode {
    /// Every client receives the stream; every client may write (default).
    #[default]
    Mirror,
    /// One client at a time: a new connection replaces the previous one, as on
    /// a serial-over-TCP bridge (clients such as TunerStudio reconnect with a
    /// fresh socket while the old one still looks open).
    Exclusive,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameMode {
    /// Bytes as they arrive (default).
    #[default]
    None,
    /// `[u16 BE length][data]` per source read (I2C/SPI sensor reads).
    LengthPrefix,
    /// One frame per `\n`, terminator included (NMEA, AT, SCPI).
    Line,
    /// One frame per `0x00`, terminator included (COBS).
    NullTerminated,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceConfig {
    Serial(SerialConfig),
    /// Connect to a TCP device (Wi-Fi serial bridge, another io-to-net).
    TcpClient(TcpClientConfig),
    Pty(PtyConfig),
    UnixSocket(UnixSocketConfig),
    I2c(I2cConfig),
    Spi(SpiConfig),
    Gpio(GpioConfig),
    Bluetooth(BluetoothConfig),
    Canbus(CanbusConfig),
    Onewire(OnewireConfig),
}

impl SourceConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            SourceConfig::Serial(_) => "serial",
            SourceConfig::TcpClient(_) => "tcp_client",
            SourceConfig::Pty(_) => "pty",
            SourceConfig::UnixSocket(_) => "unix_socket",
            SourceConfig::I2c(_) => "i2c",
            SourceConfig::Spi(_) => "spi",
            SourceConfig::Gpio(_) => "gpio",
            SourceConfig::Bluetooth(_) => "bluetooth",
            SourceConfig::Canbus(_) => "canbus",
            SourceConfig::Onewire(_) => "onewire",
        }
    }

    /// `serial:///dev/ttyUSB0@115200` style description for logs, API, metrics.
    pub fn describe(&self) -> String {
        match self {
            SourceConfig::Serial(s) => format!("serial://{}@{}", s.port, s.baud),
            SourceConfig::TcpClient(s) => format!("tcp://{}", s.address),
            SourceConfig::Pty(s) => format!("pty://{}", s.symlink.as_deref().unwrap_or("(auto)")),
            SourceConfig::UnixSocket(s) => format!("unix://{}", s.path),
            SourceConfig::I2c(s) => format!("i2c://{}@0x{:02X}", s.bus, s.address),
            SourceConfig::Spi(s) => format!("spi://{}", s.device),
            SourceConfig::Gpio(s) => format!("gpio://{}:{}", s.chip, s.line),
            SourceConfig::Bluetooth(s) => format!("bluetooth://{}", s.device),
            SourceConfig::Canbus(s) => format!("canbus://{} ({})", s.interface, s.format.name()),
            SourceConfig::Onewire(s) => format!("onewire://{}", s.device),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SerialConfig {
    /// A device path (`/dev/ttyUSB0`, `COM3`) or a USB selector that survives
    /// re-plugging: `usb:VID:PID`, `usb:VID:PID:SERIAL` (hex IDs), or
    /// `usb:name=<substring of product/manufacturer>`.
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: SerialParity,
    pub stop_bits: u8,
    pub flow_control: SerialFlowControl,
    /// How long one read waits for data before re-checking for shutdown.
    pub read_timeout_ms: u64,
    /// Data Terminal Ready after opening. Unset: leave as the OS opened it.
    /// `false` keeps Arduino/ESP boards with auto-reset from restarting.
    pub dtr: Option<bool>,
    /// Request To Send after opening (same rules as `dtr`).
    pub rts: Option<bool>,
}

impl Default for SerialConfig {
    fn default() -> Self {
        SerialConfig {
            port: "/dev/ttyUSB0".into(),
            baud: 115200,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: 1,
            flow_control: SerialFlowControl::None,
            read_timeout_ms: 50,
            dtr: None,
            rts: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SerialParity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SerialFlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TcpClientConfig {
    /// `host:port` of the device.
    pub address: String,
    pub connect_timeout_ms: u64,
}

impl Default for TcpClientConfig {
    fn default() -> Self {
        TcpClientConfig {
            address: "127.0.0.1:5000".into(),
            connect_timeout_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct PtyConfig {
    /// Symlink to the slave end (e.g. /tmp/ttyECU) so programs find it.
    pub symlink: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnixSocketConfig {
    pub path: String,
}

impl Default for UnixSocketConfig {
    fn default() -> Self {
        UnixSocketConfig {
            path: "/tmp/io-to-net.sock".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct I2cConfig {
    pub bus: String,
    pub address: u16,
    /// Bytes read per poll.
    pub read_len: usize,
    pub poll_ms: u64,
    /// Register written before each read (sets the device's data pointer).
    pub read_reg: Option<u8>,
    /// Write transactions sent once after opening, e.g. [[0x6B, 0x00]].
    pub init_bytes: Vec<Vec<u8>>,
}

impl Default for I2cConfig {
    fn default() -> Self {
        I2cConfig {
            bus: "/dev/i2c-1".into(),
            address: 0x68,
            read_len: 6,
            poll_ms: 100,
            read_reg: None,
            init_bytes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpiConfig {
    pub device: String,
    pub speed_hz: u32,
    pub mode: u8,
    pub poll_ms: u64,
}

impl Default for SpiConfig {
    fn default() -> Self {
        SpiConfig {
            device: "/dev/spidev0.0".into(),
            speed_hz: 1_000_000,
            mode: 0,
            poll_ms: 50,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GpioConfig {
    pub chip: String,
    pub line: u32,
    pub edge: GpioEdge,
}

impl Default for GpioConfig {
    fn default() -> Self {
        GpioConfig {
            chip: "/dev/gpiochip0".into(),
            line: 0,
            edge: GpioEdge::Both,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpioEdge {
    Rising,
    Falling,
    #[default]
    Both,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BluetoothConfig {
    /// MAC address (Linux/Windows) or the peripheral's identifier (macOS).
    pub device: String,
    pub service_uuid: Option<String>,
    /// Characteristic to subscribe to (default: the first that notifies).
    pub characteristic_uuid: Option<String>,
    /// Characteristic client writes go to (default: none, read-only).
    pub write_characteristic_uuid: Option<String>,
    pub scan_secs: u64,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        BluetoothConfig {
            device: "AA:BB:CC:DD:EE:FF".into(),
            service_uuid: None,
            characteristic_uuid: None,
            write_characteristic_uuid: None,
            scan_secs: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CanbusConfig {
    pub interface: String,
    /// Accept frames whose `id & filter_mask == filter_id`.
    pub filter_id: Option<u32>,
    pub filter_mask: Option<u32>,
    pub format: CanFormat,
}

impl Default for CanbusConfig {
    fn default() -> Self {
        CanbusConfig {
            interface: "can0".into(),
            filter_id: None,
            filter_mask: None,
            format: CanFormat::Json,
        }
    }
}

/// How CAN frames are encoded on the TCP stream.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CanFormat {
    /// One JSON object per line: `{"id":864,"extended":false,"timestampMs":…,"data":[…]}`
    /// (the ecu-to-mqtt CAN gateway format).
    #[default]
    Json,
    /// Lawicel/slcan ASCII: `t1238DEADBEEF00112233\r`, `T…` for extended IDs.
    Slcan,
    /// `[u32 LE id][u8 len][data]` (io-to-net 0.1).
    Binary,
}

impl CanFormat {
    pub fn name(self) -> &'static str {
        match self {
            CanFormat::Json => "json",
            CanFormat::Slcan => "slcan",
            CanFormat::Binary => "binary",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OnewireConfig {
    pub device: String,
    pub poll_ms: u64,
}

impl Default for OnewireConfig {
    fn default() -> Self {
        OnewireConfig {
            device: "/sys/bus/w1/devices/28-000000000000".into(),
            poll_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransportConfig {
    Tcp(TcpTransportConfig),
    Tls(TlsTransportConfig),
    #[serde(alias = "websocket")]
    WebSocket(WebSocketTransportConfig),
}

impl TransportConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            TransportConfig::Tcp(_) => "tcp",
            TransportConfig::Tls(_) => "tls",
            TransportConfig::WebSocket(_) => "websocket",
        }
    }
    pub fn listen(&self) -> &str {
        match self {
            TransportConfig::Tcp(c) => &c.listen,
            TransportConfig::Tls(c) => &c.listen,
            TransportConfig::WebSocket(c) => &c.listen,
        }
    }
    pub fn write_timeout_ms(&self) -> u64 {
        match self {
            TransportConfig::Tcp(c) => c.write_timeout_ms,
            TransportConfig::Tls(c) => c.write_timeout_ms,
            TransportConfig::WebSocket(c) => c.write_timeout_ms,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TcpTransportConfig {
    /// `[::]:9000` listens on IPv4 and IPv6.
    pub listen: String,
    /// A client that cannot take data this long is disconnected.
    pub write_timeout_ms: u64,
}

impl Default for TcpTransportConfig {
    fn default() -> Self {
        TcpTransportConfig {
            listen: "[::]:9000".into(),
            write_timeout_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsTransportConfig {
    pub listen: String,
    pub cert: String,
    pub key: String,
    pub write_timeout_ms: u64,
    /// PEM bundle of CAs that client certificates must chain to (mutual TLS).
    pub client_ca: Option<String>,
}

impl Default for TlsTransportConfig {
    fn default() -> Self {
        TlsTransportConfig {
            listen: "[::]:9443".into(),
            cert: "/etc/io-to-net/cert.pem".into(),
            key: "/etc/io-to-net/key.pem".into(),
            write_timeout_ms: 5000,
            client_ca: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSocketTransportConfig {
    pub listen: String,
    pub write_timeout_ms: u64,
}

impl Default for WebSocketTransportConfig {
    fn default() -> Self {
        WebSocketTransportConfig {
            listen: "[::]:9080".into(),
            write_timeout_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpApiConfig {
    pub enabled: bool,
    /// Loopback by default; use 0.0.0.0 (with a token) to reach it remotely.
    pub listen: String,
    /// Bearer token for /api/* (health, metrics and the dashboard assets stay open).
    pub auth: Option<AuthConfig>,
    /// Serve the web dashboard at `/`.
    pub ui: bool,
    /// Access-Control-Allow-Origin for dashboards on another origin.
    pub cors_origin: Option<String>,
}

impl Default for HttpApiConfig {
    fn default() -> Self {
        HttpApiConfig {
            enabled: true,
            listen: "127.0.0.1:9106".into(),
            auth: None,
            ui: true,
            cors_origin: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub listen: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig {
            enabled: true,
            listen: "127.0.0.1:9105".into(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_src: &str) -> AppConfig {
        toml::from_str(toml_src).unwrap()
    }

    #[test]
    fn defaults_are_safe() {
        let cfg = AppConfig::default();
        assert!(cfg.bridges.is_empty());
        assert_eq!(HttpApiConfig::default().listen, "127.0.0.1:9106");
        assert_eq!(MetricsConfig::default().listen, "127.0.0.1:9105");
        assert_eq!(SerialConfig::default().read_timeout_ms, 50);
    }

    #[test]
    fn serial_bridge_with_usb_selector_and_dtr() {
        let cfg = parse(
            r#"
            [[bridges]]
            name = "ecu"
            mode = "exclusive"
            [bridges.source]
            type = "serial"
            port = "usb:303a:1001"
            dtr = false
            [bridges.transport]
            type = "tcp"
            listen = "127.0.0.1:9000"
            "#,
        );
        let b = &cfg.bridges[0];
        assert_eq!(b.mode, BridgeMode::Exclusive);
        let SourceConfig::Serial(s) = &b.source else {
            panic!()
        };
        assert_eq!(
            (s.port.as_str(), s.dtr, s.baud),
            ("usb:303a:1001", Some(false), 115200)
        );
    }

    #[test]
    fn every_source_and_transport_parses() {
        for src in [
            r#"type = "tcp_client"
address = "10.0.0.5:23""#,
            r#"type = "pty"
symlink = "/tmp/ttyECU""#,
            r#"type = "unix_socket""#,
            r#"type = "i2c"
address = 0x77
read_reg = 0xF7
init_bytes = [[0xF4, 0x2F]]"#,
            r#"type = "gpio""#,
            r#"type = "bluetooth""#,
            r#"type = "canbus"
format = "slcan"
filter_id = 0x360
filter_mask = 0x7F0"#,
            r#"type = "onewire""#,
        ] {
            for transport in [
                r#"type = "tcp""#,
                r#"type = "tls""#,
                r#"type = "web_socket""#,
            ] {
                let doc = format!(
                    "[[bridges]]\nname = \"x\"\n[bridges.source]\n{src}\n[bridges.transport]\n{transport}\n"
                );
                toml::from_str::<AppConfig>(&doc)
                    .unwrap_or_else(|e| panic!("{src} / {transport}: {e}"));
            }
        }
    }

    #[test]
    fn unknown_keys_and_removed_modes_are_rejected() {
        let typo = "[[bridges]]\nname = \"x\"\nmax_client = 2\n[bridges.source]\ntype = \"pty\"\n[bridges.transport]\ntype = \"tcp\"\n";
        assert!(
            toml::from_str::<AppConfig>(typo).is_err(),
            "misspelt key must not be ignored"
        );
        let proxy = "[[bridges]]\nname = \"x\"\nmode = \"proxy\"\n[bridges.source]\ntype = \"pty\"\n[bridges.transport]\ntype = \"tcp\"\n";
        assert!(toml::from_str::<AppConfig>(proxy).is_err());
    }

    #[test]
    fn frame_and_can_formats() {
        assert_eq!(FrameMode::default(), FrameMode::None);
        assert_eq!(CanbusConfig::default().format, CanFormat::Json);
        let cfg = parse(
            "[[bridges]]\nname = \"imu\"\nframe_mode = \"length_prefix\"\n[bridges.source]\ntype = \"i2c\"\n[bridges.transport]\ntype = \"tcp\"\n",
        );
        assert_eq!(cfg.bridges[0].frame_mode, FrameMode::LengthPrefix);
    }
}
