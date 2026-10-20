# io-to-net firmware

A standalone Wi-Fi bridge node for **ESP32**, **ESP32-S3** and **ESP8266**.
The node joins your Wi-Fi network and exposes its UART, an I²C sensor and a
GPIO input as raw TCP servers. The desktop `io-to-net` can relay them further
with a `tcp_client` source, or a client can connect directly.

| Toolchain | Version |
|---|---|
| PlatformIO Core | 6.1+ |
| `espressif32` platform (ESP32, ESP32-S3) | 8.0.0 |
| `espressif8266` platform | 4.2.1 |

## Secrets

The Wi-Fi credentials and the optional token live in `src/secrets.h`. That
file is gitignored and must never be committed.

```bash
cp src/secrets.h.example src/secrets.h
$EDITOR src/secrets.h      # WIFI_SSID, WIFI_PASSWORD, AUTH_TOKEN ("" = no token)
```

## Configuration

`src/config.h` turns bridges on and off and sets their ports and pins:

```c
#define ENABLE_UART_BRIDGE        // port 9000, UART_BAUD_RATE 115200
// #define ENABLE_I2C_BRIDGE      // port 9001, polls I2C_TARGET_ADDR every I2C_POLL_MS
// #define ENABLE_GPIO_BRIDGE     // port 9002, 0x01 rising / 0x02 falling
#define MAX_CLIENTS_PER_BRIDGE 4
```

## Build and flash

```bash
pio run -e esp32dev   --target upload    # classic ESP32
pio run -e esp32s3    --target upload    # ESP32-S3 DevKitC
pio run -e esp8266dev --target upload    # NodeMCU / ESP-12
pio device monitor --baud 115200
```

On Apple Silicon the ESP8266 toolchain is x86-only. Build that environment in
an arm64 Linux container, or under Rosetta.

## Pins

| Function | ESP32 | ESP32-S3 | ESP8266 |
|---|---|---|---|
| UART RX / TX (data) | 16 / 17 | 18 / 17 | GPIO3 / GPIO1 (UART0) |
| Debug log | USB serial | USB serial | GPIO2 (UART1, TX only) |
| I²C SDA / SCL | 21 / 22 | 21 / 22 | GPIO4 / GPIO5 (D2 / D1) |
| GPIO input | 4 | 4 | GPIO14 (D5) |
| Status LED | 2 | off | 2 |

The ESP8266 has a single full UART, so data uses UART0 and the debug log goes
to UART1. Disconnect the USB-serial adapter from RX while a device is wired to
it.

## Behaviour

- The UART is read in blocks and each block goes out in one TCP write. Client
  bytes reach the UART on the next loop pass. TCP_NODELAY is set, so
  request–reply protocols (Speeduino, MODBUS) see no Nagle delay.
- A token check (the client's first line) runs in the background and never
  stalls the UART. A wrong token or 5 s of silence gets `auth failed` and a
  disconnect.
- When every slot is taken, a new connection replaces the oldest client. A
  tool that reconnects while its previous socket still looks open is not
  locked out.
- After the first connect, Wi-Fi reconnects in the background; the bridges
  keep running and accept clients again once the link is back.

## Relaying through desktop io-to-net

Use desktop io-to-net to add TLS or WebSocket, a token, line framing, metrics
or the dashboard in front of the node:

```toml
[[bridges]]
name = "esp-uart"
mode = "exclusive"

[bridges.source]
type = "tcp_client"
address = "192.168.1.50:9000"   # the node's IP

[bridges.transport]
type = "tcp"
listen = "[::]:9100"
```

Or connect directly: `nc <node-ip> 9000`.
