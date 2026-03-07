# io-to-net — Implementation Strategy

This document defines the internal architecture and coding strategy for the io-to-net project.

It specifies the async model, trait abstractions, buffering approach, and runtime behavior.  
All implementations must follow this design to ensure stability, performance, and maintainability.

Implementation language: Rust  
Async runtime: Tokio

------------------------------------------------

RUNTIME MODEL

The entire system runs on a Tokio async runtime.

Each bridge instance runs independently as an async task.

Example runtime structure:

main
 ├─ config loader
 ├─ bridge manager
 │   ├─ bridge task
 │   ├─ bridge task
 │   ├─ bridge task
 │
 ├─ connection manager
 ├─ metrics collector
 └─ TUI renderer

Each bridge is isolated so failures do not affect other bridges.

------------------------------------------------

CORE COMPONENTS

The system consists of the following modules:

Config Loader  
Bridge Manager  
IO Drivers  
Transport Layer  
Connection Manager  
Metrics System  
TUI Renderer  

Each module must remain loosely coupled.

------------------------------------------------

BRIDGE MODEL

A bridge connects a hardware interface to a network endpoint.

Example:

serial port → TCP server

Each bridge contains:

source driver  
transport server  
client list  
metrics  

Example configuration:

[[bridges]]
name = "ecu"

[bridges.source]
type = "serial"
port = "/dev/ttyUSB0"
baud = 115200

[bridges.transport]
type = "tcp"
listen = "[::]:9000"

------------------------------------------------

BRIDGE LIFECYCLE

A bridge runs through these stages:

1 initialize driver  
2 open hardware interface  
3 start network listener  
4 accept client connections  
5 forward data between endpoints  
6 collect metrics  
7 shutdown gracefully  

If a driver fails the bridge must automatically restart.

------------------------------------------------

IO DRIVER ABSTRACTION

Drivers must implement a unified interface.

Example trait:

pub trait IoDriver {
    async fn open(&mut self) -> Result<()>;
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn close(&mut self) -> Result<()>;
}

Drivers produce raw byte streams.

Drivers must not interpret protocols.

Supported drivers:

serial  
i2c  
spi  
gpio  
bluetooth  
onewire  
canbus  
pty  
unix socket  

Platform-specific drivers may use conditional compilation.

------------------------------------------------

TRANSPORT ABSTRACTION

Network transports expose IO streams to remote clients.

Example transport types:

tcp server  
tls server  
websocket server  

Transport interface example:

pub trait Transport {
    async fn start(&self) -> Result<()>;
}

Each transport may serve multiple clients simultaneously.

------------------------------------------------

CLIENT CONNECTION MODEL

Each client runs as an async task.

Typical behavior:

loop:
  read from device
  send to client

  read from client
  send to device

Connections must terminate cleanly on error.

Client tasks must never block driver tasks.

------------------------------------------------

DATA FLOW

The bridge must forward raw byte streams.

Example flow:

hardware device
    ↓
driver read
    ↓
broadcast to clients
    ↓
client socket write

Reverse flow:

client socket write
    ↓
driver write
    ↓
hardware device

Payload data must remain untouched.

------------------------------------------------

BROADCAST MODEL

Multiple clients may connect to one bridge.

Device output must be broadcast to all connected clients.

Possible implementations:

tokio broadcast channel  
tokio mpsc fan-out  

The system must handle dynamic connection changes.

------------------------------------------------

BUFFER STRATEGY

Buffers must be reused.

Recommended buffer size:

4096 bytes

Avoid allocations inside the main data path.

Example:

let mut buffer = [0u8; 4096];

------------------------------------------------

BACKPRESSURE HANDLING

If a client becomes slow:

apply write timeout

If timeout exceeded:

disconnect the client.

Slow clients must not block the entire bridge.

------------------------------------------------

ERROR HANDLING

Errors must be categorized:

driver errors  
network errors  
configuration errors  

All errors must be logged using tracing.

Example log events:

driver failure  
client connect  
client disconnect  
transport error  

------------------------------------------------

METRICS COLLECTION

Each bridge must track:

connected clients  
bytes transmitted  
bytes received  
transfer rate  
error counters  

Metrics must be available to the TUI.

------------------------------------------------

TUI UPDATE MODEL

The TUI must refresh periodically.

Recommended interval:

100 milliseconds

State snapshot should be taken before rendering to avoid locking shared state during UI drawing.

------------------------------------------------

ASYNC TASK MODEL

Typical runtime tasks:

bridge task  
connection task per client  
metrics collector task  
TUI task  

Use Tokio spawn for concurrency.

------------------------------------------------

SHUTDOWN HANDLING

Graceful shutdown must support:

SIGINT  
SIGTERM  

Shutdown sequence:

stop accepting new connections  
close client connections  
close drivers  
flush logs  

------------------------------------------------

DEPENDENCIES

The following crates should be used:

tokio  
ratatui  
crossterm  
tracing  
serde  
toml  
rustls  
anyhow  

------------------------------------------------

PERFORMANCE TARGETS

Bridge latency overhead:

less than 5 milliseconds

Serial throughput target:

921600 baud or higher

Memory usage:

less than 100MB

------------------------------------------------

IMPORTANT DESIGN RULE

io-to-net must remain transport transparent.

No modification of payload data is allowed.

The bridge must behave exactly like a direct hardware connection.

------------------------------------------------

END OF DOCUMENT