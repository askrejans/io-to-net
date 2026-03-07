#pragma once
#include <Arduino.h>
#include <Wire.h>
#include "config.h"
#include "tcp_bridge.h"

/// I2C polling bridge.
/// Periodically reads from an I2C slave and broadcasts the data over TCP.
class I2cBridge {
public:
    I2cBridge(TcpBridge& tcp)
        : _tcp(tcp), _lastPoll(0) {}

    void begin(int sda, int scl, uint8_t addr, size_t readLen) {
        _addr = addr;
        _readLen = min(readLen, sizeof(_buf));
        Wire.begin(sda, scl);
        DEBUG_SERIAL.printf("[i2c] opened bus SDA=%d SCL=%d addr=0x%02X\n", sda, scl, addr);
    }

    void process() {
        unsigned long now = millis();
        if (now - _lastPoll < I2C_POLL_MS) return;
        _lastPoll = now;

        Wire.requestFrom(_addr, (uint8_t)_readLen);
        size_t n = 0;
        while (Wire.available() && n < _readLen) {
            _buf[n++] = Wire.read();
        }
        if (n > 0) {
            _tcp.broadcast(_buf, n);
        }
    }

private:
    TcpBridge& _tcp;
    uint8_t _addr = I2C_TARGET_ADDR;
    size_t _readLen = I2C_READ_LEN;
    uint8_t _buf[32];
    unsigned long _lastPoll;
};
