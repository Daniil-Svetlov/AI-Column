# Сервер AI-Column

Node.js 20.12+. Принимает звук от колонок, распознаёт речь, выбирает агента (Claude, Qwen, Kimi…), стримит ответ и озвучивает его.

```bash
npm install
cp .env.example .env && cp config/config.example.yaml config/config.yaml
npm run check-config        # конфиг + доступность агентов (--say — проверить STT/TTS)
npm start                   # :8080 — WebSocket /ws для колонок, REST /api, веб-панель /
npm test                    # 50 тестов: логика, провайдеры против поддельных API, сквозные через WebSocket
npm run simulate -- --text "Клод, привет"     # «колонка» из терминала
```

| Файл | Что настраивает |
|---|---|
| `.env` | секреты: `AUTH_TOKEN`, `ANTHROPIC_API_KEY`, адреса серверов (`OLLAMA_URL`, `STT_URL`, `TTS_URL`…) |
| `config/config.yaml` | агенты, роутер, речь, инструменты, персона ассистента |

Подробности:

- [../docs/AGENTS.md](../docs/AGENTS.md) — агенты;
- [../docs/API.md](../docs/API.md) — протокол колонки и REST API;
- [../docs/SETUP.md](../docs/SETUP.md) — установка речи и моделей.
