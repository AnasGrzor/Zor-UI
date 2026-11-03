import { zebar, $, $$, esc, icon, ring, setHTML, prettyApp, config, watchTheme, run, bridge, popupBehaviour, monthGrid, openWidget, openOnTargetMonitor, closeWindow } from './lib/core.js';
import * as timer from './lib/timer.js';

await openOnTargetMonitor();
const cfg = await config();
await watchTheme();
popupBehaviour();

const providers = zebar.createProviderGroup({
  audio: { type: 'audio' },
  host: { type: 'host', refreshInterval: 60000 },
  glazewm: { type: 'glazewm' },
  network: { type: 'network', refreshInterval: 5000 },
});
let out = providers.outputMap;

// System state that Zebar doesn't expose comes from toggles.ps1.
let sys = { loaded: false, wifi: null, bluetooth: null, caffeine: false, darkMode: null, brightness: null, night: false };
const busy = new Set();
bridge('nightlight', 'status').then(r => { sys.night = !!r.on; renderToggles(); });
run('toggles', '-Action', 'powerplan').then(r => { sys.plan = r.plans?.find(p => p.active)?.name || null; renderToggles(); });
run('toggles', '-Action', 'status').then(s => { if (!s.error) { delete s.darkMode; sys = { ...sys, ...s }; } sys.loaded = true; renderToggles(); renderSliders(); });

// ---------------------------------------------------------------- header
function renderHeader() {
  const name = cfg.userName || out.host?.hostname || 'Windows';
  setHTML($('#avatar'), esc(name.charAt(0).toUpperCase()));
  setHTML($('#user'), esc(name));
  const up = Number(out.host?.uptime || 0) / 60000;
  const h = Math.floor(up / 60), m = Math.floor(up % 60);
  setHTML($('#uptime'), up ? `Up ${h ? h + 'h ' : ''}${m}m · ${esc(out.host?.friendlyOsVersion || '')}` : '');
}

// ---------------------------------------------------------------- toggles
const onOff = v => (!sys.loaded ? '…' : v == null ? 'Unavailable' : v ? 'On' : 'Off');
function toggleDefs() {
  const a = out.audio;
  const spk = a?.defaultPlaybackDevice, mic = a?.defaultRecordingDevice;
  return [
    { id: 'wifi', icon: sys.wifi === false ? 'signal_wifi_off' : 'wifi', label: 'Wi-Fi', on: sys.wifi,
      sub: sys.wifi ? (out.network?.defaultGateway?.ssid || 'On') : onOff(sys.wifi), settings: 'network-wifi' },
    { id: 'bluetooth', icon: sys.bluetooth ? 'bluetooth' : 'bluetooth_disabled', label: 'Bluetooth', on: sys.bluetooth, sub: onOff(sys.bluetooth), settings: 'bluetooth' },
    { id: 'sound', icon: spk?.isMuted ? 'volume_off' : 'volume_up', label: 'Sound', on: spk && !spk.isMuted, sub: spk ? (spk.isMuted ? 'Muted' : `${Math.round(spk.volume)}%`) : 'No device', settings: 'sound' },
    { id: 'mic', icon: mic?.isMuted ? 'mic_off' : 'mic', label: 'Microphone', on: mic && !mic.isMuted, sub: mic ? (mic.isMuted ? 'Muted' : 'Listening') : 'No device', settings: 'sound' },
    { id: 'dark', icon: sys.darkMode === false ? 'light_mode' : 'dark_mode', label: 'Dark mode', on: sys.darkMode, sub: sys.darkMode === false ? 'Light' : 'Dark', settings: 'personalization-colors' },
    { id: 'caffeine', icon: 'coffee', label: 'Keep awake', on: sys.caffeine, sub: sys.caffeine ? 'Screen stays on' : 'Off', settings: 'powersleep' },
    { id: 'tiling', icon: 'grid_view', label: 'Tiling', on: out.glazewm ? !out.glazewm.isPaused : null, sub: out.glazewm ? (out.glazewm.isPaused ? 'Paused' : 'GlazeWM') : 'Not running' },
    { id: 'nightlight', icon: 'nightlight', label: 'Night light', on: sys.night, sub: sys.nightError || (sys.night ? `${cfg.nightLight.kelvin}K` : 'Off'), settings: 'nightlight' },
    { id: 'snip', icon: 'screenshot_region', label: 'Screen snip', on: false, sub: 'Region capture' },
    { id: 'osk', icon: 'keyboard', label: 'Keyboard', on: false, sub: 'On-screen' },
    { id: 'power', icon: /saver/i.test(sys.plan) ? 'energy_savings_leaf' : /perform/i.test(sys.plan) ? 'bolt' : 'balance', label: 'Power plan',
      on: !!sys.plan && !/saver/i.test(sys.plan), sub: sys.plan === undefined ? '…' : sys.plan || 'Unavailable', settings: 'powersleep' },
    { id: 'colorpick', icon: 'colorize', label: 'Color picker', on: false, sub: 'Copies the hex' },
    { id: 'screentranslate', icon: 'translate', label: 'Screen translate', on: false, sub: `Into ${cfg.translator.target}` },
    { id: 'record', icon: recStarted() ? 'stop_circle' : 'screen_record', label: 'Screen record', on: !!recStarted(),
      sub: recStarted() ? `Recording ${timer.clock((Date.now() - recStarted()) / 1000)}` : 'Primary monitor' },
  ];
}

// Screen recording: the bar shows it too (red chip) and can stop it.
const recStarted = () => Number(localStorage.getItem('end4-pC.rec')) || 0;
setInterval(() => recStarted() && renderToggles(), 1000);

function renderToggles() {
  setHTML($('#toggles'), toggleDefs().map(t => `
    <button class="tg${t.on ? ' on' : ''}${busy.has(t.id) ? ' busy' : ''}" data-tg="${t.id}" data-settings="${t.settings || ''}" title="${t.settings ? 'Right-click for settings' : ''}">
      <span class="bubble">${icon(t.icon, { fill: !!t.on })}</span>
      <span class="txt"><b>${t.label}</b><span class="ellipsis">${esc(t.sub)}</span></span>
    </button>`).join(''));
}

async function toggle(id) {
  const a = out.audio;
  if (busy.has(id)) return;
  const exec = async fn => { busy.add(id); renderToggles(); try { await fn(); } finally { busy.delete(id); renderToggles(); } };
  switch (id) {
    case 'wifi': return exec(async () => { const r = await run('toggles', '-Action', 'wifi'); if ('wifi' in r) sys.wifi = r.wifi; });
    case 'bluetooth': return exec(async () => { const r = await run('toggles', '-Action', 'bluetooth'); if ('bluetooth' in r) sys.bluetooth = r.bluetooth; });
    case 'caffeine': return exec(async () => { const r = await run('toggles', '-Action', 'caffeine'); if ('caffeine' in r) sys.caffeine = r.caffeine; });
    case 'dark': return exec(() => run('switchwall', '-ToggleMode'));
    case 'sound': return a?.defaultPlaybackDevice && a.setMute(!a.defaultPlaybackDevice.isMuted);
    case 'mic': return a?.defaultRecordingDevice && a.setMute(!a.defaultRecordingDevice.isMuted, { deviceId: a.defaultRecordingDevice.deviceId });
    case 'tiling': return out.glazewm?.runCommand('wm-toggle-pause');
    case 'nightlight': return exec(async () => {
      const r = await bridge('nightlight', 'toggle');
      sys.night = !!r.on;
      sys.nightError = r.ok === false ? 'Not supported here' : '';
      if (r.ok === false) run('toggles', '-Action', 'settings', '-Value', 'nightlight');   // fall back to Windows' own
    });
    case 'snip': closeWindow(); return run('toggles', '-Action', 'snip');
    case 'osk': return run('toggles', '-Action', 'osk');
    case 'power': return exec(async () => { const r = await run('toggles', '-Action', 'powerplan', '-Value', 'next'); if (r.plans) sys.plan = r.plans.find(p => p.active)?.name || null; });
    case 'record': return exec(async () => {
      if (recStarted()) { await bridge('record', 'stop'); localStorage.removeItem('end4-pC.rec'); return; }
      const r = await bridge('record', 'start');
      if (r.recording) { localStorage.setItem('end4-pC.rec', Math.round(r.started * 1000)); closeWindow(); }
    });
    case 'colorpick': case 'screentranslate': closeWindow(); return run('toggles', '-Action', id);
  }
}

$('#toggles').addEventListener('click', e => { const b = e.target.closest('[data-tg]'); if (b) toggle(b.dataset.tg); });
$('#toggles').addEventListener('contextmenu', e => {
  e.preventDefault();
  const s = e.target.closest('[data-tg]')?.dataset.settings;
  if (s) run('toggles', '-Action', 'settings', '-Value', s);
});

// ---------------------------------------------------------------- sliders
const sliders = {
  volume: { icon: 'volume_up', get: () => out.audio?.defaultPlaybackDevice?.volume, set: v => out.audio?.setVolume(v) },
  mic: { icon: 'mic', get: () => out.audio?.defaultRecordingDevice?.volume,
    set: v => out.audio?.setVolume(v, { deviceId: out.audio?.defaultRecordingDevice?.deviceId }) },
  brightness: { icon: 'light_mode', get: () => sys.brightness, set: debounce(v => run('toggles', '-Action', 'brightness', '-Value', v), 120) },
};

function renderSliders() {
  const host = $('#sliders');
  for (const [id, s] of Object.entries(sliders)) {
    const v = s.get();
    let el = host.querySelector(`[data-sl="${id}"]`);
    if (v == null) { el?.remove(); continue; }
    if (!el) {
      el = document.createElement('div');
      el.className = 'slider';
      el.dataset.sl = id;
      el.innerHTML = `<div class="fill"></div>${icon(s.icon, { fill: true })}<span class="val"></span><input type="range" min="0" max="100" step="1" />`;
      const input = el.querySelector('input');
      input.addEventListener('input', () => { paint(el, +input.value); s.set(+input.value); });
      input.addEventListener('pointerdown', () => (el.dragging = true));
      input.addEventListener('pointerup', () => (el.dragging = false));
      host.append(el);
    }
    if (!el.dragging) { el.querySelector('input').value = Math.round(v); paint(el, v); }
  }
}
function paint(el, v) {
  el.querySelector('.fill').style.width = `max(44px, ${v}%)`;
  el.querySelector('.val').textContent = `${Math.round(v)}%`;
}
function debounce(fn, ms) { let t; return v => { clearTimeout(t); t = setTimeout(() => fn(v), ms); }; }

// ---------------------------------------------------------------- per-app volume (end4's volume mixer)
let mixerOpen = false, mixer = [];
const setAppVolume = {};   // one debounced setter per app
async function loadMixer() {
  const r = await bridge('mixer');
  mixer = r.apps || [];
  renderMixer();
}
function renderMixer() {
  $('#mixer-btn').classList.toggle('sel', mixerOpen);
  const host = $('#mixer');
  host.hidden = !mixerOpen;
  if (!mixerOpen) return;
  const key = mixer.map(m => m.app).join('|');
  if (host.__key !== key) {   // rebuild only when apps come or go, so a drag isn't interrupted
    host.__key = key;
    host.innerHTML = mixer.length ? mixer.map(m => `<div class="mix" data-app="${esc(m.app)}">
      <div class="slider"><div class="fill"></div><span class="name">${esc(prettyApp(m.app))}</span><span class="val"></span><input type="range" min="0" max="100" step="1" /></div>
      <button class="icon-btn" data-mute title="Mute"></button></div>`).join('') : '<div class="muted" style="font-size:13px">No app is playing audio</div>';
    for (const row of host.querySelectorAll('.mix')) {
      const app = row.dataset.app, input = row.querySelector('input'), sl = row.querySelector('.slider');
      setAppVolume[app] ??= debounce(v => bridge('mixer', 'volume', app, v), 150);
      input.addEventListener('input', () => { paint(sl, +input.value); setAppVolume[app](+input.value); });
      input.addEventListener('pointerdown', () => (sl.dragging = true));
      input.addEventListener('pointerup', () => (sl.dragging = false));
    }
  }
  for (const m of mixer) {
    const row = host.querySelector(`[data-app="${CSS.escape(m.app)}"]`), sl = row.querySelector('.slider');
    row.classList.toggle('muted', m.muted);
    setHTML(row.querySelector('[data-mute]'), icon(m.muted ? 'volume_off' : 'volume_up'));
    if (!sl.dragging) { row.querySelector('input').value = m.volume; paint(sl, m.volume); }
  }
}
$('#mixer-btn').addEventListener('click', () => { mixerOpen = !mixerOpen; renderMixer(); if (mixerOpen) loadMixer(); });
$('#mixer').addEventListener('click', async e => {
  const row = e.target.closest('[data-mute]')?.closest('.mix');
  if (!row) return;
  const m = mixer.find(x => x.app === row.dataset.app);
  m.muted = !m.muted;
  renderMixer();
  const r = await bridge('mixer', 'mute', m.app, m.muted ? 1 : 0);
  if (r.apps) { mixer = r.apps; renderMixer(); }
});
setInterval(() => mixerOpen && loadMixer(), 3000);   // apps start and stop playing

// ---------------------------------------------------------------- calendar
let calMonth = new Date();
function renderCalendar() {
  const cells = monthGrid(calMonth);
  setHTML($('#page-cal'), `
    <div class="cal-head">
      <span class="chip">${calMonth.toLocaleString(undefined, { month: 'long', year: 'numeric' })}</span>
      <span><button class="icon-btn" data-m="-1">${icon('chevron_left')}</button><button class="icon-btn" data-m="1">${icon('chevron_right')}</button></span>
    </div>
    <div class="cal">
      ${['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'].map(d => `<div class="dow">${d}</div>`).join('')}
      ${cells.map(c => `<div class="d${c.out ? ' out' : ''}${c.today ? ' today' : ''}">${c.d}</div>`).join('')}
    </div>`);
}
$('#page-cal').addEventListener('click', e => {
  const m = e.target.closest('[data-m]')?.dataset.m;
  if (m) { calMonth = new Date(calMonth.getFullYear(), calMonth.getMonth() + +m, 1); renderCalendar(); }
});

// ---------------------------------------------------------------- to-do
const TODO_KEY = 'end4-pC.todo';
const loadTodos = () => { try { return JSON.parse(localStorage.getItem(TODO_KEY)) || []; } catch { return []; } };
const saveTodos = t => { try { localStorage.setItem(TODO_KEY, JSON.stringify(t)); } catch {} };
function renderTodos() {
  const todos = loadTodos();
  setHTML($('#todo-list'), todos.length ? todos.map((t, i) => `
    <div class="todo${t.done ? ' done' : ''}">
      <button class="chk" data-done="${i}">${t.done ? icon('check') : ''}</button>
      <span class="t">${esc(t.text)}</span>
      <button class="icon-btn" data-del="${i}" title="Delete">${icon('delete')}</button>
    </div>`).join('') : `<div class="empty">${icon('checklist')}Nothing to do</div>`);
}
$('#todo-list').addEventListener('click', e => {
  const todos = loadTodos();
  const d = e.target.closest('[data-done]')?.dataset.done, x = e.target.closest('[data-del]')?.dataset.del;
  if (d != null) todos[d].done = !todos[d].done;
  if (x != null) todos.splice(x, 1);
  saveTodos(todos); renderTodos();
});
$('#todo-form').addEventListener('submit', e => {
  e.preventDefault();
  const text = $('#todo-input').value.trim();
  if (!text) return;
  saveTodos([...loadTodos(), { text, done: false }]);
  $('#todo-input').value = '';
  renderTodos();
});

// ---------------------------------------------------------------- timer (pomodoro + stopwatch)
let timerMode = localStorage.getItem('end4-pC.timerMode') || 'pomo';
function renderTimer() {
  const s = timer.load();
  const modes = `<div class="chips mode">${[['pomo', 'Pomodoro'], ['sw', 'Stopwatch']].map(([m, l]) =>
    `<button class="chip${m === timerMode ? ' sel' : ''}" data-tmode="${m}">${l}</button>`).join('')}</div>`;
  if (timerMode === 'pomo') {
    const left = timer.left(s, cfg);
    return setHTML($('#page-timer'), `${modes}
      <div class="dial-wrap">${ring(left / timer.total(s, cfg), { size: 200, stroke: 10 })}
        <div class="mid"><b>${timer.clock(left)}</b><span>${timer.PHASES[s.phase]} · ${(s.phase === 'focus' ? s.cycle : s.cycle - 1) % (cfg.timer?.cycles || 4) + 1}/${cfg.timer?.cycles || 4}</span></div></div>
      <div class="t-ctl">
        <button class="icon-btn tonal" data-t="pomo-reset" title="Reset">${icon('replay')}</button>
        <button class="icon-btn filled" data-t="pomo-toggle" title="${s.running ? 'Pause' : 'Start'}">${icon(s.running ? 'pause' : 'play_arrow', { fill: true })}</button>
        <button class="icon-btn tonal" data-t="pomo-skip" title="Skip to next phase">${icon('skip_next')}</button>
      </div>`);
  }
  const e = timer.swElapsed(s);
  setHTML($('#page-timer'), `${modes}
    <div class="sw-big">${timer.clock(e)}.${Math.floor((e % 1) * 10)}</div>
    <div class="t-ctl">
      <button class="icon-btn tonal" data-t="sw-reset" title="Reset">${icon('replay')}</button>
      <button class="icon-btn filled" data-t="sw-toggle" title="${s.sw.running ? 'Pause' : 'Start'}">${icon(s.sw.running ? 'pause' : 'play_arrow', { fill: true })}</button>
      <button class="icon-btn tonal" data-t="sw-lap" title="Lap"${s.sw.running ? '' : ' disabled'}>${icon('flag')}</button>
    </div>
    <div class="laps scroll">${s.sw.laps.map((l, i) => `<div><span>Lap ${s.sw.laps.length - i}</span><span>${timer.clock(l)}.${Math.floor((l % 1) * 10)}</span></div>`).join('')}</div>`);
}
$('#page-timer').addEventListener('click', e => {
  const b = e.target.closest('button');
  if (b?.dataset.tmode) { timerMode = b.dataset.tmode; localStorage.setItem('end4-pC.timerMode', timerMode); }
  const t = b?.dataset.t;
  if (t === 'pomo-toggle') timer.pomo.toggle(cfg);
  if (t === 'pomo-reset') timer.pomo.reset();
  if (t === 'pomo-skip') timer.pomo.skip(cfg);
  if (t === 'sw-toggle') timer.sw.toggle();
  if (t === 'sw-lap') timer.sw.lap();
  if (t === 'sw-reset') timer.sw.reset();
  renderTimer();
});
setInterval(() => $('#page-timer').classList.contains('sel') && renderTimer(), 100);

function showTab(tab) {
  if (!$(`.tabs [data-tab="${tab}"]`)) tab = 'notif';
  $$('.tabs button').forEach(b => b.classList.toggle('sel', b.dataset.tab === tab));
  $$('.page').forEach(p => p.classList.toggle('sel', p.id === 'page-' + tab));
  localStorage.setItem('end4-pC.rightTab', tab);
}
$('.tabs').addEventListener('click', e => {
  const tab = e.target.closest('[data-tab]')?.dataset.tab;
  if (tab) showTab(tab);
});
showTab(localStorage.getItem('end4-pC.rightTab') || 'notif');

// ---------------------------------------------------------------- notifications (Windows' notification history)
let notifs = [], notifErr = '';
const ago = t => {
  const s = Date.now() / 1000 - t;
  return s < 60 ? 'now' : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : new Date(t * 1000).toLocaleDateString([], { day: 'numeric', month: 'short' });
};
function renderNotifs() {
  setHTML($('#notif-count'), notifs.length ? String(notifs.length) : '');
  if (notifErr) return setHTML($('#notif-list'), `<div class="err">${esc(notifErr)}</div>`);
  if (!notifs.length) return setHTML($('#notif-list'), `<div class="empty">${icon('notifications_off')}Nothing</div>`);
  const groups = [];
  for (const n of notifs) {
    const g = groups.find(x => x.app === n.app);
    g ? g.items.push(n) : groups.push({ app: n.app, items: [n] });
  }
  setHTML($('#notif-list'), groups.map(g => `
    <div class="ngroup">
      <div class="ngroup-head"><span class="app">${esc(g.app.charAt(0))}</span>${esc(g.app)}<span class="sp"></span>
        <button data-dismiss="${g.items.map(i => i.id).join(',')}" title="Dismiss">${icon('close')}</button></div>
      ${g.items.slice(0, 5).map(n => `<div class="notif"><div class="tx"><b>${esc(n.title)}</b>${n.body ? `<span>${esc(n.body)}</span>` : ''}</div><time>${ago(n.time)}</time></div>`).join('')}
      ${g.items.length > 5 ? `<div class="muted" style="font-size:11.5px;padding-top:4px">+${g.items.length - 5} more</div>` : ''}
    </div>`).join(''));
}
async function loadNotifs() {
  const r = await bridge('notifications');
  notifErr = r.error && !(r.items || []).length ? r.error : '';
  notifs = r.items || [];
  renderNotifs();
}
$('#notif-list').addEventListener('click', e => {
  const ids = e.target.closest('[data-dismiss]')?.dataset.dismiss;
  if (!ids) return;
  const set = new Set(ids.split(','));
  notifs = notifs.filter(n => !set.has(n.id));
  renderNotifs();
  bridge('notifications-dismiss', ...set);
});
$('#notif-clear').addEventListener('click', () => { notifs = []; renderNotifs(); bridge('notifications-dismiss', 'all'); });
$('#notif-center').addEventListener('click', () => run('toggles', '-Action', 'actioncenter'));

// ---------------------------------------------------------------- header buttons
$('header').addEventListener('contextmenu', e => {
  if (e.target.closest('[data-cmd="settings"]')) { e.preventDefault(); run('toggles', '-Action', 'settings', '-Value', ''); }
});
$('header').addEventListener('click', e => {
  const c = e.target.closest('[data-cmd]')?.dataset.cmd;
  if (c === 'settings') { openWidget('settings'); closeWindow(); }
  if (c === 'reload') out.glazewm?.runCommand('wm-reload-config');
  if (c === 'power') $('#power').classList.toggle('open');
});
$('#power').addEventListener('click', e => {
  const b = e.target.closest('[data-p]');
  if (!b) return;
  // Restart / shut down need a second click, like end4's session screen.
  if ((b.dataset.p === 'restart' || b.dataset.p === 'shutdown') && !b.dataset.armed) {
    b.dataset.armed = '1';
    b.lastChild.textContent = 'Confirm';
    return;
  }
  run('toggles', '-Action', b.dataset.p);
});

// ---------------------------------------------------------------- go
function render() {
  out = providers.outputMap;
  renderHeader();
  renderToggles();
  renderSliders();
}
providers.onOutput(render);
render();
watchTheme(t => { sys.darkMode = t.mode === 'dark'; renderToggles(); });
renderCalendar();
renderTodos();
renderTimer();
loadNotifs();
setInterval(loadNotifs, 5000);
