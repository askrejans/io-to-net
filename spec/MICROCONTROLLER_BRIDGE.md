# Microcontroller Bridge

A PlatformIO project will provide a lightweight IO bridge firmware.

Targets:

ESP32
ESP8266

Capabilities:

WiFi connection
serial bridging
bluetooth bridging

Architecture:

UART → TCP socket

Example configuration:

wifi ssid
wifi password
remote server address
remote port

This allows hardware to connect to the io-to-net server automatically.