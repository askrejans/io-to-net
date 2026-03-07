# io-to-net

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

---

# Ecosystem

| Tool | Role |
|------|------|
| io-to-net | edge hardware networking layer |
| speeduino-to-mqtt | ECU telemetry → MQTT |
| gps-to-mqtt | GPS stream → MQTT |
| sensors-to-mqtt | sensor data → MQTT |

---

# Project Status

Active development. Interfaces and configuration format may change before 1.0.

Contributions and feedback welcome.

---

# Key Concept

Traditional tools expose **one type of hardware interface** over the network.

io-to-net exposes **any hardware IO interface** using a unified system.

It is designed as a universal **edge hardware networking runtime**.

This allows:

serial devices to be accessed remotely  
Bluetooth sensors to be forwarded to applications  
I²C buses to be bridged across systems  
GPIO events to be streamed over TCP  
CAN bus data to be monitored remotely  

All without modifying the applications consuming the data.

---

# Features

Universal hardware bridging:

serial  
I²C  
SPI  
GPIO  
Bluetooth  
BLE  
CAN bus  
OneWire  
PTY / pseudo terminals  
Unix sockets  

Network transports:

TCP  
TLS  
WebSocket  

Network capabilities:

IPv4 and IPv6 support  
multi-client connections  
connection limits  
automatic reconnect  
high-throughput streaming  

Observability:

terminal dashboard (TUI)  
connection table  
bandwidth statistics  
live logs  

Monitoring:

Prometheus metrics endpoint  
HTTP admin API  

Device discovery:

hardware scan command  
serial device detection  
Bluetooth discovery  
I²C bus scanning  

Service operation:

daemon mode  
systemd service support  
structured logging  

Cross-platform support:

Linux  
MacOS  
Unix  
Windows  

Embedded node support:

ESP32 firmware  
ESP8266 firmware  

Microcontrollers can act as remote io-to-net nodes exposing their hardware over WiFi.

---

# Example Architecture

Example telemetry pipeline:

```
ECU → serial → io-to-net → TCP → speeduino-to-mqtt → MQTT → telemetry apps
```

Example sensor deployment:

```
sensor → ESP32 io-to-net node → WiFi → TCP → client application
```

Example distributed hardware lab:

```
lab machine → io-to-net → network → remote debugging tools
```

---

# Installation

Build from source:

```
cargo build --release
```

Binary will be located at:

```
target/release/io-to-net
```

---

# Configuration

Configuration uses TOML.

Default location:

```
/etc/io-to-net/config.toml
```

A fully annotated example covering all source types and transports is provided at:

```
config/example.toml
```

Copy it to get started:

```bash
sudo mkdir -p /etc/io-to-net
sudo cp config/example.toml /etc/io-to-net/config.toml
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

# Device Discovery

io-to-net can scan the system for hardware devices.

```
io-to-net --scan
```

Example output:

```
Detected devices:

SERIAL
  /dev/ttyUSB0
  /dev/ttyUSB1

I2C
  /dev/i2c-1

GPIO
  gpiochip0

BLUETOOTH
  AA:BB:CC:DD:EE:FF  OBD-II Adapter
```

---

# Prometheus Metrics

Metrics endpoint:

```
http://localhost:9105/metrics
```

Example metrics:

```
io_to_net_bridges_total
io_to_net_clients_total
io_to_net_bytes_rx_total
io_to_net_bytes_tx_total
```

These can be visualized using Prometheus and Grafana.

---

# HTTP Admin API

Optional HTTP control interface.

Default:

```
http://localhost:9106
```

Example endpoints:

```
GET /health
GET /bridges
GET /connections
POST /bridges/{name}/restart
POST /reload-config
```

---

# Terminal Interface

When run interactively, io-to-net launches a terminal dashboard.

Layout:

header  
tabs  
main view  
log panel  

Tabs include:

Ports  
Connections  
Stats  
Logs  

The interface shows:

active bridges  
connected clients  
bandwidth usage  
live logs  

---

# Microcontroller Nodes

The repository includes a PlatformIO firmware project.

Targets:

ESP32  
ESP8266  

These nodes expose microcontroller hardware interfaces over WiFi.

Supported interfaces:

UART  
I²C  
SPI  
GPIO  
OneWire  

This enables distributed sensor nodes using the same protocol as the desktop application.

---

# Packaging

Packaging scripts are included for:

Debian packages (.deb)  
RPM packages (.rpm)  

Example:

```
./scripts/build_packages.sh
```

---

# License

This project is licensed under the GNU General Public License v3.

See LICENSE file for details.

---

# Ecosystem

Related tools:

speeduino-to-mqtt  
gps-to-mqtt  
sensors-to-mqtt  

Together they form an open telemetry pipeline for embedded systems and motorsport data.

---

# Project Status

Early development.

Interfaces and configuration formats may change.

Contributions and feedback are welcome.