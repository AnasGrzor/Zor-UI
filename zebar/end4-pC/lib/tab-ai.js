// AI chat tab (end4's sidebarLeft/AiChat): streaming chat with Gemini, Claude,
// OpenAI, Mistral, OpenRouter or local Ollama models. Keys live in the bridge
// (DPAPI-encrypted), never in the webview.
import { $, esc, icon, md, bridge, bridgeStream, config, openUrl } from './core.js';

const STORE = 'end4-pC.ai.history';
const PROVIDERS = ['gemini', 'anthropic', 'openai', 'mistral', 'openrouter'];
const HELP = [
  ['/key <provider> <key>', `Save an API key (${PROVIDERS.join(', ')}). Without a key it removes it.`],
  ['/model <name>', 'Switch model (or click the model chip).'],
  ['/prompt <text>', 'Set the system prompt. "/prompt" alone resets it.'],
  ['/temp <0-2>', 'Set the temperature.'],
  ['/clear', 'Clear the conversation.'],
];

export async function mountAI(root, { onBusy } = {}) {
  const cfg = await config();
  let models = [], keys = {};
  let model = cfg.ai.model, temperature = cfg.ai.temperature, system = cfg.ai.systemPrompt || '';
  let history = load();
  let proc = null;

  root.innerHTML = `
    <div class="ai-head">
      <button class="chip" id="ai-model">${icon('neurology')}<span class="ellipsis"></span>${icon('expand_more')}</button>
      <span style="flex:1"></span>
      <button class="icon-btn" id="ai-clear" title="Clear chat">${icon('delete_sweep')}</button>
    </div>
    <div class="ai-menu scroll" id="ai-menu" hidden></div>
    <div class="ai-msgs scroll" id="ai-msgs"></div>
    <form class="ai-input" id="ai-form">
      <textarea id="ai-text" rows="1" placeholder="Message the model…  (/ for commands)"></textarea>
      <button class="icon-btn filled" id="ai-send" type="submit">${icon('arrow_upward')}</button>
    </form>`;
  const msgs = $('#ai-msgs', root), text = $('#ai-text', root), menu = $('#ai-menu', root);

  const modelInfo = () => models.find(m => m.id === model) || { id: model, name: model, provider: '?' };
  function renderHead() {
    $('#ai-model span.ellipsis', root).textContent = modelInfo().name;
  }
  function renderMenu() {
    const groups = {};
    for (const m of models) (groups[m.provider] ||= []).push(m);
    menu.innerHTML = Object.entries(groups).map(([p, list]) => `
      <div class="ai-menu-group">${esc(p)}${p !== 'ollama' ? ` · <span class="${keys[p] ? 'ok' : 'warn'}">${keys[p] ? 'key set' : 'no key'}</span>` : ''}</div>
      ${list.map(m => `<button class="ai-menu-item${m.id === model ? ' sel' : ''}" data-model="${esc(m.id)}">${esc(m.name)}<small>${esc(m.id)}</small></button>`).join('')}`).join('')
      || '<div class="muted" style="padding:12px">Loading models…</div>';
  }

  function renderMsgs() {
    if (!history.length) {
      msgs.innerHTML = `<div class="ai-empty">
        ${icon('neurology', { cls: 'big' })}
        <b>Large language models</b>
        <p class="muted">Chat with ${esc(modelInfo().name)}. Type <code>/key gemini YOUR_KEY</code> to add a key, or run Ollama for local models.</p>
        <div class="ai-help">${HELP.map(([c, d]) => `<div><code>${esc(c)}</code><span>${esc(d)}</span></div>`).join('')}</div>
      </div>`;
      return;
    }
    msgs.innerHTML = history.map((m, i) => msgHTML(m, i)).join('');
    msgs.scrollTop = msgs.scrollHeight;
  }
  const msgHTML = (m, i) => m.role === 'note'
    ? `<div class="ai-note">${esc(m.content)}</div>`
    : `<div class="msg ${m.role}${m.error ? ' error' : ''}" data-i="${i}">
         ${m.role === 'assistant' ? `<div class="who">${icon('neurology')}${esc(m.model || '')}</div>` : ''}
         <div class="body">${m.role === 'assistant' ? md(m.content) || '<span class="dots"><i></i><i></i><i></i></span>' : esc(m.content)}</div>
       </div>`;

  function note(t) { history.push({ role: 'note', content: t }); save(); renderMsgs(); }

  async function command(line) {
    const [cmd, ...rest] = line.slice(1).split(/\s+/);
    const arg = rest.join(' ').trim();
    if (cmd === 'clear') { history = []; save(); return renderMsgs(); }
    if (cmd === 'help') return note(HELP.map(h => h.join(' — ')).join('\n'));
    if (cmd === 'key') {
      const [prov, ...k] = rest;
      if (!PROVIDERS.includes(prov) && prov !== 'wallhaven') return note(`Usage: /key <${PROVIDERS.join('|')}> <key>`);
      const r = await bridge('secret-set', prov, k.join(''));
      keys[prov] = !!k.join('');
      return note(r.error ? `Couldn't save key: ${r.error}` : k.length ? `Saved the ${prov} key (encrypted for your Windows account).` : `Removed the ${prov} key.`);
    }
    if (cmd === 'model') {
      const m = models.find(x => x.id === arg) || models.find(x => x.name.toLowerCase().includes(arg.toLowerCase()));
      if (!m) return note(`No model matching "${arg}". Available: ${models.map(x => x.id).join(', ')}`);
      return pickModel(m.id);
    }
    if (cmd === 'prompt') { system = arg; bridge('config-set', 'ai.systemPrompt', JSON.stringify(arg)); return note(arg ? 'System prompt set.' : 'System prompt reset.'); }
    if (cmd === 'temp') {
      const t = Number(arg);
      if (!(t >= 0 && t <= 2)) return note('Temperature must be between 0 and 2.');
      temperature = t; bridge('config-set', 'ai.temperature', String(t)); return note(`Temperature set to ${t}.`);
    }
    note(`Unknown command /${cmd}. Type /help.`);
  }

  function pickModel(id) {
    model = id;
    bridge('config-set', 'ai.model', JSON.stringify(id));
    renderHead(); renderMenu();
    menu.hidden = true;
    note(`Model: ${modelInfo().name}`);
  }

  async function send(content) {
    history.push({ role: 'user', content });
    const reply = { role: 'assistant', content: '', model: modelInfo().name };
    history.push(reply);
    renderMsgs();
    setBusy(true);
    const convo = history.filter(m => (m.role === 'user' || m.role === 'assistant') && !m.error && m !== reply)
      .map(({ role, content }) => ({ role, content })).slice(-30);
    let raf = 0;
    const paint = () => {
      raf = 0;
      const el = msgs.querySelector(`[data-i="${history.indexOf(reply)}"] .body`);
      if (el) el.innerHTML = md(reply.content) || '<span class="dots"><i></i><i></i><i></i></span>';
      msgs.scrollTop = msgs.scrollHeight;
    };
    proc = await bridgeStream('ai-chat', [], ev => {
      if (ev.delta) { reply.content += ev.delta; raf ||= requestAnimationFrame(paint); }
      if (ev.error) { reply.content += (reply.content ? '\n\n' : '') + ev.error; reply.error = true; renderMsgs(); }
    }, {
      stdin: JSON.stringify({ model, messages: convo, system, temperature }),
      onExit: () => { if (!reply.content) { reply.content = 'No response.'; reply.error = true; } save(); renderMsgs(); setBusy(false); },
    });
    if (!proc) setBusy(false);
  }

  function setBusy(b) {
    $('#ai-send', root).innerHTML = icon(b ? 'stop' : 'arrow_upward');
    root.classList.toggle('busy', b);
    if (!b) proc = null;
    onBusy?.(b);
  }

  $('#ai-form', root).addEventListener('submit', e => {
    e.preventDefault();
    if (proc) { proc.kill(); return; }
    const t = text.value.trim();
    if (!t) return;
    text.value = ''; text.style.height = '';
    t.startsWith('/') ? command(t) : send(t);
  });
  text.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); $('#ai-form', root).requestSubmit(); }
  });
  text.addEventListener('input', () => { text.style.height = ''; text.style.height = Math.min(160, text.scrollHeight) + 'px'; });
  $('#ai-model', root).addEventListener('click', () => { renderMenu(); menu.hidden = !menu.hidden; });
  menu.addEventListener('click', e => { const m = e.target.closest('[data-model]'); if (m) pickModel(m.dataset.model); });
  $('#ai-clear', root).addEventListener('click', () => { history = []; save(); renderMsgs(); });
  msgs.addEventListener('click', e => {
    const copy = e.target.closest('.copy');
    if (copy) {
      navigator.clipboard?.writeText(copy.closest('.code').querySelector('code').textContent);
      copy.innerHTML = icon('check');
      setTimeout(() => (copy.innerHTML = icon('content_copy')), 1200);
    }
    const a = e.target.closest('a[data-url]');
    if (a) openUrl(a.dataset.url);
  });

  function save() { try { localStorage.setItem(STORE, JSON.stringify(history.slice(-100))); } catch {} }
  function load() { try { return JSON.parse(localStorage.getItem(STORE)) || []; } catch { return []; } }

  renderHead();
  renderMsgs();
  bridge('ai-models').then(r => { models = r.models || []; keys = r.keys || {}; renderHead(); renderMenu(); if (!history.length) renderMsgs(); });
  return { focus: () => text.focus(), stop: () => proc?.kill() };
}
