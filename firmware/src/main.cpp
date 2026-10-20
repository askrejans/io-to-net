#include <Arduino.h>
#include "config.h"
#include "wifi_manager.h"
#include "tcp_bridge.h"
#include "uart_bridge.h"
#include "i2c_bridge.h"
#include "gpio_bridge.h"

// ─── Global objects ───────────────────────────────────────────────

WiFiManager wifiManager;

#ifdef ENABLE_UART_BRIDGE
TcpBridge  uartTcpBridge(UART_TCP_PORT, "uart");
#ifdef ARDUINO_ARCH_ESP32
  UartBridge uartBridge(Serial2, uartTcpBridge);  // pins from config.h
#else
  UartBridge uartBridge(Serial, uartTcpBridge);   // UART0; debug is on Serial1
#endif
#endif

#ifdef ENABLE_I2C_BRIDGE
TcpBridge i2cTcpBridge(I2C_TCP_PORT, "i2c");
I2cBridge i2cBridge(i2cTcpBridge);
#endif

#ifdef ENABLE_GPIO_BRIDGE
TcpBridge  gpioTcpBridge(GPIO_TCP_PORT, "gpio");
GpioBridge gpioBridge(gpioTcpBridge, GPIO_PIN, GPIO_PULL);
#endif

// ─── setup() ─────────────────────────────────────────────────────

void setup() {
    // Debug serial
    DEBUG_SERIAL.begin(DEBUG_BAUD);
    delay(200);
    DEBUG_SERIAL.printf("\n\nio-to-net firmware v%s\n", FIRMWARE_VERSION);
    DEBUG_SERIAL.printf("Build: %s %s\n", __DATE__, __TIME__);

#if STATUS_LED_PIN != -1
    pinMode(STATUS_LED_PIN, OUTPUT);
    digitalWrite(STATUS_LED_PIN, LOW);
#endif

    // Connect to WiFi (blocks until connected or timeout)
    wifiManager.begin(WIFI_SSID, WIFI_PASSWORD);

    // Initialise bridges
#ifdef ENABLE_UART_BRIDGE
    uartBridge.begin(UART_BAUD_RATE);
    uartTcpBridge.begin();
#endif

#ifdef ENABLE_I2C_BRIDGE
    i2cBridge.begin(I2C_SDA_PIN, I2C_SCL_PIN, I2C_TARGET_ADDR, I2C_READ_LEN);
    i2cTcpBridge.begin();
#endif

#ifdef ENABLE_GPIO_BRIDGE
    gpioBridge.begin();
    gpioTcpBridge.begin();
#endif

    DEBUG_SERIAL.printf("Ready. IP: %s\n", wifiManager.ipAddress().c_str());

#ifdef ENABLE_UART_BRIDGE
    DEBUG_SERIAL.printf("  UART bridge: %s:%u\n", wifiManager.ipAddress().c_str(), UART_TCP_PORT);
#endif
#ifdef ENABLE_I2C_BRIDGE
    DEBUG_SERIAL.printf("  I2C bridge:  %s:%u\n", wifiManager.ipAddress().c_str(), I2C_TCP_PORT);
#endif
#ifdef ENABLE_GPIO_BRIDGE
    DEBUG_SERIAL.printf("  GPIO bridge: %s:%u\n", wifiManager.ipAddress().c_str(), GPIO_TCP_PORT);
#endif
}

// ─── loop() ──────────────────────────────────────────────────────

void loop() {
    // Maintain WiFi connection
    wifiManager.maintain();

    // Accept any new TCP clients
#ifdef ENABLE_UART_BRIDGE
    uartTcpBridge.acceptClients();
    uartBridge.process();
#endif

#ifdef ENABLE_I2C_BRIDGE
    i2cTcpBridge.acceptClients();
    i2cBridge.process();
#endif

#ifdef ENABLE_GPIO_BRIDGE
    gpioTcpBridge.acceptClients();
    gpioBridge.process();
#endif
}
