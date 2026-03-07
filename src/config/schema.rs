use serde::{Deserialize, Serialize};

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    /// Global log level override (trace/debug/info/warn/error)
    pub log_level: String,

    /// Bridge definitions
    pub bridges: Vec<BridgeConfig>,

    /// HTTP admin API configuration (optional)
    pub http_api: Option<HttpApiConfig>,

    /// Prometheus metrics configuration (optional)
    pub metrics: Option<MetricsConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            log_level: "info".to_string(),
            bridges: Vec::new(),
            http_api: None,
            metrics: None,
        }
    }
}

/// Single bridge: one IO source mapped to one network transport.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BridgeConfig {
    /// Human-readable bridge name (used in logs, metrics, API)
    pub name: String,

    /// Hardware source configuration
    pub source: SourceConfig,

    /// Network transport configuration
    pub transport: TransportConfig,

    /// TLS configuration (optional, overrides transport-level TLS)
    pub tls: Option<TlsConfig>,

    /// Authentication configuration (optional)
    pub auth: Option<AuthConfig>,

    /// Maximum simultaneous client connections (default: unlimited)
    pub max_clients: Option<usize>,

    /// Client connection mode (default: mirror)
    #[serde(default)]
    pub mode: BridgeMode,

    /// Read-only mode: clients cannot send data to the hardware
    #[serde(default)]
    pub read_only: bool,

    /// Auto-restart the bridge on source failure (default: true)
    #[serde(default = "default_true")]
    pub auto_restart: bool,

    /// Frame each driver read before sending to clients.
    /// Use `length_prefix` for fixed-size sensor reads (I2C, SPI) to prevent
    /// TCP stream fragmentation from splitting frames across multiple reads.
    /// Clients must read: 2 bytes (big-endian u16 length) then that many bytes.
    #[serde(default)]
    pub frame_mode: FrameMode,
}

/// Bridge data forwarding mode.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BridgeMode {
    /// All clients receive the same data stream (default)
    #[default]
    Mirror,
    /// Only one client allowed at a time
    Exclusive,
    /// Forward client data to another TCP endpoint (proxy)
    Proxy,
}

/// How each hardware read is packaged before sending to TCP clients.
///
/// Applies to any bridge source (serial, I2C, Bluetooth, etc.).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameMode {
    /// Raw bytes with no extra framing (default).
    /// The TCP stream is forwarded as-is; the client is responsible for
    /// its own framing logic. Best for serial/UART stream protocols.
    #[default]
    None,

    /// Prepend each driver read with a 2-byte big-endian length header.
    /// Wire format: `[u16 length][data]`.
    /// Client reads 2 bytes → parse length N → read exactly N bytes.
    /// Best for fixed-size binary sensor reads (I2C, SPI) where TCP
    /// fragmentation would otherwise split frames.
    LengthPrefix,

    /// Buffer incoming bytes and emit one frame per newline (`\n`).
    /// The `\n` is included in the frame sent to clients.
    /// Best for ASCII line-based protocols: NMEA (GPS), AT commands, SCPI.
    /// Partial lines are held until the terminator arrives.
    Line,

    /// Buffer incoming bytes and emit one frame per null byte (`0x00`).
    /// The `0x00` terminator is included in the frame sent to clients.
    /// Best for COBS-encoded streams where each packet ends with 0x00.
    NullTerminated,
}

/// IO source (hardware side of the bridge).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceConfig {
    /// UART / RS-232 / RS-485 serial port
    Serial(SerialConfig),
    /// I²C bus device (Linux only)
    I2c(I2cConfig),
    /// SPI bus device (Linux only)
    Spi(SpiConfig),
    /// GPIO line (Linux only)
    Gpio(GpioConfig),
    /// Bluetooth Classic or BLE device
    Bluetooth(BluetoothConfig),
    /// CAN bus socket (Linux only)
    Canbus(CanbusConfig),
    /// 1-Wire bus device (Linux only)
    Onewire(OnewireConfig),
    /// Pseudo-terminal (PTY)
    Pty(PtyConfig),
    /// Unix domain socket
    UnixSocket(UnixSocketConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SerialConfig {
    /// Serial port path (e.g. /dev/ttyUSB0, COM3)
    pub port: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: SerialParity,
    pub stop_bits: u8,
    pub flow_control: SerialFlowControl,
    pub read_timeout_ms: u64,
}

impl Default for SerialConfig {
    fn default() -> Self {
        SerialConfig {
            port: "/dev/ttyUSB0".to_string(),
            baud: 115200,
            data_bits: 8,
            parity: SerialParity::None,
            stop_bits: 1,
            flow_control: SerialFlowControl::None,
            read_timeout_ms: 100,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SerialParity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SerialFlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct I2cConfig {
    /// I2C bus path (e.g. /dev/i2c-1)
    pub bus: String,
    /// Device address in hex or decimal
    pub address: u16,
    /// Number of bytes to read per poll cycle
    pub read_len: usize,
    /// Poll interval in milliseconds
    pub poll_ms: u64,
    /// Register address to write before each read (sets the device's internal register pointer).
    /// Required for most I2C sensors — e.g. 0x3B for MPU-6050 accelerometer, 0x28 for LSM6DS3.
    /// Omit only if the device streams data without needing a register select.
    pub read_reg: Option<u8>,
    /// Initialization sequences sent to the device on open, before the first read.
    /// Each inner array is one I2C write transaction.
    /// Example for MPU-6050 (wake from sleep): [[0x6B, 0x00]]
    pub init_bytes: Vec<Vec<u8>>,
}

impl Default for I2cConfig {
    fn default() -> Self {
        I2cConfig {
            bus: "/dev/i2c-1".to_string(),
            address: 0x68,
            read_len: 6,
            poll_ms: 100,
            read_reg: None,
            init_bytes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SpiConfig {
    pub device: String,
    pub speed_hz: u32,
    pub mode: u8,
    pub poll_ms: u64,
}

impl Default for SpiConfig {
    fn default() -> Self {
        SpiConfig { device: "/dev/spidev0.0".to_string(), speed_hz: 1_000_000, mode: 0, poll_ms: 50 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GpioConfig {
    /// GPIO chip path (e.g. /dev/gpiochip0)
    pub chip: String,
    /// GPIO line offset
    pub line: u32,
    /// Rising, falling, or both
    pub edge: GpioEdge,
}

impl Default for GpioConfig {
    fn default() -> Self {
        GpioConfig { chip: "/dev/gpiochip0".to_string(), line: 0, edge: GpioEdge::Both }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GpioEdge {
    Rising,
    Falling,
    #[default]
    Both,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct BluetoothConfig {
    /// Bluetooth MAC address (e.g. AA:BB:CC:DD:EE:FF)
    pub device: String,
    /// BLE GATT service UUID to subscribe to (optional, for BLE mode)
    pub service_uuid: Option<String>,
    /// BLE GATT characteristic UUID to subscribe to (optional)
    pub characteristic_uuid: Option<String>,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        BluetoothConfig {
            device: "AA:BB:CC:DD:EE:FF".to_string(),
            service_uuid: None,
            characteristic_uuid: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CanbusConfig {
    /// CAN interface name (e.g. can0, vcan0)
    pub interface: String,
    /// Optional frame ID filter mask
    pub filter: Option<u32>,
}

impl Default for CanbusConfig {
    fn default() -> Self {
        CanbusConfig { interface: "can0".to_string(), filter: None }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OnewireConfig {
    /// 1-Wire device path (e.g. /sys/bus/w1/devices/28-xxxx)
    pub device: String,
    pub poll_ms: u64,
}

impl Default for OnewireConfig {
    fn default() -> Self {
        OnewireConfig { device: "/sys/bus/w1/devices/28-000000000000".to_string(), poll_ms: 1000 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PtyConfig {
    /// Optional symlink to create for the PTY slave end
    pub symlink: Option<String>,
}

impl Default for PtyConfig {
    fn default() -> Self {
        PtyConfig { symlink: None }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct UnixSocketConfig {
    /// Socket file path
    pub path: String,
}

impl Default for UnixSocketConfig {
    fn default() -> Self {
        UnixSocketConfig { path: "/tmp/io-to-net.sock".to_string() }
    }
}

/// Network transport (client-facing side of the bridge).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransportConfig {
    Tcp(TcpTransportConfig),
    Tls(TlsTransportConfig),
    WebSocket(WebSocketTransportConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TcpTransportConfig {
    /// Listen address (e.g. [::]:9000 for IPv4+IPv6 dual-stack)
    pub listen: String,
    /// Write timeout for slow clients in milliseconds
    pub write_timeout_ms: u64,
}

impl Default for TcpTransportConfig {
    fn default() -> Self {
        TcpTransportConfig { listen: "[::]:9000".to_string(), write_timeout_ms: 5000 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TlsTransportConfig {
    pub listen: String,
    pub cert: String,
    pub key: String,
    pub write_timeout_ms: u64,
    /// Require client certificates
    pub require_client_cert: bool,
}

impl Default for TlsTransportConfig {
    fn default() -> Self {
        TlsTransportConfig {
            listen: "[::]:9443".to_string(),
            cert: "/etc/io-to-net/cert.pem".to_string(),
            key: "/etc/io-to-net/key.pem".to_string(),
            write_timeout_ms: 5000,
            require_client_cert: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WebSocketTransportConfig {
    pub listen: String,
    pub write_timeout_ms: u64,
}

impl Default for WebSocketTransportConfig {
    fn default() -> Self {
        WebSocketTransportConfig { listen: "[::]:9080".to_string(), write_timeout_ms: 5000 }
    }
}

/// TLS configuration (standalone, for bridges using non-TLS transport with wrapper).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsConfig {
    pub enabled: bool,
    pub cert: String,
    pub key: String,
}

/// Token-based authentication configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    /// Bearer token clients must present
    pub token: String,
}

/// HTTP admin API configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct HttpApiConfig {
    pub enabled: bool,
    pub listen: String,
    pub auth: Option<AuthConfig>,
    pub rate_limit_per_second: Option<u64>,
}

impl Default for HttpApiConfig {
    fn default() -> Self {
        HttpApiConfig {
            enabled: true,
            listen: "0.0.0.0:9106".to_string(),
            auth: None,
            rate_limit_per_second: None,
        }
    }
}

/// Prometheus metrics configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub listen: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig { enabled: true, listen: "0.0.0.0:9105".to_string() }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AppConfig defaults ────────────────────────────────────────────────

    #[test]
    fn app_config_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.log_level, "info");
        assert!(cfg.bridges.is_empty());
        assert!(cfg.http_api.is_none());
        assert!(cfg.metrics.is_none());
    }

    // ── TOML round-trip for every source type ─────────────────────────────

    fn minimal_bridge(source_toml: &str, transport_toml: &str) -> BridgeConfig {
        let doc = format!(
            r#"
name = "test"
{source_toml}
{transport_toml}
"#
        );
        toml::from_str(&doc).expect("parse")
    }

    #[test]
    fn parse_serial_source() {
        let b = minimal_bridge(
            r#"[source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 9600"#,
            r#"[transport]
type = "tcp"
listen = "[::]:9000""#,
        );
        match b.source {
            SourceConfig::Serial(s) => {
                assert_eq!(s.port, "/dev/ttyUSB0");
                assert_eq!(s.baud, 9600);
            }
            _ => panic!("expected Serial"),
        }
    }

    #[test]
    fn parse_bluetooth_source() {
        let b = minimal_bridge(
            r#"[source]
type = "bluetooth"
device = "AA:BB:CC:DD:EE:FF""#,
            r#"[transport]
type = "tcp"
listen = "[::]:9001""#,
        );
        assert!(matches!(b.source, SourceConfig::Bluetooth(_)));
    }

    #[test]
    fn parse_pty_source() {
        let b = minimal_bridge(
            r#"[source]
type = "pty""#,
            r#"[transport]
type = "tcp"
listen = "[::]:9002""#,
        );
        assert!(matches!(b.source, SourceConfig::Pty(_)));
    }

    #[test]
    fn parse_unix_socket_source() {
        let b = minimal_bridge(
            r#"[source]
type = "unix_socket"
path = "/tmp/test.sock""#,
            r#"[transport]
type = "tcp"
listen = "[::]:9003""#,
        );
        match b.source {
            SourceConfig::UnixSocket(u) => assert_eq!(u.path, "/tmp/test.sock"),
            _ => panic!("expected UnixSocket"),
        }
    }

    #[test]
    fn parse_onewire_source() {
        let b = minimal_bridge(
            r#"[source]
type = "onewire"
device = "/sys/bus/w1/devices/28-aabbccdd"
poll_ms = 2000"#,
            r#"[transport]
type = "tcp"
listen = "[::]:9004""#,
        );
        match b.source {
            SourceConfig::Onewire(o) => {
                assert_eq!(o.device, "/sys/bus/w1/devices/28-aabbccdd");
                assert_eq!(o.poll_ms, 2000);
            }
            _ => panic!("expected Onewire"),
        }
    }

    #[test]
    fn parse_tcp_transport() {
        let b = minimal_bridge(
            r#"[source]
type = "pty""#,
            r#"[transport]
type = "tcp"
listen = "0.0.0.0:8080"
write_timeout_ms = 3000"#,
        );
        match b.transport {
            TransportConfig::Tcp(t) => {
                assert_eq!(t.listen, "0.0.0.0:8080");
                assert_eq!(t.write_timeout_ms, 3000);
            }
            _ => panic!("expected Tcp"),
        }
    }

    #[test]
    fn parse_websocket_transport() {
        let b = minimal_bridge(
            r#"[source]
type = "pty""#,
            r#"[transport]
type = "web_socket"
listen = "[::]:9080""#,
        );
        assert!(matches!(b.transport, TransportConfig::WebSocket(_)));
    }

    #[test]
    fn parse_tls_transport() {
        let b = minimal_bridge(
            r#"[source]
type = "pty""#,
            r#"[transport]
type = "tls"
listen = "[::]:9443"
cert = "/etc/cert.pem"
key = "/etc/key.pem""#,
        );
        match b.transport {
            TransportConfig::Tls(t) => {
                assert_eq!(t.cert, "/etc/cert.pem");
                assert!(!t.require_client_cert);
            }
            _ => panic!("expected Tls"),
        }
    }

    // ── Bridge defaults ───────────────────────────────────────────────────

    #[test]
    fn bridge_mode_default_is_mirror() {
        let b = minimal_bridge(
            r#"[source]
type = "pty""#,
            r#"[transport]
type = "tcp"
listen = "[::]:9000""#,
        );
        assert_eq!(b.mode, BridgeMode::Mirror);
        assert!(!b.read_only);
        assert!(b.auto_restart); // default_true
        assert!(b.max_clients.is_none());
    }

    #[test]
    fn bridge_mode_exclusive_parse() {
        let doc = r#"
name = "excl"
mode = "exclusive"

[source]
type = "pty"

[transport]
type = "tcp"
listen = "[::]:9000"
"#;
        let b: BridgeConfig = toml::from_str(doc).unwrap();
        assert_eq!(b.mode, BridgeMode::Exclusive);
    }

    // ── SerialConfig defaults ─────────────────────────────────────────────

    #[test]
    fn serial_config_defaults() {
        let s = SerialConfig::default();
        assert_eq!(s.baud, 115200);
        assert_eq!(s.data_bits, 8);
        assert_eq!(s.stop_bits, 1);
        assert!(matches!(s.parity, SerialParity::None));
        assert!(matches!(s.flow_control, SerialFlowControl::None));
        assert_eq!(s.read_timeout_ms, 100);
    }

    // ── HttpApiConfig ─────────────────────────────────────────────────────

    #[test]
    fn http_api_config_parse() {
        let doc = r#"
enabled = true
listen = "127.0.0.1:9200"

[auth]
token = "secret"
"#;
        let c: HttpApiConfig = toml::from_str(doc).unwrap();
        assert!(c.enabled);
        assert_eq!(c.listen, "127.0.0.1:9200");
        assert_eq!(c.auth.unwrap().token, "secret");
    }

    // ── MetricsConfig ─────────────────────────────────────────────────────

    #[test]
    fn metrics_config_defaults() {
        let m = MetricsConfig::default();
        assert!(m.enabled);
        assert_eq!(m.listen, "0.0.0.0:9105");
    }

    // ── I2cConfig ─────────────────────────────────────────────────────────

    #[test]
    fn i2c_config_defaults() {
        let i = I2cConfig::default();
        assert_eq!(i.bus, "/dev/i2c-1");
        assert_eq!(i.address, 0x68);
        assert_eq!(i.read_len, 6);
        assert_eq!(i.poll_ms, 100);
        assert!(i.read_reg.is_none());
        assert!(i.init_bytes.is_empty());
    }

    #[test]
    fn parse_i2c_source_minimal() {
        let b = minimal_bridge(
            r#"[source]
type = "i2c"
bus = "/dev/i2c-1"
address = 0x68
read_len = 14
poll_ms = 10"#,
            r#"[transport]
type = "tcp"
listen = "[::]:9002""#,
        );
        match b.source {
            SourceConfig::I2c(i) => {
                assert_eq!(i.bus, "/dev/i2c-1");
                assert_eq!(i.address, 0x68);
                assert_eq!(i.read_len, 14);
                assert_eq!(i.poll_ms, 10);
                assert!(i.read_reg.is_none());
                assert!(i.init_bytes.is_empty());
            }
            _ => panic!("expected I2c"),
        }
    }

    #[test]
    fn parse_i2c_source_with_read_reg_and_init_bytes() {
        // MPU-6050 IMU: wake from sleep with [0x6B, 0x00], then read 14 bytes from reg 0x3B
        let b = minimal_bridge(
            r#"[source]
type = "i2c"
bus = "/dev/i2c-1"
address = 0x68
read_len = 14
poll_ms = 10
read_reg = 0x3B
init_bytes = [[0x6B, 0x00]]"#,
            r#"[transport]
type = "tcp"
listen = "[::]:9002""#,
        );
        match b.source {
            SourceConfig::I2c(i) => {
                assert_eq!(i.read_reg, Some(0x3B));
                assert_eq!(i.init_bytes, vec![vec![0x6B, 0x00]]);
            }
            _ => panic!("expected I2c"),
        }
    }

    #[test]
    fn parse_i2c_source_multiple_init_sequences() {
        // Device requiring two init writes before first read
        let b = minimal_bridge(
            r#"[source]
type = "i2c"
bus = "/dev/i2c-1"
address = 0x77
read_len = 8
poll_ms = 50
read_reg = 0xF7
init_bytes = [[0xF4, 0x2F], [0xF5, 0xA0]]"#,
            r#"[transport]
type = "tcp"
listen = "[::]:9003""#,
        );
        match b.source {
            SourceConfig::I2c(i) => {
                assert_eq!(i.address, 0x77);
                assert_eq!(i.read_reg, Some(0xF7));
                assert_eq!(i.init_bytes.len(), 2);
                assert_eq!(i.init_bytes[0], vec![0xF4, 0x2F]);
                assert_eq!(i.init_bytes[1], vec![0xF5, 0xA0]);
            }
            _ => panic!("expected I2c"),
        }
    }

    #[test]
    fn frame_mode_variants_parse() {
        for (s, expected) in [
            ("none", FrameMode::None),
            ("length_prefix", FrameMode::LengthPrefix),
            ("line", FrameMode::Line),
            ("null_terminated", FrameMode::NullTerminated),
        ] {
            // FrameMode is serde-transparent; wrap in a trivial struct for round-trip.
            #[derive(serde::Deserialize)]
            struct W { frame_mode: FrameMode }
            let w: W = toml::from_str(&format!("frame_mode = \"{}\"", s)).unwrap();
            assert_eq!(w.frame_mode, expected, "failed for {}", s);
        }
    }

    #[test]
    fn parse_bridge_frame_mode_length_prefix() {
        let doc = r#"
name = "imu"
frame_mode = "length_prefix"
read_only = true

[source]
type = "i2c"
bus = "/dev/i2c-1"
address = 0x68
read_len = 14
poll_ms = 10
read_reg = 0x3B
init_bytes = [[0x6B, 0x00]]

[transport]
type = "tcp"
listen = "[::]:9002"
"#;
        let b: BridgeConfig = toml::from_str(doc).unwrap();
        assert_eq!(b.frame_mode, FrameMode::LengthPrefix);
        assert!(b.read_only);
    }

    #[test]
    fn parse_bridge_frame_mode_default_is_none() {
        let b = minimal_bridge(
            r#"[source]
type = "pty""#,
            r#"[transport]
type = "tcp"
listen = "[::]:9000""#,
        );
        assert_eq!(b.frame_mode, FrameMode::None);
    }

    #[test]
    fn parse_bridge_frame_mode_line_on_serial() {
        let doc = r#"
name = "gps"
frame_mode = "line"

[source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 9600

[transport]
type = "tcp"
listen = "[::]:9010"
"#;
        let b: BridgeConfig = toml::from_str(doc).unwrap();
        assert_eq!(b.frame_mode, FrameMode::Line);
    }

    #[test]
    fn parse_bridge_frame_mode_null_terminated_on_serial() {
        let doc = r#"
name = "cobs-sensor"
frame_mode = "null_terminated"

[source]
type = "serial"
port = "/dev/ttyUSB1"
baud = 115200

[transport]
type = "tcp"
listen = "[::]:9011"
"#;
        let b: BridgeConfig = toml::from_str(doc).unwrap();
        assert_eq!(b.frame_mode, FrameMode::NullTerminated);
    }

    #[test]
    fn i2c_config_serde_roundtrip() {
        let original = I2cConfig {
            bus: "/dev/i2c-2".to_string(),
            address: 0x1D,
            read_len: 6,
            poll_ms: 20,
            read_reg: Some(0x28),
            init_bytes: vec![vec![0x20, 0x57]],
        };
        let serialised = toml::to_string(&original).unwrap();
        let parsed: I2cConfig = toml::from_str(&serialised).unwrap();
        assert_eq!(parsed.bus, original.bus);
        assert_eq!(parsed.address, original.address);
        assert_eq!(parsed.read_reg, original.read_reg);
        assert_eq!(parsed.init_bytes, original.init_bytes);
    }

    // ── Serialise AppConfig round-trip ────────────────────────────────────

    #[test]
    fn app_config_serde_roundtrip() {
        let doc = r#"
log_level = "debug"

[[bridges]]
name = "rt"

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 115200

[bridges.transport]
type = "tcp"
listen = "[::]:9000"
"#;
        let cfg: AppConfig = toml::from_str(doc).unwrap();
        let serialised = toml::to_string(&cfg).unwrap();
        let cfg2: AppConfig = toml::from_str(&serialised).unwrap();
        assert_eq!(cfg.bridges.len(), cfg2.bridges.len());
        assert_eq!(cfg.bridges[0].name, cfg2.bridges[0].name);
    }
}
