// «Мозг» колонки: речь → текст → выбор агента → ответ (стриминг) → речь.
import { performance } from 'node:perf_hooks';
import { SentenceSplitter } from './text/sentences.js';
import { cleanForSpeech } from './text/clean.js';
import { SpeechStreamer } from './speech/streamer.js';
import { isLikelyHallucination } from './speech/stt.js';
import { pcmBufferToInt16 } from './audio/wav.js';
import { normalizePeak } from './audio/resample.js';
import { AgentError } from './agents/base.js';

const now = () => performance.now();
const ms = (a, b) => (a && b ? Math.round(b - a) : null);

export class Assistant {
  constructor({ config, registry, router, toolbox, stt, tts, store, timers, logger }) {
    Object.assign(this, { config, registry, router, toolbox, stt, tts, store, timers, log: logger });
  }

  // ------------------------------------------------------------------ системный промпт и история

  buildSystem(agent) {
    const a = this.config.assistant;
    const tz = this.config.server.timezone;
    let s = a.persona.replaceAll('{assistant}', a.name).replaceAll('{agent}', agent.name);
    if (agent.cfg.system) s += `\n${agent.cfg.system}`;
    const others = this.registry.list().filter((x) => x.id !== agent.id && x.available);
    if (others.length && this.config.tools.enabled.includes('ask_agent') && agent.toolsEnabled) {
      s += `\nЕсли вопрос лучше задать другому агенту, используй инструмент ask_agent. Другие агенты: ${others.map((x) => `${x.name} — ${x.description || 'универсальный'}`).join('; ')}.`;
    }
    const nowStr = new Intl.DateTimeFormat('ru-RU', { timeZone: tz, dateStyle: 'full', timeStyle: 'short' }).format(new Date());
    s += `\nСейчас: ${nowStr} (${tz}).`;
    return s;
  }

  _historyFor(session) {
    if (!session) return [];
    const idleMs = this.config.assistant.idle_reset_minutes * 60000;
    if (session.lastTurnAt && Date.now() - session.lastTurnAt > idleMs) session.history = [];
    return session.history.slice(-this.config.assistant.history_turns * 2);
  }

  _remember(session, userText, reply) {
    if (!session || !reply) return;
    session.history.push({ role: 'user', content: userText }, { role: 'assistant', content: reply });
    const max = this.config.assistant.history_turns * 2;
    if (session.history.length > max) session.history.splice(0, session.history.length - max);
    session.lastTurnAt = Date.now();
  }

  // ------------------------------------------------------------------ управление ходами

  /** Отменяет текущий ход (перебивание, новая команда, отключение). */
  cancelTurn(session, reason = 'отменено') {
    if (session.turn) {
      session.turn.ctrl.abort(new AgentError(reason, { kind: 'aborted' }));
      session.turn = null;
    }
  }

  _startTurn(session, fn) {
    this.cancelTurn(session, 'перебили новой командой');
    const ctrl = new AbortController();
    const id = ++session.turnSeq;
    const turn = { id, ctrl, promise: null };
    session.turn = turn;
    turn.promise = (async () => {
      try {
        await fn(ctrl.signal, id);
      } catch (e) {
        if (!ctrl.signal.aborted) {
          this.log.error(`[${session.deviceId}] ход упал: ${e.stack || e.message}`);
          session.send({ type: 'error', message: 'Внутренняя ошибка сервера' });
          session.setState('idle');
        }
      } finally {
        if (session.turn === turn) session.turn = null;
      }
    })();
    return turn.promise;
  }

  /** Колонка прислала фразу (PCM16 моно). */
  handleUtterance(session, pcmBuffer, rate) {
    return this._startTurn(session, async (signal, turnId) => {
      const t0 = now();
      const samples = normalizePeak(pcmBufferToInt16(pcmBuffer));
      const seconds = samples.length / rate;
      if (seconds < 0.3) {
        session.send({ type: 'error', message: 'Слишком коротко' });
        session.setState('idle');
        return;
      }
      session.setState('thinking');
      let text;
      try {
        text = await this.stt.transcribe(samples, rate, { signal });
      } catch (e) {
        if (signal.aborted) return;
        this.log.error(`[${session.deviceId}] STT: ${e.message}`);
        session.send({ type: 'error', message: 'Распознавание речи недоступно' });
        await this.say(session, 'Не могу распознать речь: сервер распознавания не отвечает.', { signal, turnId });
        return;
      }
      const tStt = now();
      this.log.debug(`[${session.deviceId}] STT ${seconds.toFixed(1)} с аудио → «${text}» за ${ms(t0, tStt)} мс`);
      if (isLikelyHallucination(text)) {
        session.send({ type: 'transcript', text: '' });
        await this.say(session, 'Не расслышал. Повтори, пожалуйста.', { signal, turnId });
        return;
      }
      await this._runTurn(session, { text, t0, sttMs: ms(t0, tStt), signal, turnId });
    });
  }

  /** Текстовый запрос (симулятор, веб-панель) — тот же путь, только без STT. */
  handleText(session, text, { agent } = {}) {
    return this._startTurn(session, async (signal, turnId) => {
      session.setState('thinking');
      await this._runTurn(session, { text, t0: now(), sttMs: null, signal, turnId, forcedAgent: agent });
    });
  }

  /** Сказать фразу от имени колонки (таймер, сообщение из панели). Ждёт окончания текущего ответа. */
  async announce(session, text, { alert } = {}) {
    if (session.turn?.promise) await session.turn.promise.catch(() => {});
    if (alert) session.send({ type: 'alert', kind: alert, text });
    return this._startTurn(session, async (signal, turnId) => {
      await this.say(session, text, { signal, turnId, showText: true });
    });
  }

  /** Озвучить системную фразу. */
  async say(session, text, { signal, turnId, showText = true } = {}) {
    const clean = cleanForSpeech(text);
    if (showText) session.send({ type: 'reply_delta', text: clean, system: true });
    const streamer = new SpeechStreamer({ tts: this.tts, session, rate: session.playRate, signal, logger: this.log, turnId });
    streamer.say(clean);
    await streamer.finish();
    if (showText) session.send({ type: 'reply_end', text: clean, system: true });
    if (!streamer.hasAudio && !signal?.aborted) session.setState('idle');
  }

  // ------------------------------------------------------------------ основной ход

  async _runTurn(session, { text, t0, sttMs, signal, turnId, forcedAgent }) {
    const log = this.log;
    session.send({ type: 'transcript', text });

    const parsed = this.router.parse(text);
    if (parsed?.kind === 'reset') {
      session.history = [];
      this.store.saveTurn({ device_id: session.deviceId, ts: Date.now(), route_reason: 'command', user_text: text, reply_text: '[новый разговор]' });
      await this.say(session, 'Хорошо, начинаем разговор заново.', { signal, turnId });
      return;
    }
    if (parsed?.kind === 'switch' && !forcedAgent) {
      session.setAgent(parsed.agentId);
      const a = this.registry.get(parsed.agentId);
      const phrase = a ? `${a.name} на связи.${a.available ? '' : ' Но сейчас он недоступен — отвечать будут запасные агенты.'}` : 'Включил автоматический выбор агента.';
      this.store.saveTurn({ device_id: session.deviceId, ts: Date.now(), agent_id: parsed.agentId, route_reason: 'command', user_text: text, reply_text: phrase });
      await this.say(session, phrase, { signal, turnId });
      return;
    }

    const choice = forcedAgent && this.registry.has(forcedAgent)
      ? { agentId: forcedAgent, text, reason: 'manual' }
      : await this.router.choose({ text, deviceAgent: session.agent, signal });
    if (signal.aborted) return;

    const result = await this._answer({
      session,
      userText: choice.text,
      primary: choice.agentId,
      reason: choice.reason,
      signal,
      turnId,
      speak: true,
    });
    if (signal.aborted) return;

    const total = now();
    this._remember(session, choice.text, result.reply);
    session.lastAgentUsed = result.agentId;
    this.store.saveTurn({
      device_id: session.deviceId,
      ts: Date.now(),
      agent_id: result.agentId,
      route_reason: result.reason,
      user_text: text,
      reply_text: result.reply,
      tools: result.toolCalls,
      stt_ms: sttMs,
      first_token_ms: ms(t0, result.firstTokenAt),
      first_audio_ms: ms(t0, result.firstAudioAt),
      total_ms: ms(t0, total),
      error: result.error,
    });
    log.info(`[${session.deviceId}] «${text}» → ${result.agentId ?? '—'} (${result.reason})`
      + `${sttMs != null ? ` | STT ${sttMs} мс` : ''} | 1-й токен ${ms(t0, result.firstTokenAt) ?? '—'} мс`
      + ` | 1-й звук ${ms(t0, result.firstAudioAt) ?? '—'} мс | всего ${((total - t0) / 1000).toFixed(1)} с`
      + `${result.error ? ` | ошибка: ${result.error}` : ''}`);
  }

  /**
   * Получает ответ агента (с запасными) и, если есть колонка, озвучивает его по предложениям.
   * Используется и для голоса, и для REST /api/ask (session = null).
   */
  async _answer({ session, userText, primary, reason, signal, turnId, speak }) {
    const candidates = this.router.candidates(primary);
    const splitter = new SentenceSplitter();
    let firstAudioAt = null;
    const streamer = speak && session
      ? new SpeechStreamer({ tts: this.tts, session, rate: session.playRate, signal, logger: this.log, turnId, onFirstAudio: () => { firstAudioAt = now(); } })
      : null;
    const speakOut = (sentence) => {
      const clean = cleanForSpeech(sentence);
      if (!clean) return;
      session?.send({ type: 'reply_delta', text: `${clean} ` });
      streamer?.say(clean);
    };

    let reply = '';
    let firstTokenAt = null;
    let used = null;
    let usedReason = reason;
    let toolCalls = [];
    let error = null;

    for (let i = 0; i < candidates.length; i++) {
      const agent = this.registry.get(candidates[i]);
      const why = i === 0 ? (candidates[0] === primary ? reason : 'fallback') : 'fallback';
      session?.send({ type: 'agent', id: agent.id, name: agent.name, color: agent.color, reason: why });
      let emitted = false;
      try {
        const toolContext = {
          session,
          agentId: agent.id,
          agentName: agent.name,
          depth: 0,
          registry: this.registry,
          timers: this.timers,
          config: this.config,
          signal,
          baseSystem: this.config.assistant.persona.replaceAll('{assistant}', this.config.assistant.name),
          onDelegate: (target) => session?.send({ type: 'delegate', from: agent.id, to: target.id, name: target.name, color: target.color }),
        };
        const res = await agent.runGuarded({
          system: this.buildSystem(agent),
          history: this._historyFor(session),
          userText,
          toolbox: this.toolbox,
          toolContext,
          signal,
          onText: (d) => {
            if (!d) return;
            if (!firstTokenAt) firstTokenAt = now();
            emitted = true;
            reply += d;
            for (const s of splitter.push(d)) speakOut(s);
          },
        });
        used = agent.id;
        usedReason = why;
        toolCalls = res.toolCalls ?? [];
        if (!reply.trim() && toolCalls.length) {
          // Модель выполнила действие и промолчала — скажем результат последнего инструмента.
          const last = String(toolCalls[toolCalls.length - 1].result ?? '');
          reply = last;
          for (const s of splitter.push(`${last}. `)) speakOut(s);
        }
        break;
      } catch (e) {
        if (signal?.aborted || e.aborted) return { agentId: used, reason: usedReason, reply, toolCalls, error: 'отменено', firstTokenAt, firstAudioAt };
        this.log.warn(`агент ${agent.id} не справился: ${e.message}`);
        if (['connection', 'timeout', 'auth'].includes(e.kind)) {
          agent.status = { ...agent.status, ok: false, error: e.message, checkedAt: Date.now() };
        }
        if (emitted) {
          error = e.message;
          used = agent.id;
          speakOut(splitter.flush().join(' '));
          speakOut('Извини, ответ оборвался.');
          break;
        }
        if (i === candidates.length - 1) {
          error = e.message;
          const msg = candidates.length > 1
            ? 'Ни один агент сейчас не отвечает. Проверь сервер с моделями.'
            : `${agent.name} сейчас не отвечает: ${e.message}.`;
          session?.send({ type: 'error', message: e.message });
          reply = msg;
          speakOut(msg);
        }
      }
    }
    for (const s of splitter.flush()) speakOut(s);
    if (streamer) await streamer.finish();
    const cleanReply = cleanForSpeech(reply);
    if (session && !signal?.aborted) {
      session.send({ type: 'reply_end', text: cleanReply, agent: used });
      if (!streamer?.hasAudio) session.setState('idle');
      if (streamer?.errors.length && !streamer.hasAudio) session.send({ type: 'error', message: 'Синтез речи недоступен' });
    }
    return { agentId: used, reason: usedReason, reply: cleanReply, toolCalls, error, firstTokenAt, firstAudioAt };
  }

  /** Текстовый вопрос без колонки (REST). */
  async ask({ text, agent, signal }) {
    const t0 = now();
    const parsed = this.router.parse(text);
    const choice = agent && this.registry.has(agent)
      ? { agentId: agent, text, reason: 'manual' }
      : parsed?.kind === 'address'
        ? { agentId: parsed.agentId, text: parsed.text, reason: 'voice' }
        : await this.router.choose({ text, deviceAgent: 'auto', signal });
    const r = await this._answer({ session: null, userText: choice.text, primary: choice.agentId, reason: choice.reason, signal, speak: false });
    this.store.saveTurn({
      device_id: 'web', ts: Date.now(), agent_id: r.agentId, route_reason: r.reason, user_text: text, reply_text: r.reply,
      tools: r.toolCalls, first_token_ms: ms(t0, r.firstTokenAt), total_ms: ms(t0, now()), error: r.error,
    });
    return { agent: r.agentId, reason: r.reason, reply: r.reply, toolCalls: r.toolCalls, error: r.error, ms: ms(t0, now()) };
  }
}
