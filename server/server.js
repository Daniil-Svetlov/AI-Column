#!/usr/bin/env node
// Точка входа сервера AI-Column.
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadConfig, ConfigError } from './src/config.js';
import { createServer } from './src/app.js';

const here = path.dirname(fileURLToPath(import.meta.url));
process.chdir(here);

try {
  process.loadEnvFile(path.join(here, '.env'));
} catch {
  // .env необязателен — переменные могут прийти из окружения (docker, systemd)
}

let app;
try {
  const config = loadConfig({ baseDir: here });
  app = await createServer({ config });
  app.log.info(`конфиг: ${path.relative(here, config._file)}`);
  await app.listen();
} catch (e) {
  if (e instanceof ConfigError) {
    console.error(e.message);
  } else if (e?.code === 'EADDRINUSE') {
    console.error(`Порт занят: ${e.message}. Поменяй PORT в .env.`);
  } else {
    console.error(e);
  }
  process.exit(1);
}

let stopping = false;
for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, async () => {
    if (stopping) process.exit(1);
    stopping = true;
    app.log.info('останавливаюсь…');
    await app.close().catch(() => {});
    process.exit(0);
  });
}
