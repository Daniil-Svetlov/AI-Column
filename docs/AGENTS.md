# Агенты: Claude, Qwen, Kimi и другие

Агент — это модель с именем, цветом и описанием. Все агенты описываются в `server/config/config.yaml`, в разделе `agents`. Сервер умеет работать с двумя видами API:

| `provider` | Что подключает |
|---|---|
| `anthropic` | Claude через Anthropic Messages API |
| `openai` | любой OpenAI-совместимый `/v1/chat/completions`: Ollama, vLLM, llama.cpp `llama-server`, LM Studio, LocalAI, SGLang, а также облака Moonshot (Kimi), OpenRouter, DeepSeek, OpenAI, Groq |
| `echo` | тестовый агент без нейросети: повторяет сказанное, умеет менять громкость и говорить время |

## Поля агента

```yaml
- id: qwen                      # латиница; по нему агент выбирается в API и на колонке
  name: Qwen                    # так агент подписан на экране
  color: "#7B61FF"              # цвет на экране и в подсветке
  provider: openai
  base_url: http://localhost:11434/v1
  api_key: ollama               # локальным серверам ключ обычно не нужен
  model: qwen3:8b
  aliases: [квен, квин, qwen]   # как к агенту обращаются голосом (Whisper пишет по-разному)
  description: быстрые бытовые вопросы   # видит автороутер и другие агенты
  tools: true                   # function calling; false — для моделей, которые его не умеют
  max_tokens: 500
  temperature: 0.6              # необязательно
  no_think: true                # Qwen3: дописать /no_think, чтобы модель не «размышляла» вслух
  system: "Отвечай как бывалый инженер."   # добавка к общей персоне
  first_token_timeout_ms: 15000 # не начал отвечать за это время → запасной агент
  timeout_ms: 60000
  extra_body: {}                # любые доп. параметры запроса (см. ниже)
  enabled: true                 # false — временно выключить
```

Блоки `<think>…</think>` (Qwen3, DeepSeek-R1 и другие «рассуждающие» модели) сервер вырезает из ответа сам, поэтому они не попадают ни в речь, ни на экран.

## Claude

```yaml
- id: claude
  name: Claude
  provider: anthropic
  model: claude-sonnet-5-5         # быстрее всех — claude-haiku-4-5-20251001
  api_key: ${ANTHROPIC_API_KEY}
  aliases: [клод, клода, клоду, клауд, claude]
  description: сложные вопросы, рассуждения, код
```

Ключ создаётся в [console.anthropic.com](https://console.anthropic.com) и кладётся в `server/.env`. Актуальные имена моделей смотри на странице [Models overview](https://platform.claude.com/docs/en/models/overview).

## Qwen локально (Ollama)

```bash
ollama pull qwen3:8b            # ~5 ГБ, нормально идёт на видеокарте с 8 ГБ
# слабее железо: qwen3:4b; мощнее: qwen3:14b / qwen3:32b
```

```yaml
- id: qwen
  provider: openai
  base_url: http://localhost:11434/v1
  api_key: ollama
  model: qwen3:8b
  no_think: true
```

Если Ollama стоит на другом компьютере, запусти её с `OLLAMA_HOST=0.0.0.0` и укажи в `base_url` IP этого компьютера.

## Kimi

**Через облачный API Moonshot** (проще всего):

```yaml
- id: kimi
  provider: openai
  base_url: https://api.moonshot.ai/v1
  api_key: ${KIMI_API_KEY}
  model: ${KIMI_MODEL}            # точное имя модели — в документации Moonshot
```

**Локально через vLLM.** Kimi K2 — очень большая MoE-модель: для неё нужен многокарточный сервер. На обычном ПК реалистичнее облако Moonshot или OpenRouter. Пример для vLLM:

```bash
vllm serve moonshotai/Kimi-K2-Instruct --port 8001 --enable-auto-tool-choice --tool-call-parser kimi_k2 --trust-remote-code
```

```yaml
- id: kimi
  provider: openai
  base_url: http://gpu-server:8001/v1
  model: moonshotai/Kimi-K2-Instruct
```

Ключи `--enable-auto-tool-choice` и `--tool-call-parser` нужны, чтобы работали инструменты. Если vLLM отвечает `400` на запрос с `tools`, сервер сам повторит запрос без инструментов и напишет об этом в лог.

## Другие серверы

| Сервер | `base_url` | Заметки |
|---|---|---|
| llama.cpp | `http://host:8080/v1` | `llama-server -m model.gguf --jinja` (с `--jinja` работают tools) |
| LM Studio | `http://host:1234/v1` | включи «Serve on local network» |
| vLLM | `http://host:8000/v1` | для tools: `--enable-auto-tool-choice --tool-call-parser …` |
| OpenRouter | `https://openrouter.ai/api/v1` | `model: moonshotai/kimi-k2` и т.п. |
| DeepSeek | `https://api.deepseek.com/v1` | |

**`extra_body`** передаёт серверу любые дополнительные поля запроса. Например, отключить «размышления» Qwen3 в vLLM:

```yaml
extra_body:
  chat_template_kwargs: { enable_thinking: false }
```

## Как выбирается агент

Для каждой фразы сервер проверяет по порядку:

1. **Обращение голосом.** «Клод, …», «Спроси Квен …», «Эй, Кими — …». Имя убирается из вопроса.
2. **Выбор на колонке.** Если на экране выбран конкретный агент, а не «Авто».
3. **Правила** `router.rules` по ключевым словам: например, всё про код — к Claude.
4. **Классификатор** `router.classifier`. Быстрая модель получает список агентов с описаниями и отвечает одним словом — id агента. Если не ответила за `timeout_ms`, этот шаг пропускается.
5. **`router.default_agent`.**

**Запасные агенты.** Если выбранный агент недоступен (проверка `/models` раз в минуту) или не начал отвечать за `first_token_timeout_ms`, сервер пробует агентов из `router.fallback` по порядку. На экране колонки при этом появится пометка «запасной агент».

**Голосовые команды**, которые не уходят в модель:

- «Переключись на Квен», «Переключись на авто» — сменить агента;
- просто имя агента («Клод!») — сменить агента;
- «Новый разговор», «Забудь всё», «Сбрось контекст» — сбросить контекст.

## Инструменты

Включаются списком в `tools.enabled`:

| Инструмент | Что делает |
|---|---|
| `datetime` | текущие дата и время (часовой пояс — `server.timezone`) |
| `weather` | погода и прогноз на сегодня и завтра через open-meteo.com (ключ не нужен) |
| `volume` | громкость колонки |
| `lights` | подсветка: `off`, `solid`, `breathe`, `rainbow`, `auto` |
| `timer` | таймер; по истечении колонка скажет об этом вслух |
| `switch_agent` | сменить агента на колонке |
| `ask_agent` | спросить другого агента и вернуть его ответ. Так агенты «советуются»: локальная модель может переадресовать сложный вопрос Claude |

Если модель плохо работает с function calling (бывает у маленьких моделей), поставь агенту `tools: false`. Он станет просто собеседником, а время и дата всё равно будут в системном промпте.

## Проверка

```bash
npm run check-config                                  # доступность каждого агента
npm run simulate -- --text "Кими, придумай загадку"   # полный ход без колонки
curl -H "Authorization: Bearer $AUTH_TOKEN" -H "Content-Type: application/json" \
     -d '{"text":"Квен, привет"}' http://localhost:8080/api/ask
```

В веб-панели (`http://сервер:8080`) видно, кто из агентов доступен. Там же история: кто ответил, почему был выбран именно он (`voice`, `device`, `rule`, `classifier`, `default`, `fallback`) и сколько прошло до первого звука.
