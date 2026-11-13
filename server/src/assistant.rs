//! «Мозг» колонки: речь → текст → выбор агента → ответ (стриминг) → речь.
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::agents::router::{Choice, Parsed};
use crate::agents::{Agent, ErrorKind, Msg, Registry, Router, RunParams, ToolCall, ToolContext, Toolbox, now_ms};
use crate::audio::{bytes_to_pcm, normalize_peak};
use crate::config::Config;
use crate::logger::Logger;
use crate::session::{DeviceSession, TurnHandle};
use crate::speech::{SpeechStreamer, Stt, Tts, is_likely_hallucination};
use crate::store::{Store, Turn};
use crate::text::{SentenceSplitter, clean_for_speech};
use crate::timers::TimerManager;

fn ms(a: Instant, b: Option<Instant>) -> Option<i64> {
    b.map(|b| b.saturating_duration_since(a).as_millis() as i64)
}

/// Итог ответа агента (с учётом запасных).
pub struct Answer {
    pub agent_id: Option<String>,
    pub reason: String,
    pub reply: String,
    pub tool_calls: Vec<ToolCall>,
    pub error: Option<String>,
    pub first_token_at: Option<Instant>,
    pub first_audio_at: Option<Instant>,
}

pub struct Assistant {
    pub config: Arc<Config>,
    pub registry: Arc<Registry>,
    pub router: Arc<Router>,
    pub toolbox: Arc<Toolbox>,
    pub stt: Arc<Stt>,
    pub tts: Arc<Tts>,
    pub store: Arc<Store>,
    pub timers: Arc<TimerManager>,
    pub log: Logger,
}

impl Assistant {
    // ─────────────────────────────────────────── системный промпт и история

    fn base_persona(&self) -> String {
        self.config.assistant.persona.replace("{assistant}", &self.config.assistant.name)
    }

    pub fn build_system(&self, agent: &Agent) -> String {
        let tz = &self.config.server.timezone;
        let mut s = self.base_persona().replace("{agent}", agent.name());
        if !agent.cfg.system.is_empty() {
            s += &format!("\n{}", agent.cfg.system);
        }
        let others: Vec<_> = self.registry.list().iter().filter(|x| x.id() != agent.id() && x.available()).collect();
        if !others.is_empty() && self.config.tools.enabled.iter().any(|t| t == "ask_agent") && agent.tools_enabled() {
            let list = others
                .iter()
                .map(|x| format!("{} — {}", x.name(), if x.description().is_empty() { "универсальный" } else { x.description() }))
                .collect::<Vec<_>>()
                .join("; ");
            s += &format!("\nЕсли вопрос лучше задать другому агенту, используй инструмент ask_agent. Другие агенты: {list}.");
        }
        let (date, time) = crate::agents::tools::ru_now(tz);
        s += &format!("\nСейчас: {date} в {time} ({tz}).");
        s
    }

    fn history_for(&self, s: &DeviceSession) -> Vec<Msg> {
        let a = &self.config.assistant;
        let mut st = s.state();
        if st.last_turn_at > 0 && now_ms() - st.last_turn_at > a.idle_reset_minutes as i64 * 60000 {
            st.history.clear();
        }
        let n = st.history.len();
        st.history[n.saturating_sub(a.history_turns * 2)..].to_vec()
    }

    fn remember(&self, s: &DeviceSession, user_text: &str, reply: &str) {
        if reply.is_empty() {
            return;
        }
        let max = self.config.assistant.history_turns * 2;
        let mut st = s.state();
        st.history.push(Msg::new("user", user_text));
        st.history.push(Msg::new("assistant", reply));
        let n = st.history.len();
        if n > max {
            st.history.drain(..n - max);
        }
        st.last_turn_at = now_ms();
    }

    // ─────────────────────────────────────────── управление ходами

    /// Отменяет текущий ход (перебивание, новая команда, отключение).
    pub fn cancel_turn(&self, s: &DeviceSession, reason: &str) {
        if let Some(t) = s.state().turn.take() {
            self.log.debug(format!("[{}] ход #{} отменён: {reason}", s.device_id(), t.id));
            t.cancel.cancel();
        }
    }

    fn start_turn<F, Fut>(self: &Arc<Self>, s: &Arc<DeviceSession>, f: F) -> watch::Receiver<bool>
    where
        F: FnOnce(Arc<Self>, Arc<DeviceSession>, CancellationToken, u64) -> Fut,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.cancel_turn(s, "перебили новой командой");
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = watch::channel(false);
        let id = {
            let mut st = s.state();
            st.turn_seq += 1;
            let id = st.turn_seq;
            st.turn = Some(TurnHandle { id, cancel: cancel.clone(), done: done_rx.clone() });
            id
        };
        let fut = f(self.clone(), s.clone(), cancel.clone(), id);
        let (me, s) = (self.clone(), s.clone());
        tokio::spawn(async move {
            if let Err(e) = fut.await {
                if !cancel.is_cancelled() {
                    me.log.error(format!("[{}] ход упал: {e}", s.device_id()));
                    s.send(json!({"type": "error", "message": "Внутренняя ошибка сервера"}));
                    s.set_state("idle");
                }
            }
            {
                let mut st = s.state();
                if st.turn.as_ref().is_some_and(|t| t.id == id) {
                    st.turn = None;
                }
            }
            let _ = done_tx.send(true);
        });
        done_rx
    }

    /// Колонка прислала фразу (PCM16LE моно).
    pub fn handle_utterance(self: &Arc<Self>, s: &Arc<DeviceSession>, pcm: Vec<u8>, rate: u32) -> watch::Receiver<bool> {
        self.start_turn(s, move |me, s, cancel, turn_id| async move {
            let t0 = Instant::now();
            let samples = normalize_peak(bytes_to_pcm(&pcm));
            let seconds = samples.len() as f64 / rate as f64;
            if seconds < 0.3 {
                s.send(json!({"type": "error", "message": "Слишком коротко"}));
                s.set_state("idle");
                return Ok(());
            }
            s.set_state("thinking");
            let text = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                r = me.stt.transcribe(&samples, rate) => r,
            };
            let text = match text {
                Ok(t) => t,
                Err(e) => {
                    me.log.error(format!("[{}] STT: {e}", s.device_id()));
                    s.send(json!({"type": "error", "message": "Распознавание речи недоступно"}));
                    me.say(&s, "Не могу распознать речь: сервер распознавания не отвечает.", &cancel, turn_id, true).await;
                    return Ok(());
                }
            };
            let stt_ms = t0.elapsed().as_millis() as i64;
            me.log.debug(format!("[{}] STT {seconds:.1} с аудио → «{text}» за {stt_ms} мс", s.device_id()));
            if is_likely_hallucination(&text) {
                s.send(json!({"type": "transcript", "text": ""}));
                me.say(&s, "Не расслышал. Повтори, пожалуйста.", &cancel, turn_id, true).await;
                return Ok(());
            }
            me.run_turn(&s, &text, t0, Some(stt_ms), &cancel, turn_id, None).await;
            Ok(())
        })
    }

    /// Текстовый запрос (симулятор, веб-панель) — тот же путь, только без STT.
    pub fn handle_text(self: &Arc<Self>, s: &Arc<DeviceSession>, text: &str, agent: Option<String>) -> watch::Receiver<bool> {
        let text = text.to_string();
        self.start_turn(s, move |me, s, cancel, turn_id| async move {
            s.set_state("thinking");
            me.run_turn(&s, &text, Instant::now(), None, &cancel, turn_id, agent).await;
            Ok(())
        })
    }

    /// Сказать фразу от имени колонки (таймер, сообщение из панели). Ждёт окончания текущего ответа.
    pub async fn announce(self: &Arc<Self>, s: &Arc<DeviceSession>, text: &str, alert: Option<&str>) {
        let done = s.state().turn.as_ref().map(|t| t.done.clone());
        if let Some(mut d) = done {
            let _ = d.wait_for(|v| *v).await;
        }
        if let Some(kind) = alert {
            s.send(json!({"type": "alert", "kind": kind, "text": text}));
        }
        let text = text.to_string();
        let mut done = self.start_turn(s, move |me, s, cancel, turn_id| async move {
            me.say(&s, &text, &cancel, turn_id, true).await;
            Ok(())
        });
        let _ = done.wait_for(|v| *v).await;
    }

    /// Озвучить системную фразу.
    pub async fn say(&self, s: &Arc<DeviceSession>, text: &str, cancel: &CancellationToken, turn_id: u64, show_text: bool) {
        let clean = clean_for_speech(text);
        if show_text {
            s.send(json!({"type": "reply_delta", "text": clean, "system": true}));
        }
        let mut streamer = SpeechStreamer::new(self.tts.clone(), s.clone(), s.play_rate(), cancel.clone(), self.log.clone(), turn_id);
        streamer.say(&clean);
        streamer.finish().await;
        if show_text {
            s.send(json!({"type": "reply_end", "text": clean, "system": true}));
        }
        if !streamer.has_audio() && !cancel.is_cancelled() {
            s.set_state("idle");
        }
    }

    // ─────────────────────────────────────────── основной ход

    #[allow(clippy::too_many_arguments)]
    async fn run_turn(
        self: &Arc<Self>,
        s: &Arc<DeviceSession>,
        text: &str,
        t0: Instant,
        stt_ms: Option<i64>,
        cancel: &CancellationToken,
        turn_id: u64,
        forced_agent: Option<String>,
    ) {
        s.send(json!({"type": "transcript", "text": text}));
        let dev = s.device_id().to_string();

        match self.router.parse(text) {
            Some(Parsed::Reset) => {
                s.state().history.clear();
                let mut t = Turn::new(&dev);
                t.route_reason = Some("command".into());
                t.user_text = Some(text.into());
                t.reply_text = Some("[новый разговор]".into());
                self.store.save_turn(t);
                self.say(s, "Хорошо, начинаем разговор заново.", cancel, turn_id, true).await;
                return;
            }
            Some(Parsed::Switch(id)) if forced_agent.is_none() => {
                s.set_agent(&id, true);
                let phrase = match self.registry.get(&id) {
                    Some(a) => format!(
                        "{} на связи.{}",
                        a.name(),
                        if a.available() { "" } else { " Но сейчас он недоступен — отвечать будут запасные агенты." }
                    ),
                    None => "Включил автоматический выбор агента.".into(),
                };
                let mut t = Turn::new(&dev);
                t.agent_id = Some(id);
                t.route_reason = Some("command".into());
                t.user_text = Some(text.into());
                t.reply_text = Some(phrase.clone());
                self.store.save_turn(t);
                self.say(s, &phrase, cancel, turn_id, true).await;
                return;
            }
            _ => {}
        }

        let choice = match forced_agent.filter(|a| self.registry.has(a)) {
            Some(a) => Choice { agent_id: a, text: text.into(), reason: "manual" },
            None => self.router.choose(text, &s.agent(), cancel).await,
        };
        if cancel.is_cancelled() {
            return;
        }
        let r = self.answer(Some(s), &choice.text, &choice.agent_id, choice.reason, cancel, turn_id).await;
        if cancel.is_cancelled() {
            return;
        }
        let total = Instant::now();
        if let Some(id) = &r.agent_id {
            // в историю — только то, что реально ответил агент
            self.remember(s, &choice.text, &r.reply);
            s.state().last_agent_used = Some(id.clone());
        }
        let mut t = Turn::new(&dev).tools(&r.tool_calls);
        t.agent_id = r.agent_id.clone();
        t.route_reason = Some(r.reason.clone());
        t.user_text = Some(text.into());
        t.reply_text = Some(r.reply.clone());
        t.stt_ms = stt_ms;
        t.first_token_ms = ms(t0, r.first_token_at);
        t.first_audio_ms = ms(t0, r.first_audio_at);
        t.total_ms = ms(t0, Some(total));
        t.error = r.error.clone();
        self.store.save_turn(t);
        let opt = |v: Option<i64>| v.map(|v| v.to_string()).unwrap_or_else(|| "—".into());
        self.log.info(format!(
            "[{dev}] «{text}» → {} ({}){} | 1-й токен {} мс | 1-й звук {} мс | всего {:.1} с{}",
            r.agent_id.as_deref().unwrap_or("—"),
            r.reason,
            stt_ms.map(|v| format!(" | STT {v} мс")).unwrap_or_default(),
            opt(ms(t0, r.first_token_at)),
            opt(ms(t0, r.first_audio_at)),
            (total - t0).as_secs_f64(),
            r.error.as_ref().map(|e| format!(" | ошибка: {e}")).unwrap_or_default(),
        ));
    }

    /// Ответ агента (с запасными); если есть колонка — озвучка по предложениям.
    /// Используется и для голоса, и для REST /api/ask (session = None).
    pub async fn answer(
        self: &Arc<Self>,
        session: Option<&Arc<DeviceSession>>,
        user_text: &str,
        primary: &str,
        reason: &str,
        cancel: &CancellationToken,
        turn_id: u64,
    ) -> Answer {
        let candidates = self.router.candidates(primary);
        let mut splitter = SentenceSplitter::default();
        let mut streamer = session
            .map(|s| SpeechStreamer::new(self.tts.clone(), s.clone(), s.play_rate(), cancel.clone(), self.log.clone(), turn_id));
        let history = session.map(|s| self.history_for(s)).unwrap_or_default();

        let mut reply = String::new();
        let mut first_token_at: Option<Instant> = None;
        let mut used: Option<String> = None;
        let mut used_reason = reason.to_string();
        let mut tool_calls = vec![];
        let mut error: Option<String> = None;

        let speak_out = |sentence: &str, streamer: &Option<SpeechStreamer>| {
            let clean = clean_for_speech(sentence);
            if clean.is_empty() {
                return;
            }
            if let Some(s) = session {
                s.send(json!({"type": "reply_delta", "text": format!("{clean} ")}));
            }
            if let Some(st) = streamer {
                st.say(&clean);
            }
        };

        for (i, id) in candidates.iter().enumerate() {
            let Some(agent) = self.registry.get(id) else { continue };
            let why = if i == 0 && candidates[0] == primary { reason.to_string() } else { "fallback".to_string() };
            if let Some(s) = session {
                s.send(json!({"type": "agent", "id": agent.id(), "name": agent.name(), "color": agent.color(), "reason": why}));
            }
            let from = agent.id().to_string();
            let sess = session.cloned();
            let ctx = ToolContext {
                session: session.cloned(),
                agent_id: agent.id().into(),
                agent_name: agent.name().into(),
                depth: 0,
                registry: Some(self.registry.clone()),
                timers: Some(self.timers.clone()),
                cancel: cancel.clone(),
                base_system: self.base_persona(),
                on_delegate: Some(Arc::new(move |t: &Agent| {
                    if let Some(s) = &sess {
                        s.send(json!({"type": "delegate", "from": from, "to": t.id(), "name": t.name(), "color": t.color()}));
                    }
                })),
            };
            let system = self.build_system(&agent);
            let mut emitted = false;
            let res = {
                let mut on_text = |d: &str| {
                    if d.is_empty() {
                        return;
                    }
                    first_token_at.get_or_insert_with(Instant::now);
                    emitted = true;
                    reply.push_str(d);
                    for sentence in splitter.push(d) {
                        speak_out(&sentence, &streamer);
                    }
                };
                agent
                    .run_guarded(
                        RunParams {
                            system: &system,
                            history: &history,
                            user_text,
                            toolbox: Some(&self.toolbox),
                            ctx: Some(&ctx),
                            max_tokens: None,
                        },
                        &mut on_text,
                        cancel,
                    )
                    .await
            };
            match res {
                Ok(r) => {
                    used = Some(agent.id().into());
                    used_reason = why;
                    tool_calls = r.tool_calls;
                    if reply.trim().is_empty() {
                        if let Some(last) = tool_calls.last() {
                            // Модель выполнила действие и промолчала — скажем результат последнего инструмента.
                            reply = last.result.clone();
                            for sentence in splitter.push(&format!("{reply}. ")) {
                                speak_out(&sentence, &streamer);
                            }
                        }
                    }
                    break;
                }
                Err(e) => {
                    if cancel.is_cancelled() || e.is_aborted() {
                        return Answer {
                            agent_id: used,
                            reason: used_reason,
                            reply,
                            tool_calls,
                            error: Some("отменено".into()),
                            first_token_at,
                            first_audio_at: streamer.as_ref().and_then(|s| s.first_audio_at()),
                        };
                    }
                    self.log.warn(format!("агент {} не справился: {e}", agent.id()));
                    if matches!(e.kind, ErrorKind::Connection | ErrorKind::Timeout | ErrorKind::Auth) {
                        agent.mark_down(&e.message);
                    }
                    if emitted {
                        error = Some(e.message.clone());
                        used = Some(agent.id().into());
                        speak_out(&splitter.flush().join(" "), &streamer);
                        speak_out("Извини, ответ оборвался.", &streamer);
                        break;
                    }
                    if i == candidates.len() - 1 {
                        error = Some(e.message.clone());
                        let msg = if candidates.len() > 1 {
                            "Ни один агент сейчас не отвечает. Проверь сервер с моделями.".to_string()
                        } else {
                            format!("{} сейчас не отвечает: {}.", agent.name(), e.message)
                        };
                        if let Some(s) = session {
                            s.send(json!({"type": "error", "message": e.message}));
                        }
                        reply = msg.clone();
                        speak_out(&msg, &streamer);
                    }
                }
            }
        }
        for sentence in splitter.flush() {
            speak_out(&sentence, &streamer);
        }
        if let Some(st) = streamer.as_mut() {
            st.finish().await;
        }
        let clean_reply = clean_for_speech(&reply);
        if let (Some(s), false) = (session, cancel.is_cancelled()) {
            s.send(json!({"type": "reply_end", "text": clean_reply, "agent": used}));
            let has_audio = streamer.as_ref().is_some_and(|st| st.has_audio());
            if !has_audio {
                s.set_state("idle");
            }
            if streamer.as_ref().is_some_and(|st| st.errors() > 0) && !has_audio {
                s.send(json!({"type": "error", "message": "Синтез речи недоступен"}));
            }
        }
        Answer {
            agent_id: used,
            reason: used_reason,
            reply: clean_reply,
            tool_calls,
            error,
            first_token_at,
            first_audio_at: streamer.as_ref().and_then(|s| s.first_audio_at()),
        }
    }

    /// Текстовый вопрос без колонки (REST).
    pub async fn ask(self: &Arc<Self>, text: &str, agent: Option<&str>, cancel: &CancellationToken) -> Value {
        let t0 = Instant::now();
        let choice = match agent.filter(|a| self.registry.has(a)) {
            Some(a) => Choice { agent_id: a.into(), text: text.into(), reason: "manual" },
            None => match self.router.parse(text) {
                Some(Parsed::Address { agent_id, text }) => Choice { agent_id, text, reason: "voice" },
                _ => self.router.choose(text, "auto", cancel).await,
            },
        };
        let r = self.answer(None, &choice.text, &choice.agent_id, choice.reason, cancel, 0).await;
        let mut t = Turn::new("web").tools(&r.tool_calls);
        t.agent_id = r.agent_id.clone();
        t.route_reason = Some(r.reason.clone());
        t.user_text = Some(text.into());
        t.reply_text = Some(r.reply.clone());
        t.first_token_ms = ms(t0, r.first_token_at);
        t.total_ms = ms(t0, Some(Instant::now()));
        t.error = r.error.clone();
        self.store.save_turn(t);
        json!({
            "agent": r.agent_id,
            "reason": r.reason,
            "reply": r.reply,
            "toolCalls": r.tool_calls,
            "error": r.error,
            "ms": t0.elapsed().as_millis() as i64,
        })
    }
}
