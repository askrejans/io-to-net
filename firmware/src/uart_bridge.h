#pragma once
#include <Arduino.h>
#include "config.h"
#include "tcp_bridge.h"

/// UART ↔ TCP bridge.
/// Reads from a hardware serial port and broadcasts over TCP.
/// Receives TCP data and writes it to the serial port.
class UartBridge {
public:
    UartBridge(HardwareSerial& serial, TcpBridge& tcp)
        : _serial(serial), _tcp(tcp) {}

    void begin(unsigned long baud) {
#ifdef ARDUINO_ARCH_ESP32
        _serial.setRxBufferSize(UART_RX_BUFFER);
        _serial.begin(baud, SERIAL_8N1, UART_RX_PIN, UART_TX_PIN);
#else
        _serial.begin(baud);
#endif
        DEBUG_SERIAL.printf("[uart] opened at %lu baud\n", baud);
    }

    /// Run one cycle: read UART → broadcast TCP, read TCP → write UART.
    void process() {
        // UART → TCP
        // One block read and one TCP write per cycle, not a write per byte.
        int avail = _serial.available();
        if (avail > 0) {
            uint8_t buf[512];
            size_t n = _serial.readBytes(buf, min((size_t)avail, sizeof(buf)));
            if (n > 0) {
                _tcp.broadcast(buf, n);
            }
        }

        // TCP → UART (clients sending to device)
        uint8_t rxBuf[256];
        size_t rxLen = _tcp.readFromClients(rxBuf, sizeof(rxBuf));
        if (rxLen > 0) {
            _serial.write(rxBuf, rxLen);
        }
    }

private:
    HardwareSerial& _serial;
    TcpBridge& _tcp;
};
