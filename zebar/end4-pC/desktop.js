import { zebar, $, esc, icon, setHTML, config, watchTheme, albumArt, weatherIcon, weatherText, temp, greeting, reloadOnConfigChange } from './lib/core.js';

const cfg = await config();
if (cfg.desktopWidgets === false) zebar.currentWidget().window.tauri.close();
reloadOnConfigChange();
await watchTheme();

const providers = zebar.createProviderGroup({
  weather: { type: 'weather', refreshInterval: 30 * 60 * 1000 },
  ip: { type: 'ip', refreshInterval: 6 * 60 * 60 * 1000 },
  host: { type: 'host', refreshInterval: 60 * 60 * 1000 },
  media: { type: 'media' },
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

function renderClock() {
  const now = new Date();
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
$('#media').addEventListener('click', e => {
  const c = e.target.closest('[data-ctl]')?.dataset.ctl;
  if (c) out.media?.[c]();
});

function render() {
  out = providers.outputMap;
  renderWeather();
  renderGreeting();
  renderMedia();
}
providers.onOutput(render);
render();
renderClock();
renderWeek();
setInterval(renderClock, 1000);
setInterval(() => { renderWeek(); renderGreeting(); }, 60000);
