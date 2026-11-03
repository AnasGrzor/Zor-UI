import { zebar, $, esc, icon, ring, setHTML, config, watchTheme, albumArt, weatherIcon, weatherText, temp, greeting, reloadOnConfigChange, monthGrid } from './lib/core.js';
import * as timer from './lib/timer.js';
import { defaultSlot, onScreen } from './lib/desk.js';

// Every desktop card is its own window ("desk-clock", "desk-weather", ...), so it can be dragged
// anywhere. The "desktop" widget in Zebar's startup list only opens the enabled cards and leaves.
const widget = zebar.currentWidget();
const tauri = widget.window.tauri;
const stop = () => { tauri.close(); return new Promise(() => {}); };
const cfg = await config();
const me = widget.name.replace(/^desk-/, '');
if (cfg.desktopWidgets === false) await stop();
if (widget.name === 'desktop') {
  for (const id of cfg.desktop.widgets) await zebar.startWidgetPreset('desk-' + id, 'default').catch(e => console.warn(e));
  await stop();
}
if (!cfg.desktop.widgets.includes(me)) await stop();   // turned off in Settings
document.documentElement.style.visibility = 'hidden';  // until it's moved to its spot
reloadOnConfigChange();
await watchTheme();

const on = new Set([me]);
$('#col').insertAdjacentHTML('beforeend', `<div class="w" id="${me}"></div>`);

const providers = zebar.createProviderGroup({
  weather: { type: 'weather', refreshInterval: 30 * 60 * 1000 },
  ip: { type: 'ip', refreshInterval: 6 * 60 * 60 * 1000 },
  host: { type: 'host', refreshInterval: 60 * 60 * 1000 },
  media: { type: 'media' },
  ...(on.has('resources') && {
    cpu: { type: 'cpu', refreshInterval: 3000 },
    memory: { type: 'memory', refreshInterval: 3000 },
    disk: { type: 'disk', refreshInterval: 60000 },
  }),
});
let out = providers.outputMap;

// ---------------------------------------------------------------- cookie clock
// Material 3 "cookie" shape: a circle with a soft scalloped edge (end4's clock).
function cookiePath(cx, cy, r, lobes = 9, depth = 0.06, steps = 360) {
  let d = '';
  for (let i = 0; i <= steps; i++) {
    const a = (i / steps) * Math.PI * 2;
    const rr = r * (1 - depth + depth * Math.cos(lobes * a));
    d += `${i ? 'L' : 'M'}${(cx + rr * Math.cos(a)).toFixed(2)},${(cy + rr * Math.sin(a)).toFixed(2)}`;
  }
  return d + 'Z';
}
const COOKIE = cookiePath(110, 110, 108);

function hand(cls, angle, len, back = 0) {
  const a = (angle - 90) * (Math.PI / 180);
  return `<line class="${cls}" x1="${110 - back * Math.cos(a)}" y1="${110 - back * Math.sin(a)}" x2="${110 + len * Math.cos(a)}" y2="${110 + len * Math.sin(a)}"/>`;
}

// 3x5 dot-matrix digits for the pixel clock (end4's PixelClock): 15 bits per digit, row by row.
const PX = ['111101101101111', '010110010010111', '111001111100111', '111001111001111', '101101111001001',
  '111100111001111', '111100111101111', '111001001001001', '111101111101111', '111101111001111'];
function pixelClock(now) {
  const hh = cfg.use24h ? now.getHours() : now.getHours() % 12 || 12;
  const glyphs = [...(String(hh).padStart(2, '0') + ':' + String(now.getMinutes()).padStart(2, '0'))];
  const cell = 15, dots = [];
  let x = 0;
  for (const g of glyphs) {
    const cols = g === ':' ? 1 : 3;
    for (let r = 0; r < 5; r++) for (let c = 0; c < cols; c++) {
      const lit = g === ':' ? r === 1 || r === 3 : PX[g][r * 3 + c] === '1';
      dots.push(`<circle class="${lit ? 'px-on' : 'px-off'}" cx="${(x + c) * cell + cell / 2}" cy="${r * cell + cell / 2}" r="${cell * 0.4}"/>`);
    }
    x += cols + 1;
  }
  const w = (x - 1) * cell;
  return `<svg width="${w}" height="${5 * cell}" viewBox="0 0 ${w} ${5 * cell}">${dots.join('')}</svg>`;
}

function analogClock(now) {
  const h = now.getHours() % 12, m = now.getMinutes(), s = now.getSeconds();
  const ticks = Array.from({ length: 60 }, (_, i) => {
    const a = (i * 6 - 90) * Math.PI / 180, r1 = i % 5 ? 94 : 86;
    return `<line class="tick" stroke-width="${i % 5 ? 1.5 : 4}" x1="${110 + r1 * Math.cos(a)}" y1="${110 + r1 * Math.sin(a)}" x2="${110 + 100 * Math.cos(a)}" y2="${110 + 100 * Math.sin(a)}"/>`;
  }).join('');
  return `<svg width="220" height="220" viewBox="0 0 220 220"><circle class="face" cx="110" cy="110" r="108"/>${ticks}
    ${hand('hand-h', (h + m / 60) * 30, 52)}${hand('hand-m', (m + s / 60) * 6, 78)}${hand('hand-s', s * 6, 86, 14)}
    <circle class="pin" cx="110" cy="110" r="7"/></svg>`;
}

function renderClock() {
  const now = new Date(), style = cfg.desktop.clock;
  $('#clock').classList.toggle('short', style === 'digital' || style === 'pixel');
  if (style === 'digital') return setHTML($('#clock'), `<div class="digital">
    <b>${now.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: !cfg.use24h }).replace(/\s?[AP]M$/i, '')}</b>
    <span>${now.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</span></div>`);
  if (style === 'pixel') return setHTML($('#clock'), pixelClock(now));
  if (style === 'analog') return setHTML($('#clock'), analogClock(now));
  const h = now.getHours() % 12, m = now.getMinutes(), s = now.getSeconds();
  setHTML($('#clock'), `<svg width="220" height="220" viewBox="0 0 220 220">
    <path class="cookie" d="${COOKIE}"/>
    <text class="dial" x="110" y="46">12</text><text class="dial" x="176" y="112">3</text>
    <text class="dial" x="110" y="176">6</text><text class="dial" x="44" y="112">9</text>
    ${hand('hand-h', (h + m / 60) * 30, 52)}
    ${hand('hand-m', (m + s / 60) * 6, 78)}
    ${hand('hand-s', s * 6, 86, 14)}
    <circle class="pin" cx="110" cy="110" r="7"/>
    <circle class="badge-bg" cx="22" cy="22" r="22"/><text class="badge-tx" x="22" y="23">${now.getDate()}</text>
    <circle class="badge-bg" cx="198" cy="198" r="22"/><text class="badge-tx" x="198" y="199">${String(now.getMonth() + 1).padStart(2, '0')}</text>
  </svg>`);
}

// ---------------------------------------------------------------- cards
function renderWeather() {
  const w = out.weather;
  if (!w) return setHTML($('#weather'), '');
  setHTML($('#weather'), `
    <div class="big">${icon(weatherIcon(w.status), { fill: true })}<b>${temp(w, cfg.tempUnit)}</b></div>
    <div class="info">
      <b class="ellipsis">${esc(out.ip?.approxCity || 'Weather')}</b>
      <small>${esc(weatherText(w.status))}</small>
      <div class="wind">${icon('air')}${w.windSpeed != null ? `${(+w.windSpeed).toFixed(1)} km/h` : ''}</div>
    </div>`);
}

function renderWeek() {
  const now = new Date();
  const monday = new Date(now);
  monday.setDate(now.getDate() - ((now.getDay() + 6) % 7));
  const days = Array.from({ length: 7 }, (_, i) => new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() + i));
  setHTML($('#week'), `
    <span class="chip">${now.toLocaleString(undefined, { month: 'long', year: 'numeric' })}</span>
    <div class="row">${['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'].map(d => `<span class="dow">${d}</span>`).join('')}</div>
    <div class="row">${days.map(d => `<span class="${d.toDateString() === now.toDateString() ? 'today' : ''}">${d.getDate()}</span>`).join('')}</div>`);
}

function renderGreeting() {
  const name = cfg.userName || out.host?.hostname || 'there';
  const now = new Date();
  setHTML($('#greet'), `
    <div class="av">${esc(name.charAt(0).toUpperCase())}</div>
    <div><b>Hi, ${esc(name)}!</b><span>${greeting(now)}</span>
    <small>Today • ${now.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'short' })}</small></div>`);
}

let art = '', artKey = '';
function renderMedia() {
  const s = out.media?.currentSession;
  if (!s?.title) return setHTML($('#media'), '');
  const key = `${s.title}|${s.artist}`;
  if (key !== artKey) {
    artKey = key; art = '';
    albumArt(s).then(u => { if (artKey === key) { art = u; renderMedia(); } });
  }
  setHTML($('#media'), `
    <div class="cover">${art ? `<img src="${art}" alt=""/>` : icon('music_note', { fill: true })}</div>
    <div class="body">
      <b class="ellipsis">${esc(s.title)}</b><small class="ellipsis">${esc(s.artist || '')}</small>
      <div class="ctl">
        <button data-ctl="previous">${icon('skip_previous', { fill: true })}</button>
        <button class="pp" data-ctl="togglePlayPause">${icon(s.isPlaying ? 'pause' : 'play_arrow', { fill: true })}</button>
        <button data-ctl="next">${icon('skip_next', { fill: true })}</button>
      </div>
    </div>`);
}
$('#media')?.addEventListener('click', e => {
  const c = e.target.closest('[data-ctl]')?.dataset.ctl;
  if (c) out.media?.[c]();
});

// ---------------------------------------------------------------- more cards (end4's background widgets)
function renderCalendar() {
  const now = new Date();
  setHTML($('#calendar'), `<div class="pad"><h3>${icon('calendar_month')}${now.toLocaleString(undefined, { month: 'long', year: 'numeric' })}</h3>
    <div class="grid">${['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'].map(d => `<span class="dow">${d}</span>`).join('')}
    ${monthGrid(now).map(c => `<span class="${c.out ? 'out' : ''}${c.today ? ' today' : ''}">${c.d}</span>`).join('')}</div></div>`);
}

// Shares the right sidebar's to-do list (same localStorage key).
const TODO_KEY = 'end4-pC.todo';
const todos = () => { try { return JSON.parse(localStorage.getItem(TODO_KEY)) || []; } catch { return []; } };
function renderTodo() {
  const list = todos();
  setHTML($('#todo'), `<div class="pad"><h3>${icon('done_outline')}To do</h3>${list.length ? list.slice(0, 8).map((t, i) => `
    <div class="item${t.done ? ' done' : ''}" data-i="${i}">${icon(t.done ? 'check_circle' : 'radio_button_unchecked', { fill: t.done })}<span>${esc(t.text)}</span></div>`).join('')
    : '<div class="muted" style="font-size:13px">Nothing to do. Add tasks in the right sidebar.</div>'}</div>`);
}
$('#todo')?.addEventListener('click', e => {
  const i = e.target.closest('[data-i]')?.dataset.i;
  if (i == null) return;
  const list = todos();
  list[i].done = !list[i].done;
  localStorage.setItem(TODO_KEY, JSON.stringify(list));
  renderTodo();
});

function renderNotes() {   // rendered once: re-rendering would drop the caret while typing
  setHTML($('#notes'), `<div class="pad"><h3>${icon('sticky_note_2')}Notes</h3><textarea placeholder="Write something" spellcheck="false"></textarea></div>`);
  const ta = $('#notes textarea');
  ta.value = localStorage.getItem('end4-pC.notes') || '';
  ta.addEventListener('input', () => localStorage.setItem('end4-pC.notes', ta.value));
}

function renderTimer() {
  const s = timer.load();
  const sw = !s.running && s.sw.running, going = sw || s.running;
  setHTML($('#timer'), `<div class="pad"><h3>${icon('timer')}${sw ? 'Stopwatch' : timer.PHASES[s.phase]}</h3>
    <div class="row"><b>${timer.clock(sw ? timer.swElapsed(s) : timer.left(s, cfg))}</b>
    <button data-t="${sw ? 'sw' : 'pomo'}" title="${going ? 'Pause' : 'Start'}">${icon(going ? 'pause' : 'play_arrow', { fill: true })}</button></div></div>`);
}
$('#timer')?.addEventListener('click', e => {
  const t = e.target.closest('[data-t]')?.dataset.t;
  if (t === 'pomo') timer.pomo.toggle(cfg);
  if (t === 'sw') timer.sw.toggle();
  renderTimer();
});

function renderWorldClock() {
  const now = new Date(), here = new Date(now.toLocaleString('en-US'));
  const rows = cfg.desktop.worldClocks.map(tz => {
    try {
      const time = now.toLocaleTimeString([], { timeZone: tz, hour: '2-digit', minute: '2-digit', hour12: !cfg.use24h });
      const diff = Math.round((new Date(now.toLocaleString('en-US', { timeZone: tz })) - here) / 18e5) / 2;   // half-hour steps
      return `<div class="tz"><span>${esc(tz.split('/').pop().replace(/_/g, ' '))}<small> ${diff === 0 ? 'local' : (diff > 0 ? '+' : '') + diff + 'h'}</small></span><b>${esc(time)}</b></div>`;
    } catch { return `<div class="tz"><span class="muted">${esc(tz)}: unknown time zone</span></div>`; }
  });
  setHTML($('#worldclock'), `<div class="pad"><h3>${icon('public')}World clock</h3>${rows.join('')}</div>`);
}

function renderResources() {
  const disk = out.disk?.disks?.find(d => /^C:/i.test(d.mountPoint)) || out.disk?.disks?.[0];
  const du = disk ? 1 - disk.availableSpace.bytes / disk.totalSpace.bytes : 0;
  const r = (v, label) => `<div class="r"><div class="ring-wrap">${ring(v, { size: 56, stroke: 5 })}<b>${Math.round(v * 100)}%</b></div>${label}</div>`;
  setHTML($('#resources'), `<div class="pad"><h3>${icon('monitoring')}Resources</h3><div class="rings">
    ${r((out.cpu?.usage ?? 0) / 100, 'CPU')}${r((out.memory?.usage ?? 0) / 100, 'RAM')}${r(du, 'Disk ' + esc((disk?.mountPoint || '').replace(/[\\/]$/, '')))}</div></div>`);
}

function renderText() {
  setHTML($('#text'), cfg.desktop.text ? `<div class="pad">${esc(cfg.desktop.text)}</div>` : '');
}

// ---------------------------------------------------------------- go
const card = (id, fn) => on.has(id) && fn();
function render() {
  out = providers.outputMap;
  card('weather', renderWeather);
  card('greet', renderGreeting);
  card('media', renderMedia);
  card('resources', renderResources);
}
function everySecond() {
  card('clock', renderClock);
  card('timer', renderTimer);
  card('worldclock', renderWorldClock);
  card('todo', renderTodo);   // picks up changes made in the right sidebar
}
function everyMinute() {
  card('week', renderWeek);
  card('greet', renderGreeting);
  card('calendar', renderCalendar);
}
providers.onOutput(render);
render();
everySecond();
everyMinute();
card('notes', renderNotes);
card('text', renderText);
setInterval(everySecond, 1000);
setInterval(everyMinute, 60000);

// ---------------------------------------------------------------- size, place and drag
const ipc = (cmd, args = {}) => window.__TAURI_INTERNALS__.invoke('plugin:window|' + cmd, { label: tauri.label, ...args });
const POS_KEY = 'end4-pC.deskPos';
const saved = () => { try { return JSON.parse(localStorage.getItem(POS_KEY)) || {}; } catch { return {}; } };

// The window follows the card's size (cards grow and shrink: to-do items, media on/off).
new ResizeObserver(() => {
  const r = $('#col').getBoundingClientRect();
  tauri.setSize({ type: 'Logical', width: Math.ceil(r.width), height: Math.max(1, Math.ceil(r.height)) }).catch(() => {});
}).observe($('#col'));

// Saved spot (physical px, from a drag) if it's still on a monitor, else the default stack.
// The window starts on the primary monitor (zpack preset), so its scale converts the default.
const monitors = await ipc('available_monitors').catch(() => []);
let pos = saved()[me];
if (!pos || (monitors.length && !onScreen(pos, monitors))) {
  const d = defaultSlot(cfg.desktop.widgets, me, screen.availHeight);
  pos = { x: Math.round(d.x * devicePixelRatio), y: Math.round(d.y * devicePixelRatio) };
}
await tauri.setPosition({ type: 'Physical', ...pos }).catch(e => console.warn(e));
document.documentElement.style.visibility = '';

// Drag the card by any spot that isn't a control. A move under 4px still counts as a click.
let drag = null, swallowClick = false;
document.addEventListener('pointerdown', e => {
  if (e.button !== 0 || e.target.closest('button, textarea, input, a')) return;
  document.body.setPointerCapture(e.pointerId);
  drag = { sx: e.screenX, sy: e.screenY, from: pos, moved: false, raf: 0 };
});
document.addEventListener('pointermove', e => {
  if (!drag) return;
  const dx = (e.screenX - drag.sx) * devicePixelRatio, dy = (e.screenY - drag.sy) * devicePixelRatio;
  if (!drag.moved && Math.hypot(dx, dy) < 4) return;
  drag.moved = true;
  pos = { x: Math.round(drag.from.x + dx), y: Math.round(drag.from.y + dy) };
  cancelAnimationFrame(drag.raf);
  drag.raf = requestAnimationFrame(() => tauri.setPosition({ type: 'Physical', ...pos }).catch(() => {}));
});
document.addEventListener('pointerup', () => {
  if (drag?.moved) {
    localStorage.setItem(POS_KEY, JSON.stringify({ ...saved(), [me]: pos }));
    swallowClick = true;
  }
  drag = null;
});
document.addEventListener('click', e => { if (swallowClick) { e.stopPropagation(); swallowClick = false; } }, true);
