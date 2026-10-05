# API: протокол колонки и REST

## WebSocket колонки

```
ws://<сервер>:8080/ws?id=<id-колонки>&token=<AUTH_TOKEN>
```

Токен можно передать и заголовком `Authorization: Bearer …`. Если токен неверный, сервер отвечает `401`.

**Как устроены сообщения:**

- **Текстовые кадры** — JSON-объекты с полем `type`.
- **Бинарные кадры** — звук PCM 16 бит little-endian, моно:
  - колонка → сервер: 16 кГц, только между `listen_start` и `listen_stop`;
  - сервер → колонка: частота из `audio_start`, только между `audio_start` и `audio_end`, кадры до 4 КБ.

### Колонка → сервер

| `type` | Поля | Когда |
|---|---|---|
| `hello` | `fw`, `mic_rate`, `play_rate`, `volume`, `battery?` | сразу после подключения |
| `listen_start` | `agent?` | начала слушать; отменяет текущий ответ (перебивание) |
| *(бинарный)* | PCM16 16 кГц | пока слушает, кадры по 40 мс |
| `listen_stop` | — | фраза закончилась (пауза или отпущенная кнопка) |
| `cancel` | — | отменить всё (ничего не услышала, перебили) |
| `set_agent` | `agent` (`auto` или id) | выбрали агента на колонке |
| `set_volume` | `value` 0–100 | поменяли громкость на колонке |
| `new_conversation` | — | сбросить контекст (SW2 удерживать) |
| `status` | `battery?`, `rssi?`, `volume?` | раз в 30 с |
| `text` | `text`, `agent?` | текстовый запрос (симулятор, отладка) |
| `ping` | `t?` | — |

### Сервер → колонка

| `type` | Поля | Смысл |
|---|---|---|
| `welcome` | `protocol`, `assistant`, `agents[]`, `agent`, `volume`, `play_rate`, `max_utterance_seconds` | ответ на `hello` |
| `agents` | `agents[]` — `{id, name, color, available}` | изменилась доступность агентов |
| `agent_selected` | `agent` | агента сменили (голосом, из панели, инструментом) |
| `state` | `state`: `thinking` / `idle` | состояние хода, если нет звука |
| `stop_listening` | `reason` | фраза слишком длинная — хватит слать звук |
| `transcript` | `text` | что распознано (пусто — не расслышали) |
| `agent` | `id`, `name`, `color`, `reason` | кто отвечает; `reason`: `voice`, `device`, `rule`, `classifier`, `default`, `fallback`, `manual` |
| `delegate` | `from`, `to`, `name`, `color` | агент спрашивает другого агента (`ask_agent`) |
| `reply_delta` | `text`, `system?` | очередное предложение ответа (уже без markdown) |
| `audio_start` | `rate`, `format: "pcm16"`, `turn` | дальше идут бинарные кадры речи |
| *(бинарный)* | PCM16 моно | речь |
| `audio_end` | `turn`, `seconds` | звук ответа закончился |
| `reply_end` | `text`, `agent` | ответ закончился |
| `set_volume` | `value` | агент поменял громкость |
| `led` | `mode`, `color?`, `brightness?` | агент управляет подсветкой |
| `alert` | `kind` (`timer` / `message`), `text` | напоминание; следом придёт речь |
| `error` | `message` | показать ошибку |
| `pong` | `t` | — |

**Пример хода:**

```
→ {"type":"listen_start","agent":"auto"}
→ [PCM 40 мс] × N
→ {"type":"listen_stop"}
← {"type":"state","state":"thinking"}
← {"type":"transcript","text":"Квен, сколько будет семь на восемь?"}
← {"type":"agent","id":"qwen","name":"Qwen","color":"#7B61FF","reason":"voice"}
← {"type":"reply_delta","text":"Пятьдесят шесть. "}
← {"type":"audio_start","rate":24000,"format":"pcm16","turn":3}
← [PCM] × M
← {"type":"audio_end","turn":3,"seconds":1.4}
← {"type":"reply_end","text":"Пятьдесят шесть.","agent":"qwen"}
```

**Темп отправки звука.** Сервер не опережает воспроизведение больше чем на 3 секунды, поэтому у колонки не переполняется буфер (1 МБ в PSRAM, около 21 секунды). Если колонка присылает `listen_start` или `cancel` во время ответа, ход прерывается. Кадры, которые уже были в пути, колонка отбрасывает до следующего `audio_start`.

## REST

Все запросы, кроме `/api/health`, требуют `Authorization: Bearer <AUTH_TOKEN>`, если токен задан.

| Метод и путь | Тело | Ответ |
|---|---|---|
| `GET /api/health` | — | версия, STT/TTS, доступность агентов |
| `GET /api/config` | — | конфиг без секретов |
| `GET /api/agents` | — | агенты и их статус |
| `POST /api/agents/check` | — | проверить агентов сейчас |
| `GET /api/devices` | — | колонки (онлайн и известные) |
| `POST /api/devices/:id/agent` | `{"agent":"qwen"}` | выбрать агента |
| `POST /api/devices/:id/volume` | `{"volume":40}` | громкость |
| `POST /api/devices/:id/say` | `{"text":"Ужин готов!"}` | сказать вслух (с подсветкой-оповещением) |
| `POST /api/devices/:id/ask` | `{"text":"…","agent?":"claude"}` | задать вопрос, ответ прозвучит из колонки |
| `POST /api/devices/:id/reset` | — | новый разговор |
| `GET /api/history?device=&limit=&before=` | — | история реплик с задержками |
| `POST /api/ask` | `{"text":"…","agent?":"…"}` | вопрос без колонки → `{agent, reason, reply, toolCalls, ms}` |

**Примеры:**

```bash
T=ваш_токен
curl -H "Authorization: Bearer $T" http://localhost:8080/api/agents
curl -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
     -d '{"text":"Клод, объясни I2S в двух предложениях"}' http://localhost:8080/api/ask
curl -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
     -d '{"text":"Через пять минут выходим!"}' http://localhost:8080/api/devices/column-a1b2c3/say
```

**mDNS.** Сервер объявляет сервис `_aicolumn._tcp` (TXT: `path=/ws`). Если в прошивке не указан `SERVER_HOST`, колонка ищет сервер именно так.
