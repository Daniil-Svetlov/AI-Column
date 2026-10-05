import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { substituteEnv, normalizeConfig, loadConfig, ConfigError } from '../src/config.js';

const serverDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

test('substituteEnv: ${VAR}, значения по умолчанию, числа', () => {
  const env = { KEY: 'abc', PORT: '9000' };
  const out = substituteEnv({ a: '${KEY}', b: 'x-${KEY}-y', c: '${MISSING:-def}', d: '${PORT}', e: ['${KEY}'], f: '${MISSING}' }, env);
  assert.deepEqual(out, { a: 'abc', b: 'x-abc-y', c: 'def', d: 9000, e: ['abc'], f: '' });
  const secret = substituteEnv({ auth_token: '${T}', api_key: '${T}', port: '${T}' }, { T: '0123' });
  assert.deepEqual(secret, { auth_token: '0123', api_key: '0123', port: 123 });
});

test('normalizeConfig заполняет значения по умолчанию', () => {
  const cfg = normalizeConfig({ agents: [{ id: 'echo', provider: 'echo' }] });
  assert.equal(cfg.router.default, 'auto');
  assert.equal(cfg.router.default_agent, 'echo');
  assert.equal(cfg.agents[0].name, 'echo');
  assert.equal(cfg.agents[0].max_tokens, 700);
  assert.equal(cfg.server.port, 8080);
});

test('normalizeConfig собирает все ошибки сразу', () => {
  assert.throws(() => normalizeConfig({
    agents: [
      { id: 'a', provider: 'openai', model: 'm' },
      { id: 'a', provider: 'nope' },
    ],
    router: { default: 'zzz', fallback: ['q'] },
    speech: { stt: { provider: 'openai' } },
  }), (e) => {
    assert.ok(e instanceof ConfigError);
    for (const part of ['base_url', 'повторяющийся id', 'provider должен', 'router.default', 'router.fallback', 'speech.stt.base_url']) {
      assert.ok(e.message.includes(part), `нет «${part}» в: ${e.message}`);
    }
    return true;
  });
});

test('агенты с enabled: false выключаются', () => {
  const cfg = normalizeConfig({ agents: [{ id: 'echo', provider: 'echo' }, { id: 'off', provider: 'echo', enabled: false }] });
  assert.deepEqual(cfg.agents.map((a) => a.id), ['echo']);
});

test('пример config.example.yaml валиден', () => {
  const cfg = loadConfig({ file: 'config/config.example.yaml', baseDir: serverDir, env: { ANTHROPIC_API_KEY: 'x' } });
  assert.deepEqual(cfg.agents.map((a) => a.id), ['claude', 'qwen', 'kimi']);
  assert.equal(cfg.router.classifier.agent, 'qwen');
  assert.equal(cfg.agents[0].api_key, 'x');
});
