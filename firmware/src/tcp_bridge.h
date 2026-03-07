#pragma once
#include <Arduino.h>

#ifdef ARDUINO_ARCH_ESP32
  #include <WiFi.h>
#else
  #include <ESP8266WiFi.h>
#endif

#include "config.h"

// Maximum number of concurrent TCP clients per bridge
static const int TCP_MAX_CLIENTS = MAX_CLIENTS_PER_BRIDGE;

/// Generic multi-client TCP bridge server.
/// Accepts up to TCP_MAX_CLIENTS simultaneous connections.
/// Received data is broadcast to every active client.
class TcpBridge {
public:
    TcpBridge(uint16_t port, const char* name)
        : _server(port), _port(port), _name(name), _clientCount(0) {}

    void begin() {
        _server.begin();
        DEBUG_SERIAL.printf("[%s] TCP server listening on port %u\n", _name, _port);
    }

    /// Accept any new clients. Call from loop().
    void acceptClients() {
#ifdef ARDUINO_ARCH_ESP32
        WiFiClient newClient = _server.accept();
#else
        WiFiClient newClient = _server.available();
#endif
        if (newClient && newClient.connected()) {
            if (_clientCount < TCP_MAX_CLIENTS) {
                // Find a free slot
                for (int i = 0; i < TCP_MAX_CLIENTS; i++) {
                    if (!_clients[i] || !_clients[i].connected()) {
                        _clients[i] = newClient;
                        _clientCount++;
                        DEBUG_SERIAL.printf("[%s] client %d connected from %s\n",
                            _name, i, newClient.remoteIP().toString().c_str());

                        // Optional token auth
                        if (strlen(AUTH_TOKEN) > 0) {
                            if (!authenticateClient(_clients[i])) {
                                DEBUG_SERIAL.printf("[%s] client %d auth failed\n", _name, i);
                                _clients[i].stop();
                                _clientCount--;
                            }
                        }
                        break;
                    }
                }
            } else {
                DEBUG_SERIAL.printf("[%s] max clients reached, refusing\n", _name);
                newClient.stop();
            }
        }
    }

    /// Broadcast data to all connected clients.
    void broadcast(const uint8_t* data, size_t len) {
        for (int i = 0; i < TCP_MAX_CLIENTS; i++) {
            if (_clients[i] && _clients[i].connected()) {
                _clients[i].write(data, len);
            } else if (_clients[i]) {
                // Cleanup stale connection
                _clients[i].stop();
                _clientCount = max(0, _clientCount - 1);
            }
        }
    }

    /// Read data from any connected client. Returns total bytes collected.
    /// All client-to-server data is placed into buf.
    size_t readFromClients(uint8_t* buf, size_t bufLen) {
        size_t total = 0;
        for (int i = 0; i < TCP_MAX_CLIENTS; i++) {
            if (_clients[i] && _clients[i].connected() && _clients[i].available() > 0) {
                int avail = _clients[i].available();
                int toRead = min((int)(bufLen - total), avail);
                if (toRead > 0) {
                    total += _clients[i].read(buf + total, toRead);
                }
            }
        }
        return total;
    }

    int clientCount() const { return _clientCount; }

private:
    WiFiServer _server;
    uint16_t _port;
    const char* _name;
    WiFiClient _clients[MAX_CLIENTS_PER_BRIDGE];
    int _clientCount;

    bool authenticateClient(WiFiClient& client) {
        // Wait up to 2000ms for the token line
        unsigned long deadline = millis() + 2000;
        String received = "";
        while (millis() < deadline) {
            while (client.available()) {
                char c = client.read();
                if (c == '\n') {
                    received.trim();
                    return received.equals(AUTH_TOKEN);
                }
                received += c;
                if (received.length() > 256) return false;
            }
            delay(10);
        }
        return false;
    }
};
