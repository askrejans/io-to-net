# ── Build stage ────────────────────────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder

# libdbus for BLE sources (btleplug → BlueZ); build with
# --build-arg FEATURES=--no-default-features to leave BLE out.
ARG FEATURES=""
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies: build a dummy main first, then the real sources.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release --locked $FEATURES 2>/dev/null || true \
    && rm -rf src

# The copied sources keep their host mtimes, which can be older than the dummy
# build above; touch them so cargo rebuilds the real binary.
COPY . .
RUN find src web -type f -exec touch {} + \
    && cargo build --release --locked $FEATURES

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        libdbus-1-3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r io-to-net \
    && useradd -r -g io-to-net --groups dialout,tty --no-create-home \
        --shell /usr/sbin/nologin io-to-net \
    && mkdir -p /etc/io-to-net

COPY --from=builder /app/target/release/io-to-net /usr/local/bin/io-to-net
COPY config/example.toml /etc/io-to-net/config.toml
COPY LICENCE /usr/share/doc/io-to-net/copyright
COPY AI_TRAINING_RESTRICTION.md /usr/share/doc/io-to-net/

USER io-to-net

# Pass devices in, e.g. --device /dev/ttyACM0 (or --network host for SocketCAN),
# and mount your config over /etc/io-to-net/config.toml. Inside a container the
# HTTP API must listen on 0.0.0.0 to be published; set [http_api.auth] then.
EXPOSE 9000 9106

# Healthy = every source open (HTTP 200 from /health).
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["io-to-net", "--healthcheck", "--config", "/etc/io-to-net/config.toml"]

ENTRYPOINT ["io-to-net", "--headless"]
CMD ["--config", "/etc/io-to-net/config.toml"]
