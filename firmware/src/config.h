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

// UART bridge — forwards a hardware UART over TCP
// (ESP32: UART2 on UART_RX_PIN/UART_TX_PIN; ESP8266: UART0 on GPIO3/GPIO1)
#define ENABLE_UART_BRIDGE
#define UART_TCP_PORT       9000
#define UART_BAUD_RATE      115200
#ifndef UART_RX_PIN
#define UART_RX_PIN         16
#endif
#ifndef UART_TX_PIN
#define UART_TX_PIN         17
#endif
#define UART_RX_BUFFER      2048    // bytes buffered while Wi-Fi is busy (ESP32)

// I2C bridge — polls all I2C addresses and streams found data over TCP
// #define ENABLE_I2C_BRIDGE
#define I2C_TCP_PORT        9001
#ifdef ARDUINO_ARCH_ESP8266
#define I2C_SDA_PIN         4       // NodeMCU D2
#define I2C_SCL_PIN         5       // NodeMCU D1
#else
#define I2C_SDA_PIN         21      // ESP32 default SDA
#define I2C_SCL_PIN         22      // ESP32 default SCL
#endif
#define I2C_POLL_MS         100     // poll interval (ms)
#define I2C_TARGET_ADDR     0x68    // I2C slave address to poll
#define I2C_READ_LEN        6       // bytes to read per poll

// GPIO event bridge — streams edge events over TCP
// #define ENABLE_GPIO_BRIDGE
#define GPIO_TCP_PORT       9002
#ifdef ARDUINO_ARCH_ESP8266
#define GPIO_PIN            14      // NodeMCU D5 (GPIO4 is I2C SDA)
#else
#define GPIO_PIN            4       // pin to monitor
#endif
#define GPIO_PULL           INPUT_PULLUP

// ── Security ──────────────────────────────────────────────────────
// AUTH_TOKEN is defined in secrets.h (gitignored).
// Set it to "" in secrets.h to disable authentication.

// ── Max clients per bridge ────────────────────────────────────────
// When all slots are taken, a new connection replaces the oldest client.
#define MAX_CLIENTS_PER_BRIDGE  4

// ── Status LED ───────────────────────────────────────────────────
// Blinks during the first Wi-Fi connect; on while connected. -1 disables.
#ifndef STATUS_LED_PIN
#define STATUS_LED_PIN      2       // on-board LED of most ESP32 / NodeMCU boards
#endif

// ── Debug serial output ───────────────────────────────────────────
// ESP8266 has one full UART, used for data, so debug goes to UART1 (GPIO2, TX only).
#ifdef ARDUINO_ARCH_ESP8266
#define DEBUG_SERIAL        Serial1
#else
#define DEBUG_SERIAL        Serial
#endif
#define DEBUG_BAUD          115200
