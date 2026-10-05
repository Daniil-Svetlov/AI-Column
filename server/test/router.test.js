import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeConfig } from '../src/config.js';
import { AgentRegistry } from '../src/agents/registry.js';
import { Router } from '../src/agents/router.js';

function setup(routerCfg = {}) {
  const cfg = normalizeConfig({
    agents: [
      { id: 'claude', name: 'Claude', provider: 'echo', aliases: ['клод', 'клауд'], description: 'сложное' },
      { id: 'qwen', name: 'Qwen', provider: 'echo', aliases: ['квен', 'куэн'], description: 'быстрое' },
      { id: 'kimi', name: 'Kimi', provider: 'echo', aliases: ['кими'], description: 'тексты' },
    ],
    router: { default_agent: 'qwen', fallback: ['qwen', 'claude'], ...routerCfg },
  });
  const registry = new AgentRegistry(cfg.agents);
  return { registry, router: new Router({ registry, routerConfig: cfg.router }) };
}

test('обращение по имени выбирает агента и убирает имя из вопроса', () => {
  const { router } = setup();
  assert.deepEqual(router.parse('Клод, сколько будет два плюс два?'), { kind: 'address', agentId: 'claude', text: 'сколько будет два плюс два?' });
  assert.deepEqual(router.parse('Спроси квен: какая столица Франции'), { kind: 'address', agentId: 'qwen', text: 'какая столица Франции' });
  assert.deepEqual(router.parse('Эй, Кими — напиши стих'), { kind: 'address', agentId: 'kimi', text: 'напиши стих' });
  assert.equal(router.parse('Клодетта пришла'), null, 'часть слова не считается обращением');
  assert.equal(router.parse('Какая погода?'), null);
});

test('команды переключения и сброса', () => {
  const { router } = setup();
  assert.deepEqual(router.parse('Переключись на Кими.'), { kind: 'switch', agentId: 'kimi' });
  assert.deepEqual(router.parse('переключись на авто'), { kind: 'switch', agentId: 'auto' });
  assert.deepEqual(router.parse('Квен'), { kind: 'switch', agentId: 'qwen' });
  assert.deepEqual(router.parse('Новый разговор'), { kind: 'reset' });
  assert.deepEqual(router.parse('Забудь всё'), { kind: 'reset' });
});

test('приоритет: голос > выбор на колонке > правила > по умолчанию', async () => {
  const { router } = setup({ rules: [{ agent: 'claude', keywords: ['код'] }] });
  assert.equal((await router.choose({ text: 'Кими, привет', deviceAgent: 'qwen' })).agentId, 'kimi');
  assert.equal((await router.choose({ text: 'напиши код', deviceAgent: 'kimi' })).agentId, 'kimi');
  const rule = await router.choose({ text: 'Напиши код сортировки', deviceAgent: 'auto' });
  assert.deepEqual([rule.agentId, rule.reason], ['claude', 'rule']);
  const def = await router.choose({ text: 'Как дела?', deviceAgent: 'auto' });
  assert.deepEqual([def.agentId, def.reason], ['qwen', 'default']);
});

test('классификатор выбирает агента по ответу маленькой модели', async () => {
  const { router, registry } = setup({ classifier: { agent: 'qwen', timeout_ms: 1000 } });
  registry.get('qwen').complete = async ({ system }) => {
    assert.ok(system.includes('claude: сложное'));
    return 'claude';
  };
  const r = await router.choose({ text: 'Докажи теорему Ферма', deviceAgent: 'auto' });
  assert.deepEqual([r.agentId, r.reason], ['claude', 'classifier']);
});

test('если классификатор завис — берём агента по умолчанию', async () => {
  const { router, registry } = setup({ classifier: { agent: 'qwen', timeout_ms: 50 } });
  registry.get('qwen').complete = ({ signal }) => new Promise((_, reject) => signal.addEventListener('abort', () => reject(new Error('timeout'))));
  const keepAlive = setInterval(() => {}, 1000); // AbortSignal.timeout не держит event loop
  const r = await router.choose({ text: 'что-нибудь', deviceAgent: 'auto' });
  clearInterval(keepAlive);
  assert.deepEqual([r.agentId, r.reason], ['qwen', 'default']);
});

test('candidates: выбранный, потом запасные; недоступные пропускаются', () => {
  const { router, registry } = setup();
  assert.deepEqual(router.candidates('kimi'), ['kimi', 'qwen', 'claude']);
  registry.get('kimi').status.ok = false;
  assert.deepEqual(router.candidates('kimi'), ['qwen', 'claude']);
  for (const a of registry.list()) a.status.ok = false;
  assert.deepEqual(router.candidates('kimi'), ['kimi', 'qwen', 'claude'], 'если все «лежат» — всё равно пробуем');
});
