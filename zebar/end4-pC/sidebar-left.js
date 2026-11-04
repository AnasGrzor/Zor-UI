import {
  zebar, $, $$, esc, icon, ring, setHTML, config, watchTheme, popupBehaviour, openOnTargetMonitor,
  mediaClock, fmtTime, albumArt, lyrics, renderLyrics } from './lib/core.js';
import { mountAI } from './lib/tab-ai.js';
import { mountTranslate } from './lib/tab-translate.js';
import { mountAnime } from './lib/tab-anime.js';
import { mountFootball } from './lib/tab-football.js';

if ((await config()).native?.['sidebar-left']) { zebar.currentWidget().window.tauri.close().catch(() => {}); await new Promise(() => {}); }   // drawn by native/end4 instead
await openOnTargetMonitor();
const cfg = await config();
await watchTheme();
const popup = popupBehaviour();

// ---------------------------------------------------------------- tabs (end4's left sidebar pages)
const mounters = { ai: mountAI, translate: mountTranslate, anime: mountAnime, football: mountFootball };
const mounted = {};
async function showTab(name) {
  if (!document.getElementById('tab-' + name)) name = 'home';
  $$('#tabs button').forEach(b => b.classList.toggle('sel', b.dataset.tab === name));
  $$('.tab-page').forEach(p => p.classList.toggle('sel', p.id === 'tab-' + name));
  try { localStorage.setItem('end4-pC.leftTab', name); } catch {}
  if (mounters[name] && !mounted[name]) mounted[name] = mountTabSafe(name);
  (await mounted[name])?.focus?.();
}
async function mountTabSafe(name) {
  try { return await mounters[name]($('#tab-' + name)); }
  catch (e) { $('#tab-' + name).innerHTML = `<div class="err">${esc(e.message || e)}</div>`; return null; }
}
$('#tabs').addEventListener('click', e => { const b = e.target.closest('[data-tab]'); if (b) showTab(b.dataset.tab); });
$('#pin').addEventListener('click', () => $('#pin').classList.toggle('on', popup.togglePin()));
window.addEventListener('keydown', e => {
  if (e.ctrlKey && (e.key === 'PageDown' || e.key === 'PageUp' || e.key === 'Tab')) {
    e.preventDefault();
    const names = $$('#tabs button').map(b => b.dataset.tab);
    const cur = names.indexOf($('#tabs button.sel')?.dataset.tab);
    const dir = e.key === 'PageUp' || (e.key === 'Tab' && e.shiftKey) ? -1 : 1;
    showTab(names[(cur + dir + names.length) % names.length]);
  }
});
let startTab = 'home';
try { startTab = localStorage.getItem('end4-pC.leftTab') || 'home'; } catch {}
showTab(new URLSearchParams(location.search).get('tab') || startTab);

const providers = zebar.createProviderGroup({
  media: { type: 'media' },
  cpu: { type: 'cpu', refreshInterval: 2000 },
  memory: { type: 'memory', refreshInterval: 2000 },
  disk: { type: 'disk', refreshInterval: 60000 },
  network: { type: 'network', refreshInterval: 2000 },
});
let out = providers.outputMap;
const clock = mediaClock({ resync: 30000 });
let chosen = null;          // sessionId picked from the chips, else current session
let art = '', lyr, trackKey = '';   // lyr: undefined = searching, null = none found

const session = () => {
  const all = out.media?.allSessions || [];
  return all.find(s => s.sessionId === chosen) || out.media?.currentSession || all[0] || null;
};

// ---------------------------------------------------------------- player
function renderPlayer() {
  const s = session();
  clock.update(s);
  if (!s) {
    trackKey = '';
    return setHTML($('#player'), `<div class="nothing">${icon('music_off')}Nothing playing</div>`);
  }
  const key = `${s.sessionId}|${s.title}|${s.artist}`;
  if (key !== trackKey) {
    trackKey = key; art = ''; lyr = undefined;
    albumArt(s).then(u => { if (trackKey === key) { art = u; renderPlayer(); } });
    if (cfg.lyrics) lyrics(s).then(l => { if (trackKey === key) { lyr = l; renderPlayer(); } });
  }
  const all = out.media?.allSessions || [];
  const dur = (s.endTime || 0) - (s.startTime || 0);
  const sessions = all.length > 1 ? `<div class="sessions">${all.map(x =>
    `<span class="chip${x.sessionId === s.sessionId ? ' sel' : ''}" data-session="${esc(x.sessionId)}">${esc(appFromSession(x.sessionId))}</span>`).join('')}</div>` : '';

  setHTML($('#player'), `
    <div class="art">${art ? `<img src="${art}" alt="" />` : icon('music_note', { fill: true })}</div>
    <div class="meta"><div class="t ellipsis">${esc(s.title)}</div><div class="a ellipsis">${esc(s.artist || s.albumTitle || '')}</div></div>
    <div id="lyrics"></div>
    <div class="progress"><span id="pos"></span><div class="bar"><div id="prog"></div></div><span>${dur > 0 ? fmtTime(dur) : ''}</span></div>
    <div class="controls">
      <button data-ctl="previous" ${s.isPreviousEnabled === false ? 'disabled' : ''}>${icon('skip_previous', { fill: true })}</button>
      <button class="main" data-ctl="togglePlayPause">${icon(s.isPlaying ? 'pause' : 'play_arrow', { fill: true })}</button>
      <button data-ctl="next" ${s.isNextEnabled === false ? 'disabled' : ''}>${icon('skip_next', { fill: true })}</button>
    </div>
    ${sessions}`);
  tick();
}

function tick() {
  const s = session();
  if (!s) return;
  const pos = clock.position();
  const dur = (s.endTime || 0) - (s.startTime || 0);
  const p = $('#prog'), t = $('#pos');
  if (p) p.style.width = dur > 0 ? `${Math.min(100, (pos / dur) * 100)}%` : '0%';
  if (t) t.textContent = fmtTime(pos);
  renderLyrics($('#lyrics'), lyr, pos, cfg.lyrics);
}

const appFromSession = id => {
  const n = String(id).split(/[\\!]/).pop().replace(/\.exe$/i, '');
  return n.length > 18 ? n.slice(0, 18) + '…' : n || 'Player';
};

$('#player').addEventListener('click', e => {
  const ctl = e.target.closest('[data-ctl]')?.dataset.ctl;
  const s = session();
  if (ctl && s) out.media?.[ctl]({ sessionId: s.sessionId });
  const bar = e.target.closest('.progress .bar');   // click to seek
  const dur = s ? (s.endTime || 0) - (s.startTime || 0) : 0;
  if (bar && dur > 0) { const r = bar.getBoundingClientRect(); clock.seek(((e.clientX - r.left) / r.width) * dur).then(tick); tick(); }
  const pick = e.target.closest('[data-session]')?.dataset.session;
  if (pick) { chosen = pick; renderPlayer(); }
});

// ---------------------------------------------------------------- resources
function renderRes() {
  const disk = (out.disk?.disks || []).find(d => /^C:/i.test(d.mountPoint)) || out.disk?.disks?.[0];
  const diskUse = disk ? 1 - disk.availableSpace.bytes / disk.totalSpace.bytes : 0;
  const rx = out.network?.traffic?.received;
  const speed = rx ? `${rx.iecValue.toFixed(rx.iecValue < 10 ? 1 : 0)} ${rx.iecUnit}/s` : '—';
  const stat = (v, ic, val, label) => `<div class="stat"><div class="ring-wrap">${ring(v, { size: 44, stroke: 4 })}${icon(ic)}</div><b>${val}</b><span>${label}</span></div>`;
  setHTML($('#res'),
    stat((out.cpu?.usage ?? 0) / 100, 'memory', `${Math.round(out.cpu?.usage ?? 0)}%`, 'CPU') +
    stat((out.memory?.usage ?? 0) / 100, 'memory_alt', `${Math.round(out.memory?.usage ?? 0)}%`, 'RAM') +
    stat(diskUse, 'hard_drive', `${Math.round(diskUse * 100)}%`, disk ? disk.mountPoint.replace(/\\$/, '') : 'Disk') +
    stat(0, 'download', speed, 'Network'));
}

function render() {
  out = providers.outputMap;
  renderPlayer();
  renderRes();
}
providers.onOutput(render);
render();
setInterval(tick, 250);
