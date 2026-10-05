#include "api.h"

#include <ESPmDNS.h>
#include <WebSocketsClient.h>
#include <WiFi.h>

#include "config.h"
#if __has_include("secrets.h")
#include "secrets.h"
#else
#warning "Нет include/secrets.h — используется secrets.example.h (скопируй и заполни!)"
#include "secrets.example.h"
#endif

namespace net {
namespace {

WebSocketsClient ws;
Handlers handlers_;
String deviceId_;
String host_ = SERVER_HOST;
uint16_t port_ = SERVER_PORT;
bool wsStarted_ = false;
bool wsConnected_ = false;
bool mdnsStarted_ = false;
uint32_t lastDiscovery_ = 0;
uint32_t lastWifiLog_ = 0;
uint32_t lostSince_ = 0;

void onEvent(WStype_t type, uint8_t* payload, size_t length) {
  switch (type) {
    case WStype_CONNECTED:
      wsConnected_ = true;
      lostSince_ = 0;
      log_i("WebSocket подключён к %s:%u", host_.c_str(), port_);
      if (handlers_.onConnected) handlers_.onConnected();
      break;
    case WStype_DISCONNECTED:
      if (!lostSince_) lostSince_ = millis();
      if (wsConnected_) {
        wsConnected_ = false;
        log_w("WebSocket отключён");
        if (handlers_.onDisconnected) handlers_.onDisconnected();
      }
      break;
    case WStype_TEXT: {
      JsonDocument doc;
      DeserializationError err = deserializeJson(doc, payload, length);
      if (err) {
        log_w("плохой JSON от сервера: %s", err.c_str());
        break;
      }
      if (handlers_.onMessage) handlers_.onMessage(doc);
      break;
    }
    case WStype_BIN:
      if (handlers_.onAudio) handlers_.onAudio(payload, length);
      break;
    case WStype_ERROR:
      log_w("ошибка WebSocket");
      break;
    default:
      break;
  }
}

// Поиск сервера по mDNS (_aicolumn._tcp). Блокирует на ~1–3 с, поэтому не чаще раза в 10 с.
bool discover() {
  if (!mdnsStarted_) {
    String host = "ai-column-" + deviceId_.substring(deviceId_.length() - 6);
    mdnsStarted_ = MDNS.begin(host.c_str());
  }
  int n = MDNS.queryService("aicolumn", "tcp");
  if (n <= 0) {
    log_i("сервер в сети не найден (mDNS), повторю позже");
    return false;
  }
  host_ = MDNS.IP(0).toString();
  port_ = MDNS.port(0);
  log_i("нашёл сервер %s:%u (%s)", host_.c_str(), port_, MDNS.hostname(0).c_str());
  return true;
}

void startWebSocket() {
  String url = "/ws?id=" + deviceId_;
  if (strlen(DEVICE_TOKEN)) url += String("&token=") + DEVICE_TOKEN;
#if SERVER_USE_TLS
  ws.beginSSL(host_.c_str(), port_, url.c_str());
#else
  ws.begin(host_.c_str(), port_, url.c_str());
#endif
  ws.onEvent(onEvent);
  ws.setReconnectInterval(WS_RECONNECT_MS);
  ws.enableHeartbeat(15000, 4000, 2);
  wsStarted_ = true;
}

}  // namespace

void begin(const Handlers& h) {
  handlers_ = h;
  uint8_t mac[6];
  WiFi.macAddress(mac);
  char id[24];
  snprintf(id, sizeof(id), "column-%02x%02x%02x", mac[3], mac[4], mac[5]);
  deviceId_ = id;
  WiFi.mode(WIFI_STA);
  WiFi.setHostname(("ai-column-" + deviceId_.substring(7)).c_str());
  WiFi.setSleep(false);  // без энергосбережения Wi-Fi звук идёт ровнее
  WiFi.setAutoReconnect(true);
  WiFi.begin(WIFI_SSID, WIFI_PASSWORD);
  log_i("устройство %s, подключаюсь к Wi-Fi «%s»", deviceId_.c_str(), WIFI_SSID);
}

void loop() {
  if (WiFi.status() != WL_CONNECTED) {
    if (millis() - lastWifiLog_ > 10000) {
      lastWifiLog_ = millis();
      log_i("жду Wi-Fi…");
    }
    if (wsStarted_) ws.loop();
    return;
  }
  if (!wsStarted_) {
    if (host_.length() == 0) {
      if (millis() - lastDiscovery_ < 10000 && lastDiscovery_ != 0) return;
      lastDiscovery_ = millis();
      if (!discover()) return;
    }
    startWebSocket();
  }
  ws.loop();
  // Сервер найден через mDNS, но долго недоступен — возможно, сменил адрес. Ищем заново.
  if (strlen(SERVER_HOST) == 0 && !wsConnected_ && lostSince_ && millis() - lostSince_ > 30000) {
    log_i("сервер недоступен 30 с — ищу его в сети заново");
    ws.disconnect();
    wsStarted_ = false;
    host_ = "";
    lostSince_ = 0;
    lastDiscovery_ = 0;
  }
}

bool wifiConnected() { return WiFi.status() == WL_CONNECTED; }
int rssi() { return WiFi.RSSI(); }
bool serverConnected() { return wsConnected_; }
const String& deviceId() { return deviceId_; }
String serverAddress() { return host_.length() ? host_ + ":" + String(port_) : String(); }

bool sendJson(const JsonDocument& doc) {
  if (!wsConnected_) return false;
  String out;
  serializeJson(doc, out);
  return ws.sendTXT(out);
}

bool sendAudio(const int16_t* pcm, size_t samples) {
  if (!wsConnected_) return false;
  return ws.sendBIN(reinterpret_cast<const uint8_t*>(pcm), samples * 2);
}

}  // namespace net
