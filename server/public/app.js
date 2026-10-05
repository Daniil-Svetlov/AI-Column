// Веб-панель AI-Column: агенты, колонки, текстовые вопросы, история. Без сборки и зависимостей.
const $ = (sel, root = document) => root.querySelector(sel);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

let token = '';
try { token = localStorage.getItem('aiColumnToken') || ''; } catch { /* приватный режим */ }
$('#token').value = token;

let agents = [];

async function api(path, opts = {}) {
  const res = await fetch(path, {
    ...opts,
    headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(opts.headers || {}) },
  });
  if (res.status === 401) throw new Error('нужен верный AUTH_TOKEN (поле сверху справа)');
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error || `HTTP ${res.status}`);
  return body;
}

function agentName(id) {
  if (!id) return '—';
  if (id === 'auto') return 'Авто';
  return agents.find((a) => a.id === id)?.name ?? id;
}

function agentColor(id) {
  return agents.find((a) => a.id === id)?.color ?? '#888';
}

async function loadHealth() {
  try {
    const h = await fetch('/api/health').then((r) => r.json());
    $('#health').textContent = `v${h.version} · STT: ${h.stt} · TTS: ${h.tts} · колонок онлайн: ${h.devices}${h.auth ? '' : ' · ⚠ без токена'}`;
  } catch {
    $('#health').textContent = 'сервер недоступен';
  }
}

function renderAgents() {
  const box = $('#agents');
  box.innerHTML = agents.map((a) => {
    const st = a.status || {};
    const ok = st.ok === true ? 'ok' : st.ok === false ? 'bad' : '';
    const label = st.ok === true ? `доступен${st.latencyMs != null ? ` · ${st.latencyMs} мс` : ''}` : st.ok === false ? 'недоступен' : 'не проверен';
    return `<article class="card">
      <div class="row"><strong><span class="dot" style="background:${esc(a.color)}"></span>${esc(a.name)}</strong><span class="badge ${ok}">${label}</span></div>
      <div class="meta">${esc(a.provider)} · ${esc(a.model ?? '—')}</div>
      ${a.description ? `<div class="meta">${esc(a.description)}</div>` : ''}
      ${st.error ? `<div class="meta err">${esc(st.error)}</div>` : ''}
      ${a.aliases?.length ? `<div class="meta">голосом: ${a.aliases.map(esc).join(', ')}</div>` : ''}
    </article>`;
  }).join('') || '<p class="meta">Агентов нет — проверь config.yaml</p>';
  const sel = $('#askAgent');
  const cur = sel.value;
  sel.innerHTML = '<option value="">Авто (как колонка)</option>' + agents.map((a) => `<option value="${esc(a.id)}">${esc(a.name)}</option>`).join('');
  sel.value = cur;
}

async function loadAgents() {
  agents = await api('/api/agents');
  renderAgents();
}

function renderDevices(list) {
  const box = $('#devices');
  const hist = $('#histDevice');
  const histCur = hist.value;
  hist.innerHTML = '<option value="">все</option><option value="web">веб-панель</option>' + list.map((d) => `<option>${esc(d.id)}</option>`).join('');
  hist.value = histCur;
  if (!list.length) {
    box.innerHTML = '<p class="meta">Колонок ещё не было. Прошей ESP32 и включи — она появится здесь.</p>';
    return;
  }
  const focused = document.activeElement;
  if (focused && box.contains(focused)) return; // не мешаем вводу
  box.innerHTML = '';
  for (const d of list) {
    const el = $('#deviceTpl').content.firstElementChild.cloneNode(true);
    $('.name', el).textContent = d.id;
    const state = $('.state', el);
    state.textContent = d.online ? ({ idle: 'ждёт', listening: 'слушает', thinking: 'думает', speaking: 'говорит' }[d.state] ?? d.state) : 'не в сети';
    state.classList.add(d.online ? 'ok' : 'bad');
    const bits = [];
    if (d.fw) bits.push(`прошивка ${d.fw}`);
    if (d.battery != null) bits.push(`АКБ ${d.battery}%`);
    if (d.rssi != null) bits.push(`Wi-Fi ${d.rssi} дБм`);
    if (d.lastAgent) bits.push(`последним отвечал ${agentName(d.lastAgent)}`);
    if (!d.online && d.lastSeen) bits.push(`был ${new Date(d.lastSeen).toLocaleString('ru-RU')}`);
    $('.meta', el).textContent = bits.join(' · ') || '—';
    const sel = $('.agent', el);
    sel.innerHTML = '<option value="auto">Авто</option>' + agents.map((a) => `<option value="${esc(a.id)}">${esc(a.name)}</option>`).join('');
    sel.value = d.agent || 'auto';
    sel.disabled = !d.online;
    sel.onchange = () => api(`/api/devices/${encodeURIComponent(d.id)}/agent`, { method: 'POST', body: JSON.stringify({ agent: sel.value }) }).catch(alert);
    const vol = $('.volume', el);
    vol.value = d.volume ?? 60;
    $('.volval', el).textContent = vol.value;
    vol.disabled = !d.online;
    vol.oninput = () => { $('.volval', el).textContent = vol.value; };
    vol.onchange = () => api(`/api/devices/${encodeURIComponent(d.id)}/volume`, { method: 'POST', body: JSON.stringify({ volume: Number(vol.value) }) }).catch(alert);
    const say = $('.say', el);
    $('input', say).disabled = !d.online;
    $('button', say).disabled = !d.online;
    say.onsubmit = (e) => {
      e.preventDefault();
      const input = $('input', say);
      if (!input.value.trim()) return;
      api(`/api/devices/${encodeURIComponent(d.id)}/say`, { method: 'POST', body: JSON.stringify({ text: input.value }) })
        .then(() => { input.value = ''; })
        .catch(alert);
    };
    box.append(el);
  }
}

async function loadDevices() {
  renderDevices(await api('/api/devices'));
}

async function loadHistory() {
  const dev = $('#histDevice').value;
  const rows = await api(`/api/history?limit=50${dev ? `&device=${encodeURIComponent(dev)}` : ''}`);
  $('#history').innerHTML = rows.map((t) => `<tr>
    <td>${new Date(t.ts).toLocaleString('ru-RU')}</td>
    <td>${esc(t.device_id)}</td>
    <td class="who">${t.agent_id ? `<span class="dot" style="background:${esc(agentColor(t.agent_id))}"></span>${esc(agentName(t.agent_id))}` : '—'}<div class="meta">${esc(t.route_reason ?? '')}</div></td>
    <td class="q">${esc(t.user_text)}</td>
    <td class="a">${esc(t.reply_text)}${t.tools?.length ? `<div class="meta">🔧 ${t.tools.map((c) => esc(c.name)).join(', ')}</div>` : ''}${t.error ? `<div class="meta err">${esc(t.error)}</div>` : ''}</td>
    <td>${t.first_audio_ms != null ? `${t.first_audio_ms} мс` : '—'}</td>
  </tr>`).join('') || '<tr><td colspan="6" class="meta">Пока пусто</td></tr>';
}

async function refresh() {
  try {
    await Promise.all([loadHealth(), loadAgents()]);
    await Promise.all([loadDevices(), loadHistory()]);
  } catch (e) {
    $('#health').textContent = e.message;
  }
}

$('#tokenForm').onsubmit = (e) => {
  e.preventDefault();
  token = $('#token').value.trim();
  try { localStorage.setItem('aiColumnToken', token); } catch { /* ок */ }
  refresh();
};

$('#checkAgents').onclick = async (e) => {
  e.target.disabled = true;
  try {
    agents = await api('/api/agents/check', { method: 'POST' });
    renderAgents();
  } catch (err) {
    alert(err.message);
  } finally {
    e.target.disabled = false;
  }
};

$('#askForm').onsubmit = async (e) => {
  e.preventDefault();
  const btn = $('button', e.target);
  const out = $('#askResult');
  btn.disabled = true;
  out.hidden = false;
  out.innerHTML = '<span class="meta">думаю…</span>';
  try {
    const r = await api('/api/ask', { method: 'POST', body: JSON.stringify({ text: $('#askText').value, agent: $('#askAgent').value || undefined }) });
    out.innerHTML = `<div class="who"><span class="dot" style="background:${esc(agentColor(r.agent))}"></span>${esc(agentName(r.agent))} <span class="meta">· ${esc(r.reason)} · ${r.ms} мс</span></div>${esc(r.reply)}`
      + (r.toolCalls?.length ? `<div class="meta">🔧 ${r.toolCalls.map((c) => `${esc(c.name)} → ${esc(c.result)}`).join('<br>')}</div>` : '')
      + (r.error ? `<div class="meta err">${esc(r.error)}</div>` : '');
    loadHistory();
  } catch (err) {
    out.innerHTML = `<span class="err">${esc(err.message)}</span>`;
  } finally {
    btn.disabled = false;
  }
};

$('#histDevice').onchange = loadHistory;

refresh();
setInterval(refresh, 5000);
