# Сервер AI-Column

Rust 1.85+ (edition 2024). Принимает звук от колонок, распознаёт речь, выбирает агента (Claude, Qwen, Kimi…), стримит ответ и озвучивает его.

```bash
cp .env.example .env && cp config/config.example.yaml config/config.yaml
cargo run --release -- check-config        # конфиг + доступность агентов (--say — проверить STT/TTS)
cargo run --release --                     # :8080 — WebSocket /ws для колонок, REST /api, веб-панель /
cargo test                                    # логика, провайдеры против поддельных API, сквозные через WebSocket
cargo run --release -- simulate --text "Клод, привет"     # «колонка» из терминала
```

| Файл | Что настраивает |
|---|---|
| `.env` | секреты: `AUTH_TOKEN`, `ANTHROPIC_API_KEY`, адреса серверов (`OLLAMA_URL`, `STT_URL`, `TTS_URL`…) |
| `config/config.yaml` | агенты, роутер, речь, инструменты, персона ассистента |

Сервер ищет `config/`, `public/` и `.env` в текущей папке (или в `$AI_COLUMN_DIR`). Готовый бинарник после `cargo build --release` — `target/release/ai-column-server`.

Подробности:

- [../docs/AGENTS.md](../docs/AGENTS.md) — агенты;
- [../docs/API.md](../docs/API.md) — протокол колонки и REST API;
- [../docs/SETUP.md](../docs/SETUP.md) — установка речи и моделей.
