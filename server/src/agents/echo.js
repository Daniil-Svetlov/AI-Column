// Агент-«эхо» без нейросети: проверить колонку, звук и инструменты, когда LLM ещё не поднята.
import { Agent } from './base.js';
import { normalize } from '../text/clean.js';

const sleep = (ms, signal) => new Promise((resolve, reject) => {
  const t = setTimeout(resolve, ms);
  signal?.addEventListener('abort', () => { clearTimeout(t); reject(signal.reason ?? new Error('aborted')); }, { once: true });
});

export class EchoAgent extends Agent {
  async run({ userText, toolbox, toolContext, onText, signal, markStarted }) {
    const toolCalls = [];
    const n = normalize(userText);
    let prefix = '';

    if (toolbox && this.toolsEnabled) {
      const names = new Set(toolbox.definitions(toolContext).map((d) => d.name));
      const vol = n.match(/громкость\D{0,20}(\d{1,3})/);
      if (vol && names.has('set_volume')) {
        markStarted?.();
        const args = { level: Number(vol[1]) };
        const result = await toolbox.execute('set_volume', args, toolContext);
        toolCalls.push({ name: 'set_volume', args, result });
        prefix = `${result}. `;
      } else if (/(который час|сколько времени)/.test(n) && names.has('get_datetime')) {
        markStarted?.();
        const result = await toolbox.execute('get_datetime', {}, toolContext);
        toolCalls.push({ name: 'get_datetime', args: {}, result });
        prefix = `${result}. `;
      }
    }

    const reply = `${prefix}Я ${this.name}, тестовый агент. Ты сказал: ${userText}`;
    const delay = Number(this.cfg.delay_ms ?? 15);
    let text = '';
    for (const word of reply.split(/(\s+)/)) {
      if (!word) continue;
      await sleep(delay, signal);
      text += word;
      onText(word);
    }
    return { text, toolCalls };
  }
}
