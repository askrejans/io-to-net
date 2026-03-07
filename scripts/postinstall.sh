#!/usr/bin/env bash
# Post-install script for deb/rpm packages
set -e

# Reload systemd and enable the service
if command -v systemctl &>/dev/null; then
    systemctl daemon-reload
    systemctl enable io-to-net.service || true
fi

# Create config directory with secure permissions
mkdir -p /etc/io-to-net
chmod 750 /etc/io-to-net

# Create log directory
mkdir -p /var/log/io-to-net
chmod 755 /var/log/io-to-net

echo "io-to-net installed."
echo "Edit /etc/io-to-net/config.toml then start the service:"
echo "  systemctl start io-to-net"
