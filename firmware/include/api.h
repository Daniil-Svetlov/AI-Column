// Связь с сервером AI-Column: Wi-Fi, поиск сервера (mDNS), WebSocket-протокол (docs/API.md).
#pragma once
#include <Arduino.h>
#include <ArduinoJson.h>

#include <functional>

namespace net {

struct Handlers {
  std::function<void()> onConnected;                     // WebSocket открыт
  std::function<void()> onDisconnected;                  // связь с сервером потеряна
  std::function<void(JsonDocument&)> onMessage;          // JSON-событие от сервера
  std::function<void(const uint8_t*, size_t)> onAudio;   // бинарный кадр PCM16
};

void begin(const Handlers& h);
void loop();

bool wifiConnected();
int rssi();
bool serverConnected();
const String& deviceId();
String serverAddress();  // "ip:port" или пусто, пока не найден

bool sendJson(const JsonDocument& doc);
bool sendAudio(const int16_t* pcm, size_t samples);

}  // namespace net
