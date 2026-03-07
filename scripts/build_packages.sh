#!/usr/bin/env bash
set -euo pipefail

# io-to-net package builder
# Produces .deb and .rpm packages using fpm.
# Requirements: cargo (release build already done), fpm, ruby

BINARY_NAME="io-to-net"
VERSION=$(grep '^version' ../Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
ARCH="amd64"
DIST_DIR="$(pwd)/dist"

echo "==> Building io-to-net v${VERSION} packages"

# Build release binary
echo "==> Compiling release binary"
cargo build --release --manifest-path ../Cargo.toml

BINARY="../target/release/${BINARY_NAME}"

mkdir -p "${DIST_DIR}"

# ── Debian (.deb) ──────────────────────────────────────────────────
echo "==> Creating .deb package"
fpm \
  --force \
  --input-type dir \
  --output-type deb \
  --name "${BINARY_NAME}" \
  --version "${VERSION}" \
  --architecture "${ARCH}" \
  --maintainer "io-to-net project" \
  --description "Universal hardware IO bridge — exposes hardware interfaces over TCP/IP" \
  --license "GPL-3.0" \
  --url "https://github.com/io-to-net/io-to-net" \
  --deb-systemd "../systemd/${BINARY_NAME}.service" \
  --config-files "/etc/${BINARY_NAME}/config.toml" \
  --after-install scripts/postinstall.sh \
  --package "${DIST_DIR}/${BINARY_NAME}_${VERSION}_${ARCH}.deb" \
  "${BINARY}=/usr/bin/${BINARY_NAME}" \
  "../config/example.toml=/etc/${BINARY_NAME}/config.toml"

echo "    => ${DIST_DIR}/${BINARY_NAME}_${VERSION}_${ARCH}.deb"

# ── RPM ───────────────────────────────────────────────────────────
echo "==> Creating .rpm package"
fpm \
  --force \
  --input-type dir \
  --output-type rpm \
  --name "${BINARY_NAME}" \
  --version "${VERSION}" \
  --architecture "${ARCH}" \
  --maintainer "io-to-net project" \
  --description "Universal hardware IO bridge — exposes hardware interfaces over TCP/IP" \
  --license "GPL-3.0" \
  --url "https://github.com/io-to-net/io-to-net" \
  --config-files "/etc/${BINARY_NAME}/config.toml" \
  --after-install scripts/postinstall.sh \
  --package "${DIST_DIR}/${BINARY_NAME}-${VERSION}.${ARCH}.rpm" \
  "${BINARY}=/usr/bin/${BINARY_NAME}" \
  "../config/example.toml=/etc/${BINARY_NAME}/config.toml"

echo "    => ${DIST_DIR}/${BINARY_NAME}-${VERSION}.${ARCH}.rpm"

echo ""
echo "==> Done. Packages in ${DIST_DIR}/"
ls -lh "${DIST_DIR}/"
