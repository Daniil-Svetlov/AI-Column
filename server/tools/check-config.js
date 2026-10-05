#!/usr/bin/env node
// Проверяет конфиг и доступность агентов, STT и TTS — без запуска сервера.
//   npm run check-config            — проверить конфиг и опросить агентов
//   npm run check-config -- --say   — ещё и синтезировать тестовую фразу
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadConfig, ConfigError } from '../src/config.js';
import { AgentRegistry } from '../src/agents/registry.js';
import { createTts } from '../src/speech/tts.js';
import { createStt } from '../src/speech/stt.js';

const here = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
try {
  process.loadEnvFile(path.join(here, '.env'));
} catch { /* нет .env */ }

let cfg;
try {
  cfg = loadConfig({ baseDir: here });
} catch (e) {
  console.error(e instanceof ConfigError ? e.message : e);
  process.exit(1);
}
console.log(`✔ конфиг ${path.relative(here, cfg._file)} корректен`);
console.log(`  агентов: ${cfg.agents.length}, по умолчанию: ${cfg.router.default}/${cfg.router.default_agent}, запасные: ${cfg.router.fallback.join(' → ') || '—'}`);
if (!cfg.server.auth_token) console.log('⚠ AUTH_TOKEN не задан — сервер будет открыт для всей сети');

const reg = new AgentRegistry(cfg.agents);
for (const info of await reg.checkAll()) {
  const st = info.status;
  console.log(`${st.ok ? '✔' : '✖'} агент ${info.id} (${info.provider}:${info.model ?? '-'}) ${st.ok ? `доступен, ${st.latencyMs} мс` : `— ${st.error}`}`);
}

const tts = createTts(cfg.speech.tts);
const stt = createStt(cfg.speech.stt);
if (process.argv.includes('--say')) {
  try {
    const t0 = Date.now();
    const pcm = await tts.synthesize('Привет! Это проверка синтеза речи.', 16000);
    console.log(`✔ TTS ${tts.name}: ${(pcm.length / 16000).toFixed(1)} с звука за ${Date.now() - t0} мс`);
    const t1 = Date.now();
    const text = await stt.transcribe(pcm, 16000);
    console.log(`✔ STT ${stt.name}: «${text}» за ${Date.now() - t1} мс`);
  } catch (e) {
    console.log(`✖ речь: ${e.message}`);
  }
} else {
  console.log(`  STT: ${stt.name}\n  TTS: ${tts.name}\n  (добавь --say, чтобы проверить синтез и распознавание по-настоящему)`);
}
