# Architecture

io-to-net consists of five major components.

1 IO Drivers
2 Transport Layer
3 Bridge Manager
4 Observability Layer
5 TUI Interface

IO Drivers provide access to local hardware.

Examples:

serial
i2c
gpio
bluetooth
canbus
unix sockets

Each driver produces a byte stream.

Transport Layer exposes that stream over network connections.

Supported transports:

TCP
TLS
WebSocket

Bridge Manager maps IO sources to network endpoints.

Example:

/dev/ttyUSB0 → tcp://0.0.0.0:9000

Observability Layer collects metrics:

bytes transmitted
connected clients
errors
bandwidth usage

TUI Interface displays live system state.

Architecture is fully async using tokio.