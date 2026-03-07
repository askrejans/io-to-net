#pragma once
#include <Arduino.h>
#include "config.h"
#include "tcp_bridge.h"

/// GPIO edge-event bridge.
/// Monitors a GPIO pin for rising/falling edges and streams 1-byte event
/// packets over TCP: 0x01 = rising, 0x02 = falling.
class GpioBridge {
public:
    GpioBridge(TcpBridge& tcp, uint8_t pin, uint8_t mode)
        : _tcp(tcp), _pin(pin), _mode(mode), _lastState(LOW) {}

    void begin() {
        pinMode(_pin, _mode);
        _lastState = digitalRead(_pin);
        DEBUG_SERIAL.printf("[gpio] monitoring pin %d\n", _pin);
    }

    void process() {
        int state = digitalRead(_pin);
        if (state != _lastState) {
            uint8_t event = (state == HIGH) ? 0x01 : 0x02;
            _tcp.broadcast(&event, 1);
            _lastState = state;
        }
    }

private:
    TcpBridge& _tcp;
    uint8_t _pin;
    uint8_t _mode;
    int _lastState;
};
