# io-to-net — technical notes

## Architecture

```
            ┌─────────────── one bridge (core/bridge.rs) ──────────────┐
 device ──► │ link::open → Link { rx, tx }                              │
            │   serial: reader + writer threads on cloned port handles  │
            │   tcp_client / unix: tokio split stream                    │
            │   pty: openpty, poll(2) threads                            │
            │   polled drivers (I²C, GPIO, CAN, 1-Wire, BLE): one task   │
            │                                                            │
            │ run loop (select!):                                        │
            │   link.rx  → counters, detector, monitor, framer → Hub     │
            │   clients  → link.tx (try_send; full → dropped, counted)   │
            │   restart / stop / 1 s tick (rates, protocol)              │
            │                                                            │
            │ Listener (bound for the bridge's whole life)               │
            │   accept → task per client: upgrade (TLS/WS, 10 s)         │
            │          → token (5 s) → Hub::admit → write/read loop      │
            └────────────────────────────────────────────────────────────┘
 Registry (core/stats.rs): BridgeStats per bridge (atomics + small mutexes),
   log ring. Read by the HTTP API, SSE, Prometheus and the TUI.
 Manager (core/manager.rs): starts, stops and diffs bridges on reload.
```

### Data path

- **Full duplex.** Every source is a `Link` with independent read and write
  channels. Serial ports use two OS threads on `try_clone`d handles, so a
  write never waits for a read timeout. That wait was the 0.1 bottleneck: a
  client request sat until the 100 ms serial read timed out.
- **Fan-out.** `Hub::broadcast` calls `try_send` on each client's bounded
  queue (1024 chunks). A full queue means the client cannot keep up, so it is
  disconnected and counted in `lagged`. The device and the other clients never
  wait for it.
- **Client writes** go through one bridge channel to `link.tx`. While the
  source is down they are dropped and counted (`tx_dropped`): a request for a
  device that isn't there has no meaning later.
- **Framing** happens once per chunk before the fan-out, not once per client.
  A runaway frame without a terminator is flushed at 64 KiB.
- **Lifetimes.** The listener and the clients outlive the source. When the
  source fails, the bridge waits (1, 2, 4 … 30 s) and reopens it. A source
  that ran for 30 s resets the backoff. The clients keep their sockets and see
  data again as soon as the device is back. USB selectors are resolved again
  on every open, so a re-plugged adapter under a new `/dev` name is found.
- **Exclusive mode.** `admit` drops the previous client's queue; its task sees
  the end of the queue and closes the socket. This is how serial-over-TCP
  bridges behave, and it is what tools that reconnect with a fresh socket
  (TunerStudio) need.

### Measurements (2026-10-20, macOS arm64, release build)

| Path | Rate | Latency |
|---|---|---|
| ecu-to-mqtt → USB → speeduino-serial-sim (ESP32-S3), direct | 50 Hz | 14 ms |
| same through io-to-net **0.1.1** (TCP) | 9 Hz | 115 ms |
| same through io-to-net **0.2.0** (TCP) | **50 Hz** | **13.9 ms** |
| u-blox GPS 9600 Bd → `line` frames → gps-to-mqtt over TCP | 9–10 NMEA lines/s | — |

Both pipelines also published to MQTT (`/ECU/*`, `/GOLF86/GPS/*`) through the
bridge. Also verified:
- SIGHUP and API reloads: bridges added and removed while unchanged ones kept
  their clients.
- WebSocket line framing (one NMEA sentence per message).
- TLS with a token (a wrong token gets `auth failed`).
- An API restart of the ECU source: ecu-to-mqtt stayed connected, lost one
  2 s poll, and was back at 50 Hz.

## Protocol detection

`protocol::ProtocolDetector` observes the stream passively and never changes
the data path. The RX sample (4 KiB rolling) is classified every 256 new
bytes, not on every chunk. Request–reply ECUs answer with opaque binary, so
they are recognised from client writes instead:
- **Speeduino:** single-letter commands (`A`, `Q`, `S`, `n`, …) or a 7-byte
  `r` page read.
- **MegaSquirt / rusEFI:** the `[u16 length][payload][CRC32]` envelope.

Either needs at least 3 matching writes, and a majority. The reported rate
follows the protocol: lines per second for line protocols, polls per second
for request–reply ECUs (with the average reply size), reads per second
otherwise.

## Security model

- Bridge ports have no protection apart from an optional token line and TLS.
  The token is compared in constant time. A wrong token or a 5 s timeout gets
  `auth failed\n` and a close. The handshake (10 s) and the token run in the
  client's task, so a slow peer never blocks accepting others.
- The HTTP API is on loopback by default. `/api/*` takes a bearer token when
  one is configured; `/health`, `/metrics` and the static dashboard files stay
  open. A POST needs `X-Requested-With`, so a cross-site page can't send one
  without a CORS preflight, which is refused unless `cors_origin` is set.
- Responses carry `nosniff` and `no-store`. Dashboard assets carry a strict
  CSP (`default-src 'none'`, `script-src 'self'`, `frame-ancestors 'none'`).
  The page builds the DOM with `textContent` only; a test forbids `innerHTML`
  and remote URLs.
- `/api/v1/config` masks every `token`.
- systemd runs the service as `io-to-net`:
  - no capabilities, `ProtectSystem=strict`, `MemoryDenyWriteExecute`
  - `RestrictAddressFamilies` limited to IP, UNIX, CAN and NETLINK
  - `@system-service` syscall filter
  - hardware through groups (`dialout`, `tty`, `gpio`, `i2c`, `bluetooth`)

## Prometheus metrics

`io_to_net_info{version}`, `io_to_net_uptime_seconds`, and
`io_to_net_bridge_info{bridge,source,source_kind,transport,listen}`. Every
metric below has a `bridge` label.

| Gauges | Counters (`_total`) |
|---|---|
| `bridge_source_open`, `bridge_clients`, `bridge_rx_bytes_per_second`, `bridge_frame_rate` | `bridge_rx_bytes`, `bridge_tx_bytes`, `bridge_rx_chunks`, `bridge_tx_chunks`, `bridge_tx_dropped`, `bridge_source_opens`, `bridge_source_open_failures`, `bridge_source_errors`, `bridge_clients_connected`, `bridge_clients_refused`, `bridge_clients_lagged`, `bridge_clients_replaced`, `bridge_auth_failures` |

The registry is rendered on each scrape; there is no global metrics state.

## 0.2 review findings (0.1.1 → 0.2.0)

| Finding in 0.1.1 | Effect | 0.2.0 |
|---|---|---|
| Client writes only forwarded between 100 ms serial read timeouts | Request–reply devices at 9 Hz / 115 ms | Full-duplex links; 50 Hz / 13.9 ms |
| `max_clients` counter in each transport never decremented | After N connections in total, every client refused for good | Limit enforced from the live client list |
| `/reload-config` sent SIGHUP without a handler | The API "reload" killed the process | SIGHUP and `POST /api/v1/reload` diff bridges in place |
| RX bytes counted once per client | Metrics multiplied by the client count | Counted once per read |
| `exclusive` and `proxy` modes accepted but not implemented; per-bridge auth parsed but unused; `BridgeConfig.tls` unused | Settings silently ignored | `exclusive` implemented, `proxy` removed, bridge tokens enforced, unknown keys rejected |
| CAN filter `x & mask != x & mask` | Filter was a no-op | `id & mask == filter_id & mask`, tested |
| Protocol stats updated only when the hint changed | Rates frozen | Updated every second |
| TLS/WS handshake awaited in the accept loop | One slow client stalled all accepts | Handshake in the client task, 10 s limit |
| Bridge shut down when its source failed; listener rebound each time | Clients dropped on every USB glitch | Listener and clients outlive the source |
| API and metrics on `0.0.0.0` by default, no CSRF guard | Anyone on the LAN could restart bridges | Loopback default, token, `X-Requested-With` |
| `spawn_blocking` serial reads cancelled at shutdown | Error spam and an unclean port close | OS threads with stop flags |
| Dependencies: OpenSSL via native-tls, unmaintained `rustls-pemfile`, global `prometheus` registry, `async-trait`, 11 `cargo audit` findings | Larger attack surface | rustls + ring only, pki-types PEM, hand-rendered metrics, native async traits; `cargo audit` clean |
| Wrong repository URL, fpm-based packaging (amd64 only) | — | `askrejans/io-to-net`; deb/rpm x86-64 + arm64, Windows zip, macOS tarballs, Homebrew |
| Firmware: drifting client counter, blocking 2 s token wait, Nagle on, ESP8266 debug on the data UART, unpinned platforms | Lockouts, lost UART data, latency | Live-slot counting, non-blocking token, TCP_NODELAY, debug on UART1, `espressif32@8.0.0` / `espressif8266@4.2.1` |

## Testing

- `cargo test` (93 tests): config schema and every example block, framer, CAN
  codec, protocol detector, hub admission, exclusive takeover and lag, bridge
  integration against a fake TCP device. The integration tests cover
  round-trip latency, single RX counting, read-only mode, token auth with
  leftover bytes, exclusive handover, source loss and reconnect, restart and
  an unreachable source. Also: the PTY link, serial selectors, TLS
  configuration, WebSocket round trips, the API (auth, CSRF guard, monitor,
  headers), the Prometheus format, manager diffing and file reload, and TUI
  rendering.
- Run on macOS and Linux (Docker, `rust:1-slim-bookworm`), with and without
  the `bluetooth` feature; `cargo clippy --all-targets -- -D warnings` is
  clean on both.
- Hardware: see the measurements above. I²C, GPIO, 1-Wire and SocketCAN
  compile and their codecs are unit-tested, but they were not exercised on
  real hardware for 0.2.0.
