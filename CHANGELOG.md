# Changelog

## 0.2.0

A rewrite of the data path, plus a web dashboard and packaging. See
[TECHNICAL.md](TECHNICAL.md#02-review-findings-011--020) for the review
findings behind each change.

### Breaking changes
- Config files are strict: unknown keys are errors. `mode = "proxy"` is
  removed. `[http_api]` now defaults to `127.0.0.1:9106` (it was
  `0.0.0.0:9106`), and the extra `[metrics]` listener is off unless
  configured. The HTTP API also serves `/metrics`. `rate_limit_per_second` is
  gone.
- HTTP API routes moved under `/api/v1/`: `/bridges` → `/api/v1/bridges`,
  `/logs` → `/api/v1/logs`, `/reload-config` → `POST /api/v1/reload`. POSTs
  need an `X-Requested-With` header.
- Prometheus metric names changed to `io_to_net_bridge_*{bridge=…}` (see
  TECHNICAL.md).
- `--daemon` is now `--headless` (the old name still works). Headless logs
  are plain text; add `--log-json` for JSON lines.
- The deb/rpm install a dedicated `io-to-net` user. Packages are built per
  architecture (amd64 and arm64).

### Added
- Full-duplex sources: request–reply devices run at direct-connection
  latency (Speeduino via ecu-to-mqtt: 50 Hz / 13.9 ms, against 9 Hz / 115 ms
  in 0.1.1).
- `tcp_client` source (Wi-Fi serial adapters, the ESP node, chained bridges).
- Serial: USB selectors `usb:VID:PID[:SERIAL]` and `usb:name=…`, resolved
  again on every open; `dtr` and `rts` options.
- SocketCAN formats `json` (the ecu-to-mqtt gateway line format), `slcan` and
  `binary`. Clients can send frames in the same format.
- BLE write characteristic and a configurable scan time.
- `exclusive` mode (a new client replaces the old one), per-bridge token auth,
  and disconnection of lagging clients.
- TLS client-certificate verification (`client_ca`); PKCS#1 and SEC1 keys
  accepted. `web_socket` also accepts `websocket` as the type name.
- Live reload with SIGHUP, `systemctl reload`, `POST /api/v1/reload`, the
  dashboard or TUI `R`. Unchanged bridges keep running; a bad file is
  rejected and the running bridges are kept.
- Web dashboard at `/`:
  - Bridges, a hex/ASCII Monitor, Clients, Ports (USB selectors to copy),
    Logs and Configuration views.
  - Restart and reload buttons; light and dark themes; strict CSP.
- API: `/api/v1/status`, `/bridges/{name}/monitor`, `/bridges/{name}/restart`,
  `/config` (masked), `/ports`, `/logs`, and an SSE `/stream`.
- `/health` returns 503 while any source is down; `--healthcheck` and
  `--check-config` added.
- TUI: bridge selection, source restart, reload, a hex monitor and log
  scrolling. It runs on its own thread.
- Protocol detection recognises Speeduino and MegaSquirt from their polls.
  Rates are reported as lines, polls or reads per second, depending on the
  protocol.
- `--scan` prints a USB selector for each port.
- Packaging: deb and rpm for x86-64 and arm64, a Windows zip, macOS tarballs
  and a Homebrew formula. The hardened systemd unit gains `ExecReload` and a
  `/run/io-to-net` runtime directory for PTY symlinks.
- Firmware 0.2.0: an ESP32-S3 environment, pinned platforms
  (`espressif32@8.0.0`, `espressif8266@4.2.1`), and configurable UART pins.

### Fixed
- The client limit counter never decremented, so the limit was eventually
  hit permanently.
- `/reload-config` killed the process: it sent SIGHUP, which had no handler.
- RX bytes were counted once per client.
- The CAN ID filter was a no-op.
- Protocol statistics froze once the protocol hint stopped changing.
- A slow TLS or WebSocket handshake blocked the accept loop.
- Clients were dropped whenever the source failed.
- At shutdown, cancelled `spawn_blocking` serial reads produced errors.
- Firmware:
  - the client count drifted until all connections were refused
  - the 2 s blocking token wait lost UART data
  - Nagle delayed replies
  - ESP8266 debug output went into the data UART
  - the ESP8266 I²C and GPIO pin defaults collided

### Changed
- Dependencies updated. OpenSSL, `async-trait`, `prometheus`,
  `rustls-pemfile`, `parking_lot`, `regex`, `once_cell`, `hyper`, `tower-http`
  and the separate `crossterm` are gone. TLS uses rustls with ring;
  `cargo audit` reports 0 vulnerabilities (11 in 0.1.1).
- serialport is built without libudev (sysfs enumeration still includes USB
  IDs); the only native Linux dependency is libdbus, for BLE. Build with
  `--no-default-features` to drop BLE.
- Rust 1.89+, edition 2024. Licence unchanged: GPL-3.0-only with the AI
  training restriction.

## 0.1.1

- Serial driver: transient error tracking and timeout-based reads.

## 0.1.0

- First release.
