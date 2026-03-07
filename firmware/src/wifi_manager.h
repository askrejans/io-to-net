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

    /// Call from loop() — reconnects automatically if link drops.
    void maintain() {
        if (WiFi.status() != WL_CONNECTED) {
            DEBUG_SERIAL.println("[wifi] disconnected — reconnecting");
            connect();
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

    void connect() {
        DEBUG_SERIAL.printf("[wifi] connecting to %s", _ssid);
        WiFi.mode(WIFI_STA);
#ifdef ARDUINO_ARCH_ESP32
        WiFi.setHostname(HOSTNAME);
#else
        WiFi.hostname(HOSTNAME);  // ESP8266 uses hostname() not setHostname()
#endif
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

        if (WiFi.status() == WL_CONNECTED) {
            DEBUG_SERIAL.printf("\n[wifi] connected, IP: %s\n", WiFi.localIP().toString().c_str());
#if STATUS_LED_PIN != -1
            digitalWrite(STATUS_LED_PIN, HIGH);
#endif
        } else {
            DEBUG_SERIAL.println("\n[wifi] connect failed — will retry");
        }
    }
};
