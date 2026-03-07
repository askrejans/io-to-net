# Configuration

Configuration uses TOML.

Location:

/etc/io-to-net/config.toml

Example:

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

[[bridges]]
name = "bluetooth_obd"

[bridges.source]
type = "bluetooth"
device = "00:1D:A5:68:98:8B"

[bridges.transport]
type = "tcp"
listen = "[::]:9001"