# Установка и запуск

Понадобится компьютер или мини-ПК в той же Wi-Fi-сети, что и колонка, — **сервер**. Если хочешь локальные модели (Qwen и т.п.) и быстрое распознавание речи, нужна видеокарта NVIDIA от 8 ГБ. Без неё тоже всё работает: Claude и Kimi через облако, Whisper и Piper на процессоре — только медленнее.

## 1. Сервер

**Требования:** Node.js 20.12+ (лучше 22 LTS) и git.

```bash
git clone https://github.com/Daniil-Svetlov/AI-Column.git
cd AI-Column/server
npm install
cp .env.example .env
cp config/config.example.yaml config/config.yaml
```

**В `.env`:**

- `AUTH_TOKEN` — придумай длинную строку. Сгенерировать: `node -e "console.log(require('crypto').randomBytes(16).toString('hex'))"`.
- `ANTHROPIC_API_KEY` — если нужен Claude.

**В `config/config.yaml`** оставь нужных агентов и поправь адреса (подробно — в [AGENTS.md](AGENTS.md)). Тестовый агент `echo` можно включить (`enabled: true`), чтобы проверить колонку без нейросетей.

```bash
npm run check-config        # проверка конфига и доступности агентов
npm start                   # сервер на :8080, веб-панель — http://localhost:8080
```

### Речь: распознавание (STT) и синтез (TTS)

**Вариант А — Docker** (проще всего; на Linux с mDNS):

```bash
cd AI-Column
docker compose --profile speech up -d --build
curl -X POST http://localhost:8000/v1/models/Systran/faster-whisper-medium   # скачать модель Whisper один раз
```

Что поднимется:

- `speaches` (faster-whisper) — на `:8000`;
- Piper с голосом `ru_RU-irina-medium` — на `:5000`.

На видеокарте поставь образ `ghcr.io/speaches-ai/speaches:latest-cuda`, раскомментируй `deploy` и возьми модель `Systran/faster-whisper-large-v3` — она распознаёт русский заметно лучше.

**Вариант Б — без Docker:**

```bash
# Синтез (Piper)
python3 -m pip install "piper-tts[http]"
python3 -m piper.download_voices --download-dir ~/piper-voices ru_RU-irina-medium
python3 -m piper.http_server -m ru_RU-irina-medium --data-dir ~/piper-voices --port 5000

# Распознавание (whisper.cpp)
./build/bin/whisper-server -m models/ggml-medium.bin --host 0.0.0.0 --port 8000 -l ru
```

Для whisper.cpp в конфиге поставь:

```yaml
stt: { provider: whispercpp, base_url: http://localhost:8000 }
```

**Вариант В — облако.** Любой OpenAI-совместимый `/v1/audio/transcriptions` и `/v1/audio/speech`: укажи `base_url`, `api_key`, `model`.

**Проверка речи:**

```bash
npm run check-config -- --say
```

Команда синтезирует фразу и распознаёт её обратно.

### Локальные модели

```bash
docker compose --profile local-llm up -d          # или поставь Ollama с ollama.com
docker compose exec ollama ollama pull qwen3:8b
```

### Проверка без колонки

```bash
npm run simulate -- --text "Квен, который час?"
npm run simulate -- --wav вопрос.wav --out ответ.wav     # как будто сказали в микрофон
```

Симулятор печатает всё, что приходит колонке, и сохраняет звук ответа в WAV.

### Автозапуск (Linux, systemd)

```ini
# /etc/systemd/system/ai-column.service
[Unit]
Description=AI-Column server
After=network-online.target

[Service]
WorkingDirectory=/home/user/AI-Column/server
ExecStart=/usr/bin/node server.js
Restart=always
User=user

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now ai-column
```

## 2. Прошивка

**Требования:** VS Code с расширением PlatformIO или `pip install platformio`.

```bash
cd firmware
cp include/secrets.example.h include/secrets.h
```

**В `include/secrets.h`:**

- `WIFI_SSID`, `WIFI_PASSWORD` — твоя сеть;
- `DEVICE_TOKEN` — тот же `AUTH_TOKEN`, что в `.env` сервера;
- `SERVER_HOST` — оставь пустым: колонка найдёт сервер по mDNS. Если не находит (бывает в гостевых и некоторых mesh-сетях), впиши IP сервера.

Подключи плату к порту **«UART» / «COM»** (не «USB») и выполни:

```bash
pio run -t upload
pio device monitor
```

**Первый запуск:**

1. Экран предложит коснуться для калибровки тачскрина. Нажми на четыре стрелки в углах. SW1 — пропустить.
2. Колонка подключится к Wi-Fi и найдёт сервер; внизу появятся агенты.
3. Нажми SW1 и задай вопрос.

**Подстройка в `include/config.h`:**

- `MIC_SHIFT` — чувствительность микрофонов;
- `MIC_CHANNELS` — работать с одним микрофоном, если второй бракованный;
- `VOLUME_MAX_GAIN_DB` — потолок громкости;
- `LED_MAX_BRIGHTNESS`, `LED_SERPENTINE` — подсветка;
- `VAD_END_SILENCE_MS` — сколько молчать, чтобы колонка поняла, что фраза закончилась.

**Тесты логики прошивки на ПК:**

```bash
pio test -e native
```

## 3. Если не работает

| Симптом | Решение |
|---|---|
| На экране «Ищу сервер в сети…» | `SERVER_HOST` в `secrets.h`; сервер слушает `0.0.0.0`; порт 8080 открыт в брандмауэре |
| «Сервер … не отвечает» сразу после подключения | неверный `DEVICE_TOKEN`: в логе сервера будет «отклонено подключение без верного токена» |
| Агент серый (недоступен) | веб-панель → «Проверить»; там текст ошибки (нет модели, нет ключа, сервер выключен) |
| Долго до первого слова | смотри историю в панели: если велико STT — нужна видеокарта или модель Whisper меньше; если первый токен — модель/сервер LLM |
| «Синтез речи недоступен» | Piper не запущен или неверный `TTS_URL` |
| В логе «SQLite недоступен — история только в памяти» | не собрался `better-sqlite3` (нет готовой сборки под твою платформу): поставь компилятор (`build-essential` / Visual Studio Build Tools) и сделай `npm install` ещё раз. Колонка работает и без него |

Проблемы с железом — в [HARDWARE.md](HARDWARE.md#если-что-то-не-работает).
