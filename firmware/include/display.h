// Экран ILI9341 320×240 + тач XPT2046 (LovyanGFX). Интерфейс колонки.
#pragma once
#include <Arduino.h>

#include <vector>

namespace ui {

struct AgentChip {
  String id;
  String name;
  uint32_t color;  // 0xRRGGBB
  bool available;
};

enum class Status : uint8_t { Boot, Connecting, Offline, Idle, Listening, Thinking, Speaking, Sleep };

enum class TouchEvent : uint8_t { None, Talk, SelectAgent, VolumeUp, VolumeDown };
struct Touch {
  TouchEvent ev = TouchEvent::None;
  String agentId;
};

// forceCalibrate — заново откалибровать тачскрин (держать SW2 при включении)
void begin(bool forceCalibrate);
void bootMessage(const String& line);

void setStatus(Status s, const String& hint = "");
void setWifi(bool connected, int rssi);
void setServer(bool connected);
void setBattery(int percent, bool low);  // -1 — АКБ нет
void setVolume(uint8_t v);
void setAgents(const std::vector<AgentChip>& list);
void setSelectedAgent(const String& id);  // "auto" или id агента
void setAnswering(const String& name, uint32_t color, const String& note = "");
void setUserText(const String& text);
void appendReply(const String& text);
void clearConversation();
void toast(const String& text, uint32_t color, uint32_t ms = 2500);
void setLevels(float mic, float out);
void setBacklight(uint8_t level);  // 0..255

Touch loop();  // перерисовка + обработка касаний; вызывать часто

uint32_t parseColor(const char* hex, uint32_t fallback);

}  // namespace ui
