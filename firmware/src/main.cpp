// AI-Column — прошивка умной колонки на ESP32-S3.
// Колонка слушает (кнопка/тач), стримит звук на сервер, показывает ответ на экране
// и проигрывает речь, которую сервер присылает потоком. Вся «умность» — на сервере.
#include <Arduino.h>
#include <ArduinoJson.h>
#include <Preferences.h>
#include <esp_sleep.h>

#include <vector>

#include "api.h"
#include "audio.h"
#include "battery.h"
#include "buttons.h"
#include "config.h"
#include "display.h"
#include "dsp.h"
#include "leds.h"

namespace {

enum class State : uint8_t { Connecting, Idle, Listening, Thinking, Speaking, Sleep };

constexpr uint32_t kColorAuto = 0x5B8CFF;
constexpr uint32_t kColorWarn = 0xF5A524;
constexpr uint32_t kColorError = 0xE5484D;
constexpr uint32_t kColorOk = 0x37C871;
constexpr size_t kTxSamples = MIC_SAMPLE_RATE * MIC_FRAME_MS / 1000;

struct AgentInfo {
  String id;
  String name;
  uint32_t color;
  bool available;
};

State state = State::Connecting;
Button btnTalk(PIN_BTN_TALK, 60000);  // «долгое» событие не нужно: удержание ловим через heldMs()
Button btnMenu(PIN_BTN_MENU, 900);
Preferences prefs;

std::vector<AgentInfo> agents;
String selectedAgent = "auto";
uint32_t maxUtteranceMs = MAX_UTTERANCE_MS;

bool holdDecided = false;
bool holdMode = false;
uint32_t listenStartedAt = 0;
dsp::Vad vad;
uint32_t vadSeenFrames = 0;
int16_t txBuf[kTxSamples];

bool acceptAudio = false;
bool audioEnded = false;
uint32_t thinkingSince = 0;

uint32_t lastStatusSent = 0;
uint32_t lastActivity = 0;
uint32_t volumeChangedAt = 0;
bool volumeDirty = false;
uint8_t backlight = 0;
uint32_t lastConnUi = 0;
uint32_t lastIndicators = 0;

// ─────────────────────────────────────────────────────────────── вспомогательное

uint32_t colorOf(const String& id) {
  if (id == "auto") return kColorAuto;
  for (auto& a : agents)
    if (a.id == id) return a.color;
  return kColorAuto;
}

void sendType(const char* type) {
  JsonDocument d;
  d["type"] = type;
  net::sendJson(d);
}

void setState(State s, const String& hint = "") {
  state = s;
  lastActivity = millis();
  switch (s) {
    case State::Connecting:
      ui::setStatus(net::wifiConnected() ? ui::Status::Offline : ui::Status::Connecting, hint);
      leds::setMode(leds::Mode::Offline);
      break;
    case State::Idle:
      ui::setStatus(ui::Status::Idle, hint);
      leds::setMode(leds::Mode::Idle);
      leds::setAccent(colorOf(selectedAgent));
      break;
    case State::Listening:
      ui::setStatus(ui::Status::Listening, hint);
      leds::setMode(leds::Mode::Listening);
      leds::setAccent(colorOf(selectedAgent));
      break;
    case State::Thinking:
      ui::setStatus(ui::Status::Thinking, hint);
      leds::setMode(leds::Mode::Thinking);
      break;
    case State::Speaking:
      ui::setStatus(ui::Status::Speaking, hint);
      leds::setMode(leds::Mode::Speaking);
      break;
    case State::Sleep:
      ui::setStatus(ui::Status::Sleep, hint);
      leds::setMode(leds::Mode::Sleep);
      break;
  }
}

void pushAgentsToUi() {
  std::vector<ui::AgentChip> chips;
  for (auto& a : agents) chips.push_back({a.id, a.name, a.color, a.available});
  ui::setAgents(chips);
}

void selectAgent(const String& id, bool notifyServer) {
  selectedAgent = id;
  ui::setSelectedAgent(id);
  leds::setAccent(colorOf(id));
  prefs.putString("agent", id);
  if (notifyServer) {
    JsonDocument d;
    d["type"] = "set_agent";
    d["agent"] = id;
    net::sendJson(d);
  }
}

void nextAgent() {
  std::vector<String> order{"auto"};
  for (auto& a : agents) order.push_back(a.id);
  size_t i = 0;
  while (i < order.size() && order[i] != selectedAgent) i++;
  String next = order[(i + 1) % order.size()];
  selectAgent(next, true);
  String name = "автовыбор";
  for (auto& a : agents)
    if (a.id == next) name = a.name + (a.available ? "" : " (недоступен)");
  ui::toast("Отвечает: " + name, colorOf(next) == kColorAuto ? kColorWarn : colorOf(next), 1500);
}

void setVolume(int v, bool notifyServer) {
  v = v < 0 ? 0 : (v > 100 ? 100 : v);
  audio::setVolume(v);
  ui::setVolume(v);
  volumeDirty = true;
  volumeChangedAt = millis();
  if (notifyServer) {
    JsonDocument d;
    d["type"] = "set_volume";
    d["value"] = v;
    net::sendJson(d);
  }
}

// ─────────────────────────────────────────────────────────────── слушаем

void startListening(bool holdKnown) {
  if (!net::serverConnected()) {
    ui::toast("Нет связи с сервером", kColorWarn);
    leds::flashAlert(kColorWarn, 600);
    return;
  }
  if (state == State::Speaking || acceptAudio) {
    audio::playbackStop();  // перебили ответ
    acceptAudio = false;
  }
  JsonDocument d;
  d["type"] = "listen_start";
  d["agent"] = selectedAgent;
  net::sendJson(d);
  audio::startCapture();
  vad.endSilenceMs = VAD_END_SILENCE_MS;
  vad.noSpeechMs = VAD_NO_SPEECH_MS;
  vad.minSpeechMs = VAD_MIN_SPEECH_MS;
  vad.reset(audio::ambientLevel());
  vadSeenFrames = audio::micFrameCounter();
  holdDecided = holdKnown;
  holdMode = false;
  listenStartedAt = millis();
  ui::clearConversation();
  setState(State::Listening, holdKnown ? "замолчи — и я отвечу" : "");
}

void pumpMic() {
  while (audio::capturedAvailable() >= kTxSamples) {
    size_t n = audio::readCaptured(txBuf, kTxSamples);
    net::sendAudio(txBuf, n);
  }
}

void finishListening(bool sendStop) {
  audio::stopCapture();
  size_t n;
  while ((n = audio::readCaptured(txBuf, kTxSamples)) > 0) net::sendAudio(txBuf, n);
  if (sendStop) sendType("listen_stop");
  thinkingSince = millis();
  ui::setAnswering("", colorOf(selectedAgent));
  setState(State::Thinking);
}

void cancelListening(const String& why) {
  audio::stopCapture();
  sendType("cancel");
  setState(State::Idle);
  ui::toast(why, kColorWarn);
}

void tickListening() {
  pumpMic();
  uint32_t now = millis();
  if (!holdDecided && btnTalk.isDown() && btnTalk.heldMs() >= HOLD_THRESHOLD_MS) {
    holdDecided = true;
    holdMode = true;
    ui::setStatus(ui::Status::Listening, "отпусти кнопку, когда закончишь");
  }
  // Детектор речи: по новым блокам микрофона (~16 мс каждый)
  uint32_t frames = audio::micFrameCounter();
  if (frames != vadSeenFrames) {
    uint32_t ms = (frames - vadSeenFrames) * 16;
    vadSeenFrames = frames;
    dsp::Vad::Event ev = vad.process(audio::lastBlockRms(), ms);
    if (!holdMode) {
      if (ev == dsp::Vad::kSpeechEnd) {
        finishListening(true);
        return;
      }
      if (ev == dsp::Vad::kNoSpeech) {
        cancelListening("Не слышу тебя");
        return;
      }
    }
  }
  if (now - listenStartedAt > maxUtteranceMs) finishListening(true);
}

// ─────────────────────────────────────────────────────────────── сервер

void onServerMessage(JsonDocument& doc) {
  const char* type = doc["type"] | "";
  if (!strcmp(type, "welcome") || !strcmp(type, "agents")) {
    agents.clear();
    for (JsonObject a : doc["agents"].as<JsonArray>()) {
      agents.push_back({String(a["id"] | ""), String(a["name"] | ""), ui::parseColor(a["color"] | "", kColorAuto), a["available"] | true});
    }
    pushAgentsToUi();
    if (!strcmp(type, "welcome")) {
      String serverAgent = doc["agent"] | "auto";
      // Выбор с колонки главнее: если он есть в списке — сообщим серверу.
      bool known = selectedAgent == "auto";
      for (auto& a : agents) known |= a.id == selectedAgent;
      if (known && selectedAgent != serverAgent) selectAgent(selectedAgent, true);
      else selectAgent(serverAgent, false);
      if (doc["volume"].is<int>()) setVolume(doc["volume"].as<int>(), false);
      maxUtteranceMs = (doc["max_utterance_seconds"] | (MAX_UTTERANCE_MS / 1000)) * 1000;
      String name = doc["assistant"] | "AI-Column";
      setState(State::Idle, "Я " + name + ". SW1 — говорить, SW2 — сменить агента");
    }
  } else if (!strcmp(type, "agent_selected")) {
    selectAgent(doc["agent"] | "auto", false);
  } else if (!strcmp(type, "state")) {
    const char* s = doc["state"] | "";
    if (!strcmp(s, "idle") && state == State::Thinking) setState(State::Idle);
  } else if (!strcmp(type, "stop_listening")) {
    if (state == State::Listening) finishListening(false);
  } else if (!strcmp(type, "transcript")) {
    String t = doc["text"] | "";
    ui::setUserText(t.length() ? t : String("…"));
  } else if (!strcmp(type, "agent")) {
    uint32_t c = ui::parseColor(doc["color"] | "", kColorAuto);
    String reason = doc["reason"] | "";
    ui::setAnswering(doc["name"] | "", c, reason == "fallback" ? "запасной агент" : "");
    leds::setAccent(c);
  } else if (!strcmp(type, "delegate")) {
    ui::toast(String("Спрашиваю ") + (doc["name"] | "другого агента") + "…", ui::parseColor(doc["color"] | "", kColorAuto), 2500);
  } else if (!strcmp(type, "reply_delta")) {
    ui::appendReply(doc["text"] | "");
  } else if (!strcmp(type, "audio_start")) {
    audio::playbackBegin(doc["rate"] | PLAY_SAMPLE_RATE);
    acceptAudio = true;
    audioEnded = false;
    if (state != State::Listening) setState(State::Speaking);
  } else if (!strcmp(type, "audio_end")) {
    acceptAudio = false;
    audioEnded = true;
    audio::playbackEnd();
  } else if (!strcmp(type, "set_volume")) {
    setVolume(doc["value"] | (int)audio::volume(), false);
    ui::toast("Громкость " + String((int)audio::volume()) + "%", kColorOk, 1200);
  } else if (!strcmp(type, "led")) {
    uint32_t c = ui::parseColor(doc["color"] | "", 0);
    leds::setOverride(doc["mode"] | "auto", c, doc["brightness"] | 0);
  } else if (!strcmp(type, "alert")) {
    leds::flashAlert(0xFF7000, 3000);
    ui::toast(doc["text"] | "Напоминание", kColorWarn, 6000);
  } else if (!strcmp(type, "error")) {
    ui::toast(doc["message"] | "Ошибка", kColorError, 3500);
    leds::flashAlert(kColorError, 500);
  }
}

void onServerConnected() {
  JsonDocument d;
  d["type"] = "hello";
  d["fw"] = FW_VERSION;
  d["mic_rate"] = MIC_SAMPLE_RATE;
  d["play_rate"] = PLAY_SAMPLE_RATE;
  d["volume"] = audio::volume();
  int bat = battery::percent();
  if (bat >= 0) d["battery"] = bat;
  net::sendJson(d);
  ui::setServer(true);
}

void onServerDisconnected() {
  ui::setServer(false);
  if (state == State::Listening) audio::stopCapture();
  acceptAudio = false;
  setState(State::Connecting, "Сервер " + net::serverAddress() + " недоступен");
}

void sendStatus() {
  JsonDocument d;
  d["type"] = "status";
  int bat = battery::percent();
  if (bat >= 0) d["battery"] = bat;
  d["rssi"] = net::rssi();
  d["volume"] = audio::volume();
  net::sendJson(d);
}

void goToSleep() {
  setState(State::Sleep);
  for (int i = 0; i < 40; i++) {
    ui::loop();
    leds::loop(0, 0);
    delay(50);
  }
  ui::setBacklight(0);
  audio::playbackStop();
  esp_deep_sleep_start();  // проснётся только по сбросу/включению — выключи S1 и заряди АКБ
}

}  // namespace

// ─────────────────────────────────────────────────────────────── setup / loop

void setup() {
  Serial.begin(115200);
  prefs.begin("column", false);
  uint8_t vol = prefs.getUChar("vol", VOLUME_DEFAULT);
  selectedAgent = prefs.getString("agent", "auto");

  btnTalk.begin();
  btnMenu.begin();
  bool recalibrate = btnMenu.isDown();  // SW2 зажата при включении — калибровка тача

  leds::begin();
  leds::setMode(leds::Mode::Boot);
  ui::begin(recalibrate);
  ui::bootMessage("Запускаю звук…");
  if (!audio::begin()) ui::bootMessage("Ошибка I2S — проверь пайку");
  audio::setVolume(vol);
  battery::begin();

  ui::bootMessage("Подключаюсь к Wi-Fi…");
  net::Handlers h;
  h.onConnected = onServerConnected;
  h.onDisconnected = onServerDisconnected;
  h.onMessage = onServerMessage;
  h.onAudio = [](const uint8_t* data, size_t len) {
    if (acceptAudio) audio::playbackWrite(data, len);
  };
  net::begin(h);

  ui::setVolume(vol);
  ui::setSelectedAgent(selectedAgent);
  setState(State::Connecting, "Подключаюсь к Wi-Fi…");
  log_i("AI-Column %s, PSRAM %u КБ, id %s", FW_VERSION, (unsigned)(ESP.getPsramSize() / 1024), net::deviceId().c_str());
}

void loop() {
  net::loop();
  uint32_t now = millis();

  // ── кнопки ──
  Button::Event talk = btnTalk.poll();
  if (talk == Button::kPressed) {
    if (state == State::Listening) {
      if (holdDecided && !holdMode) finishListening(true);  // второе нажатие в режиме «нажал-отпустил»
    } else if (state == State::Idle || state == State::Speaking || state == State::Thinking) {
      startListening(false);
    } else {
      ui::toast("Нет связи с сервером", kColorWarn);
    }
  } else if (talk == Button::kReleased) {
    if (state == State::Listening) {
      if (!holdDecided) {
        holdDecided = true;  // короткое нажатие — дальше сам определю конец фразы
        holdMode = false;
        ui::setStatus(ui::Status::Listening, "говори — я пойму, когда закончишь");
      } else if (holdMode) {
        finishListening(true);
      }
    }
  }

  Button::Event menu = btnMenu.poll();
  if (menu == Button::kReleased && state != State::Connecting) {
    nextAgent();
  } else if (menu == Button::kLongPress && net::serverConnected()) {
    sendType("new_conversation");
    ui::clearConversation();
    ui::toast("Новый разговор", kColorOk, 1500);
  }

  // ── тачскрин ──
  ui::Touch t = ui::loop();
  if (t.ev != ui::TouchEvent::None) lastActivity = now;
  switch (t.ev) {
    case ui::TouchEvent::Talk:
      if (state == State::Listening) finishListening(true);
      else if (state == State::Idle || state == State::Speaking || state == State::Thinking) startListening(true);
      break;
    case ui::TouchEvent::SelectAgent:
      selectAgent(t.agentId, true);
      break;
    case ui::TouchEvent::VolumeUp:
      setVolume(audio::volume() + 10, true);
      break;
    case ui::TouchEvent::VolumeDown:
      setVolume((int)audio::volume() - 10, true);
      break;
    default:
      break;
  }

  // ── состояния ──
  switch (state) {
    case State::Connecting:
      if (now - lastConnUi > 500) {
        lastConnUi = now;
        ui::setStatus(net::wifiConnected() ? ui::Status::Offline : ui::Status::Connecting,
                    net::wifiConnected() ? (net::serverAddress().length() ? String("Сервер " + net::serverAddress() + " не отвечает") : String("Ищу сервер в сети…"))
                                         : String("Подключаюсь к Wi-Fi…"));
      }
      break;
    case State::Listening:
      tickListening();
      break;
    case State::Thinking:
      if (now - thinkingSince > 60000) {
        ui::toast("Сервер не ответил", kColorError);
        sendType("cancel");
        setState(State::Idle);
      }
      break;
    case State::Speaking:
      if (audioEnded && !audio::playing()) {
        audioEnded = false;
        setState(State::Idle);
      }
      break;
    default:
      break;
  }

  // ── батарея, индикаторы, подсветка ──
  battery::loop();
  if (now - lastIndicators > 1000) {
    lastIndicators = now;
    ui::setBattery(battery::percent(), battery::low());
    ui::setWifi(net::wifiConnected(), net::rssi());
  }
  ui::setServer(net::serverConnected());
  ui::setLevels(audio::micLevel(), audio::outLevel());
  leds::loop(audio::micLevel(), audio::outLevel());
  if (battery::critical()) goToSleep();

  uint8_t wantLight = (state == State::Idle && now - lastActivity > 60000) ? 40 : 200;
  if (wantLight != backlight) {
    backlight = wantLight;
    ui::setBacklight(backlight);
  }
  if (net::serverConnected() && now - lastStatusSent > STATUS_INTERVAL_MS) {
    lastStatusSent = now;
    sendStatus();
  }
  if (volumeDirty && now - volumeChangedAt > 3000) {
    volumeDirty = false;
    prefs.putUChar("vol", audio::volume());
  }
  delay(1);
}
