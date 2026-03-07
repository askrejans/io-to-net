# io-to-net — General Design Notes

This document defines the global design philosophy and constraints for the io-to-net project.

Claude must read this document before implementing any code.

The goal is to build a universal hardware interface bridge that exposes local hardware interfaces over TCP/IP.

The system must be reliable, observable, and modular.

It must integrate cleanly with the existing ecosystem:

speeduino-to-mqtt  
gps-to-mqtt  
sensors-to-mqtt  

io-to-net becomes the universal transport layer underneath those systems.

Example pipeline:

hardware device → io-to-net → TCP/IP → sensors-to-mqtt → MQTT → telemetry platform


------------------------------------------------

PROJECT GOALS

1. Universal IO access

The system must expose hardware interfaces as network streams.

Supported IO types:

serial  
i2c  
spi  
gpio  
bluetooth  
onewire  
canbus  
pty  
unix sockets  

Each IO source becomes a byte stream that can be transmitted over network connections.


------------------------------------------------

NETWORK MODEL

io-to-net must support the following transports:

TCP (primary transport)  
TLS (secure TCP)  
WebSocket (browser / cloud compatibility)  

UDP must NOT be used for stream transport because serial and binary protocols require reliable ordering.

IPv4 and IPv6 must both be supported.

Listening endpoints should default to:

[::]:PORT

This allows dual-stack operation.


------------------------------------------------

CLIENT MODEL

Multiple clients may connect to the same IO bridge.

Modes:

mirror mode  
all clients receive the same data stream.

exclusive mode  
only one client allowed.

proxy mode  
TCP client connects to another remote endpoint.

This allows:

serial → TCP  
TCP → serial  
TCP → TCP


------------------------------------------------

SECURITY MODEL

Security must support the following:

TLS 1.3 encryption using rustls  
optional authentication tokens  
optional IP allowlists  

TLS configuration must allow certificate and key configuration.

Example:

[tls]
enabled = true
cert = "/etc/io-to-net/cert.pem"
key = "/etc/io-to-net/key.pem"


------------------------------------------------

OBSERVABILITY

The system must provide detailed runtime metrics.

Required metrics:

active bridges  
connected clients  
bytes transmitted  
bytes received  
bandwidth rate  
error counters  

Per connection metrics must also exist.

These metrics are displayed in the TUI.


------------------------------------------------

PROTOCOL TRANSPARENCY

io-to-net must not interpret or modify payload data.

It is a transparent byte stream bridge.

However optional protocol detection may be implemented for observability.

Example protocol hints:

NMEA  
Speeduino  
Megasquirt  
ASCII  
Binary  


------------------------------------------------

HIGH LEVEL ARCHITECTURE

The application consists of the following modules:

config loader  
bridge manager  
io drivers  
network transports  
connection manager  
metrics system  
TUI interface


------------------------------------------------

DIRECTORY STRUCTURE

The repository should use the following layout.

io-to-net/

Cargo.toml

src/

main.rs

config/
  mod.rs
  loader.rs
  schema.rs

core/
  bridge_manager.rs
  connection_manager.rs
  metrics.rs

drivers/
  serial.rs
  i2c.rs
  gpio.rs
  bluetooth.rs
  onewire.rs
  canbus.rs

transports/
  tcp.rs
  tls.rs
  websocket.rs

tui/
  mod.rs
  tabs.rs
  widgets.rs

scripts/
  build_packages.sh

service/
  io-to-net.service

platformio/
  esp-io-to-net/


------------------------------------------------

TUI DESIGN

The TUI must closely match the layout style used in the existing sensors-to-mqtt application.

The layout is fixed and must always be:

header (3 lines)  
tabs (1 line)  
main content area (minimum 8 lines)  
log panel (8 lines)

Layout example:

┌─────────────────────────────────────────────────────┐
│ io-to-net v0.1 | bridges: 4 | clients: 7 | 2.1MB/s │
│ host: node01 | uptime: 02:31:12                     │
│ build: release | rust async runtime: tokio         │
├─────────────────────────────────────────────────────┤
│ Ports | Connections | Stats | Logs                 │
├─────────────────────────────────────────────────────┤
│                                                     │
│ (Main tab content)                                  │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
├─────────────────────────────────────────────────────┤
│ tracing log output                                  │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
│                                                     │
└─────────────────────────────────────────────────────┘


------------------------------------------------

TABS DESCRIPTION

Ports Tab

Shows all configured bridges.

Columns:

name  
source type  
source path  
transport  
connected clients  
bandwidth  


Example:

name        source            transport       clients     bandwidth
ecu         /dev/ttyUSB0      tcp:9000        2           150KB/s
gps         /dev/ttyUSB1      tcp:9001        1           30KB/s
i2c_bus     /dev/i2c-1        tcp:9002        0           0


------------------------------------------------

Connections Tab

Displays active network clients.

Columns:

client ip  
bridge  
connected time  
bytes in  
bytes out  
transfer rate  


Example:

client           bridge    duration     rx        tx
192.168.1.50     ecu       00:02:11     340KB     210KB
192.168.1.60     gps       00:10:02     18KB      20KB


------------------------------------------------

Stats Tab

Displays global metrics.

Metrics:

total bridges  
active clients  
bytes transmitted  
bytes received  
system uptime  
error counters  

Traffic graph or sparkline may be shown.


------------------------------------------------

Logs Tab

Displays tracing logs.

Uses tracing crate.

Maximum log buffer size:

1000 entries.


------------------------------------------------

LOG PANEL

The bottom panel always shows recent logs.

This panel is always visible regardless of active tab.

It should display the most recent 8 log lines.


------------------------------------------------

DAEMON MODE

When run with:

--mode daemon

The TUI must be disabled.

Logs must be written to stdout or journald.

The program must run as a system service.


------------------------------------------------

SERVICE INSTALLATION

The project must include a systemd service file.

The service must support:

automatic restart  
resource limits  
security hardening  


------------------------------------------------

CROSS PLATFORM SUPPORT

Primary targets:

Linux  
MacOS  
BSD  
Windows  

The project must compile on all these platforms.

Platform-specific drivers may be conditionally compiled.


------------------------------------------------

MICROCONTROLLER VERSION

The microcontroller version must implement the same concept.

PlatformIO project:

platformio/esp-io-to-net

Targets:

ESP32  
ESP8266  

The firmware must expose hardware interfaces over TCP and WebSocket.

Supported IO on microcontroller:

UART  
I2C  
SPI  
GPIO  
OneWire  


------------------------------------------------

MICROCONTROLLER ARCHITECTURE

The ESP firmware acts as a lightweight version of io-to-net.

Key differences:

no TUI  
web configuration interface  
serial debugging console  

Example architecture:

sensor → ESP32 → TCP socket → io-to-net clients


------------------------------------------------

ESP FEATURES

WiFi connection

multiple bridge definitions

websocket transport

TCP transport

configuration stored in flash


------------------------------------------------

CONFIGURATION PHILOSOPHY

Configuration must use TOML.

Location:

/etc/io-to-net/config.toml

Environment variables must override configuration values.


------------------------------------------------

PERFORMANCE TARGETS

Latency:

< 5 ms bridge overhead

Throughput:

must support high speed serial:

921600 baud or higher

Memory footprint:

< 100 MB


------------------------------------------------

RUST IMPLEMENTATION REQUIREMENTS

Use the following crates:

tokio  
ratatui  
crossterm  
tracing  
serde  
toml  
rustls  


------------------------------------------------

IMPORTANT DESIGN RULE

The system must be transport transparent.

No transformation of the byte stream may occur.

The bridge must behave exactly like a local hardware connection.


------------------------------------------------

END OF DOCUMENT