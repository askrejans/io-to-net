# io-to-net — agent guide

A transparent hardware-to-network byte bridge (Rust, tokio): serial, SocketCAN,
I²C, GPIO, 1-Wire, BLE, PTY and Unix sockets to TCP, TLS and WebSocket
clients, with an HTTP API, a web dashboard and a TUI. Part of `g86racing`:
read the [project guidance](../AGENTS.md) before changing anything another
component consumes (ecu-to-mqtt and gps-to-mqtt connect through it). Preserve
the current branch and any local edits.

Design, measurements, metrics and the security model:
[TECHNICAL.md](TECHNICAL.md). User documentation: [README.md](README.md).

## Skills

| Task | Skill |
|------|-------|
| Data path, sources, transports, client handling, framing | [skills/bridge-core](skills/bridge-core/SKILL.md) |
| Testing with real devices (ECU simulator, GPS, latency) | [skills/hardware-validation](skills/hardware-validation/SKILL.md) |
| Version bump, dependencies, audit, packages, Homebrew, firmware builds | [skills/release](skills/release/SKILL.md) |
| Cross-component telemetry contracts | [../skills/automotive-telemetry](../skills/automotive-telemetry/SKILL.md) |
| Lap/sector timing consumers | [../skills/race-timing-analysis](../skills/race-timing-analysis/SKILL.md) |

## Map

```
src/config/        schema (deny_unknown_fields), loader + validation, masking
src/link/          Link { rx, tx }: serial (threads), stream (tcp_client, unix),
                   pty, polled (adapter for drivers/)          ← untrusted input
src/drivers/       IoDriver: i2c, gpio, onewire, canbus (codec + SocketCAN), bluetooth
src/transports/    Listener + Upgrader (tcp / tls / websocket), ClientReader/Writer
src/core/          bridge (run loop, backoff), clients (Hub, auth, serve),
                   framer, stats (Registry, counters, monitor), manager (reload)
src/protocol/      passive protocol detector
src/api/           axum router, metrics (Prometheus text), web (embedded assets)
src/tui/           ratatui dashboard (blocking thread)
src/scan/          --scan (serial ports with USB selectors, I²C, GPIO, BLE)
web/               dashboard, embedded with include_str!
firmware/          ESP32 / ESP32-S3 / ESP8266 Wi-Fi node (PlatformIO)
config/example.toml   every source/transport; tests parse every block
systemd/, scripts/build_packages.sh, Cross.toml
```

## Commands

```bash
cargo test                                    # must pass
cargo clippy --all-targets -- -D warnings     # must be clean (also --no-default-features)
cargo fmt
cargo audit
cargo run -- --scan                           # ports + USB selectors
cargo run -- -c config.toml                   # TUI; --headless for logs
cargo run -- --check-config -c config.toml
```

Linux-only drivers: check them in Docker (`rust:1-slim-bookworm` with
`libdbus-1-dev pkg-config`): `cargo clippy --all-targets -- -D warnings && cargo test`.

## Invariants

1. **Bytes are never modified** except by the configured `frame_mode`.
   Protocol detection only observes.
2. **The device never waits for a client.** Fan-out uses `try_send`; a full
   client queue disconnects that client. Device writes use `try_send` and
   count drops.
3. **Reads and writes are independent.** No source may serialise a write
   behind a read timeout; that was the 0.1 latency bug.
4. **Listeners and clients outlive the source.** Source failures back off
   and reopen; they never drop clients.
5. **Untrusted input is bounded**: client lines (token ≤ 512 B), frames
   (64 KiB), handshakes (10 s), auth (5 s), the monitor (200 chunks / 16 KiB).
   No `unwrap` on network or device input.
6. **The config is strict and documented.** New keys need a serde default,
   validation in `loader.rs`, an entry in `config/example.toml` (tested), and
   the README table.
7. **API:** a token protects `/api/*`, POSTs need `X-Requested-With`, tokens
   are masked in `/api/v1/config`. The dashboard has no remote resources and
   no `innerHTML` (tested).
8. **Counters are counted once** (RX per read, not per client) and live in
   `BridgeStats` atomics. Metric names are a contract for dashboards: rename
   only with a CHANGELOG entry.

## Conventions

- Edition 2024, native async traits (no `async-trait`), `anyhow` for
  application errors, `tracing` for logs. std mutexes, never held across
  `.await`.
- Tests sit next to the code. Prefer local sockets and fake devices
  (`core/bridge.rs` tests) to mocks.
- Comments explain *why*: device quirks, measurements.
- Licence: GPL-3.0-only (`LICENCE`) with `AI_TRAINING_RESTRICTION.md`; both
  ship in every package.
