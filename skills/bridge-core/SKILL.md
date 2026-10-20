---
name: bridge-core
description: Change io-to-net's data path — sources (link/, drivers/), transports, client admission and fan-out, framing, counters — without adding latency or letting one client stall the device.
---

# Bridge core

## Where things go

- **New source type:** add a `SourceConfig` variant (schema, `kind()`,
  `describe()`), validation in `config/loader.rs` and a block in
  `config/example.toml` (the `every_example_block_is_valid` test lists the
  kinds).
  - Stream-like devices get their own `link/` module that returns a `Link`.
  - Polled devices implement `drivers::IoDriver` and are dispatched in
    `link/polled.rs`.
  - Linux-only code sits behind `#[cfg(target_os = "linux")]`; other
    platforms get a clear error message.
- **New transport:** an `Upgrader` variant in `transports/mod.rs`, plus
  `ClientReader`/`ClientWriter` arms. The handshake must stay inside
  `upgrade()`, which runs in the client task under `HANDSHAKE_TIMEOUT`.
- **Client policy** (admission, modes, auth) lives in `core/clients.rs`.
  **Source lifetime** (open, backoff, restart) lives in `core/bridge.rs`.

## Rules

- A `Link` reads and writes independently. Never write from the read loop or
  wait for a read before writing.
- The run loop must not await anything slow. Use `try_send` to clients
  (lagging clients are dropped) and to the device (drops are counted).
- Count once: RX per device read, TX per forwarded client chunk.
- Every buffer fed by a client or device has a limit.
- Keep `Status` transitions truthful: `Opening`, then `Open` or `Waiting`
  (with the error as `detail`), and `Stopped` only when `auto_restart` is off
  or on shutdown.

## Verify

- The `core/bridge.rs` tests use a fake TCP device: 20 request–reply round
  trips in under 500 ms, reconnects, exclusive takeover, tokens. Add a case
  for any new behaviour.
- Then run the latency check in the hardware-validation skill.
