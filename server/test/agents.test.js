// Провайдеры против поддельных серверов: проверяем реальные SDK (стриминг, tools, think-блоки, ошибки).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeConfig } from '../src/config.js';
import { AgentRegistry } from '../src/agents/registry.js';
import { Toolbox } from '../src/agents/tools.js';
import { fakeHttp, sse } from './helpers.js';

function makeAgent(agentCfg) {
  const cfg = normalizeConfig({ agents: [agentCfg], tools: { enabled: ['datetime', 'volume'] } });
  const registry = new AgentRegistry(cfg.agents);
  return { agent: registry.get(agentCfg.id), toolbox: new Toolbox({ config: cfg }), registry, cfg };
}

function chunk(delta, finish = null) {
  return { data: { id: 'c1', object: 'chat.completion.chunk', created: 1, model: 'qwen3:8b', choices: [{ index: 0, delta, finish_reason: finish }] } };
}

test('OpenAI-совместимый: стриминг, вырезание <think>, вызов инструмента и второй шаг', async () => {
  const fake = await fakeHttp((req, res, body) => {
    if (req.url === '/v1/models') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      return res.end(JSON.stringify({ object: 'list', data: [{ id: 'qwen3:8b', object: 'model' }] }));
    }
    const hasToolResult = body.messages.some((m) => m.role === 'tool');
    if (!hasToolResult) {
      return sse(res, [
        chunk({ role: 'assistant', content: '<think>пользователь хочет время' }),
        chunk({ content: '</think>\n\nСекунду.' }),
        chunk({ tool_calls: [{ index: 0, id: 'call_1', type: 'function', function: { name: 'get_datetime', arguments: '' } }] }),
        chunk({ tool_calls: [{ index: 0, function: { arguments: '{}' } }] }),
        chunk({}, 'tool_calls'),
        { data: '[DONE]' },
      ]);
    }
    return sse(res, [chunk({ content: 'Сейчас ' }), chunk({ content: 'полдень.' }), chunk({}, 'stop'), { data: '[DONE]' }]);
  });
  try {
    const { agent, toolbox, registry } = makeAgent({ id: 'qwen', provider: 'openai', base_url: `${fake.url}/v1`, model: 'qwen3:8b', no_think: true });
    assert.deepEqual(await agent.health().then((s) => s.ok), true);

    let streamed = '';
    const res = await agent.runGuarded({
      system: 'Ты помощник.',
      history: [{ role: 'assistant', content: 'лишнее' }, { role: 'user', content: 'Привет' }, { role: 'assistant', content: 'Привет!' }],
      userText: 'Который час?',
      toolbox,
      toolContext: { registry, agentId: 'qwen', depth: 0 },
      onText: (d) => { streamed += d; },
    });
    assert.equal(res.text, 'Секунду. Сейчас полдень.');
    assert.equal(streamed, res.text);
    assert.equal(res.toolCalls[0].name, 'get_datetime');
    assert.match(res.toolCalls[0].result, /^Сейчас \d{2}:\d{2}/);

    const chats = fake.requests.filter((r) => r.url === '/v1/chat/completions');
    assert.equal(chats.length, 2);
    const first = chats[0].body;
    assert.equal(first.stream, true);
    assert.match(first.messages[0].content, /\/no_think$/);
    assert.deepEqual(first.messages.slice(1).map((m) => m.role), ['user', 'assistant', 'user'], 'история нормализована');
    assert.ok(first.tools.some((t) => t.function.name === 'get_datetime'));
    const second = chats[1].body.messages;
    const toolMsg = second.find((m) => m.role === 'tool');
    assert.equal(toolMsg.tool_call_id, 'call_1');
    const asst = second.find((m) => m.tool_calls);
    assert.equal(asst.content, 'Секунду.');
  } finally {
    await fake.close();
  }
});

test('OpenAI-совместимый: если сервер не умеет tools — повтор без них', async () => {
  const fake = await fakeHttp((req, res, body) => {
    if (body.tools) {
      res.writeHead(400, { 'Content-Type': 'application/json' });
      return res.end(JSON.stringify({ error: { message: 'model does not support tools' } }));
    }
    return sse(res, [chunk({ content: 'Ок.' }), chunk({}, 'stop'), { data: '[DONE]' }]);
  });
  try {
    const { agent, toolbox, registry } = makeAgent({ id: 'q', provider: 'openai', base_url: `${fake.url}/v1`, model: 'm' });
    const res = await agent.runGuarded({ system: 's', history: [], userText: 'x', toolbox, toolContext: { registry, agentId: 'q' }, onText: () => {} });
    assert.equal(res.text, 'Ок.');
    assert.equal(agent.toolsSupported, false);
  } finally {
    await fake.close();
  }
});

test('OpenAI-совместимый: health сообщает, что модели нет на сервере', async () => {
  const fake = await fakeHttp((req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ object: 'list', data: [{ id: 'llama3:8b', object: 'model' }] }));
  });
  try {
    const { agent } = makeAgent({ id: 'q', provider: 'openai', base_url: `${fake.url}/v1`, model: 'qwen3:8b' });
    const st = await agent.health();
    assert.equal(st.ok, false);
    assert.match(st.error, /нет модели «qwen3:8b»/);
  } finally {
    await fake.close();
  }
});

test('недоступный сервер → AgentError(kind=connection)', async () => {
  const { agent } = makeAgent({ id: 'q', provider: 'openai', base_url: 'http://127.0.0.1:9/v1', model: 'm' });
  await assert.rejects(
    agent.runGuarded({ system: 's', history: [], userText: 'x', toolbox: null, onText: () => {} }),
    (e) => e.kind === 'connection',
  );
});

test('таймаут первого токена → AgentError(kind=timeout)', async () => {
  const fake = await fakeHttp(() => { /* никогда не отвечаем */ });
  try {
    const { agent } = makeAgent({ id: 'q', provider: 'openai', base_url: `${fake.url}/v1`, model: 'm', first_token_timeout_ms: 150 });
    await assert.rejects(
      agent.runGuarded({ system: 's', history: [], userText: 'x', toolbox: null, onText: () => {} }),
      (e) => e.kind === 'timeout' && /не начал отвечать/.test(e.message),
    );
  } finally {
    await fake.close();
  }
});

function anthropicEvents(blocks, stopReason) {
  const ev = [{ event: 'message_start', data: { type: 'message_start', message: { id: 'msg_1', type: 'message', role: 'assistant', model: 'claude-test', content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 5, output_tokens: 1 } } } }];
  blocks.forEach((b, i) => {
    if (b.type === 'text') {
      ev.push({ event: 'content_block_start', data: { type: 'content_block_start', index: i, content_block: { type: 'text', text: '' } } });
      for (const part of b.parts) ev.push({ event: 'content_block_delta', data: { type: 'content_block_delta', index: i, delta: { type: 'text_delta', text: part } } });
    } else {
      ev.push({ event: 'content_block_start', data: { type: 'content_block_start', index: i, content_block: { type: 'tool_use', id: b.id, name: b.name, input: {} } } });
      ev.push({ event: 'content_block_delta', data: { type: 'content_block_delta', index: i, delta: { type: 'input_json_delta', partial_json: JSON.stringify(b.input) } } });
    }
    ev.push({ event: 'content_block_stop', data: { type: 'content_block_stop', index: i } });
  });
  ev.push({ event: 'message_delta', data: { type: 'message_delta', delta: { stop_reason: stopReason, stop_sequence: null }, usage: { output_tokens: 10 } } });
  ev.push({ event: 'message_stop', data: { type: 'message_stop' } });
  return ev;
}

test('Anthropic: стриминг текста, tool_use → tool_result → финальный ответ', async () => {
  const fake = await fakeHttp((req, res, body) => {
    assert.equal(req.headers['x-api-key'], 'sk-test');
    const last = body.messages[body.messages.length - 1];
    if (Array.isArray(last.content) && last.content[0]?.type === 'tool_result') {
      return sse(res, anthropicEvents([{ type: 'text', parts: ['Поставил ', 'громкость.'] }], 'end_turn'));
    }
    return sse(res, anthropicEvents([
      { type: 'text', parts: ['Хорошо.'] },
      { type: 'tool_use', id: 'toolu_1', name: 'set_volume', input: { level: 25 } },
    ], 'tool_use'));
  });
  try {
    const { agent, toolbox, registry } = makeAgent({ id: 'claude', provider: 'anthropic', base_url: fake.url, model: 'claude-test', api_key: 'sk-test' });
    const session = { volume: 50, setVolume(v) { this.volume = v; } };
    let streamed = '';
    const res = await agent.runGuarded({
      system: 'sys',
      history: [],
      userText: 'Сделай потише',
      toolbox,
      toolContext: { registry, agentId: 'claude', depth: 0, session },
      onText: (d) => { streamed += d; },
    });
    assert.equal(res.text, 'Хорошо. Поставил громкость.');
    assert.equal(streamed, res.text);
    assert.equal(session.volume, 25);
    const reqs = fake.requests.filter((r) => r.url === '/v1/messages');
    assert.equal(reqs.length, 2);
    assert.equal(reqs[0].body.system, 'sys');
    assert.ok(reqs[0].body.tools.some((t) => t.name === 'set_volume' && t.input_schema));
    const toolResult = reqs[1].body.messages.at(-1).content[0];
    assert.deepEqual([toolResult.type, toolResult.tool_use_id, toolResult.content], ['tool_result', 'toolu_1', 'Громкость 25%']);
  } finally {
    await fake.close();
  }
});

test('Anthropic без ключа → понятная ошибка auth', async () => {
  const { agent } = makeAgent({ id: 'claude', provider: 'anthropic', model: 'm', api_key: '' });
  assert.equal((await agent.health()).ok, false);
  await assert.rejects(agent.runGuarded({ system: 's', history: [], userText: 'x', onText: () => {} }), (e) => e.kind === 'auth');
});
