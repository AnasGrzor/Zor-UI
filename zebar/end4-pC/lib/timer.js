// end4-pC for Windows - pomodoro + stopwatch (end4's TimerService).
// State lives in localStorage as timestamps, so it survives the sidebar closing and every
// widget (bar, sidebar, desktop) sees the same timer. The bar calls tick() every second and
// announces finished phases.

const KEY = 'end4-pC.timer';
const fresh = () => ({ phase: 'focus', cycle: 0, running: false, endsAt: 0, left: null, sw: { running: false, start: 0, acc: 0, laps: [] } });

export function load() {
  try { return { ...fresh(), ...JSON.parse(localStorage.getItem(KEY)) }; } catch { return fresh(); }
}
const save = s => { localStorage.setItem(KEY, JSON.stringify(s)); return s; };

export const PHASES = { focus: 'Focus', short: 'Short break', long: 'Long break' };
const minutes = (cfg, phase) => (cfg.timer?.[phase] ?? { focus: 25, short: 5, long: 15 }[phase]);

/** Seconds left in the current pomodoro phase. */
export function left(s, cfg) {
  if (s.running) return Math.max(0, (s.endsAt - Date.now()) / 1000);
  return s.left ?? minutes(cfg, s.phase) * 60;
}
export const total = (s, cfg) => minutes(cfg, s.phase) * 60;

function advance(s, cfg) {
  if (s.phase === 'focus') {
    s.cycle++;
    s.phase = s.cycle % (cfg.timer?.cycles || 4) === 0 ? 'long' : 'short';
  } else s.phase = 'focus';
  s.running = false; s.left = null;
  return s;
}

export const pomo = {
  toggle(cfg) {
    const s = load();
    if (s.running) { s.left = left(s, cfg); s.running = false; }
    else { s.endsAt = Date.now() + left(s, cfg) * 1000; s.left = null; s.running = true; }
    return save(s);
  },
  reset() { const s = load(); return save({ ...fresh(), sw: s.sw }); },
  skip(cfg) { return save(advance(load(), cfg)); },
};

/** Ends a finished phase and returns its name ('focus' | 'short' | 'long'), else null. */
export function tick(cfg) {
  const s = load();
  if (!s.running || Date.now() < s.endsAt) return null;
  const done = s.phase;
  save(advance(s, cfg));
  return done;
}

export const swElapsed = s => (s.sw.acc + (s.sw.running ? Date.now() - s.sw.start : 0)) / 1000;
export const sw = {
  toggle() {
    const s = load();
    if (s.sw.running) { s.sw.acc += Date.now() - s.sw.start; s.sw.running = false; }
    else { s.sw.start = Date.now(); s.sw.running = true; }
    return save(s);
  },
  lap() { const s = load(); if (s.sw.running) s.sw.laps.unshift(swElapsed(s)); return save(s); },
  reset() { const s = load(); s.sw = fresh().sw; return save(s); },
};

export const clock = sec => {
  sec = Math.max(0, Math.floor(sec));
  const h = Math.floor(sec / 3600), m = Math.floor(sec / 60) % 60, x = sec % 60;
  return (h ? `${h}:${String(m).padStart(2, '0')}` : String(m)) + ':' + String(x).padStart(2, '0');
};
