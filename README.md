# io-to-net

# Project Status

Active development. Not expected to work yet. Tested with I2C sensors in read only mode. These and serial ports should work. Framing works and essential for I2C.

Contributions and feedback welcome.


-------
**io-to-net** is an **Edge Hardware Networking Layer** that exposes local hardware interfaces over TCP/IP.

It allows devices connected to a machine (serial, I²C, GPIO, Bluetooth, CAN, etc.) to be accessed remotely as if they were locally connected.

The system acts as a transparent byte-stream bridge between hardware interfaces and network clients.

```
hardware device → io-to-net → TCP/IP → application
```

io-to-net is designed as the transport layer for telemetry pipelines such as:

- speeduino-to-mqtt
- gps-to-mqtt
- sensors-to-mqtt

Together these tools form a modular open telemetry stack.

---

# Key Concept

Traditional tools expose **one type of hardware interface** over the network.

io-to-net exposes **any hardware IO interface** using a unified system.

It is designed as a universal **edge hardware networking runtime**.

The bridge never modifies payload data — it behaves exactly like a direct hardware connection.

---

# Features

**Universal hardware bridging**

| Source | Notes |
|--------|-------|
| Serial | any baud rate, flow control |
| I²C | Linux only |
| SPI | Linux only |
| GPIO | Linux only |
| Bluetooth / BLE | active 5-second scan |
| CAN bus | Linux only |
| 1-Wire | Linux only |
| PTY | pseudo-terminal, Unix |
| Unix socket | Unix |

**Network transports**

| Transport | Notes |
|-----------|-------|
| TCP | raw byte stream, IPv4/IPv6 |
| TLS | self-signed cert generator included |
| WebSocket | binary frames |

**Network capabilities**

- multi-client fan-out (mirror / exclusive / proxy modes)
- per-bridge connection limits
- read-only mode (clients cannot write to device)
- automatic bridge restart on failure

**Protocol observation**

io-to-net passively inspects RX traffic to identify the stream protocol.
Detection is a side-channel observer — the data path is never modified.

Detectable protocols:

| Protocol | Category |
|----------|----------|
| NMEA (GPS) | ASCII |
| AT commands | ASCII |
| SCPI | ASCII |
| CAN-over-serial | ASCII |
| Modbus RTU | binary |
| MAVLink | binary |
| SLIP | binary |
| COBS | binary |
| Speeduino | binary |
| Megasquirt | binary |

Protocol hints are shown in the TUI Ports tab, HTTP API, and Prometheus metrics.

**Observability**

- TUI dashboard with Ports, Connections, Stats, Logs tabs
- Protocol hint column in Ports tab
- Prometheus metrics endpoint
- HTTP admin API

**Device discovery**

- Quick scan: `--scan`
- Deep scan: `--scan --deep` (BLE service discovery, I2C probing)
- JSON output: `--scan --json`

**Security**

- TLS support
- Bearer token authentication on HTTP API and TCP bridges
- No automatic device exposure — bridges only start from explicit config

**Service operation**

- Daemon mode (`--daemon`) with JSON logging
- systemd unit included
- Debian / RPM packaging scripts

**Cross-platform**

- Linux (full, all drivers)
- macOS / Unix (serial, BLE, PTY, Unix socket)
- Windows (serial, BLE)

**Embedded nodes**

- ESP32 and ESP8266 PlatformIO firmware
- Microcontrollers expose UART, I²C, GPIO over WiFi using the same protocol

---

# Example Architecture

Telemetry pipeline:

```
ECU → serial → io-to-net → TCP → speeduino-to-mqtt → MQTT → telemetry apps
```

Distributed sensor node:

```
sensor → ESP32 io-to-net node → WiFi → TCP → client application
```

Distributed hardware lab:

```
lab machine → io-to-net → network → remote debugging tools
```

---

# Installation

```bash
cargo build --release
```

Binary:

```
target/release/io-to-net
```

---

# Configuration

Configuration uses TOML. io-to-net looks for a config file in this order:

1. `./config.toml` — current working directory
2. `/etc/io-to-net/config.toml` — system default
3. Path given by `--config <path>` or `IO_TO_NET_CONFIG` env var

A fully annotated example covering all source types and transports:

```
config/example.toml
```

Quick start:

```bash
sudo mkdir -p /etc/io-to-net
sudo cp config/example.toml /etc/io-to-net/config.toml
# edit config.toml, then:
./io-to-net
```

Minimal example:

```toml
log_level = "info"

[[bridges]]
name = "ecu"

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 115200

[bridges.transport]
type = "tcp"
listen = "[::]:9000"
```

---

# Bridge Architecture

Each bridge runs three independent async tasks:

```
┌─ transport task ─────────────────────────────────────────────┐
│  Accepts TCP connections. Always running. Clients can connect │
│  even while the hardware is offline or being retried.        │
└─────────────────────────────────────────────────────────────┘
         │ ClientHandle (tx/rx channels)
         ▼
┌─ client intake task ─────────────────────────────────────────┐
│  Receives new clients from the transport task and registers   │
│  them in the ConnectionManager. Manages per-client fan-out.  │
└─────────────────────────────────────────────────────────────┘
         │ broadcast channel
         ▼
┌─ driver loop ────────────────────────────────────────────────┐
│  Opens hardware, reads data, broadcasts to all clients.       │
│  Retries with exponential backoff on failure. Client          │
│  connections survive driver restarts intact.                  │
└─────────────────────────────────────────────────────────────┘
```

**Clients always connectable**: the TCP port is bound and ready before the hardware driver opens. Clients connecting while the hardware is offline simply receive no data until the driver comes up — they do not need to reconnect.

**Driver restart transparency**: when the hardware fails and restarts (e.g. sensor disconnected then reconnected), existing TCP clients stay connected and resume receiving data automatically.

---

# Frame Mode

By default, io-to-net forwards the raw byte stream from the hardware directly to TCP clients (`frame_mode = "none"`). This is correct for UART/serial streams where the client handles its own framing. For sources that produce discrete messages (I2C sensor reads, NMEA sentences, COBS packets), you can enable automatic framing to ensure clients always receive complete, aligned frames regardless of how TCP delivers the bytes.

`frame_mode` is set at the bridge level and works with any source type:

| Value | Description | Best for |
|-------|-------------|----------|
| `none` | Raw stream, no framing (default) | serial, PTY, Unix socket |
| `length_prefix` | `[u16 big-endian length][data]` | I2C, SPI — fixed-size binary reads |
| `line` | Buffer until `\n`, emit one frame per line (including `\n`) | NMEA (GPS), AT commands, SCPI |
| `null_terminated` | Buffer until `0x00`, emit one frame per packet (including `0x00`) | COBS-encoded streams |

**Client read pattern per mode:**

```
none            →  stream.read(&mut buf)   # may get partial data — client must frame
length_prefix   →  stream.read_exact(&mut [0u8; 2])  → parse u16 N
                   stream.read_exact(&mut buf[..N])   # always N complete bytes
line            →  BufReader::lines()  or  stream.read_until(b'\n', &mut buf)
null_terminated →  stream.read_until(0x00, &mut buf)
```

**Serial NMEA GPS example** (`line` mode):

```toml
[[bridges]]
name = "gps"
frame_mode = "line"    # each NMEA sentence ($GPGGA,...\n) arrives as one complete frame

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 9600

[bridges.transport]
type = "tcp"
listen = "[::]:9001"
```

**Serial COBS sensor example** (`null_terminated` mode):

```toml
[[bridges]]
name = "sensor"
frame_mode = "null_terminated"   # each COBS packet (ends with 0x00) arrives complete

[bridges.source]
type = "serial"
port = "/dev/ttyUSB1"
baud = 115200

[bridges.transport]
type = "tcp"
listen = "[::]:9002"
```

---

# I2C Sensor Configuration

Most I2C sensors require two things that are often missed:

1. **Wake from sleep** — many sensors (e.g. MPU-6050) power on in sleep mode. An init write is needed to start them.
2. **Register pointer** — before reading sensor data, the device's internal address pointer must be set to the correct register. Without this, reads return garbage or data from the wrong register.

All three are configured per-bridge:

| Field | Scope | Type | Purpose |
|-------|-------|------|---------|
| `read_reg` | `[bridges.source]` | `integer` (optional) | Register address written before each read |
| `init_bytes` | `[bridges.source]` | `[[integer]]` (optional) | Write transactions sent once on open |
| `frame_mode` | `[[bridges]]` | `"none"` \| `"length_prefix"` | How each read is wrapped before sending to clients |

**`frame_mode = "length_prefix"`** is strongly recommended for I2C/SPI bridges. TCP is a stream protocol — without framing, a client `read()` call may return a partial frame. With `length_prefix`, each driver read is sent as `[2-byte big-endian length][data]`, so the client always reads a complete frame:

```
client reads: 2 bytes → u16 length N
client reads: N bytes → complete sensor frame
```

**MPU-6050 IMU example** (accelerometer + gyroscope, 14 bytes at 100 Hz):

```toml
[[bridges]]
name = "imu"
read_only = true
auto_restart = true
frame_mode = "length_prefix"   # each 14-byte read arrives as [0x00, 0x0E, ...14 bytes]

[bridges.source]
type = "i2c"
bus = "/dev/i2c-1"
address = 0x68          # MPU-6050 default I2C address (AD0 low)
read_len = 14           # ACCEL_XYZ (6) + TEMP (2) + GYRO_XYZ (6)
poll_ms = 10            # 100 Hz
read_reg = 0x3B         # ACCEL_XOUT_H — start of data registers
init_bytes = [[0x6B, 0x00]]  # PWR_MGMT_1 = 0: wake from sleep

[bridges.transport]
type = "tcp"
listen = "[::]:9002"
```

**BMP280 pressure sensor example** (8 raw bytes at 20 Hz):

```toml
[[bridges]]
name = "baro"
read_only = true
auto_restart = true
frame_mode = "length_prefix"

[bridges.source]
type = "i2c"
bus = "/dev/i2c-1"
address = 0x77
read_len = 8
poll_ms = 50
read_reg = 0xF7
init_bytes = [
  [0xF4, 0x2F],   # ctrl_meas: temp×1, press×1, normal mode
  [0xF5, 0xA0],   # config: standby 1s, filter off
]

[bridges.transport]
type = "tcp"
listen = "[::]:9003"
```

**Finding register addresses**: consult the sensor datasheet. Look for the "data registers" or "output registers" table. The `read_reg` value is the address of the first byte you want to read. Common values:

| Sensor | Address | read_reg | read_len | init_bytes |
|--------|---------|----------|----------|------------|
| MPU-6050 accel+gyro | 0x68 | 0x3B | 14 | `[[0x6B, 0x00]]` |
| MPU-6050 accel only | 0x68 | 0x3B | 6 | `[[0x6B, 0x00]]` |
| LSM6DS3 accel+gyro | 0x6A | 0x28 | 12 | `[[0x10, 0x60], [0x11, 0x60]]` |
| BMP280 raw data | 0x77 | 0xF7 | 8 | `[[0xF4, 0x2F], [0xF5, 0xA0]]` |
| ADS1115 | 0x48 | 0x00 | 2 | _(none required)_ |

I2C drivers run blocking operations on a dedicated thread pool (`spawn_blocking`) so the async runtime is never stalled.

---

# Device Discovery

```bash
# Quick scan (~5 seconds)
io-to-net --scan

# Deep scan — BLE service discovery, I2C address probing (~10 seconds)
io-to-net --scan --deep

# Machine-readable JSON output
io-to-net --scan --json
```

Example output:

```
io-to-net — hardware device scan

SERIAL
  /dev/ttyUSB0
  /dev/ttyUSB1

I2C
  /dev/i2c-1

GPIO
  gpiochip0

BLUETOOTH (scanning for 5s...)
  AA:BB:CC:DD:EE:FF  OBD-II Adapter
  11:22:33:44:55:66  BLE Sensor
```

---

# TLS Certificate Generation

```bash
# Generate self-signed cert and key in ./certs/
io-to-net generate-cert --out ./certs --cn my-host --days 365
```

---

# Protocol Detection

io-to-net passively observes RX data for each bridge and classifies the stream.

Detection accumulates a 4096-byte rolling sample and applies heuristics (printable-ASCII ratio, frame delimiters, protocol-specific byte patterns). Detection never blocks the data path.

The result is visible in:

- **TUI** → Ports tab → Protocol column
- **HTTP API** → `GET /bridges` → `protocol_hint`, `message_rate`, `avg_frame_size`

---

# HTTP Admin API

Default: `http://localhost:9106`

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/health` | Uptime and status |
| GET | `/bridges` | All bridges with protocol hints |
| GET | `/bridges/{name}` | Single bridge |
| GET | `/connections` | Active client connections |
| GET | `/logs` | Recent log lines (`?limit=N`) |
| POST | `/bridges/{name}/restart` | Restart a bridge |
| POST | `/reload-config` | Reload config file (Unix: SIGHUP) |

Enable in config:

```toml
[http_api]
enabled = true
listen = "0.0.0.0:9106"

[http_api.auth]
token = "your-secret-token"
```

---

# Prometheus Metrics

Default: `http://localhost:9105/metrics`

```
io_to_net_bridges_total
io_to_net_clients_total
io_to_net_bytes_rx_total
io_to_net_bytes_tx_total
```

Enable in config:

```toml
[metrics]
enabled = true
listen = "0.0.0.0:9105"
```

---

# Terminal Interface

Running without `--daemon` launches the TUI:

```
┌ header: version, bridges, clients, bandwidth, uptime ─┐
├ Ports [1] │ Connections [2] │ Stats [3] │ Logs [4] ────┤
├ main content area ─────────────────────────────────────┤
└ live log panel ────────────────────────────────────────┘
```

**Ports tab** — bridge name, source, listen address, clients, RX rate, protocol hint, status  
**Connections tab** — active client IPs, bridge, uptime, bytes  
**Stats tab** — aggregate totals, error counters  
**Logs tab** — full scrollback of log lines (all entries routed here in TUI mode, stdout stays clean)

Keys: `q` quit · `Tab`/`←``→` switch tabs · `1`–`4` jump to tab

---

# Microcontroller Nodes

The `firmware/` directory contains a PlatformIO project for ESP32 and ESP8266.

Nodes expose UART, I²C, and GPIO over WiFi using bare TCP — compatible with the desktop io-to-net protocol.

See [firmware/README.md](firmware/README.md) for build and flash instructions.

WiFi credentials are stored in `firmware/src/secrets.h` (gitignored):

```bash
cp firmware/src/secrets.h.example firmware/src/secrets.h
# edit secrets.h with your SSID and password
pio run -e esp32dev --target upload
```

---

# Packaging

```bash
./scripts/build_packages.sh
```

Produces Debian (`.deb`) and RPM (`.rpm`) packages.

---

# License

GNU General Public License v3. See [LICENCE](LICENCE).
