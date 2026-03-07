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
        _serial.begin(baud);
        DEBUG_SERIAL.printf("[uart] opened at %lu baud\n", baud);
    }

    /// Run one cycle: read UART → broadcast TCP, read TCP → write UART.
    void process() {
        // UART → TCP
        if (_serial.available() > 0) {
            uint8_t buf[256];
            size_t n = 0;
            while (_serial.available() > 0 && n < sizeof(buf)) {
                buf[n++] = _serial.read();
            }
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
