---
name: hardware-validation
description: Validate io-to-net with real devices — the speeduino-serial-sim ESP32-S3 over USB with ecu-to-mqtt (rate and latency), a u-blox GPS with gps-to-mqtt, the dashboard, reload and restart.
---

# Hardware validation

## Setup

- `io-to-net --scan` lists the ports and their selectors. The bench uses the
  simulator (`usb:303A:1001`, 115200) and a u-blox GPS (`usb:1546:01A8`,
  9600). Don't open ports that belong to other projects; check with the user
  when unsure.
- A serial port has one owner: close TunerStudio and any monitor first.
- On a real ECU, only the client's own read-only polls may pass (ecu-to-mqtt
  sends `Q`, `A`). Don't type commands into the bridge by hand.
- No real GPS coordinates in commits, docs or screenshots. NMEA in a terminal
  is fine; don't paste it anywhere persistent.
- Throwaway broker: `mosquitto -p 18830`.

## Checks

| What | How | Pass |
|---|---|---|
| ECU latency | ecu-to-mqtt with `CONNECTION_TYPE=tcp` to the bridge, then its `/health` | about 50 Hz, under 20 ms (direct USB: 50 Hz / 14 ms) |
| Rate shown | `/api/v1/bridges/ecu` | `Speeduino`, about 50 polls/s, avg 130 B |
| GPS | gps-to-mqtt over TCP, or `nc` | NMEA sentences; `line` framing yields whole sentences |
| MQTT | `mosquitto_sub -p 18830 -t '#' -v -W 3` | `/ECU/*` and GPS topics present |
| Reload | add a bridge, `kill -HUP`; then remove it | `started` / `stopped`; others `unchanged` with their clients |
| Restart | `curl -X POST -H 'X-Requested-With: x' …/restart` | `opens` +1, the client stays connected, back to full rate |
| WebSocket / TLS | chain `tcp_client` bridges to the GPS bridge | one sentence per message; a wrong token gets `auth failed` |
| Dashboard | `http://127.0.0.1:9106/`, every view, both themes | live lamp green, the monitor shows polls and replies |

Report the numbers you measured and anything you skipped.
