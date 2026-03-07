# io-to-net Firmware

Standalone io-to-net bridge firmware for **ESP32** and **ESP8266**.

The node connects to WiFi and exposes hardware interfaces (UART, I2C, GPIO) as TCP servers — implementing the same protocol as the desktop `io-to-net` application.

---

## Prerequisites

- [PlatformIO](https://platformio.org/) (CLI or VS Code extension)
- ESP32 or ESP8266 board

---

## Secrets (WiFi credentials)

WiFi credentials and the auth token are kept in `src/secrets.h`, which is **gitignored** and must never be committed.

Create it from the provided example before building:

```bash
cp firmware/src/secrets.h.example firmware/src/secrets.h
```

Then edit `src/secrets.h`:

```c
#define WIFI_SSID       "YourNetworkName"
#define WIFI_PASSWORD   "YourNetworkPassword"

// Set to "" to disable token auth
#define AUTH_TOKEN      "my-secret-token"
```

`secrets.h.example` is committed to the repository as a reference template.

---

## Configuration

Enable or disable bridges and adjust ports in `src/config.h`:

```c
// Enable the bridges you need
#define ENABLE_UART_BRIDGE
// #define ENABLE_I2C_BRIDGE
// #define ENABLE_GPIO_BRIDGE

// Port numbers
#define UART_TCP_PORT  9000
#define UART_BAUD_RATE 115200
```

---

## Build & Flash

```bash
# ESP32
pio run -e esp32dev --target upload

# ESP8266
pio run -e esp8266dev --target upload

# Open serial monitor
pio device monitor --baud 115200
```

---

## Bridge Ports

| Bridge | Default Port | Protocol |
|--------|-------------|----------|
| UART   | 9000        | Raw bytes bidirectional |
| I2C    | 9001        | Read-only byte stream |
| GPIO   | 9002        | 1-byte events: 0x01=rising 0x02=falling |

---

## Connecting from io-to-net Desktop

Add the firmware node as a TCP source in your desktop config:

```toml
[[bridges]]
name = "esp32-uart"

[bridges.source]
type = "unix_socket"   # or configure a TCP proxy bridge

[bridges.transport]
type = "tcp"
listen = "[::]:8000"
```

Or connect directly using `netcat`:

```
nc <ESP32-IP> 9000
```

---

## Hardware Pinout (ESP32)

| Function | GPIO |
|----------|------|
| UART RX  | 16   |
| UART TX  | 17   |
| I2C SDA  | 21   |
| I2C SCL  | 22   |
| GPIO mon | 4    |
| Status LED | 2  |

---

## Authentication

Set `AUTH_TOKEN` in `config.h` to a non-empty string. Clients must send the token followed by `\n` as the first message after connecting. Empty string disables auth.

---

## Max Clients

`MAX_CLIENTS_PER_BRIDGE` (default 4) — connections beyond this limit are refused.
