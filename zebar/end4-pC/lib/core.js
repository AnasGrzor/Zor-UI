// end4-pC for Windows - shared helpers for every widget in the pack.
import * as zebar from 'https://esm.sh/zebar@3.0';

export { zebar };

const DEFAULTS = {
  scriptsDir: '',
  userName: '',
  workspaces: 10,
  clockFormat: "EEE, dd/MM '•' HH:mm",
  use24h: true,
  tempUnit: 'C',
  showKeyboardLayout: false,
  showNetSpeed: false,
  lyrics: true,
  pythonPath: '',
  bridgePath: '',
  workspaceLabels: [],
  alwaysShowNumbers: false,
  transparency: 0,
  desktopWidgets: true,
  syncLockScreen: true,
  ai: { model: 'gemini-2.5-flash', temperature: 0.5, systemPrompt: '' },
  translator: { source: 'auto', target: 'en' },
  booru: { source: 'yandere', allowNsfw: false },
  football: { leagues: [] },
  visualizer: { desktop: true, bar: false, bars: 64 },
  nightLight: { kelvin: 4500 },
  wallpaperEngine: { follow: true, path: '' },
  timer: { focus: 25, short: 5, long: 15, cycles: 4 },
  desktop: { widgets: ['clock', 'weather', 'week', 'greet', 'media'], clock: 'cookie', worldClocks: ['Europe/London', 'America/New_York', 'Asia/Tokyo'], text: '' },
};

// ------------------------------------------------------------------ DOM
export const $ = (sel, root = document) => root.querySelector(sel);
export const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
export const esc = s =>
  String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
export const icon = (name, { fill = false, cls = '' } = {}) =>
  `<span class="ms${fill ? ' fill' : ''}${cls ? ' ' + cls : ''}" aria-hidden="true">${name}</span>`;

/** Only touch the DOM when the markup actually changed (keeps hover/press states). */
export function setHTML(el, html) {
  if (el && el.__html !== html) {
    el.innerHTML = html;
    el.__html = html;
  }
}

/** SVG ring used by end4's circular progress indicators. */
export function ring(value, { size = 26, stroke = 2.5 } = {}) {
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const v = Math.max(0, Math.min(1, value || 0));
  return `<svg class="ring" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">
    <circle class="ring-track" cx="${size / 2}" cy="${size / 2}" r="${r}" stroke-width="${stroke}"/>
    <circle class="ring-value" cx="${size / 2}" cy="${size / 2}" r="${r}" stroke-width="${stroke}"
      stroke-dasharray="${c}" stroke-dashoffset="${c * (1 - v)}" transform="rotate(-90 ${size / 2} ${size / 2})"/>
  </svg>`;
}

// ------------------------------------------------------------------ config
let cfgPromise;
export function config() {
  cfgPromise ??= fetch('./config.json?t=' + Date.now(), { cache: 'no-store' })
    .then(r => (r.ok ? r.json() : {}))
    .catch(() => ({}))
    .then(c => {
      const merged = { ...DEFAULTS, ...c };
      for (const k of ['ai', 'translator', 'booru', 'football', 'visualizer', 'nightLight', 'wallpaperEngine', 'timer', 'desktop'])
        merged[k] = { ...DEFAULTS[k], ...(c[k] || {}) };
      document.documentElement.style.setProperty('--opaque', `${Math.round((1 - (merged.transparency || 0)) * 100)}%`);
      return merged;
    });
  return cfgPromise;
}

/** Long-lived widgets (bar, desktop, visualizer) reload when config.json changes. */
export function reloadOnConfigChange() {
  let first = null;
  setInterval(async () => {
    try {
      const txt = await (await fetch('./config.json?t=' + Date.now(), { cache: 'no-store' })).text();
      if (first === null) first = txt;
      else if (txt !== first) location.reload();
    } catch { /* ignore */ }
  }, 3000);
}

// ------------------------------------------------------------------ theme
const kebab = s => s.replace(/_/g, '-').replace(/[A-Z]/g, m => '-' + m.toLowerCase());
let lastTheme = '';
let themeListeners = [];

function applyTheme(t) {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(t.colors || {})) root.style.setProperty('--' + kebab(k), v);
  root.dataset.mode = t.mode || 'dark';
  themeListeners.forEach(fn => fn(t));
}

/** Polls theme.json (written by end4_theme.py) so wallpaper changes apply live. */
let polling = false;
export async function watchTheme(onChange) {
  if (onChange) themeListeners.push(onChange);
  if (polling) {
    if (onChange && lastTheme) onChange(JSON.parse(lastTheme));
    return;
  }
  polling = true;
  const tick = async () => {
    try {
      const res = await fetch('./theme.json?t=' + Date.now(), { cache: 'no-store' });
      if (!res.ok) return;
      const txt = await res.text();
      if (txt && txt !== lastTheme) {
        const theme = JSON.parse(txt);  // parse first so a bad file never becomes lastTheme
        lastTheme = txt;
        applyTheme(theme);
      }
    } catch {
      /* keep the defaults from base.css */
    }
  };
  await tick();
  setInterval(tick, 2000);
}

// ------------------------------------------------------------------ scripts
/** Runs one of the PowerShell helpers in scriptsDir and returns its JSON line. */
export async function run(script, ...args) {
  const c = await config();
  if (!c.scriptsDir) return { error: 'scriptsDir missing from config.json (run install.ps1)' };
  try {
    const out = await zebar.shellExec('powershell', [
      '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
      '-File', `${c.scriptsDir}\\${script}.ps1`, ...args.map(String),
    ]);
    const line = String(out.stdout || '').trim().split(/\r?\n/).pop();
    return line ? JSON.parse(line) : { error: out.stderr };
  } catch (err) {
    console.warn('[end4-pC] script failed', script, args, err);
    return { error: String(err?.message || err) };
  }
}

/** Runs a command of scripts/end4_bridge.py (Python) and returns its JSON line. */
export async function bridge(cmd, ...args) {
  const c = await config();
  if (!c.pythonPath || !c.bridgePath) return { error: 'pythonPath/bridgePath missing from config.json (run install.ps1)' };
  try {
    const out = await zebar.shellExec(c.pythonPath, ['-X', 'utf8', c.bridgePath, cmd, ...args.map(String)]);
    const line = String(out.stdout || '').trim().split(/\r?\n/).pop();
    return line ? JSON.parse(line) : { error: String(out.stderr || 'no output').slice(-400) };
  } catch (err) {
    return { error: String(err?.message || err) };
  }
}

/** Starts a streaming bridge command; onLine gets each parsed JSON (or raw) line. */
export async function bridgeStream(cmd, args, onLine, { stdin, raw = false, onExit } = {}) {
  const c = await config();
  if (!c.pythonPath || !c.bridgePath) { onLine({ error: 'pythonPath/bridgePath missing from config.json' }); return null; }
  const proc = await zebar.shellSpawn(c.pythonPath, ['-X', 'utf8', c.bridgePath, cmd, ...args.map(String)]);
  // Zebar 3.3's app reads `pid` but its client library sends `processId`, so write()/kill() fail.
  // Send both keys; Tauri ignores the one a given Zebar build doesn't use.
  const ipc = (name, extra) => window.__TAURI_INTERNALS__.invoke(name, { pid: proc.processId, processId: proc.processId, ...extra });
  proc.write = buffer => ipc('shell_write', { buffer });
  proc.kill = () => ipc('shell_kill');
  let pending = '';
  proc.onStdout(chunk => {
    pending += chunk;
    const parts = pending.split(/\r?\n/);
    pending = parts.pop();
    for (const p of parts) {
      if (!p.trim()) continue;
      if (raw) onLine(p);
      else { try { onLine(JSON.parse(p)); } catch { /* partial */ } }
    }
  });
  proc.onExit(e => { if (pending.trim()) { try { onLine(raw ? pending : JSON.parse(pending)); } catch {} } onExit?.(e); });
  if (stdin != null) proc.write(stdin + '\n');
  return proc;
}

export const openUrl = url => bridge('open', url);

/** Small Markdown renderer for AI replies (escapes everything first). */
export function md(src) {
  const blocks = [];
  let text = String(src || '').replace(/```(\w*)\n?([\s\S]*?)(```|$)/g, (_, lang, code) => {
    blocks.push(`<div class="code"><div class="code-head"><span>${esc(lang || 'code')}</span><button class="copy" data-copy="${blocks.length}">${icon('content_copy')}</button></div><pre><code>${esc(code.replace(/\n$/, ''))}</code></pre></div>`);
    md.raw = md.raw || [];
    md.raw[blocks.length - 1] = code;
    return `\u0000${blocks.length - 1}\u0000`;
  });
  text = esc(text);
  const inline = s => s
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, '$1<i>$2</i>')
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a data-url="$2">$1</a>');
  const out = [];
  let list = null;
  for (const line of text.split('\n')) {
    const h = line.match(/^(#{1,3})\s+(.*)/), li = line.match(/^\s*(?:[-*]|\d+\.)\s+(.*)/), q = line.match(/^&gt;\s?(.*)/);
    if (!li && list) { out.push(`</${list}>`); list = null; }
    if (/^\u0000\d+\u0000$/.test(line.trim())) out.push(line.trim());
    else if (h) out.push(`<h${h[1].length + 2}>${inline(h[2])}</h${h[1].length + 2}>`);
    else if (li) { const t = /^\s*\d+\./.test(line) ? 'ol' : 'ul'; if (!list) { list = t; out.push(`<${t}>`); } out.push(`<li>${inline(li[1])}</li>`); }
    else if (q) out.push(`<blockquote>${inline(q[1])}</blockquote>`);
    else if (line.trim()) out.push(`<p>${inline(line)}</p>`);
  }
  if (list) out.push(`</${list}>`);
  return out.join('').replace(/\u0000(\d+)\u0000/g, (_, i) => blocks[i]);
}

export const openWidget = name => zebar.startWidgetPreset(name, 'default').catch(e => console.warn(e));

/**
 * Bar side of openOnTargetMonitor: open (or close) a popup on this bar's monitor. Starts a fresh
 * copy with the preset's placement, because Zebar ignores starting a preset that's already open.
 */
let zpack;
export async function openHere(name, monitor) {
  if (monitor) localStorage.setItem('end4.openOn', JSON.stringify({ monitor: monitor.id, at: Date.now() }));
  zpack ??= await (await fetch('/zpack.json')).json();
  const { name: _, ...placement } = zpack.widgets.find(w => w.name === name).presets[0];
  return zebar.startWidget(name, placement, {}).catch(e => console.warn(e));
}

/**
 * Popups (sidebars, overview) open on every monitor at once (zpack "all"), which is as fast as
 * opening one. Each copy keeps itself only on the target monitor: the bar that was clicked
 * (openHere), else the monitor GlazeWM has focused. A second shortcut press closes the popup:
 * GlazeWM swallows the key and Zebar ignores starting an open preset, so shortcuts start a new
 * copy (`zebar start-widget`), which tells the open one to close and leaves. Also restarts the
 * bar if Zebar was started by a shortcut, which opens only that one widget.
 */
export async function openOnTargetMonitor() {
  document.documentElement.style.visibility = 'hidden';
  const win = zebar.currentWidget();
  const leave = async () => { win.window.tauri.close(); await new Promise(() => {}); };   // stops this copy's module
  // The press or click that just closed this popup (blur, focus change) must not reopen it.
  if (Date.now() - Number(localStorage.getItem('end4.closedAt.' + win.name) || 0) < 600) await leave();
  const provider = zebar.createProvider({ type: 'glazewm' });
  const wm = provider.output ?? await new Promise(res => {
    provider.onOutput(res);
    setTimeout(() => res(null), 1500);   // GlazeWM not answering: show every copy
  });
  provider.stop?.();
  const hint = JSON.parse(localStorage.getItem('end4.openOn') || 'null');
  const target = hint && Date.now() - hint.at < 3000 ? hint.monitor : wm?.focusedMonitor?.id;
  // Copies on other monitors leave before asking anything, so the copies of one key press
  // (which load up to ~0.5 s apart) can't mistake each other for an open popup.
  if (target && wm?.currentMonitor?.id !== target) await leave();
  // zpack opens popups unfocused so the copies don't steal focus from each other; take it now.
  // Windows refuses Zebar's own setFocus after a shortcut (the key went to GlazeWM), so the bridge
  // does it too, started here so it overlaps the checks below. Without focus, Esc and typing go
  // to the window underneath. Harmless if this copy turns out to be closing.
  win.window.tauri.setFocus().catch(() => {});
  bridge('focus-widget', win.name, Math.round(window.screenX * devicePixelRatio), Math.round(window.screenY * devicePixelRatio));
  const channel = new BroadcastChannel('end4-popup-' + win.name);
  const alreadyOpen = await new Promise(res => {
    channel.onmessage = e => e.data === 'open' && res(true);
    channel.postMessage('ping');
    setTimeout(() => res(false), 150);
  });
  if (alreadyOpen) { channel.postMessage('close'); await leave(); }
  // This copy is the one showing: answer later presses, and close on request (Esc path animates).
  channel.onmessage = e => {
    if (e.data === 'ping') channel.postMessage('open');
    if (e.data === 'close') window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
  };
  if (Date.now() - Number(localStorage.getItem('end4.barAlive') || 0) > 6000)
    for (const w of ['bar', 'desktop', 'visualizer']) openWidget(w);   // no duplicates if already open
  document.documentElement.style.visibility = '';
}

/**
 * Popups close on Esc, a click outside, or when another window gets focus, like end4. A popup
 * opened by a shortcut often never gets keyboard focus (Windows blocks background processes
 * from taking it), so it gets no blur event either; GlazeWM's focus changes cover that case.
 */
export function popupBehaviour({ onClose } = {}) {
  let closing = false;
  const close = async (reason = 'esc') => {
    if (closing) return;
    closing = true;
    localStorage.setItem('end4.closedAt.' + zebar.currentWidget().name, Date.now());
    console.debug('[end4] popup closed:', reason);
    document.body.classList.add('leaving');
    onClose?.();
    await new Promise(r => setTimeout(r, 160));
    zebar.currentWidget().window.tauri.close();
  };
  let armed = false, pinned = false;
  setTimeout(() => (armed = true), 500);
  const away = () => armed && !pinned && close('focus-moved');
  window.addEventListener('keydown', e => e.key === 'Escape' && close());
  window.addEventListener('blur', () => armed && !pinned && close('blur'));
  document.addEventListener('mousedown', e => e.target === document.body && !pinned && close('click-outside'));
  let focused;
  zebar.createProvider({ type: 'glazewm' }).onOutput(o => {
    const id = o?.focusedContainer?.id;
    if (focused !== undefined && id !== focused) away();
    focused = id;
  });
  close.togglePin = () => (pinned = !pinned);
  return close;
}

// ------------------------------------------------------------------ media
/** Interpolates the playback position between provider updates. */
// Zebar's position is a snapshot without the time Windows took it (often "0" from minutes ago),
// so the clock syncs from Windows' timeline (media-art.ps1 -Timeline: position + when it was taken)
// on track or play/pause changes and every `resync` ms, and interpolates in between.
export function mediaClock({ resync = 0 } = {}) {
  let base = 0, at = performance.now(), playing = false, key = '', reported = null, syncing = false, lastSync = 0;
  const fromTimeline = t => {
    if (!t?.ok) return;
    playing = t.playing;
    base = t.position + (t.playing ? (Date.now() - t.updated) / 1000 : 0);
    at = performance.now();
  };
  const clock = {
    update(s) {
      if (!s) { key = ''; return; }
      const k = s.sessionId + '|' + s.title;
      const now = performance.now();
      if (k !== key || s.position !== reported) { base = s.position; at = now; }   // Windows published a new timeline
      else if (s.isPlaying !== playing) { base = clock.position(); at = now; }   // pause/resume where we are
      const changed = k !== key || s.isPlaying !== playing;
      key = k; reported = s.position; playing = s.isPlaying;
      // media-art.ps1 reads Windows' *current* session, so only sync when that's the one shown.
      if (s.isCurrentSession !== false && (changed || (resync && Date.now() - lastSync > resync))) clock.sync();
    },
    async sync() {
      if (syncing) return;
      syncing = true; lastSync = Date.now();
      try { fromTimeline(await run('media-art', '-Timeline')); } finally { syncing = false; }
    },
    async seek(seconds) {
      base = seconds; at = performance.now();   // move the bar at once; Windows confirms below
      fromTimeline(await run('media-art', '-Seek', Math.max(0, seconds).toFixed(1)));
    },
    position() { return playing ? base + (performance.now() - at) / 1000 : base; },
  };
  return clock;
}

export const fmtTime = s => {
  s = Math.max(0, Math.floor(s || 0));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

/** Album art: Windows media session thumbnail via media-art.ps1. */
let artKey = '', artUrl = '';
export async function albumArt(session) {
  if (!session) return '';
  const key = `${session.title}|${session.artist}`;
  if (key === artKey) return artUrl;
  artKey = key;
  artUrl = '';
  const res = await run('media-art');
  artUrl = res?.ok ? './cover.jpg?t=' + Date.now() : '';
  return artUrl;
}

/**
 * Synced lyrics from lrclib.net (same source end4 uses). Music apps give clean title/artist, so
 * try lrclib's exact lookup first. Browser players (YouTube) give titles like
 * "Lyrical: Song | Movie | Singer" and a label channel as "artist", so then search free text with
 * pieces of the title, accepting only results whose length matches the video (±5 s) so a
 * different song with a similar name can't slip in. Results (and misses) are kept in localStorage:
 * every sidebar open is a fresh page, and lrclib rate-limits repeated searches.
 */
const LYRIC_NOISE = /\b(official|lyrics?|lyrical|video|audio|full song|full video|hd|4k|hq|visuali[sz]er|ost|season \d+|coke studio|remaster(ed)?|live)\b/i;
const LABELS = /^(t-series|zee music company|sony music.*|saregama.*|tips official|yrf|speed records|eros now music|.*vevo)$/i;

export function lyricQueries(title, artist = '') {
  const parts = title.replace(/[[(【][^\])】]*[\])】]/g, ' ')                  // (Official Video), [LYRICS]
    .replace(/^\s*(lyrical|full video|full song|video|audio)\s*:\s*/i, '')
    .split(/\s*[|｜•]\s*|\s+[-–—]\s+/).map(p => p.trim()).filter(p => p && !LYRIC_NOISE.test(p));
  const who = artist.replace(/\s*-\s*Topic$/i, '').trim();
  const withArtist = q => who && !LABELS.test(who) && !q.toLowerCase().includes(who.toLowerCase()) ? `${q} ${who}` : q;
  const qs = [parts.length > 1 ? withArtist(parts.slice(0, 2).join(' ')) : null, parts[0] && withArtist(parts[0]), parts[1]];
  return [...new Set(qs.filter(Boolean))];
}

const LYRIC_STORE = 'end4.lyrics';
const lyricStore = () => { try { return JSON.parse(localStorage.getItem(LYRIC_STORE)) || {}; } catch { return {}; } };
function remember(key, value) {
  const all = lyricStore();
  all[key] = { v: value, at: Date.now() };
  const keys = Object.keys(all).sort((a, b) => all[b].at - all[a].at);
  for (const k of keys.slice(40)) delete all[k];   // keep the 40 most recent tracks
  try { localStorage.setItem(LYRIC_STORE, JSON.stringify(all)); } catch { /* full: skip caching */ }
}

// lrclib sits behind Cloudflare, which answers 503 "retry-after: 1" whenever it's busy (about half
// of searches); the same request usually works a second later.
async function lrclibFetch(url) {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch(url);
    if (!(r.status === 429 || r.status >= 500) || attempt === 3) return r;
    await new Promise(res => setTimeout(res, 1000 * (attempt + 1)));
  }
}

export async function lyrics(session) {
  if (!session?.title) return null;
  const key = `${session.artist}|${session.title}`;
  const hit = lyricStore()[key];
  if (hit && (hit.v || Date.now() - hit.at < 6 * 3600e3)) return hit.v;   // retry misses after 6 h
  const dur = Math.round((session.endTime || 0) - (session.startTime || 0));
  const parse = d => parseLrc(d?.syncedLyrics) || (d?.plainLyrics ? { plain: d.plainLyrics.split('\n') } : null);
  try {
    const q = new URLSearchParams({ track_name: session.title, artist_name: session.artist || '' });
    if (session.albumTitle) q.set('album_name', session.albumTitle);
    if (dur > 0) q.set('duration', dur);
    const r = await lrclibFetch('https://lrclib.net/api/get?' + q);
    if (r.status === 429 || r.status >= 500) return null;   // rate-limited or down: try again next time, don't cache
    let found = r.ok ? parse(await r.json()) : null;
    for (const query of found ? [] : lyricQueries(session.title, session.artist)) {
      const s = await lrclibFetch('https://lrclib.net/api/search?' + new URLSearchParams({ q: query }));
      if (s.status === 429 || s.status >= 500) return null;
      const list = s.ok ? await s.json() : [];
      const fits = (Array.isArray(list) ? list : []).filter(x => !(dur > 0) || Math.abs(x.duration - dur) <= 5);
      found = parse(fits.find(x => x.syncedLyrics) || fits.find(x => x.plainLyrics));
      if (found) break;
    }
    remember(key, found);
    return found;
  } catch {
    return null;                                       // offline: don't cache, try again next time
  }
}

function parseLrc(text) {
  if (!text) return null;
  const lines = [];
  for (const raw of text.split('\n')) {
    const m = raw.match(/^\[(\d+):(\d+(?:\.\d+)?)\]\s*(.*)$/);
    if (m) lines.push({ t: +m[1] * 60 + +m[2], text: m[3] });
  }
  return lines.length ? { synced: lines } : null;
}

export function currentLyricIndex(synced, pos) {
  let i = -1;
  for (let k = 0; k < synced.length && synced[k].t <= pos + 0.25; k++) i = k;
  return i;
}

// ------------------------------------------------------------------ misc
export const weatherIcon = status => ({
  clear_day: 'sunny', clear_night: 'clear_night',
  cloudy_day: 'partly_cloudy_day', cloudy_night: 'partly_cloudy_night',
  light_rain_day: 'rainy', light_rain_night: 'rainy',
  heavy_rain_day: 'rainy', heavy_rain_night: 'rainy',
  snow_day: 'weather_snowy', snow_night: 'weather_snowy',
  thunder_day: 'thunderstorm', thunder_night: 'thunderstorm',
})[status] || 'cloud';

export const weatherText = status =>
  (status || '').replace(/_(day|night)$/, '').replace(/_/g, ' ').replace(/^clear$/, 'clear sky');

export const temp = (w, unit) =>
  w ? `${Math.round(unit === 'F' ? w.fahrenheitTemp : w.celsiusTemp)}°${unit === 'F' ? 'F' : 'C'}` : '';

export function volumeIcon(dev) {
  if (!dev) return 'volume_off';
  if (dev.isMuted || dev.volume === 0) return 'volume_off';
  return dev.volume < 34 ? 'volume_mute' : dev.volume < 67 ? 'volume_down' : 'volume_up';
}

export function networkIcon(net) {
  const t = net?.defaultInterface?.type;
  if (t === 'ethernet') return 'lan';
  if (t === 'wifi') {
    const s = net.defaultGateway?.signalStrength ?? 0;
    return s >= 80 ? 'signal_wifi_4_bar' : s >= 60 ? 'network_wifi_3_bar' : s >= 40 ? 'network_wifi_2_bar' : s >= 20 ? 'network_wifi_1_bar' : 'signal_wifi_0_bar';
  }
  if (t === 'proprietary_virtual') return 'vpn_lock';
  return 'signal_wifi_off';
}

export function batteryIcon(b) {
  if (!b) return '';
  if (b.isCharging) return 'battery_charging_full';
  const p = b.chargePercent;
  return p > 95 ? 'battery_full' : `battery_${Math.max(0, Math.min(6, Math.round(p / 16.6)))}_bar`;
}

export const prettyApp = name => {
  if (!name) return '';
  const n = name.replace(/\.exe$/i, '');
  const known = { msedge: 'Microsoft Edge', explorer: 'File Explorer', WindowsTerminal: 'Terminal', Code: 'Visual Studio Code', chrome: 'Google Chrome', firefox: 'Firefox', ApplicationFrameHost: 'App' };
  return known[n] || n.charAt(0).toUpperCase() + n.slice(1);
};

export const greeting = (d = new Date()) => {
  const h = d.getHours();
  return h < 5 ? 'Good Night' : h < 12 ? 'Good Morning' : h < 17 ? 'Good Afternoon' : h < 22 ? 'Good Evening' : 'Good Night';
};

/** Month grid (Monday first, like end4's calendar). */
export function monthGrid(date = new Date()) {
  const y = date.getFullYear(), m = date.getMonth();
  const first = new Date(y, m, 1);
  const offset = (first.getDay() + 6) % 7;
  const days = new Date(y, m + 1, 0).getDate();
  const prevDays = new Date(y, m, 0).getDate();
  const cells = [];
  for (let i = offset - 1; i >= 0; i--) cells.push({ d: prevDays - i, out: true });
  const now = new Date();
  for (let d = 1; d <= days; d++) cells.push({ d, today: d === now.getDate() && sameMonth(date, now) });
  while (cells.length % 7) cells.push({ d: cells.length - offset - days + 1, out: true });
  return cells;
}
const sameMonth = (a, b) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth();

/** Stable colour slot for an app name (overview tiles). */
export const hashSlot = (s, n = 3) => [...String(s)].reduce((a, c) => (a * 31 + c.charCodeAt(0)) >>> 0, 7) % n;
