#pragma once
#include <Arduino.h>

#ifdef ARDUINO_ARCH_ESP32
  #include <WiFi.h>
#else
  #include <ESP8266WiFi.h>
#endif

#include "config.h"

class WiFiManager {
public:
    WiFiManager() {}

    /// Block until connected. Blinks STATUS_LED_PIN while trying.
    void begin(const char* ssid, const char* password) {
        _ssid = ssid;
        _password = password;
        connect();
    }

    /// Call from loop(). Reconnection runs in the Wi-Fi stack (auto-reconnect),
    /// so the bridges keep being served; this only reports and nudges it.
    void maintain() {
        bool up = WiFi.status() == WL_CONNECTED;
        if (up != _wasUp) {
            _wasUp = up;
            if (up) DEBUG_SERIAL.printf("[wifi] connected, IP: %s\n", WiFi.localIP().toString().c_str());
            else DEBUG_SERIAL.println("[wifi] link lost, reconnecting in the background");
#if STATUS_LED_PIN != -1
            digitalWrite(STATUS_LED_PIN, up ? HIGH : LOW);
#endif
            _lastAttempt = millis();
        }
        if (!up && millis() - _lastAttempt > 30000) {
            _lastAttempt = millis();
            WiFi.reconnect();
        }
    }

    bool connected() {
        return WiFi.status() == WL_CONNECTED;
    }

    String ipAddress() {
        return WiFi.localIP().toString();
    }

private:
    const char* _ssid;
    const char* _password;
    bool _wasUp = false;
    unsigned long _lastAttempt = 0;

    void connect() {
        DEBUG_SERIAL.printf("[wifi] connecting to %s", _ssid);
        WiFi.mode(WIFI_STA);
#ifdef ARDUINO_ARCH_ESP32
        WiFi.setHostname(HOSTNAME);
#else
        WiFi.hostname(HOSTNAME);  // ESP8266 uses hostname() not setHostname()
#endif
        WiFi.setAutoReconnect(true);
        WiFi.persistent(false);  // do not rewrite flash with credentials on every boot
        WiFi.begin(_ssid, _password);

        int attempts = 0;
        while (WiFi.status() != WL_CONNECTED && attempts < 60) {
            delay(500);
            DEBUG_SERIAL.print(".");
            attempts++;
#if STATUS_LED_PIN != -1
            digitalWrite(STATUS_LED_PIN, !digitalRead(STATUS_LED_PIN));
#endif
        }

        _wasUp = WiFi.status() == WL_CONNECTED;
        if (_wasUp) {
            DEBUG_SERIAL.printf("\n[wifi] connected, IP: %s\n", WiFi.localIP().toString().c_str());
#if STATUS_LED_PIN != -1
            digitalWrite(STATUS_LED_PIN, HIGH);
#endif
        } else {
            DEBUG_SERIAL.println("\n[wifi] connect failed — will retry");
        }
    }
};
