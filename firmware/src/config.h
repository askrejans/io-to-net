#pragma once

// ─────────────────────────────────────────────────────────────────
//  io-to-net firmware configuration
//  Edit this file before flashing.
//
//  WiFi credentials and the auth token live in secrets.h which is
//  gitignored.  Copy secrets.h.example → secrets.h to get started:
//
//    cp firmware/src/secrets.h.example firmware/src/secrets.h
// ─────────────────────────────────────────────────────────────────

#include "secrets.h"

// ── WiFi ──────────────────────────────────────────────────────────
// WIFI_SSID, WIFI_PASSWORD and AUTH_TOKEN are defined in secrets.h.

// Hostname advertised on the network (mDNS)
#define HOSTNAME        "io-to-net-node"

// ── Bridge definitions ────────────────────────────────────────────
// Enable/disable individual bridge types by commenting out defines.

// UART bridge — forwards Serial2 (GPIO16 RX, GPIO17 TX on ESP32) over TCP
#define ENABLE_UART_BRIDGE
#define UART_TCP_PORT       9000
#define UART_BAUD_RATE      115200
#define UART_READ_TIMEOUT   10       // ms

// I2C bridge — polls all I2C addresses and streams found data over TCP
// #define ENABLE_I2C_BRIDGE
#define I2C_TCP_PORT        9001
#define I2C_SDA_PIN         21      // ESP32 default SDA
#define I2C_SCL_PIN         22      // ESP32 default SCL
#define I2C_POLL_MS         100     // poll interval (ms)
#define I2C_TARGET_ADDR     0x68    // I2C slave address to poll
#define I2C_READ_LEN        6       // bytes to read per poll

// GPIO event bridge — streams edge events over TCP
// #define ENABLE_GPIO_BRIDGE
#define GPIO_TCP_PORT       9002
#define GPIO_PIN            4       // pin to monitor
#define GPIO_PULL           INPUT_PULLUP

// ── Security ──────────────────────────────────────────────────────
// AUTH_TOKEN is defined in secrets.h (gitignored).
// Set it to "" in secrets.h to disable authentication.

// ── Max clients per bridge ────────────────────────────────────────
#define MAX_CLIENTS_PER_BRIDGE  4

// ── Status LED ───────────────────────────────────────────────────
// LED_BUILTIN blinks during WiFi connect; solid on when all bridges running.
// Set to -1 to disable.
#define STATUS_LED_PIN      LED_BUILTIN

// ── Debug serial output ───────────────────────────────────────────
#define DEBUG_SERIAL        Serial
#define DEBUG_BAUD          115200
