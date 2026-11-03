import {
  zebar, $, esc, icon, ring, setHTML, config, watchTheme, run, openWidget, mediaClock,
  weatherIcon, temp, volumeIcon, networkIcon, batteryIcon, prettyApp, reloadOnConfigChange, bridgeStream,
} from './lib/core.js';
import { drawBars } from './lib/viz.js';

const cfg = await config();
reloadOnConfigChange();
await watchTheme(t => setHTML($('#mode-icon'), t.mode === 'dark' ? 'dark_mode' : 'light_mode'));

const providers = zebar.createProviderGroup({
  glazewm: { type: 'glazewm' },
  media: { type: 'media' },
  cpu: { type: 'cpu', refreshInterval: 3000 },
  memory: { type: 'memory', refreshInterval: 3000 },
  battery: { type: 'battery', refreshInterval: 15000 },
  network: { type: 'network', refreshInterval: 5000 },
  audio: { type: 'audio' },
  weather: { type: 'weather', refreshInterval: 30 * 60 * 1000 },
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

function renderMedia() {
  const s = out.media?.currentSession;
  clock.update(s);
  if (!s || !s.title) return setHTML($('#g-media'), '');
  const dur = (s.endTime || 0) - (s.startTime || 0);
  const prog = dur > 0 ? clock.position() / dur : 0;
  const text = s.artist ? `${s.title} • ${s.artist}` : s.title;
  setHTML($('#g-media'), `
    <div class="ring-wrap">${ring(prog, { size: 26, stroke: 2.5 })}${icon(s.isPlaying ? 'pause' : 'play_arrow', { fill: true })}</div>
    <span class="title ellipsis">${esc(text)}</span>`);
}

function renderWorkspaces() {
  const wm = out.glazewm;
  if (!wm) return setHTML($('#g-ws'), '');
  const shown = wm.displayedWorkspace?.name;
  const byName = Object.fromEntries((wm.allWorkspaces || []).map(w => [w.name, w]));
  let html = '';
  for (let i = 1; i <= cfg.workspaces; i++) {
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

function render() {
  out = providers.outputMap;
  renderActiveWindow();
  renderResources();
  renderMedia();
  renderWorkspaces();
  renderClock();
  renderWeather();
  renderWm();
  renderTray();
  renderSystem();
}

providers.onOutput(render);
render();
// Keep the media progress ring moving between provider updates.
setInterval(() => out.media?.currentSession?.isPlaying && renderMedia(), 1000);

// ---------------------------------------------------------------- events
$('#btn-left').addEventListener('click', () => openWidget('sidebar-left'));
$('#g-sys').addEventListener('click', () => openWidget('sidebar-right'));
$('#g-clock').addEventListener('click', () => openWidget('sidebar-right'));

$('#g-ws').addEventListener('click', e => {
  const b = e.target.closest('[data-ws]');
  if (b) out.glazewm?.runCommand(`focus --workspace ${b.dataset.ws}`);
});
$('#g-ws').addEventListener('wheel', e => {
  out.glazewm?.runCommand(e.deltaY > 0 ? 'focus --next-active-workspace' : 'focus --prev-active-workspace');
}, { passive: true });
$('#g-ws').addEventListener('contextmenu', e => { e.preventDefault(); openWidget('overview'); });

const media = $('#g-media');
media.addEventListener('click', () => out.media?.togglePlayPause());
media.addEventListener('auxclick', e => e.button === 1 && out.media?.next());
media.addEventListener('contextmenu', e => { e.preventDefault(); openWidget('sidebar-left'); });

$('#g-utils').addEventListener('click', e => {
  const act = e.target.closest('[data-act]')?.dataset.act;
  if (act === 'snip') run('toggles', '-Action', 'snip');
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
if (cfg.visualizer.bar) {
  const canvas = document.createElement('canvas');
  canvas.id = 'bar-viz';
  $('#g-media').after(canvas);
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
