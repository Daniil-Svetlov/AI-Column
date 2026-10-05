#include "display.h"

#define LGFX_USE_V1
#include <LovyanGFX.hpp>
#include <Preferences.h>

#include "config.h"

// ── Описание железа для LovyanGFX: ILI9341 + XPT2046 на общей шине SPI2 ──
class LGFX : public lgfx::LGFX_Device {
  lgfx::Panel_ILI9341 panel_;
  lgfx::Bus_SPI bus_;
  lgfx::Light_PWM light_;
  lgfx::Touch_XPT2046 touch_;

 public:
  LGFX() {
    {
      auto cfg = bus_.config();
      cfg.spi_host = SPI2_HOST;
      cfg.spi_mode = 0;
      cfg.freq_write = 40000000;
      cfg.freq_read = 16000000;
      cfg.spi_3wire = false;
      cfg.use_lock = true;
      cfg.dma_channel = SPI_DMA_CH_AUTO;
      cfg.pin_sclk = PIN_TFT_SCK;
      cfg.pin_mosi = PIN_TFT_MOSI;
      cfg.pin_miso = PIN_TFT_MISO;
      cfg.pin_dc = PIN_TFT_DC;
      bus_.config(cfg);
      panel_.setBus(&bus_);
    }
    {
      auto cfg = panel_.config();
      cfg.pin_cs = PIN_TFT_CS;
      cfg.pin_rst = PIN_TFT_RST;
      cfg.pin_busy = -1;
      cfg.panel_width = 240;
      cfg.panel_height = 320;
      cfg.offset_x = 0;
      cfg.offset_y = 0;
      cfg.offset_rotation = 0;
      cfg.readable = false;  // SDO дисплея не подключён
      cfg.invert = false;
      cfg.rgb_order = false;
      cfg.dlen_16bit = false;
      cfg.bus_shared = true;  // шину делит тачскрин
      panel_.config(cfg);
    }
    {
      auto cfg = light_.config();
      cfg.pin_bl = PIN_TFT_LED;
      cfg.invert = false;
      cfg.freq = 44100;
      cfg.pwm_channel = 7;
      light_.config(cfg);
      panel_.setLight(&light_);
    }
    {
      auto cfg = touch_.config();
      cfg.x_min = 200;
      cfg.x_max = 3900;
      cfg.y_min = 200;
      cfg.y_max = 3900;
      cfg.pin_int = PIN_TOUCH_IRQ;
      cfg.bus_shared = true;
      cfg.offset_rotation = 0;
      cfg.spi_host = SPI2_HOST;
      cfg.freq = 1000000;
      cfg.pin_sclk = PIN_TFT_SCK;
      cfg.pin_mosi = PIN_TFT_MOSI;
      cfg.pin_miso = PIN_TFT_MISO;
      cfg.pin_cs = PIN_TOUCH_CS;
      touch_.config(cfg);
      panel_.setTouch(&touch_);
    }
    setPanel(&panel_);
  }
};

namespace ui {
namespace {

LGFX lcd;
LGFX_Sprite canvas(&lcd);

constexpr int W = 320, H = 240, TOP_H = 26, BOT_H = 40;
constexpr int MID_Y = TOP_H, MID_H = H - TOP_H - BOT_H;

// ВАЖНО: цвета — именно uint32_t (RGB888). Обычный int LovyanGFX считает RGB565.
constexpr uint32_t COL_BG = 0x0E1116;
constexpr uint32_t COL_PANEL = 0x161B22;
constexpr uint32_t COL_TEXT = 0xECEFF4;
constexpr uint32_t COL_DIM = 0x8A93A3;
constexpr uint32_t COL_FAINT = 0x2C3440;
constexpr uint32_t COL_OK = 0x37C871;
constexpr uint32_t COL_BAD = 0xE5484D;
constexpr uint32_t COL_WARN = 0xF5A524;
constexpr uint32_t COL_USER = 0x9AA4B5;
constexpr uint32_t COL_AUTO = 0x5B8CFF;
constexpr uint32_t COL_BLACK = 0x000000;
constexpr uint32_t COL_WHITE = 0xFFFFFF;

const lgfx::IFont* F_SMALL = &fonts::efontJA_12;
const lgfx::IFont* F_TEXT = &fonts::efontJA_16;
const lgfx::IFont* F_BIG = &fonts::efontJA_24;

struct Rect {
  int x, y, w, h;
  bool hit(int px, int py) const { return px >= x && px < x + w && py >= y && py < y + h; }
};

Status status_ = Status::Boot;
String hint_;
bool wifi_ = false, server_ = false, batLow_ = false;
int rssi_ = 0, bat_ = -1;
uint8_t vol_ = VOLUME_DEFAULT;
std::vector<AgentChip> agents_;
String selected_ = "auto";
String answerName_, answerNote_;
uint32_t answerColor_ = COL_AUTO;
String userText_, replyText_;
std::vector<String> userLines_, replyLines_;
bool wrapDirty_ = true;
String toast_;
uint32_t toastColor_ = COL_WARN, toastUntil_ = 0;
float mic_ = 0, out_ = 0;
bool topDirty_ = true, botDirty_ = true, midDirty_ = true;
uint32_t lastMidDraw_ = 0;
std::vector<std::pair<Rect, String>> chipRects_;
const Rect volDownRect_{W - 78, H - BOT_H + 5, 36, BOT_H - 10};
const Rect volUpRect_{W - 39, H - BOT_H + 5, 36, BOT_H - 10};
bool touching_ = false;
int32_t touchX_ = 0, touchY_ = 0;
bool spriteOk_ = false;

// ── UTF-8 и перенос строк ──
int utf8Len(uint8_t c) {
  if (c < 0x80) return 1;
  if ((c >> 5) == 0x6) return 2;
  if ((c >> 4) == 0xE) return 3;
  if ((c >> 3) == 0x1E) return 4;
  return 1;
}

template <class G>
int fitBytes(G& g, const String& s, int maxW) {
  int i = 0, best = 0;
  while (i < (int)s.length()) {
    int n = utf8Len((uint8_t)s[i]);
    if (g.textWidth(s.substring(0, i + n)) > maxW) break;
    i += n;
    best = i;
  }
  return best > 0 ? best : utf8Len((uint8_t)s[0]);
}

template <class G>
std::vector<String> wrap(G& g, const String& text, int maxW) {
  std::vector<String> lines;
  String line;
  int i = 0, n = text.length();
  while (i < n) {
    int j = i;
    while (j < n && text[j] != ' ' && text[j] != '\n') j++;
    String word = text.substring(i, j);
    if (word.length()) {
      String trial = line.length() ? line + " " + word : word;
      if (g.textWidth(trial) <= maxW) {
        line = trial;
      } else {
        if (line.length()) lines.push_back(line);
        line = "";
        while (g.textWidth(word) > maxW) {
          int cut = fitBytes(g, word, maxW);
          lines.push_back(word.substring(0, cut));
          word = word.substring(cut);
        }
        line = word;
      }
    }
    if (j < n && text[j] == '\n') {
      lines.push_back(line);
      line = "";
    }
    i = j + 1;
  }
  if (line.length()) lines.push_back(line);
  return lines;
}

String agentName(const String& id) {
  if (id == "auto") return "Авто";
  for (auto& a : agents_)
    if (a.id == id) return a.name;
  return id;
}

uint32_t agentColor(const String& id) {
  for (auto& a : agents_)
    if (a.id == id) return a.color;
  return COL_AUTO;
}

// ── Верхняя панель ──
void drawBattery(int x, int y) {
  if (bat_ < 0) {
    lcd.setFont(F_SMALL);
    lcd.setTextColor(COL_DIM, COL_PANEL);
    lcd.drawString("USB", x, y + 1);
    return;
  }
  uint32_t c = batLow_ ? COL_BAD : (bat_ < 30 ? COL_WARN : COL_OK);
  lcd.drawRect(x, y + 2, 20, 11, COL_DIM);
  lcd.fillRect(x + 20, y + 5, 2, 5, COL_DIM);
  int w = (bat_ * 16) / 100;
  if (w > 0) lcd.fillRect(x + 2, y + 4, w, 7, c);
  lcd.setFont(F_SMALL);
  lcd.setTextColor(COL_TEXT, COL_PANEL);
  lcd.drawString(String(bat_) + "%", x + 25, y + 1);
}

void drawTop() {
  lcd.fillRect(0, 0, W, TOP_H, COL_PANEL);
  // слева — выбранный агент
  uint32_t c = selected_ == "auto" ? COL_AUTO : agentColor(selected_);
  lcd.fillCircle(10, 13, 5, c);
  lcd.setFont(F_TEXT);
  lcd.setTextColor(COL_TEXT, COL_PANEL);
  lcd.drawString(agentName(selected_), 21, 5);
  // справа налево: батарея, громкость, сервер, Wi-Fi
  int x = W - 56;
  drawBattery(x, 5);
  x -= 48;
  lcd.fillRect(x, 9, 4, 7, COL_DIM);
  lcd.fillTriangle(x + 4, 9, x + 9, 4, x + 9, 20, COL_DIM);
  lcd.fillTriangle(x + 4, 16, x + 9, 20, x + 4, 9, COL_DIM);
  lcd.setFont(F_SMALL);
  lcd.setTextColor(COL_TEXT, COL_PANEL);
  lcd.drawString(String(vol_), x + 13, 6);
  x -= 16;
  lcd.fillCircle(x, 13, 4, server_ ? COL_OK : COL_BAD);
  x -= 26;
  int bars = !wifi_ ? 0 : (rssi_ > -55 ? 4 : rssi_ > -65 ? 3 : rssi_ > -75 ? 2 : 1);
  for (int i = 0; i < 4; i++) {
    int h = 3 + i * 3;
    lcd.fillRect(x + i * 5, 19 - h, 3, h, i < bars ? COL_TEXT : COL_FAINT);
  }
  topDirty_ = false;
}

// ── Нижняя панель: агенты и громкость ──
void drawBottom() {
  int y = H - BOT_H;
  lcd.fillRect(0, y, W, BOT_H, COL_PANEL);
  chipRects_.clear();
  std::vector<std::pair<String, String>> items;
  items.push_back({"auto", "Авто"});
  for (auto& a : agents_) {
    if (items.size() >= 5) break;
    items.push_back({a.id, a.name});
  }
  int areaW = W - 84;
  int cw = areaW / (int)items.size();
  for (size_t i = 0; i < items.size(); i++) {
    Rect r{4 + (int)i * cw, y + 5, cw - 4, BOT_H - 10};
    chipRects_.push_back({r, items[i].first});
    bool sel = items[i].first == selected_;
    uint32_t c = items[i].first == "auto" ? COL_AUTO : agentColor(items[i].first);
    bool avail = true;
    for (auto& a : agents_)
      if (a.id == items[i].first) avail = a.available;
    if (sel) lcd.fillRoundRect(r.x, r.y, r.w, r.h, 8, c);
    else lcd.drawRoundRect(r.x, r.y, r.w, r.h, 8, avail ? c : COL_FAINT);
    const lgfx::IFont* f = F_TEXT;
    lcd.setFont(f);
    if (lcd.textWidth(items[i].second) > r.w - 6) lcd.setFont(F_SMALL);
    lcd.setTextColor(sel ? COL_BLACK : (avail ? COL_TEXT : COL_DIM));
    lcd.setTextDatum(textdatum_t::middle_center);
    lcd.drawString(items[i].second, r.x + r.w / 2, r.y + r.h / 2 + 1);
    lcd.setTextDatum(textdatum_t::top_left);
  }
  for (auto* r : {&volDownRect_, &volUpRect_}) {
    lcd.drawRoundRect(r->x, r->y, r->w, r->h, 8, COL_DIM);
    int cx = r->x + r->w / 2, cy = r->y + r->h / 2;
    lcd.fillRect(cx - 7, cy - 1, 14, 3, COL_TEXT);
    if (r == &volUpRect_) lcd.fillRect(cx - 1, cy - 7, 3, 14, COL_TEXT);
  }
  botDirty_ = false;
}

// ── Центральная область (через спрайт, без мерцания) ──
void centerText(const String& s, int y, const lgfx::IFont* f, uint32_t c) {
  canvas.setFont(f);
  canvas.setTextColor(c);
  canvas.setTextDatum(textdatum_t::top_center);
  canvas.drawString(s, W / 2, y);
  canvas.setTextDatum(textdatum_t::top_left);
}

void drawCenteredBlock(const String& title, const String& sub, uint32_t titleColor) {
  canvas.setFont(F_BIG);
  auto lines = wrap(canvas, title, W - 24);
  int y = MID_H / 2 - (int)lines.size() * 14 - (sub.length() ? 12 : 0);
  for (auto& l : lines) {
    centerText(l, y, F_BIG, titleColor);
    y += 28;
  }
  if (sub.length()) {
    canvas.setFont(F_TEXT);
    for (auto& l : wrap(canvas, sub, W - 24)) {
      centerText(l, y + 4, F_TEXT, COL_DIM);
      y += 19;
    }
  }
}

void rewrap() {
  canvas.setFont(F_SMALL);
  userLines_ = userText_.length() ? wrap(canvas, String("Вы: ") + userText_, W - 16) : std::vector<String>{};
  canvas.setFont(F_TEXT);
  replyLines_ = replyText_.length() ? wrap(canvas, replyText_, W - 16) : std::vector<String>{};
  wrapDirty_ = false;
}

void drawConversation(int bottomReserve) {
  if (wrapDirty_) rewrap();
  const int lhUser = 15, lhReply = 20;
  int avail = MID_H - 8 - bottomReserve;
  int need = (int)userLines_.size() * lhUser + (answerName_.length() ? 18 : 0) + (int)replyLines_.size() * lhReply;
  int skip = need > avail ? need - avail : 0;  // прокрутка к последним строкам
  int y = 4 - skip;
  canvas.setFont(F_SMALL);
  canvas.setTextColor(COL_USER);
  for (auto& l : userLines_) {
    if (y > -lhUser) canvas.drawString(l, 8, y);
    y += lhUser;
  }
  if (answerName_.length()) {
    if (y > -18) {
      canvas.fillCircle(12, y + 8, 4, answerColor_);
      canvas.setFont(F_SMALL);
      canvas.setTextColor(answerColor_);
      String label = answerName_;
      if (answerNote_.length()) label += String(" · ") + answerNote_;
      canvas.drawString(label, 22, y + 2);
    }
    y += 18;
  }
  canvas.setFont(F_TEXT);
  canvas.setTextColor(COL_TEXT);
  for (auto& l : replyLines_) {
    if (y > -lhReply) canvas.drawString(l, 8, y);
    y += lhReply;
  }
}

void drawMiddle() {
  uint32_t now = millis();
  canvas.fillScreen(COL_BG);
  switch (status_) {
    case Status::Boot:
    case Status::Connecting:
      drawCenteredBlock(hint_.length() ? hint_ : "Подключаюсь…", "", COL_TEXT);
      break;
    case Status::Offline:
      drawCenteredBlock("Нет связи", hint_, COL_WARN);
      break;
    case Status::Sleep:
      drawCenteredBlock("Заряди меня", "Аккумулятор разряжен — колонка засыпает", COL_BAD);
      break;
    case Status::Idle:
      if (!userText_.length() && !replyText_.length())
        drawCenteredBlock("Нажми и говори", hint_.length() ? hint_ : "SW1 — говорить · SW2 — другой агент", COL_TEXT);
      else
        drawConversation(0);
      break;
    case Status::Listening: {
      int cx = W / 2, cy = MID_H / 2 - 14;
      float lvl = mic_ < 1e-5f ? 0 : (20.f * log10f(mic_) + 50.f) / 40.f;
      lvl = lvl < 0 ? 0 : (lvl > 1 ? 1 : lvl);
      uint32_t c = selected_ == "auto" ? COL_AUTO : agentColor(selected_);
      canvas.fillCircle(cx, cy, 30 + (int)(lvl * 26), COL_FAINT);
      canvas.fillCircle(cx, cy, 30, c);
      canvas.fillRoundRect(cx - 7, cy - 16, 14, 22, 7, COL_WHITE);  // микрофон
      canvas.drawArc(cx, cy - 2, 13, 11, 0, 180, COL_WHITE);
      canvas.fillRect(cx - 1, cy + 11, 3, 6, COL_WHITE);
      centerText("Слушаю…", cy + 60, F_BIG, COL_TEXT);
      if (hint_.length()) centerText(hint_, cy + 88, F_SMALL, COL_DIM);
      break;
    }
    case Status::Thinking: {
      if (wrapDirty_) rewrap();
      canvas.setFont(F_TEXT);
      canvas.setTextColor(COL_USER);
      int y = 6;
      for (size_t i = 0; i < userLines_.size() && i < 3; i++, y += 15) {
        canvas.setFont(F_SMALL);
        canvas.drawString(userLines_[i], 8, y);
      }
      int cx = W / 2, cy = MID_H / 2 + 10;
      int a = (now / 4) % 360;
      canvas.drawArc(cx, cy, 26, 21, a, a + 100, answerColor_);
      canvas.drawArc(cx, cy, 26, 21, a + 180, a + 280, answerColor_);
      String who = answerName_.length() ? String(answerName_ + " думает…") : String("Думаю…");
      centerText(who, cy + 36, F_TEXT, COL_TEXT);
      if (answerNote_.length()) centerText(answerNote_, cy + 56, F_SMALL, COL_DIM);
      break;
    }
    case Status::Speaking: {
      drawConversation(16);
      int n = 24, bw = 8, gap = 4, x0 = (W - n * (bw + gap)) / 2, base = MID_H - 3;
      float lvl = out_ < 1e-5f ? 0 : (20.f * log10f(out_) + 45.f) / 40.f;
      lvl = lvl < 0 ? 0 : (lvl > 1 ? 1 : lvl);
      for (int i = 0; i < n; i++) {
        float wob = 0.5f + 0.5f * sinf(now / 90.0f + i * 0.9f);
        int h = 2 + (int)(lvl * 11 * wob);
        canvas.fillRect(x0 + i * (bw + gap), base - h, bw, h, answerColor_);
      }
      break;
    }
  }
  if (now < toastUntil_ && toast_.length()) {
    canvas.setFont(F_TEXT);
    auto lines = wrap(canvas, toast_, W - 40);
    int h = (int)lines.size() * 19 + 10;
    int y = MID_H - h - 4;
    canvas.fillRoundRect(10, y, W - 20, h, 8, toastColor_);
    int ly = y + 5;
    for (auto& l : lines) {
      centerText(l, ly, F_TEXT, COL_BLACK);
      ly += 19;
    }
  }
  canvas.pushSprite(0, MID_Y);
  midDirty_ = false;
}

bool animating() {
  return status_ == Status::Listening || status_ == Status::Thinking || status_ == Status::Speaking ||
         (toastUntil_ && millis() < toastUntil_ + 100);
}

void calibrate(Preferences& prefs) {
  uint16_t cal[8];
  lcd.fillScreen(COL_BG);
  lcd.setFont(F_TEXT);
  lcd.setTextColor(COL_TEXT, COL_BG);
  lcd.setTextDatum(textdatum_t::middle_center);
  lcd.drawString("Калибровка тачскрина", W / 2, H / 2 - 12);
  lcd.setFont(F_SMALL);
  lcd.drawString("коснись стрелок в углах по очереди", W / 2, H / 2 + 12);
  lcd.setTextDatum(textdatum_t::top_left);
  lcd.calibrateTouch(cal, COL_WHITE, COL_BG, 18);
  prefs.putBytes("tcal", cal, sizeof(cal));
  lcd.fillScreen(COL_BG);
}

// Первый запуск: предлагаем калибровку, но не блокируемся, если тач не подключён.
bool offerCalibration() {
  lcd.fillScreen(COL_BG);
  lcd.setFont(F_TEXT);
  lcd.setTextColor(COL_TEXT, COL_BG);
  lcd.setTextDatum(textdatum_t::middle_center);
  lcd.drawString("Коснись экрана —", W / 2, H / 2 - 22);
  lcd.drawString("откалибруем тачскрин", W / 2, H / 2);
  lcd.setFont(F_SMALL);
  lcd.setTextColor(COL_DIM, COL_BG);
  lcd.drawString("SW1 — пропустить (потом: держи SW2 при включении)", W / 2, H / 2 + 30);
  lcd.setTextDatum(textdatum_t::top_left);
  uint32_t t0 = millis();
  while (millis() - t0 < 10000) {
    int32_t x, y;
    if (lcd.getTouch(&x, &y)) {
      while (lcd.getTouch(&x, &y)) delay(10);  // ждём, пока отпустят
      return true;
    }
    if (digitalRead(PIN_BTN_TALK) == LOW) break;
    delay(20);
  }
  lcd.fillScreen(COL_BG);
  return false;
}

}  // namespace

uint32_t parseColor(const char* hex, uint32_t fallback) {
  if (!hex) return fallback;
  if (*hex == '#') hex++;
  if (strlen(hex) != 6) return fallback;
  char* end = nullptr;
  uint32_t v = strtoul(hex, &end, 16);
  return (end && *end == 0) ? v : fallback;
}

void begin(bool forceCalibrate) {
  lcd.init();
  lcd.setRotation(1);
  lcd.setBrightness(200);
  lcd.fillScreen(COL_BG);
  canvas.setColorDepth(16);
  canvas.setPsram(true);
  spriteOk_ = canvas.createSprite(W, MID_H) != nullptr;
  if (!spriteOk_) {
    canvas.setColorDepth(8);
    canvas.setPsram(false);
    spriteOk_ = canvas.createSprite(W, MID_H) != nullptr;
  }
  Preferences prefs;
  prefs.begin("ui", false);
  uint16_t cal[8];
  if (!forceCalibrate && prefs.getBytesLength("tcal") == sizeof(cal)) {
    prefs.getBytes("tcal", cal, sizeof(cal));
    lcd.setTouchCalibrate(cal);
  } else if (forceCalibrate || offerCalibration()) {
    calibrate(prefs);
  }
  prefs.end();
  lcd.setFont(F_BIG);
  lcd.setTextColor(COL_TEXT, COL_BG);
  lcd.setTextDatum(textdatum_t::middle_center);
  lcd.drawString("AI-Column", W / 2, H / 2 - 20);
  lcd.setFont(F_SMALL);
  lcd.setTextColor(COL_DIM, COL_BG);
  lcd.drawString("прошивка " FW_VERSION, W / 2, H / 2 + 8);
  lcd.setTextDatum(textdatum_t::top_left);
}

void bootMessage(const String& line) {
  lcd.fillRect(0, H / 2 + 24, W, 24, COL_BG);
  lcd.setFont(F_TEXT);
  lcd.setTextColor(COL_TEXT, COL_BG);
  lcd.setTextDatum(textdatum_t::top_center);
  lcd.drawString(line, W / 2, H / 2 + 26);
  lcd.setTextDatum(textdatum_t::top_left);
}

void setStatus(Status s, const String& hint) {
  if (s != status_ || hint != hint_) midDirty_ = true;
  if (status_ == Status::Boot && s != Status::Boot) topDirty_ = botDirty_ = true;
  status_ = s;
  hint_ = hint;
}

void setWifi(bool connected, int rssi) {
  int bars = rssi > -55 ? 4 : rssi > -65 ? 3 : rssi > -75 ? 2 : 1;
  int oldBars = rssi_ > -55 ? 4 : rssi_ > -65 ? 3 : rssi_ > -75 ? 2 : 1;
  if (connected != wifi_ || bars != oldBars) topDirty_ = true;
  wifi_ = connected;
  rssi_ = rssi;
}

void setServer(bool connected) {
  if (connected != server_) topDirty_ = true;
  server_ = connected;
}

void setBattery(int percent, bool low) {
  if (percent != bat_ || low != batLow_) topDirty_ = true;
  bat_ = percent;
  batLow_ = low;
}

void setVolume(uint8_t v) {
  if (v != vol_) topDirty_ = true;
  vol_ = v;
}

void setAgents(const std::vector<AgentChip>& list) {
  agents_ = list;
  topDirty_ = botDirty_ = midDirty_ = true;
}

void setSelectedAgent(const String& id) {
  if (id != selected_) topDirty_ = botDirty_ = midDirty_ = true;
  selected_ = id;
}

void setAnswering(const String& name, uint32_t color, const String& note) {
  answerName_ = name;
  answerColor_ = color;
  answerNote_ = note;
  midDirty_ = wrapDirty_ = true;
}

void setUserText(const String& text) {
  userText_ = text;
  midDirty_ = wrapDirty_ = true;
}

void appendReply(const String& text) {
  replyText_ += text;
  if (replyText_.length() > 1500) {
    int cut = replyText_.indexOf(' ', replyText_.length() - 1200);
    replyText_ = "…" + replyText_.substring(cut > 0 ? cut : replyText_.length() - 1200);
  }
  midDirty_ = wrapDirty_ = true;
}

void clearConversation() {
  userText_ = "";
  replyText_ = "";
  answerName_ = "";
  answerNote_ = "";
  midDirty_ = wrapDirty_ = true;
}

void toast(const String& text, uint32_t color, uint32_t ms) {
  toast_ = text;
  toastColor_ = color;
  toastUntil_ = millis() + ms;
  midDirty_ = true;
}

void setLevels(float mic, float out) {
  mic_ = mic;
  out_ = out;
}

void setBacklight(uint8_t level) { lcd.setBrightness(level); }

Touch loop() {
  Touch result;
  uint32_t now = millis();
  if (status_ != Status::Boot) {
    if (topDirty_) drawTop();
    if (botDirty_) drawBottom();
    bool due = now - lastMidDraw_ >= (animating() ? 50u : 250u);
    if (spriteOk_ && (midDirty_ || (animating() && due))) {
      drawMiddle();
      lastMidDraw_ = now;
    }
    if (toastUntil_ && now > toastUntil_ + 100) {
      toastUntil_ = 0;
      midDirty_ = true;
    }
  }

  static uint32_t lastTouchPoll = 0;  // тач опрашиваем ~60 раз/с, а не на каждом проходе loop
  if (now - lastTouchPoll < 16) return result;
  lastTouchPoll = now;
  int32_t x, y;
  bool pressed = lcd.getTouch(&x, &y);
  if (pressed && !touching_) {
    touching_ = true;
    touchX_ = x;
    touchY_ = y;
  } else if (!pressed && touching_) {
    touching_ = false;
    for (auto& cr : chipRects_)
      if (cr.first.hit(touchX_, touchY_)) {
        result.ev = TouchEvent::SelectAgent;
        result.agentId = cr.second;
        return result;
      }
    if (volDownRect_.hit(touchX_, touchY_)) result.ev = TouchEvent::VolumeDown;
    else if (volUpRect_.hit(touchX_, touchY_)) result.ev = TouchEvent::VolumeUp;
    else if (touchY_ >= MID_Y && touchY_ < MID_Y + MID_H) result.ev = TouchEvent::Talk;
  }
  return result;
}

}  // namespace ui
