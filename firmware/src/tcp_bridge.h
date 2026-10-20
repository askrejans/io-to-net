#pragma once
#include <Arduino.h>

#ifdef ARDUINO_ARCH_ESP32
  #include <WiFi.h>
#else
  #include <ESP8266WiFi.h>
#endif

#include "config.h"

/// Multi-client TCP server for one bridge. Never blocks the main loop:
/// token checks run incrementally, and the client count is derived from the
/// live connections (no counter that can drift). When every slot is taken a
/// new connection replaces the oldest one, so a tool that reconnects while
/// its previous socket still looks open is not locked out.
class TcpBridge {
public:
    TcpBridge(uint16_t port, const char* name) : _server(port), _port(port), _name(name) {}

    void begin() {
        _server.begin();
        _server.setNoDelay(true);
        DEBUG_SERIAL.printf("[%s] TCP server listening on port %u\n", _name, _port);
    }

    /// Accept new clients and advance pending token checks. Call from loop().
    void acceptClients() {
        WiFiClient c = _server.accept();
        if (c) {
            c.setNoDelay(true);  // request–reply protocols need every byte now
            int slot = freeSlot();
            if (slot < 0) {
                slot = oldestSlot();
                DEBUG_SERIAL.printf("[%s] all %d slots busy, replacing client %d\n", _name, MAX_CLIENTS_PER_BRIDGE, slot);
                _slots[slot].client.stop();
            }
            Slot& s = _slots[slot];
            s.client = c;
            s.since = millis();
            s.authed = strlen(AUTH_TOKEN) == 0;
            s.line = "";
            DEBUG_SERIAL.printf("[%s] client %d connected from %s\n", _name, slot, c.remoteIP().toString().c_str());
        }
        for (int i = 0; i < MAX_CLIENTS_PER_BRIDGE; i++) {
            Slot& s = _slots[i];
            if (s.client && !s.authed) checkToken(i);
        }
    }

    /// Send to every authenticated client.
    void broadcast(const uint8_t* data, size_t len) {
        for (int i = 0; i < MAX_CLIENTS_PER_BRIDGE; i++) {
            Slot& s = _slots[i];
            if (!s.client) continue;
            if (!s.client.connected()) { s.client.stop(); continue; }
            if (s.authed) s.client.write(data, len);
        }
    }

    /// Bytes authenticated clients sent, up to bufLen.
    size_t readFromClients(uint8_t* buf, size_t bufLen) {
        size_t total = 0;
        for (int i = 0; i < MAX_CLIENTS_PER_BRIDGE && total < bufLen; i++) {
            Slot& s = _slots[i];
            if (!s.client || !s.authed) continue;
            int avail = s.client.available();
            if (avail <= 0) continue;
            int n = s.client.read(buf + total, min((int)(bufLen - total), avail));
            if (n > 0) total += n;
        }
        return total;
    }

    int clientCount() {
        int n = 0;
        for (int i = 0; i < MAX_CLIENTS_PER_BRIDGE; i++) {
            if (_slots[i].client && _slots[i].client.connected()) n++;
        }
        return n;
    }

private:
    struct Slot {
        WiFiClient client;
        unsigned long since = 0;
        bool authed = false;
        String line;
    };

    WiFiServer _server;
    uint16_t _port;
    const char* _name;
    Slot _slots[MAX_CLIENTS_PER_BRIDGE];

    int freeSlot() {
        for (int i = 0; i < MAX_CLIENTS_PER_BRIDGE; i++) {
            if (!_slots[i].client || !_slots[i].client.connected()) {
                _slots[i].client.stop();
                return i;
            }
        }
        return -1;
    }

    int oldestSlot() {
        int oldest = 0;
        for (int i = 1; i < MAX_CLIENTS_PER_BRIDGE; i++) {
            if (millis() - _slots[i].since > millis() - _slots[oldest].since) oldest = i;
        }
        return oldest;
    }

    /// The token line, read as far as it has arrived; 5 s to finish it.
    void checkToken(int i) {
        Slot& s = _slots[i];
        bool rejected = false;
        while (s.client.available()) {
            char c = s.client.read();
            if (c == '\n') {
                s.line.trim();
                if (s.line.equals(AUTH_TOKEN)) {
                    s.authed = true;
                    s.line = "";
                    return;  // what follows the line is device data
                }
                rejected = true;
                break;
            }
            s.line += c;
            if (s.line.length() > 256) { rejected = true; break; }
        }
        if (rejected || millis() - s.since > 5000) {
            DEBUG_SERIAL.printf("[%s] client %d failed authentication\n", _name, i);
            s.client.print("auth failed\n");
            s.client.stop();
        }
    }
};
