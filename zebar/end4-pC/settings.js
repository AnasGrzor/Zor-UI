// end4-pC for Windows - settings app (end4's Settings window + presets gallery).
import { zebar, $, $$, esc, icon, config, watchTheme, run, bridge, popupBehaviour, openUrl } from './lib/core.js';
import { LANGS } from './lib/tab-translate.js';

let cfg = await config();
let theme = {};
await watchTheme(t => { theme = t; if (page === 'quick') render(); });
const close = popupBehaviour();
close.togglePin();   // a real window: stays open when the wallpaper picker takes focus; Esc or Close shuts it

const PAGES = [
  ['quick', 'tune', 'Quick'], ['keys', 'keyboard', 'Keybinds'], ['wallpapers', 'animated_images', 'Wallpapers'], ['presets', 'palette', 'Presets'], ['bar', 'toolbar', 'Bar & desktop'],
  ['services', 'hub', 'Services'], ['about', 'info', 'About'],
];
const SCHEMES = [
  ['auto', 'auto_awesome', 'Auto'], ['content', 'image', 'Content'], ['expressive', 'palette', 'Expressive'],
  ['fidelity', 'tonality', 'Fidelity'], ['fruit-salad', 'nutrition', 'Fruit Salad'], ['monochrome', 'contrast', 'Monochrome'],
  ['neutral', 'circle', 'Neutral'], ['rainbow', 'looks', 'Rainbow'], ['tonal-spot', 'radio_button_checked', 'Tonal Spot'],
  ['vibrant', 'flare', 'Vibrant'],
];
const LEAGUES = [['esp.1', 'LaLiga'], ['eng.1', 'Premier League'], ['ita.1', 'Serie A'], ['fra.1', 'Ligue 1'], ['por.1', 'Primeira Liga'],
  ['tur.1', 'Süper Lig'], ['ned.1', 'Eredivisie'], ['ger.1', 'Bundesliga'], ['mex.1', 'Liga MX'], ['arg.1', 'Liga Profesional'],
  ['bra.1', 'Brasileirão'], ['ksa.1', 'Saudi Pro League'], ['uefa.champions', 'Champions League'], ['uefa.europa', 'Europa League']];
const KEY_PROVIDERS = [['gemini', 'Google Gemini'], ['anthropic', 'Anthropic (Claude)'], ['openai', 'OpenAI'], ['mistral', 'Mistral'],
  ['openrouter', 'OpenRouter'], ['wallhaven', 'Wallhaven (for NSFW search)']];

// The "keybinds" widget (Mod+/) is this same page, opened on the Keybinds tab.
let page = zebar.currentWidget().name === 'keybinds' ? 'keys' : new URLSearchParams(location.search).get('page') || 'quick';
let online = null, local = null, keys = null, models = null, busy = '';
let we = null, weList = null, weQuery = '', weType = 'all';
const loading = new Set();

// ---------------------------------------------------------------- helpers
const get = path => path.split('.').reduce((o, k) => o?.[k], cfg);
async function set(path, value) {
  const parts = path.split('.');
  let o = cfg;
  for (const k of parts.slice(0, -1)) o = o[k] ||= {};
  o[parts.at(-1)] = value;
  const r = await bridge('config-set', path, JSON.stringify(value));
  if (r.error) toast(r.error);
}
function toast(msg) {
  $('.toast')?.remove();
  const t = document.createElement('div');
  t.className = 'toast'; t.textContent = msg;
  document.body.append(t);
  setTimeout(() => t.remove(), 3200);
}
const sw = (path, label, sub = '', ic = '') => `<div class="row">${ic ? icon(ic) : ''}<div class="lbl">${label}${sub ? `<small>${sub}</small>` : ''}</div>
  <button class="switch${get(path) ? ' on' : ''}" data-switch="${path}"></button></div>`;
const seg = (path, opts, label, ic = '') => `<div class="row">${ic ? icon(ic) : ''}<div class="lbl">${label}</div><div class="seg">
  ${opts.map(([v, l, i]) => `<button class="${get(path) === v ? 'sel' : ''}" data-seg="${path}" data-v='${JSON.stringify(v)}'>${i ? icon(i) : ''}${l}</button>`).join('')}</div></div>`;
const num = (path, label, min, max, ic = '') => `<div class="row">${ic ? icon(ic) : ''}<div class="lbl">${label}</div>
  <div class="num"><button class="icon-btn" data-step="${path}" data-d="-1" data-min="${min}" data-max="${max}">${icon('remove')}</button>
  <input class="field" value="${get(path)}" data-num="${path}" data-min="${min}" data-max="${max}" /><button class="icon-btn" data-step="${path}" data-d="1" data-min="${min}" data-max="${max}">${icon('add')}</button></div></div>`;
const text = (path, label, sub = '', ic = '') => `<div class="row">${ic ? icon(ic) : ''}<div class="lbl">${label}${sub ? `<small>${sub}</small>` : ''}</div>
  <input class="field" style="max-width:300px" value="${esc(Array.isArray(get(path)) ? get(path).join(', ') : get(path) ?? '')}" data-text="${path}" ${Array.isArray(get(path)) ? 'data-list="1"' : ''}/></div>`;
const slider = (path, label, min, max, step, fmt, ic = '') => `<div class="row">${ic ? icon(ic) : ''}<div class="lbl">${label}</div>
  <input type="range" min="${min}" max="${max}" step="${step}" value="${get(path)}" data-range="${path}" style="width:220px;accent-color:var(--primary)" />
  <span class="muted" style="width:56px;text-align:right" data-range-val="${path}">${fmt(get(path))}</span></div>`;

// ---------------------------------------------------------------- pages
const pages = {
  quick() {
    const mode = theme.mode || 'dark', scheme = theme.scheme || 'auto';
    return `<h2>${icon('wallpaper')}Wallpaper & colors</h2>
    <div class="card"><div class="wall">
      <div class="img" style="background-image:url('./wallpaper.jpg?t=${Date.now() >> 12}')">
        ${theme.live ? `<span class="chip live">${icon('movie')}Live wallpaper</span>` : ''}
        <div class="acts">
          <button class="icon-btn filled" data-act="wall" title="Change wallpaper (images or videos)">${icon('image')}</button>
          <button class="icon-btn tonal" data-act="random-online" title="Random from Wallhaven">${icon('shuffle')}</button>
          <button class="icon-btn tonal" data-page="wallpapers" title="Wallpaper Engine library">${icon('animated_images')}</button>
        </div>
      </div>
      <div style="display:flex;flex-direction:column;gap:8px">
        <div class="modes">
          <button class="${mode === 'light' ? 'sel' : ''}" data-mode="light">${icon('light_mode')}Light</button>
          <button class="${mode === 'dark' ? 'sel' : ''}" data-mode="dark">${icon('dark_mode')}Dark</button>
        </div>
        <div class="schemes">${SCHEMES.slice(0, 9).map(([v, i, l]) => `<button class="${scheme === v ? 'sel' : ''}" data-scheme="${v}">${icon(i)}${l}</button>`).join('')}</div>
      </div>
    </div>
    <div class="row">${icon('colorize')}<div class="lbl">Theme from a color<small>Keeps your wallpaper</small></div>
      <input type="color" value="${esc(theme.seed || '#6750a4')}" id="seed" style="width:48px;height:36px;border:0;background:none" /><button class="chip" data-act="seed">Apply</button></div>
    ${busy ? '<div class="spinner"></div>' : ''}
    </div>
    <div class="card">
      ${slider('transparency', 'Bar & panel transparency', 0, 0.6, 0.02, v => `${Math.round(v * 100)}%`, 'opacity')}
      ${slider('nightLight.kelvin', 'Night light warmth', 2500, 6000, 100, v => `${v}K`, 'nightlight')}
      ${sw('syncLockScreen', 'Use the wallpaper on the lock screen', 'Windows may block this on managed PCs', 'lock')}
    </div>`;
  },

  wallpapers() {
    if (we === null) return '<div class="spinner"></div>';
    if (!we.installed) return `<h2>${icon('animated_images')}Wallpaper Engine</h2>
      <div class="card"><div class="muted">Wallpaper Engine wasn't found in your Steam libraries. Videos picked with
      Change wallpaper play in Lively instead (install it with <code>install.ps1 -WithLively</code>).</div></div>`;
    const cur = (we.current || '').toLowerCase();
    const isCur = it => cur && (cur === it.path.toLowerCase() || cur.startsWith(it.path.toLowerCase().replace(/project\.json$/, '')));
    const q = weQuery.toLowerCase();
    const list = (weList || []).filter(it => (weType === 'all' || it.type === weType) &&
      (!q || it.title.toLowerCase().includes(q) || (it.tags || []).some(t => String(t).toLowerCase().includes(q))));
    const types = ['all', ...new Set((weList || []).map(i => i.type).filter(Boolean))];
    return `<h2>${icon('animated_images')}Wallpaper Engine</h2>
    <div class="card">
      ${sw('wallpaperEngine.follow', 'Follow Wallpaper Engine', 'Re-theme end4 when you change wallpaper in Wallpaper Engine or a playlist moves on', 'sync')}
      <div class="row">${icon(we.running ? 'play_circle' : 'pause_circle')}<div class="lbl">${we.running ? 'Running' : 'Not running'}
        <small>${esc(we.current ? 'Playing: ' + ((weList || []).find(isCur)?.title || we.current.split('\\').slice(-2).join('\\')) : 'No live wallpaper from end4')}</small></div>
        <button class="chip" data-act="we-close">${icon('stop')}Stop live wallpaper</button></div>
    </div>
    <div class="card">
      <div class="row"><input class="field" id="we-q" placeholder="Search ${weList ? weList.length : ''} wallpapers by name or tag" value="${esc(weQuery)}" />
        <div class="seg">${types.map(t => `<button class="${t === weType ? 'sel' : ''}" data-we-type="${t}">${t === 'all' ? 'All' : t.charAt(0).toUpperCase() + t.slice(1)}</button>`).join('')}</div></div>
      ${weList === null ? '<div class="spinner"></div>' : `<div class="grid">${list.slice(0, 120).map(it => `
        <div class="preset${isCur(it) ? ' current' : ''}">
          <div class="pv" style="background-image:url('./${esc(it.thumb)}')">${it.type ? `<span class="chip wetype">${esc(it.type)}</span>` : ''}</div>
          <div class="meta"><div><b class="ellipsis">${esc(it.title)}</b><small>${esc(it.origin === 'workshop' ? 'Workshop' : 'My projects')}${it.rating && it.rating !== 'Everyone' ? ' · ' + esc(it.rating) : ''}</small></div>
          <button class="icon-btn tonal" data-we-apply="${esc(it.id)}" title="Set as wallpaper">${busy === it.id ? '<span class="spinner small"></span>' : icon(isCur(it) ? 'check' : 'wallpaper')}</button></div></div>`).join('')}</div>
        ${list.length > 120 ? `<div class="muted">Showing 120 of ${list.length}. Search to narrow it down.</div>` : ''}`}
    </div>`;
  },

  presets() {
    const onlineHTML = online === null ? '<div class="spinner"></div>' : online.error ? `<div class="err">${esc(online.error)}</div>`
      : `<div class="grid">${online.presets.map(p => `<div class="preset">
          <div class="pv" style="background-image:url('${esc(p.preview)}')"></div>
          <div class="meta"><div><b class="ellipsis">${esc(p.name.replace(/_/g, ' '))}</b><small>${esc(p.author ? 'by ' + p.author : '')}</small></div>
          <button class="icon-btn tonal" data-apply-online="${esc(p.name)}" title="Apply">${busy === p.name ? '<span class="spinner small"></span>' : icon('download')}</button></div></div>`).join('')}</div>`;
    const localHTML = local === null ? '<div class="spinner"></div>' : local.presets.length ? local.presets.map(p => `
      <div class="row">${icon(p.from === 'online' ? 'cloud_done' : 'bookmark')}<div class="lbl">${esc(p.name)}<small>${esc([p.scheme, p.mode, p.wallpaper ? p.wallpaper.split('\\').pop() : ''].filter(Boolean).join(' · '))}</small></div>
        <button class="chip" data-apply-local="${esc(p.id)}">${busy === p.id ? '<span class="spinner small"></span>' : 'Apply'}</button>
        <button class="icon-btn" data-delete-local="${esc(p.id)}" title="Delete">${icon('delete')}</button></div>`).join('') : '<div class="muted">No saved presets yet.</div>';
    return `<h2>${icon('bookmark')}Your presets</h2>
      <div class="card">
        <form class="row" id="save-form">${icon('add_circle')}<div class="lbl">Save current setup<small>Wallpaper, colors, bar and widget options</small></div>
          <input class="field" id="save-name" placeholder="Preset name" style="max-width:220px" /><button class="chip" type="submit">Save</button></form>
        ${localHTML}
      </div>
      <h2>${icon('public')}Community gallery <span class="muted" style="font-size:12px;font-weight:400">github.com/pctrade/end4-pCpresets</span></h2>
      <div class="card"><div class="muted" style="font-size:12px">Applies each preset's wallpaper, color scheme, workspace labels and transparency. Linux-only parts (Hyprland, Quickshell layouts) are skipped.</div>${onlineHTML}</div>`;
  },

  bar() {
    return `<h2>${icon('toolbar')}Bar</h2>
    <div class="card">
      ${num('workspaces', 'Workspaces shown', 1, 20, 'grid_view')}
      ${text('workspaceLabels', 'Workspace labels', 'Comma separated, e.g. 一, 二, 三 (from end4 presets)', 'label')}
      ${sw('alwaysShowNumbers', 'Always show workspace labels', 'Otherwise only the active one shows a label', 'pin')}
      ${text('clockFormat', 'Clock format', "Luxon tokens, e.g. EEE, dd/MM '•' HH:mm", 'schedule')}
      ${seg('tempUnit', [['C', '°C'], ['F', '°F']], 'Temperature', 'thermostat')}
      ${sw('showKeyboardLayout', 'Keyboard layout indicator', '', 'keyboard')}
      ${sw('visualizer.bar', 'Visualizer in the bar', 'Small cava-style bars next to the media', 'graphic_eq')}
    </div>
    <h2>${icon('desktop_windows')}Desktop</h2>
    <div class="card">
      ${sw('desktopWidgets', 'Desktop widgets', 'Clock, weather, calendar, greeting and media', 'widgets')}
      ${sw('visualizer.desktop', 'Desktop visualizer', 'Audio bars along the bottom of the screen (cava)', 'equalizer')}
      ${num('visualizer.bars', 'Visualizer bars', 16, 160, 'bar_chart')}
      ${text('userName', 'Your name', 'Shown in the greeting and sidebar', 'person')}
    </div>`;
  },

  services() {
    const keyRows = KEY_PROVIDERS.map(([id, n]) => `<form class="row keyrow" data-key="${id}">${icon('key')}<div class="lbl">${n}
        <small class="${keys?.[id] ? 'ok' : 'warn'}">${keys === null ? '…' : keys[id] ? 'Key saved (encrypted with Windows DPAPI)' : 'No key'}</small></div>
        <input class="field" type="password" placeholder="${keys?.[id] ? '••••••••  (enter to replace)' : 'Paste API key'}" style="max-width:240px" autocomplete="off" />
        <button class="chip" type="submit">Save</button>${keys?.[id] ? `<button class="icon-btn" type="button" data-clear-key="${id}" title="Remove">${icon('delete')}</button>` : ''}</form>`).join('');
    const modelOpts = (models || [{ id: get('ai.model'), name: get('ai.model') }]).map(m => `<option value="${esc(m.id)}"${m.id === get('ai.model') ? ' selected' : ''}>${esc(m.name)}</option>`).join('');
    const langOpts = Object.entries(LANGS).filter(([k]) => k !== 'auto').map(([k, v]) => `<option value="${k}"${k === get('translator.target') ? ' selected' : ''}>${esc(v)}</option>`).join('');
    const leagues = get('football.leagues') || [];
    return `<h2>${icon('neurology')}AI</h2>
    <div class="card">
      <div class="row">${icon('model_training')}<div class="lbl">Default model<small>Ollama models appear when Ollama is running</small></div><select class="field" data-select="ai.model" style="max-width:260px;height:40px">${modelOpts}</select></div>
      ${slider('ai.temperature', 'Temperature', 0, 2, 0.1, v => (+v).toFixed(1), 'thermostat')}
      <div class="row" style="align-items:flex-start">${icon('description')}<div class="lbl">System prompt<small>Leave empty for the default</small></div>
        <textarea class="field" rows="3" data-text="ai.systemPrompt" style="max-width:300px">${esc(get('ai.systemPrompt') || '')}</textarea></div>
      ${keyRows}
    </div>
    <h2>${icon('translate')}Translator</h2>
    <div class="card"><div class="row">${icon('language')}<div class="lbl">Translate into</div><select class="field" data-select="translator.target" style="max-width:260px;height:40px">${langOpts}</select></div></div>
    <h2>${icon('photo_library')}Anime & wallpapers</h2>
    <div class="card">
      ${seg('booru.source', [['yandere', 'yande.re'], ['konachan', 'Konachan'], ['danbooru', 'Danbooru'], ['waifuim', 'waifu.im'], ['wallhaven', 'Wallhaven']], 'Default source', 'image_search')}
      ${sw('booru.allowNsfw', 'Allow NSFW results', 'Off by default. Applies to every source.', 'explicit')}
    </div>
    <h2>${icon('sports_soccer')}Football</h2>
    <div class="card"><div class="lbl" style="font-size:12px;color:var(--on-surface-variant)">Leagues to show (none selected = all)</div>
      <div class="chips">${LEAGUES.map(([id, n]) => `<button class="chip${leagues.includes(id) ? ' sel' : ''}" data-league="${id}">${n}</button>`).join('')}</div></div>
    <h2>${icon('lyrics')}Media</h2>
    <div class="card">${sw('lyrics', 'Synced lyrics', 'From lrclib.net, in the left sidebar', 'lyrics')}</div>`;
  },

  keys() {
    // Mirrors glazewm/config.template.yaml; keep both in sync.
    const m = cfg.modKey === 'lwin' ? 'Win' : 'Alt';
    const groups = [
      ['dashboard', 'Shell', [[`${m}+A`, 'Left sidebar: home, AI, translator, anime, football'], [`${m}+N`, 'Right sidebar: toggles, notifications, calendar'],
        [`${m}+I`, 'Settings'], [`${m}+/`, 'This keybinds list'], [cfg.modKey === 'lwin' ? 'Win+Tab' : 'Alt+`', 'Overview'], ['Alt+Space', 'Launcher'],
        [`${m}+Ctrl+T`, 'Pick a wallpaper and regenerate colors'], [`${m}+Ctrl+D`, 'Dark / light'], [`${m}+Ctrl+R`, 'Restart the bar and reload GlazeWM'],
        ['Ctrl+PgUp / PgDn', 'Switch tabs in a sidebar or here'], ['Esc', 'Close a sidebar or this window']]],
      ['apps', 'Apps', [[`${m}+Enter / ${m}+T`, 'Terminal'], [`${m}+E`, 'File Explorer'], [`${m}+W`, 'Edge'], [`${m}+C`, 'VS Code'],
        [`${m}+Shift+S`, 'Screen snip'], [`${m}+K`, 'On-screen keyboard']]],
      ['view_quilt', 'Windows', [[`${m}+Arrows`, 'Focus the window in that direction'], [`${m}+Shift+Arrows`, 'Move the window in that direction (also to another monitor)'],
        [`${m}+V`, 'Next window opens beside / below'], [`${m}+R`, 'Resize mode: arrows to resize, Esc or Enter to finish'], [`${m}+- / ${m}+=`, 'Narrower / wider'],
        [`${m}+Q`, 'Close'], [`${m}+F`, 'Fullscreen'], [`${m}+Shift+Space`, 'Float / tile'], [`${m}+Shift+T`, 'Tile a floating window'], [`${m}+M`, 'Minimize']]],
      ['grid_view', 'Workspaces', [[`${m}+1…0`, 'Go to workspace 1–10'], [`${m}+Shift+1…0`, 'Send the window to workspace 1–10'],
        [`${m}+Ctrl+← / →`, 'Previous / next workspace'], [`${m}+PgUp / PgDn`, 'Previous / next workspace'], [`${m}+Ctrl+Shift+← / →`, 'Move the workspace to another monitor']]],
      ['build', 'GlazeWM', [[`${m}+Shift+P`, 'Pause tiling (press again to resume)'], [`${m}+Shift+W`, 'Redraw all windows'], [`${m}+Shift+E`, 'Quit GlazeWM']]],
    ];
    return groups.map(([ic, title, list]) => `<h2>${icon(ic)}${title}</h2>
    <div class="card"><div class="keys">${list.map(([k, d]) => `<kbd>${k}</kbd><span>${d}</span>`).join('')}</div></div>`).join('')
      + `<div class="muted" style="margin:12px 4px">Games are left alone by GlazeWM (window rule in %USERPROFILE%\\.glzr\\glazewm\\config.yaml).</div>`;
  },

  about() {
    return `<h2>${icon('info')}About</h2>
    <div class="card"><div class="row"><div class="lbl"><b style="font-weight:500">end4-pC for Windows</b><small>A port of pctrade's end4-pC (illogical-impulse by end-4) to GlazeWM + Zebar</small></div>
      <button class="chip" data-url="https://github.com/pctrade/end4-pC">${icon('open_in_new')}end4-pC</button></div>
      <div class="row"><div class="lbl">Windows settings</div><button class="chip" data-act="winsettings">${icon('settings')}Open</button></div>
      <div class="row"><div class="lbl">Config file<small>${esc(cfg.scriptsDir ? '%USERPROFILE%\\.glzr\\zebar\\end4-pC\\config.json' : '')}</small></div><button class="chip" data-act="editcfg">${icon('edit')}Edit</button></div></div>`;
  },
};

// ---------------------------------------------------------------- render
function render() {
  const name = cfg.userName || 'You';
  $('#nav').innerHTML = `<div class="me"><div class="av">${esc(name.charAt(0).toUpperCase())}</div><div><b>${esc(name)}</b><small>Windows</small></div></div>
    <button class="cfgbtn" data-act="editcfg">${icon('edit')}Config file</button>
    ${PAGES.map(([id, ic, l]) => `<button class="item${id === page ? ' sel' : ''}" data-page="${id}">${icon(ic)}${l}</button>`).join('')}
    <span style="flex:1"></span><button class="item" data-act="close">${icon('close')}Close</button>`;
  const scroll = $('#main').scrollTop;
  $('#main').innerHTML = pages[page]();
  $('#main').scrollTop = scroll;
  const once = (key, fn) => { if (!loading.has(key)) { loading.add(key); fn().finally(() => { loading.delete(key); render(); }); } };
  if (page === 'presets') {
    if (online === null) once('online', () => bridge('presets-online').then(r => { online = r.error ? r : { presets: r.presets || [] }; }));
    if (local === null) once('local', () => bridge('presets-local').then(r => { local = { presets: r.presets || [] }; }));
  }
  if (page === 'wallpapers') {
    if (we === null) once('we', () => bridge('we-status').then(r => { we = r.error ? { installed: false } : r; }));
    if (weList === null && we?.installed) once('welist', () => bridge('we-list').then(r => { weList = r.items || []; }));
  }
  if (page === 'services') {
    if (keys === null) once('keys', () => bridge('secret-status').then(r => { keys = r.error ? {} : r; }));
    if (models === null) once('models', () => bridge('ai-models').then(r => { models = r.models || []; }));
  }
}

async function retheme(args, label) {
  busy = label; render();
  const r = await run('switchwall', ...args);
  busy = ''; render();
  if (r.error || r.ok === false) toast(r.error || 'Failed');
  return r;
}

// ---------------------------------------------------------------- events
document.addEventListener('click', async e => {
  const t = e.target.closest('button, [data-url]');
  if (!t) return;
  const d = t.dataset;
  if (d.page) { page = d.page; return render(); }
  if (d.url) return openUrl(d.url);
  if (d.mode) return retheme(['-Mode', d.mode], 'mode');
  if (d.scheme) return retheme(['-Scheme', d.scheme], 'scheme');
  if (d.act === 'wall') return retheme([], 'wall');
  if (d.act === 'seed') return retheme(['-Color', $('#seed').value], 'seed');
  if (d.act === 'random-online') {
    busy = 'wall'; render();
    const r = await bridge('booru', 'wallhaven', String(1 + Math.floor(Math.random() * 5)));
    const it = r.items?.[Math.floor(Math.random() * (r.items?.length || 1))];
    if (!it) { busy = ''; render(); return toast(r.error || 'Nothing found'); }
    await bridge('download-wallpaper', it.file, '--apply');
    busy = ''; render(); return;
  }
  if (d.act === 'close') return close();
  if (d.act === 'winsettings') return run('toggles', '-Action', 'settings', '-Value', '');
  if (d.act === 'editcfg') return run('toggles', '-Action', 'editconfig');
  if (d.switch) {
    const v = !get(d.switch);
    t.classList.toggle('on', v);
    await set(d.switch, v);
    if (d.switch === 'visualizer.desktop' && v) zebar.startWidgetPreset('visualizer', 'default').catch(() => {});
    if (d.switch === 'desktopWidgets' && v) zebar.startWidgetPreset('desktop', 'default').catch(() => {});
    return;
  }
  if (d.seg) { await set(d.seg, JSON.parse(d.v)); return render(); }
  if (d.step) {
    const v = Math.max(+d.min, Math.min(+d.max, (get(d.step) || 0) + +d.d));
    await set(d.step, v); return render();
  }
  if (d.league) {
    const cur = new Set(get('football.leagues') || []);
    cur.has(d.league) ? cur.delete(d.league) : cur.add(d.league);
    await set('football.leagues', [...cur]); return render();
  }
  if (d.applyOnline) {
    busy = d.applyOnline; render();
    const r = await bridge('preset-apply-online', d.applyOnline);
    busy = ''; local = null; render();
    return toast(r.ok ? `Applied ${d.applyOnline.replace(/_/g, ' ')}` : r.error || 'Failed');
  }
  if (d.applyLocal) {
    busy = d.applyLocal; render();
    const r = await bridge('preset-apply-local', d.applyLocal);
    busy = ''; render();
    return toast(r.ok ? 'Preset applied' : r.error || 'Failed');
  }
  if (d.weType) { weType = d.weType; return render(); }
  if (d.weApply) {
    busy = d.weApply; render();
    const r = await bridge('we-apply', d.weApply);
    busy = ''; we = null; render();
    return toast(r.ok ? 'Wallpaper set' : r.error || 'Failed');
  }
  if (d.act === 'we-close') { await bridge('we-close'); we = null; return render(); }
  if (d.deleteLocal) { await bridge('preset-delete', d.deleteLocal); local = null; return render(); }
  if (d.clearKey) { await bridge('secret-set', d.clearKey); keys = null; return render(); }
});

document.addEventListener('change', async e => {
  const t = e.target, d = t.dataset;
  if (d.select) { await set(d.select, t.value); toast('Saved'); }
  if (d.text) {
    const v = d.list ? t.value.split(',').map(s => s.trim()).filter(Boolean) : t.value;
    await set(d.text, v); toast('Saved');
  }
  if (d.num) {
    const v = Math.max(+d.min, Math.min(+d.max, parseInt(t.value, 10) || +d.min));
    await set(d.num, v); render();
  }
  if (d.range) {
    await set(d.range, +t.value);
    if (d.range === 'nightLight.kelvin') bridge('nightlight', 'status').then(r => r.on && bridge('nightlight', 'on', t.value));
  }
});
document.addEventListener('input', e => {
  if (e.target.id === 'we-q') {
    weQuery = e.target.value;
    const pos = e.target.selectionStart;
    render();
    const q = $('#we-q'); q.focus(); q.setSelectionRange(pos, pos);
    return;
  }
  const d = e.target.dataset;
  if (d.range) {
    const span = document.querySelector(`[data-range-val="${d.range}"]`);
    const v = +e.target.value;
    if (span) span.textContent = d.range === 'transparency' ? `${Math.round(v * 100)}%` : d.range === 'nightLight.kelvin' ? `${v}K` : v.toFixed(1);
    if (d.range === 'transparency') document.documentElement.style.setProperty('--opaque', `${Math.round((1 - v) * 100)}%`);
  }
});
document.addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.target;
  if (f.id === 'save-form') {
    const name = $('#save-name').value.trim();
    if (!name) return;
    const r = await bridge('preset-save', name);
    local = null; render();
    return toast(r.ok ? `Saved “${name}”` : r.error || 'Failed');
  }
  if (f.dataset.key) {
    const v = f.querySelector('input').value.trim();
    if (!v) return;
    const r = await bridge('secret-set', f.dataset.key, v);
    keys = null; render();
    toast(r.ok ? 'Key saved' : r.error || 'Failed');
  }
});
window.addEventListener('keydown', e => {
  if (e.ctrlKey && (e.key === 'PageDown' || e.key === 'PageUp')) {
    const i = PAGES.findIndex(p => p[0] === page);
    page = PAGES[(i + (e.key === 'PageDown' ? 1 : -1) + PAGES.length) % PAGES.length][0];
    render();
  }
});
render();
