import {
  zebar, $, esc, icon, ring, setHTML, config, watchTheme, run, openHere, mediaClock,
  weatherIcon, weatherText, temp, volumeIcon, networkIcon, batteryIcon, prettyApp, reloadOnConfigChange, bridgeStream, bridge, monthGrid, albumArt, greeting,
} from './lib/core.js';
import { drawBars } from './lib/viz.js';
import * as timer from './lib/timer.js';
import { barLayout } from './lib/barlayout.js';

const cfg = await config();
reloadOnConfigChange();

// ---------------------------------------------------------------- layout (Settings > Bar)
// Modules move into their side in the saved order; hidden ones stay in the page (hidden) so the
// rendering code doesn't have to care.
const MODULE_EL = {
  launcher: ['#btn-left'], window: ['.aw'], resources: ['#g-res'], media: ['#g-media'], island: ['#g-island'], workspaces: ['#g-ws'],
  clock: ['#g-clock', '#g-timer', '#g-rec'], utils: ['#g-utils'], weather: ['#g-weather'], wm: ['#g-wm'],
  netspeed: ['#g-net'], tray: ['#g-tray'], system: ['#g-sys'],
};
const SIDE_EL = { left: '#bar > .left', center: '#bar > .center', right: '#bar > .right' };
const layout = barLayout(cfg.bar.modules, { netSpeed: cfg.showNetSpeed });
for (const [id, side] of layout)
  for (const sel of MODULE_EL[id]) {
    const el = $(sel);
    el.hidden = side === 'hidden';
    if (!el.hidden) $(SIDE_EL[side]).append(el);
  }
const netShown = layout.some(m => m[0] === 'netspeed' && m[1] !== 'hidden');
const islandShown = layout.some(m => m[0] === 'island' && m[1] !== 'hidden');
if (islandShown) for (const sel of ['#g-media', '#g-timer', '#g-rec']) $(sel).hidden = true;   // the island shows these
$('#bar').classList.add(cfg.bar.style, 'g-' + cfg.bar.groups);
for (const b of document.querySelectorAll('#g-utils [data-act]')) b.hidden = !cfg.bar.utils.includes(b.dataset.act);
await watchTheme(t => setHTML($('#mode-icon'), t.mode === 'dark' ? 'dark_mode' : 'light_mode'));

const providers = zebar.createProviderGroup({
  glazewm: { type: 'glazewm' },
  media: { type: 'media' },
  cpu: { type: 'cpu', refreshInterval: 3000 },
  memory: { type: 'memory', refreshInterval: 3000 },
  battery: { type: 'battery', refreshInterval: 15000 },
  network: { type: 'network', refreshInterval: netShown ? 2000 : 5000 },
  audio: { type: 'audio' },
  weather: { type: 'weather', refreshInterval: 30 * 60 * 1000 },
  ip: { type: 'ip', refreshInterval: 6 * 60 * 60 * 1000 },
  date: { type: 'date', formatting: cfg.clockFormat, refreshInterval: 1000 },
  systray: { type: 'systray' },
  keyboard: { type: 'keyboard', refreshInterval: 2000 },
});

const clock = mediaClock();
let out = providers.outputMap;

// ---------------------------------------------------------------- render
function renderActiveWindow() {
  const wm = out.glazewm;
  const c = wm?.focusedContainer;
  if (c?.type === 'window') {
    setHTML($('#aw-app'), esc(prettyApp(c.processName)));
    setHTML($('#aw-title'), esc(c.title || prettyApp(c.processName)));
  } else {
    const ws = wm?.displayedWorkspace;
    setHTML($('#aw-app'), 'Desktop');
    setHTML($('#aw-title'), ws ? `Workspace ${esc(ws.displayName || ws.name)}` : 'Windows');
  }
}

function renderResources() {
  const cpu = out.cpu?.usage ?? 0, mem = out.memory?.usage ?? 0;
  setHTML($('#g-res'), `
    <div class="res" title="CPU"><div class="ring-wrap">${ring(cpu / 100, { size: 24, stroke: 2.5 })}${icon('memory')}</div>${Math.round(cpu)}</div>
    <div class="res" title="RAM"><div class="ring-wrap">${ring(mem / 100, { size: 24, stroke: 2.5 })}${icon('memory_alt')}</div>${Math.round(mem)}</div>`);
}

// Dynamic Island (end4's DiOsd): for a moment after the volume or mic changes, the media pill
// shows the new level, then goes back to the song.
let osd = null, osdTimer, lastAudio = null;
function watchAudio() {
  const a = out.audio?.defaultPlaybackDevice, m = out.audio?.defaultRecordingDevice;
  if (!a) return;
  const now = `${Math.round(a.volume)}|${a.isMuted}|${!!m?.isMuted}`;
  if (lastAudio !== null && now !== lastAudio) {
    osd = lastAudio.split('|')[2] !== String(!!m?.isMuted)
      ? { icon: m.isMuted ? 'mic_off' : 'mic', text: m.isMuted ? 'Microphone muted' : 'Microphone on' }
      : { icon: volumeIcon(a), value: a.isMuted ? 0 : a.volume, text: a.isMuted ? 'Muted' : `${Math.round(a.volume)}%` };
    clearTimeout(osdTimer);
    osdTimer = setTimeout(() => { osd = null; renderMedia(); renderIsland(); }, 1800);
  }
  lastAudio = now;
}

function renderMedia() {
  const s = out.media?.currentSession;
  clock.update(s);
  $('#g-media').classList.toggle('osd', !!osd);
  if (osd) return setHTML($('#g-media'), `${icon(osd.icon, { fill: true })}${osd.value != null
    ? `<span class="osd-bar"><span style="width:${Math.round(osd.value)}%"></span></span>` : ''}<span class="title">${osd.text}</span>`);
  if (!s || !s.title) return setHTML($('#g-media'), '');
  const dur = (s.endTime || 0) - (s.startTime || 0);
  const prog = dur > 0 ? clock.position() / dur : 0;
  const text = s.artist ? `${s.title} • ${s.artist}` : s.title;
  setHTML($('#g-media'), `
    <div class="ring-wrap">${ring(prog, { size: 26, stroke: 2.5 })}${icon(s.isPlaying ? 'pause' : 'play_arrow', { fill: true })}</div>
    <span class="title ellipsis">${esc(text)}</span>`);
}

// Workspaces 1-5 are bound to the main monitor and 6-10 to the other (bind_to_monitor in GlazeWM's
// config), so each bar shows only its own monitor's set.
// shortcut: a 3rd monitor shows the same 6-10 set; give it its own range if anyone runs three.
function myWorkspaces(wm) {
  const all = Array.from({ length: cfg.workspaces }, (_, i) => i + 1);
  if ((wm.allMonitors?.length || 1) < 2) return all;
  const m = wm.currentMonitor;
  return all.filter(i => (m && (m.x || m.y) ? i > 5 : i <= 5));
}

function renderWorkspaces() {
  const wm = out.glazewm;
  if (!wm) return setHTML($('#g-ws'), '');
  const shown = wm.displayedWorkspace?.name;
  const byName = Object.fromEntries((wm.allWorkspaces || []).map(w => [w.name, w]));
  let html = '';
  for (const i of myWorkspaces(wm)) {
    const w = byName[String(i)];
    const occ = w && w.children?.length > 0;
    const active = shown === String(i);
    const label = esc(cfg.workspaceLabels?.[i - 1] || i);   // end4 presets' numberMap (e.g. 一 二 三)
    const showLabel = active || cfg.alwaysShowNumbers;
    html += `<button class="ws${occ ? ' occ' : ''}${active ? ' active' : ''}${showLabel ? ' lbl' : ''}" data-ws="${i}" title="Workspace ${i}">${showLabel ? label : '<span class="dot"></span>'}</button>`;
  }
  setHTML($('#g-ws'), html);
}

function renderClock() {
  setHTML($('#g-clock'), esc(out.date?.formatted || ''));
}

function renderWeather() {
  const w = out.weather;
  setHTML($('#g-weather'), w ? `${icon(weatherIcon(w.status))}<span>${temp(w, cfg.tempUnit)}</span>` : '');
}

function renderWm() {
  const wm = out.glazewm;
  if (!wm) return setHTML($('#g-wm'), '');
  let html = '';
  if (wm.isPaused) html += `<button class="chip paused" data-wm="wm-toggle-pause">${icon('pause_circle')} Tiling paused</button>`;
  for (const m of wm.bindingModes || [])
    html += `<button class="chip" data-wm="wm-disable-binding-mode --name ${esc(m.name)}">${esc(m.displayName || m.name)}</button>`;
  setHTML($('#g-wm'), html);
  $('#g-wm').style.display = html ? '' : 'none';
}

function renderTray() {
  const icons = out.systray?.icons || [];
  setHTML($('#g-tray'), icons.map(i => `<img src="${i.iconUrl}" data-tray="${esc(i.id)}" title="${esc(i.tooltip)}" />`).join(''));
}

function renderSystem() {
  const a = out.audio?.defaultPlaybackDevice;
  const mic = out.audio?.defaultRecordingDevice;
  const b = out.battery;
  const hasBattery = b && b.state !== 'unknown' && b.chargePercent != null;
  let html = '';
  if (cfg.showKeyboardLayout && out.keyboard?.layout) html += `<span class="kb">${esc(out.keyboard.layout)}</span>`;
  if (mic?.isMuted) html += icon('mic_off');
  html += `<span title="${a ? Math.round(a.volume) + '%' : ''}">${icon(volumeIcon(a))}</span>`;
  html += `<span title="${esc(out.network?.defaultGateway?.ssid || '')}">${icon(networkIcon(out.network))}</span>`;
  if (hasBattery)
    html += `<span class="batt${b.chargePercent < 20 && !b.isCharging ? ' low' : ''}">${icon(batteryIcon(b))}${Math.round(b.chargePercent)}</span>`;
  setHTML($('#g-sys'), html);
}

// Pomodoro / stopwatch chip (end4's Dynamic Island timer). Whichever bar sees a phase end first
// ends it and shows the toast; the "done" chip is shared so every monitor's bar shows it.
// Screen recording chip (started from the right sidebar). Checks now and then that ffmpeg is
// still running, so a recording that died doesn't leave the chip behind.
function renderRec() {
  const started = Number(localStorage.getItem('end4-pC.rec')) || 0;
  setHTML($('#g-rec'), started ? `${icon('stop_circle', { fill: true })}${timer.clock((Date.now() - started) / 1000)}` : '');
}
setInterval(async () => {
  if (localStorage.getItem('end4-pC.rec') && !(await bridge('record')).recording) { localStorage.removeItem('end4-pC.rec'); renderRec(); }
}, 15000);
$('#g-rec').addEventListener('click', async () => {
  localStorage.removeItem('end4-pC.rec');
  renderRec();
  await bridge('record', 'stop');   // opens Explorer on the new file
});

// The timer chip's content ('' when no timer runs); also used by the island.
function timerChip() {
  const timerDone = localStorage.getItem('end4-pC.timerDone'), s = timer.load();
  if (timerDone) return { done: true, html: `${icon('alarm', { fill: true })}${timerDone === 'focus' ? 'Break time' : 'Focus time'}` };
  if (s.running) return { html: `${icon(s.phase === 'focus' ? 'timer' : 'coffee')}${timer.clock(timer.left(s, cfg))}` };
  if (s.sw.running) return { html: `${icon('timer_play')}${timer.clock(timer.swElapsed(s))}` };
  return { html: '' };
}
function renderTimer() {
  const done = timer.tick(cfg);
  if (done) { localStorage.setItem('end4-pC.timerDone', done); run('toggles', '-Action', 'timer-done', '-Value', done); }
  const t = timerChip();
  $('#g-timer').classList.toggle('done', !!t.done);
  setHTML($('#g-timer'), t.html);
}

const rate = b => (b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB/s` : `${Math.round(b / 1e3)} KB/s`);
function renderNet() {
  const t = netShown && out.network?.traffic;
  setHTML($('#g-net'), t ? `<span>${icon('arrow_upward')}${rate(t.transmitted.bytes)}</span><span>${icon('arrow_downward')}${rate(t.received.bytes)}</span>` : '');
}

// ---------------------------------------------------------------- Dynamic Island (end4's DynamicIsland)
// One pill for the most important thing going on: a new notification or a volume change for a
// moment, else recording, timer or media (the others as badges beside it; click one to switch),
// else the time (or weather, when the clock module is on the bar) and status icons. Hovering media
// shows its controls, and so does a track change, for 3 s.
let diNotif = null, diNotifTimer, diFocus = '', diHover = false, diTrack = '', diTrackUntil = 0, diArt = '';
const DI_BADGE = { recording: 'screen_record', timer: 'timer', media: 'music_note', agent: 'smart_toy' };
// AI agents (scripts/end4_agent.py -> bridge agents-watch). One bar runs the watcher and shares the
// list through localStorage, which the other bars and the island panel read.
let agents = JSON.parse(localStorage.getItem('end4.agents') || '[]');
function diProviders() {
  const s = out.media?.currentSession;
  const asks = agents.some(a => a.state === 'waiting');
  // An agent waiting for you comes before everything but a new notification; one at work only
  // takes the island when nothing else does.
  return [['notification', !!diNotif, true], ['osd', !!osd, true], ['agent', asks], ['recording', !!localStorage.getItem('end4-pC.rec')],
    ['timer', !!timerChip().html], ['media', !!s?.title], ['agent', !asks && agents.some(a => a.state === 'working')]].filter(p => p[1]);
}
// Sets the HTML and springs the width from the old size to the new one.
function morph(el, html) {
  if (el.__html === html) return;
  const from = el.offsetWidth;
  el.style.width = '';
  setHTML(el, html);
  const to = el.offsetWidth;
  if (!from || from === to) return;
  el.style.width = `${from}px`;
  el.offsetWidth;   // reflow, so the change below animates
  el.style.width = `${to}px`;
  clearTimeout(el.__morph);
  el.__morph = setTimeout(() => (el.style.width = ''), 450);
}
function renderIsland() {
  if (!islandShown) return;
  const list = diProviders(), win = list.find(p => p[2]);
  const main = win || list.find(p => p[0] === diFocus) || list[0];
  const state = main?.[0] || 'idle';
  const s = out.media?.currentSession;
  const key = s?.title ? `${s.title}|${s.artist}` : '';
  if (key !== diTrack) {
    if (diTrack && key) diTrackUntil = Date.now() + 3000;
    diTrack = key; diArt = '';
    if (key) albumArt(s).then(u => { if (diTrack === key) { diArt = u; renderIsland(); } });
  }
  const t = timerChip();
  let html = '', wide = false;
  if (state === 'notification') {
    wide = true;
    html = `${icon('notifications', { fill: true, cls: 'big' })}<span class="di-text ellipsis"><b>${esc(diNotif.app)}</b> · ${esc(diNotif.title || diNotif.body)}</span>`;
  } else if (state === 'agent') {
    const w = agents.filter(a => a.state === 'waiting'), a = w[0] || agents.find(x => x.state === 'working');
    const u = a.usage?.five_hour || Object.values(a.usage || {})[0];
    const more = w.reduce((n, x) => n + x.waiting, 0) - 1;   // other sessions waiting too
    wide = !!w.length;
    html = w.length
      ? `<span class="di-dot"></span>${icon('smart_toy', { fill: true, cls: 'big' })}<span class="di-text ellipsis"><b>${esc(a.label)}</b> · ${esc(a.message || 'Needs your input')}</span>${more > 0 ? `<span>+${more}</span>` : ''}`
      : `${u ? `<div class="ring-wrap">${ring(u.used_percentage / 100, { size: 28, stroke: 2.5 })}${icon('smart_toy', { fill: true })}</div>` : icon('smart_toy', { fill: true, cls: 'big' })}<span class="di-text ellipsis">${esc(a.label)}${u ? ` · ${Math.round(u.used_percentage)}%` : ''}</span>`;
  } else if (state === 'osd') {
    html = `${icon(osd.icon, { fill: true, cls: 'big' })}${osd.value != null ? `<span class="osd-bar"><span style="width:${Math.round(osd.value)}%"></span></span>` : ''}<span>${osd.text}</span>`;
  } else if (state === 'recording') {
    html = `<span class="di-dot"></span>Recording ${timer.clock((Date.now() - Number(localStorage.getItem('end4-pC.rec'))) / 1000)}`;
  } else if (state === 'timer') {
    html = t.html;
  } else if (state === 'media') {
    wide = diHover || Date.now() < diTrackUntil;
    const dur = (s.endTime || 0) - (s.startTime || 0);
    html = `<div class="ring-wrap">${ring(dur > 0 ? clock.position() / dur : 0, { size: 28, stroke: 2.5 })}${diArt ? `<img src="${diArt}" alt="" />` : icon('music_note', { fill: true })}</div>
      <span class="di-text ellipsis">${esc(wide && s.artist ? `${s.title} • ${s.artist}` : s.title)}</span>
      ${wide ? `<span class="di-ctl"><button data-ctl="previous">${icon('skip_previous', { fill: true })}</button><button data-ctl="togglePlayPause">${icon(s.isPlaying ? 'pause' : 'play_arrow', { fill: true })}</button><button data-ctl="next">${icon('skip_next', { fill: true })}</button></span>` : ''}`;
  } else {
    const a = out.audio, w = out.weather;
    const onBar = id => layout.some(m => m[0] === id && m[1] !== 'hidden');
    const status = [a?.defaultRecordingDevice?.isMuted && 'mic_off', a?.defaultPlaybackDevice?.isMuted && 'volume_off',
      out.network && !out.network.defaultInterface && 'wifi_off'].filter(Boolean);
    // Whatever the bar doesn't already show: the time, else the weather, else a greeting.
    html = `${!onBar('clock') ? `<span>${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: !cfg.use24h })}</span>`
      : w && !onBar('weather') ? `${icon(weatherIcon(w.status), { cls: 'big' })}<span>${temp(w, cfg.tempUnit)}</span>`
      : `${icon('waving_hand', { cls: 'big' })}<span>${greeting()}</span>`}
      ${status.length ? `<span class="di-status">${status.map(i => icon(i)).join('')}</span>` : ''}`;
  }
  const badges = win ? [] : list.filter(p => p !== main);
  const el = $('#g-island');
  el.dataset.state = state;
  el.classList.toggle('wide', wide);
  el.classList.toggle('done', state === 'timer' && !!t.done);
  el.classList.toggle('ask', state === 'agent' && wide);
  morph(el, `<div class="di-main">${html}</div>${badges.map(([id]) => `<button class="di-badge" data-di-focus="${id}" title="Show ${id}">${icon(DI_BADGE[id], { fill: true })}</button>`).join('')}`);
}

function render() {
  out = providers.outputMap;
  watchAudio();
  renderNet();
  renderActiveWindow();
  renderResources();
  renderMedia();
  renderWorkspaces();
  renderClock();
  renderWeather();
  renderWm();
  renderTray();
  renderSystem();
  renderIsland();
}

providers.onOutput(render);
render();
// Keep the media progress ring moving between provider updates.
setInterval(() => { out.media?.currentSession?.isPlaying && renderMedia(); renderTimer(); renderRec(); renderIsland(); }, 1000);
renderTimer();

// ---------------------------------------------------------------- events
const here = name => openHere(name, out.glazewm?.currentMonitor);   // popups open on this bar's monitor

// ---------------------------------------------------------------- hover popups (end4's bar popups)
// One "bar-popup" window serves every bar (bar-popup.html); this bar sends it the hovered group's
// details and the spot under that group.
zebar.startWidgetPreset('bar-popup', 'default').catch(() => {});   // ignored when it's already open
const popChan = new BroadcastChannel('end4-bar-popup');
const gb = n => `${(n / 1073741824).toFixed(1)} GB`;
const span = ms => { const m = Math.round(ms / 60000); return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}m`; };
const kv = (ic, k, v) => `<div class="kv"><span>${icon(ic)}${k}</span><b>${v}</b></div>`;
const POPUPS = {
  'g-clock': () => {
    const now = new Date();
    return `<h4>${icon('calendar_month')}${now.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })}</h4>
      <div class="cal">${['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'].map(d => `<span class="dow">${d}</span>`).join('')}
      ${monthGrid(now).map(c => `<span class="${c.out ? 'out' : ''}${c.today ? ' today' : ''}">${c.d}</span>`).join('')}</div>`;
  },
  'g-res': () => {
    const c = out.cpu, m = out.memory;
    return `<h4>${icon('monitoring')}Resources</h4>${kv('memory', 'CPU', `${Math.round(c?.usage ?? 0)}%`)}
      ${c ? kv('speed', 'Clock', `${(c.frequency / 1000).toFixed(2)} GHz`) + kv('developer_board', 'Cores', `${c.physicalCoreCount} cores, ${c.logicalCoreCount} threads`) : ''}
      ${m ? kv('memory_alt', 'RAM', `${gb(m.usedMemory)} of ${gb(m.totalMemory)}`) : ''}
      ${m?.totalSwap ? kv('swap_horiz', 'Page file', `${gb(m.usedSwap)} of ${gb(m.totalSwap)}`) : ''}`;
  },
  'g-weather': () => {
    const w = out.weather;
    return w && `<h4>${icon(weatherIcon(w.status))}${esc(out.ip?.approxCity || 'Weather')}</h4><div class="big">${temp(w, cfg.tempUnit)}</div>
      ${kv('cloud', 'Sky', esc(weatherText(w.status)))}${w.windSpeed != null ? kv('air', 'Wind', `${(+w.windSpeed).toFixed(1)} km/h`) : ''}`;
  },
  'g-net': () => {
    const n = out.network, t = n?.traffic, ip = n?.defaultInterface?.ipv4Addresses?.[0];
    return t && `<h4>${icon(networkIcon(n))}${esc(n.defaultGateway?.ssid || n.defaultInterface?.friendlyName || 'Network')}</h4>
      ${kv('arrow_downward', 'Download', rate(t.received.bytes))}${kv('arrow_upward', 'Upload', rate(t.transmitted.bytes))}
      ${kv('data_usage', 'Since start-up', `${gb(t.totalReceived.bytes)} down, ${gb(t.totalTransmitted.bytes)} up`)}${ip ? kv('lan', 'IP', esc(ip)) : ''}`;
  },
  'g-sys': () => {
    const a = out.audio?.defaultPlaybackDevice, n = out.network, b = out.battery;
    const batt = b && b.state !== 'unknown' && b.chargePercent != null;
    return `<h4>${icon('tune')}Quick settings <span class="muted" style="font-weight:400">(click to open)</span></h4>
      ${kv(volumeIcon(a), 'Volume', a ? `${Math.round(a.volume)}%${a.isMuted ? ', muted' : ''}` : 'No device')}
      ${kv(networkIcon(n), 'Network', esc(n?.defaultGateway?.ssid || (n?.defaultInterface ? prettyNet(n.defaultInterface.type) : 'Offline')))}
      ${batt ? kv(batteryIcon(b), 'Battery', `${Math.round(b.chargePercent)}%, ${b.state}`) : ''}
      ${batt && b.isCharging && b.timeTillFull ? kv('schedule', 'Full in', span(b.timeTillFull)) : ''}
      ${batt && !b.isCharging && b.timeTillEmpty ? kv('schedule', 'Time left', span(b.timeTillEmpty)) : ''}
      ${batt && b.healthPercent ? kv('health_and_safety', 'Health', `${Math.round(b.healthPercent)}%`) : ''}`;
  },
};
const prettyNet = t => ({ ethernet: 'Ethernet', wifi: 'Wi-Fi', proprietary_virtual: 'VPN' })[t] || 'Connected';

let hoverId = '', hoverTimer;
async function postPopup() {
  const el = $('#' + hoverId), html = hoverId && POPUPS[hoverId]();
  if (!el || !html) return;
  const r = el.getBoundingClientRect(), dpr = devicePixelRatio;
  const p = await zebar.currentWidget().window.tauri.outerPosition();   // physical px
  popChan.postMessage({ show: true, id: hoverId, html, x: p.x + (r.left + r.width / 2) * dpr, y: p.y + (r.bottom + 4) * dpr, minX: p.x });
}
const hidePopup = () => { clearTimeout(hoverTimer); if (hoverId) popChan.postMessage({ show: false, id: hoverId }); hoverId = ''; };
for (const id of Object.keys(POPUPS)) {
  const el = $('#' + id);
  el.addEventListener('mouseenter', () => { clearTimeout(hoverTimer); hoverTimer = setTimeout(() => { hoverId = id; postPopup(); }, 350); });
  el.addEventListener('mouseleave', hidePopup);
  el.addEventListener('mousedown', hidePopup);   // a click opens a sidebar instead
}
setInterval(() => hoverId && postPopup(), 1000);   // live numbers while it's open
window.addEventListener('beforeunload', hidePopup);
localStorage.setItem('end4.barAlive', Date.now());
setInterval(() => localStorage.setItem('end4.barAlive', Date.now()), 2000);
$('#btn-left').addEventListener('click', () => here('sidebar-left'));
$('#g-sys').addEventListener('click', () => here('sidebar-right'));
$('#g-clock').addEventListener('click', () => here('sidebar-right'));
$('#g-timer').addEventListener('click', () => {
  localStorage.removeItem('end4-pC.timerDone');
  localStorage.setItem('end4-pC.rightTab', 'timer');
  renderTimer();
  here('sidebar-right');
});

$('#g-ws').addEventListener('click', e => {
  const b = e.target.closest('[data-ws]');
  if (b) out.glazewm?.runCommand(`focus --workspace ${b.dataset.ws}`);
});
$('#g-ws').addEventListener('wheel', e => {
  // GlazeWM's next/prev-active-workspace crosses monitors; cycle this monitor's open workspaces instead.
  const wm = out.glazewm, mine = myWorkspaces(wm || {}).map(String);
  const open = (wm?.currentMonitor?.children || []).map(w => w.name).filter(n => mine.includes(n)).sort((a, b) => a - b);
  const i = open.indexOf(wm?.displayedWorkspace?.name);
  if (open.length > 1) wm.runCommand(`focus --workspace ${open[(i + (e.deltaY > 0 ? 1 : -1) + open.length) % open.length]}`);
}, { passive: true });
$('#g-ws').addEventListener('contextmenu', e => { e.preventDefault(); here('overview'); });

const island = $('#g-island');
island.addEventListener('mouseenter', () => { diHover = true; renderIsland(); });
island.addEventListener('mouseleave', () => { diHover = false; renderIsland(); });
island.addEventListener('click', e => {
  const focus = e.target.closest('[data-di-focus]')?.dataset.diFocus;
  if (focus) { diFocus = focus; return renderIsland(); }
  const ctl = e.target.closest('[data-ctl]')?.dataset.ctl;
  if (ctl) return out.media?.[ctl]();
  openIslandPanel();
});
// Click: the island expands into a panel under it (island-panel.html). The click that closed the
// panel (it closes when it loses focus) mustn't open it again.
async function openIslandPanel() {
  if (Date.now() - Number(localStorage.getItem('end4.closedAt.island-panel') || 0) < 600) return;
  diNotif = null;
  renderIsland();
  const r = island.getBoundingClientRect(), dpr = devicePixelRatio;
  const p = await zebar.currentWidget().window.tauri.outerPosition();
  localStorage.setItem('end4.islandAt', JSON.stringify({ x: p.x + (r.left + r.width / 2) * dpr, y: p.y + (r.bottom + 2) * dpr }));
  zebar.startWidgetPreset('island-panel', 'default').catch(e => console.warn(e));
}
island.addEventListener('contextmenu', e => { e.preventDefault(); here('sidebar-left'); });   // player and lyrics
island.addEventListener('wheel', e => {   // like the volume icon: scroll for volume
  const a = out.audio, dev = a?.defaultPlaybackDevice;
  if (dev) a.setVolume(Math.max(0, Math.min(100, Math.round(dev.volume + (e.deltaY < 0 ? 5 : -5)))));
}, { passive: true });

const media = $('#g-media');
media.addEventListener('click', () => out.media?.togglePlayPause());
media.addEventListener('auxclick', e => e.button === 1 && out.media?.next());
media.addEventListener('contextmenu', e => { e.preventDefault(); here('sidebar-left'); });

$('#g-utils').addEventListener('click', e => {
  const act = e.target.closest('[data-act]')?.dataset.act;
  if (act === 'snip') run('toggles', '-Action', 'snip');
  if (act === 'pick') run('toggles', '-Action', 'colorpick');
  if (act === 'wall') run('switchwall');
  if (act === 'mode') run('switchwall', '-ToggleMode');
});

$('#g-wm').addEventListener('click', e => {
  const c = e.target.closest('[data-wm]')?.dataset.wm;
  if (c) out.glazewm?.runCommand(c);
});

const tray = $('#g-tray');
tray.addEventListener('click', e => { const id = e.target.dataset.tray; if (id) out.systray?.onLeftClick(id); });
tray.addEventListener('contextmenu', e => { e.preventDefault(); const id = e.target.dataset.tray; if (id) out.systray?.onRightClick(id); });
tray.addEventListener('auxclick', e => { const id = e.target.dataset.tray; if (id && e.button === 1) out.systray?.onMiddleClick(id); });
tray.addEventListener('dblclick', e => { const id = e.target.dataset.tray; if (id) out.systray?.onLeftDoubleClick(id); });

$('#g-sys').addEventListener('wheel', e => {
  const a = out.audio;
  const dev = a?.defaultPlaybackDevice;
  if (!dev) return;
  a.setVolume(Math.max(0, Math.min(100, Math.round(dev.volume + (e.deltaY < 0 ? 5 : -5)))));
}, { passive: true });

// ---------------------------------------------------------------- optional bar visualizer (end4's bar "Visualizer" module)
const vizAnchor = islandShown ? '#g-island' : '#g-media';
if (cfg.visualizer.bar && !$(vizAnchor).hidden) {
  const canvas = document.createElement('canvas');
  canvas.id = 'bar-viz';
  $(vizAnchor).after(canvas);
  let levels = [];
  const proc = await bridgeStream('visualizer', [20], line => { levels = line.trim().split(' ').map(Number); }, { raw: true });
  window.addEventListener('beforeunload', () => proc?.kill());
  (function frame() { drawBars(canvas, levels, null, { style: 'bar' }); requestAnimationFrame(frame); })();
}

// ---------------------------------------------------------------- Wallpaper Engine: re-theme when its wallpaper changes
// (one watcher for all bars; the bridge exits at once if Wallpaper Engine isn't installed)
if (cfg.wallpaperEngine.follow) {
  const we = await bridgeStream('we-sync', [], () => {});
  window.addEventListener('beforeunload', () => we?.kill());
}

// ---------------------------------------------------------------- island: AI agents
window.addEventListener('storage', e => { if (e.key === 'end4.agents') { agents = JSON.parse(e.newValue || '[]'); renderIsland(); } });
if (islandShown) {
  const watch = await bridgeStream('agents-watch', [], m => {
    if (!m.agents) return;
    agents = m.agents;
    localStorage.setItem('end4.agents', JSON.stringify(agents));
    renderIsland();
  });
  window.addEventListener('beforeunload', () => watch?.kill());
}

// ---------------------------------------------------------------- island: new notifications
// One bar runs the watcher (notifications-watch allows one copy) and passes each new
// notification to the other bars.
const notifChan = new BroadcastChannel('end4-notifications');
function showNotification(n) {
  diNotif = n;
  clearTimeout(diNotifTimer);
  diNotifTimer = setTimeout(() => { diNotif = null; renderIsland(); }, 5000);
  renderIsland();
}
notifChan.onmessage = e => showNotification(e.data);
if (islandShown) {
  const watch = await bridgeStream('notifications-watch', [], m => { if (m.new) { showNotification(m.new); notifChan.postMessage(m.new); } });
  window.addEventListener('beforeunload', () => watch?.kill());
}
