// end4-pC for Windows - the Dynamic Island, expanded (opened by clicking the island in the bar).
// Shows everything going on: the song (with progress, controls and lyrics), a timer, a recording;
// or, when nothing is, the time, date and weather. The latest notifications are at the bottom.
import {
  zebar, $, esc, icon, setHTML, config, watchTheme, popupBehaviour, openWidget, mediaClock, albumArt,
  lyrics, renderLyrics, fmtTime, waveSvg, weatherIcon, weatherText, temp, bridge,
} from './lib/core.js';
import * as timer from './lib/timer.js';

document.documentElement.style.visibility = 'hidden';   // until it's under the island
const tauri = zebar.currentWidget().window.tauri;
const cfg = await config();
if (cfg.native?.['island-panel']) { tauri.close().catch(() => {}); await new Promise(() => {}); }   // drawn by native/end4 instead
await watchTheme();
const close = popupBehaviour();

const providers = zebar.createProviderGroup({
  media: { type: 'media' },
  weather: { type: 'weather', refreshInterval: 30 * 60 * 1000 },
  ip: { type: 'ip', refreshInterval: 6 * 60 * 60 * 1000 },
});
let out = providers.outputMap;
const mclock = mediaClock({ resync: 30000 });
let np = { key: '', art: '', lyr: undefined }, notifs = [];
const dp = cfg.island.panel;   // which sections show (Settings > Dynamic Island)
const showLyrics = cfg.lyrics && dp.lyrics;
if (dp.notifications) bridge('notifications').then(r => { notifs = (r.items || []).slice(0, dp.notifCount); render(); });

const ago = t => {
  const s = Date.now() / 1000 - t;
  return s < 60 ? 'now' : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
};

// ---------------------------------------------------------------- sections
// The HTML only changes when a section starts, stops or the song/play state changes; times, the
// wave and the lyric highlight are updated in place (tick) so the lyrics can slide.
function sections() {
  const s = out.media?.currentSession, t = timer.load();
  const rec = Number(localStorage.getItem('end4-pC.rec')) || 0, done = localStorage.getItem('end4-pC.timerDone');
  const parts = [];
  if (dp.media && s?.title) {
    const key = `${s.title}|${s.artist}`;
    if (key !== np.key) {
      np = { key, art: '', lyr: undefined };
      albumArt(s).then(u => { if (np.key === key) { np.art = u; render(); } });
      if (showLyrics) lyrics(s).then(l => { if (np.key === key) { np.lyr = l; render(); } });
    }
    parts.push(`<section>
      <div class="np"><div class="art">${np.art ? `<img src="${np.art}" alt="" />` : icon('music_note', { fill: true })}</div>
        <div class="meta"><b class="ellipsis">${esc(s.title)}</b><span class="ellipsis">${esc(s.artist || '')}</span></div></div>
      <div class="seekrow"><span id="pos"></span><div class="seek" title="Click to seek"></div><span id="dur"></span></div>
      <div class="ctl"><button data-ctl="previous">${icon('skip_previous', { fill: true })}</button>
        <button class="big" data-ctl="togglePlayPause">${icon(s.isPlaying ? 'pause' : 'play_arrow', { fill: true })}</button>
        <button data-ctl="next">${icon('skip_next', { fill: true })}</button></div>
      ${showLyrics ? '<div class="lyr"></div>' : ''}</section>`);
  }
  if (dp.recording && rec) parts.push(`<section class="rec"><div class="bigtime"><b><span class="dot"></span><span id="rec"></span></b>
    <button class="chip" data-act="rec-stop">${icon('stop_circle', { fill: true })}Stop and show</button></div></section>`);
  if (dp.timer && (done || t.running || t.sw.running)) {
    const sw = !done && !t.running && t.sw.running;
    parts.push(`<section><h4>${icon(sw ? 'timer_play' : 'timer')}${done ? (done === 'focus' ? 'Focus done: break time' : 'Break over: focus time') : sw ? 'Stopwatch' : timer.PHASES[t.phase]}</h4>
      <div class="bigtime"><b id="timer"></b><div class="row">
        ${done ? `<button class="chip" data-act="timer-next">${icon('play_arrow', { fill: true })}Start ${timer.PHASES[t.phase].toLowerCase()}</button>`
          : `<button class="chip" data-act="${sw ? 'sw' : 'pomo'}-toggle">${icon(sw || t.running ? 'pause' : 'play_arrow', { fill: true })}${sw || t.running ? 'Pause' : 'Resume'}</button>
             <button class="chip" data-act="${sw ? 'sw' : 'pomo'}-reset">${icon('replay')}Reset</button>`}</div></div></section>`);
  }
  if (dp.clock && !parts.length) {
    const w = out.weather, now = new Date();
    parts.push(`<section class="idle"><div class="when"><b id="clock"></b><span>${now.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</span></div>
      ${w ? `<div class="wx">${icon(weatherIcon(w.status), { fill: true })}<b>${temp(w, cfg.tempUnit)}</b><br />${esc(weatherText(w.status))}${out.ip?.approxCity ? ` · ${esc(out.ip.approxCity)}` : ''}</div>` : ''}</section>`);
  }
  if (dp.agents) parts.push(agentSection());
  if (dp.notifications) parts.push(`<section><h4>${icon('notifications')}Notifications${notifs.length ? `<button class="clear" data-act="notifs-clear">${icon('clear_all')}Clear all</button>` : ''}</h4>${notifs.length ? notifs.map(n => `
    <div class="notif" data-act="notifs"><div class="tx"><b class="ellipsis">${esc(n.app)} · ${esc(n.title)}</b>${n.body ? `<span class="ellipsis">${esc(n.body)}</span>` : ''}</div><time>${ago(n.time)}</time></div>`).join('')
    : '<div class="muted-line">Nothing new</div>'}</section>`);
  return parts.join('') || '<section><div class="muted-line">Nothing to show. Pick sections in Settings > Dynamic Island</div></section>';
}

// AI agents: what needs you and how much of each usage limit is left (the bar keeps the list).
const WINDOWS = { five_hour: '5-hour', seven_day: 'Weekly', spend_limit: 'Spend' };
function agentSection() {
  const list = JSON.parse(localStorage.getItem('end4.agents') || '[]');
  if (!list.length) return '';
  const resets = t => {
    if (!t) return '';
    const d = new Date(t * 1000), hm = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: !cfg.use24h });
    return `resets ${d - Date.now() < 86400000 ? hm : `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${hm}`}`;
  };
  return `<section><h4>${icon('smart_toy')}AI agents</h4>${list.map(a => `
    <div class="agent"><div class="ah"><b>${esc(a.label)}</b>${a.model ? `<small class="ellipsis">${esc(a.model)}</small>` : ''}
      <span class="st ${a.state}">${a.state === 'waiting' ? 'Needs you' : a.state === 'working' ? 'Working' : 'Idle'}</span></div>
      ${a.state === 'waiting' ? `<div class="ask ellipsis" title="${esc(a.cwd)}">${esc(a.message || 'Needs your input')}</div>` : ''}
      ${Object.entries(a.usage || {}).map(([k, u]) => `<div class="use"><span>${esc(WINDOWS[k] || k)}</span>
        <div class="bar"><span style="width:${Math.min(100, u.used_percentage)}%"></span></div><b>${Math.round(u.used_percentage)}%</b><small>${resets(u.resets_at)}</small></div>`).join('')}
    </div>`).join('')}</section>`;
}
window.addEventListener('storage', e => e.key === 'end4.agents' && render());

function tick() {
  const s = out.media?.currentSession, t = timer.load();
  if (s?.title && $('.seek')) {
    const pos = mclock.position(), dur = (s.endTime || 0) - (s.startTime || 0);
    setHTML($('#pos'), fmtTime(pos));
    setHTML($('#dur'), dur > 0 ? fmtTime(dur) : '');
    setHTML($('.seek'), waveSvg($('.seek').clientWidth || 260, dur > 0 ? pos / dur : 0, s.isPlaying));
    renderLyrics($('.lyr'), np.lyr, pos, showLyrics);
  }
  if ($('#rec')) setHTML($('#rec'), timer.clock((Date.now() - Number(localStorage.getItem('end4-pC.rec'))) / 1000));
  if ($('#timer')) setHTML($('#timer'), timer.clock(!t.running && t.sw.running ? timer.swElapsed(t) : timer.left(t, cfg)));
  if ($('#clock')) setHTML($('#clock'), new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: !cfg.use24h }));
}

function render() {
  out = providers.outputMap;
  mclock.update(out.media?.currentSession);
  setHTML($('#panel'), sections());
  tick();
}
providers.onOutput(render);
render();
setInterval(tick, 100);
setInterval(render, 1000);   // timers starting/ending, recording stopped from the bar

// ---------------------------------------------------------------- actions
$('#panel').addEventListener('click', async e => {
  const ctl = e.target.closest('[data-ctl]')?.dataset.ctl;
  if (ctl) return out.media?.[ctl]();
  const seek = e.target.closest('.seek'), s = out.media?.currentSession;
  const dur = s ? (s.endTime || 0) - (s.startTime || 0) : 0;
  if (seek && dur > 0) { const r = seek.getBoundingClientRect(); return mclock.seek(((e.clientX - r.left) / r.width) * dur).then(tick); }
  const act = e.target.closest('[data-act]')?.dataset.act;
  if (act === 'rec-stop') { localStorage.removeItem('end4-pC.rec'); bridge('record', 'stop'); return close(); }
  if (act === 'pomo-toggle') timer.pomo.toggle(cfg);
  if (act === 'pomo-reset') timer.pomo.reset();
  if (act === 'sw-toggle') timer.sw.toggle();
  if (act === 'sw-reset') timer.sw.reset();
  if (act === 'timer-next') { localStorage.removeItem('end4-pC.timerDone'); timer.pomo.toggle(cfg); }
  if (act === 'notifs-clear') { notifs = []; bridge('notifications-dismiss', 'all'); return render(); }
  if (act === 'notifs') { localStorage.setItem('end4-pC.rightTab', 'notif'); openWidget('sidebar-right'); return close(); }
  render();
});

// ---------------------------------------------------------------- size and place under the island
// The bar saved the spot under the island (physical px) just before opening this.
const at = JSON.parse(localStorage.getItem('end4.islandAt') || 'null');
async function place() {
  const r = $('#panel').getBoundingClientRect();
  const w = Math.ceil(r.width + 24), h = Math.ceil(r.height + 24);
  await tauri.setSize({ type: 'Logical', width: w, height: h }).catch(() => {});
  if (at) await tauri.setPosition({ type: 'Physical', x: Math.round(at.x - (w * devicePixelRatio) / 2), y: Math.round(at.y) }).catch(() => {});
}
new ResizeObserver(place).observe($('#panel'));
await place();
document.documentElement.style.visibility = '';
tauri.setFocus().catch(() => {});   // so Esc and a click elsewhere close it
bridge('focus-widget', 'island-panel', Math.round(window.screenX * devicePixelRatio), Math.round(window.screenY * devicePixelRatio));
