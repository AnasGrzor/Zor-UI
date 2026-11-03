// Run: node tests/timer.test.mjs
import assert from 'node:assert/strict';

const store = {};
globalThis.localStorage = { getItem: k => store[k] ?? null, setItem: (k, v) => { store[k] = String(v); } };
let now = 1_000_000;
Date.now = () => now;

const timer = await import('../zebar/end4-pC/lib/timer.js');
const cfg = { timer: { focus: 25, short: 5, long: 15, cycles: 2 } };

// Fresh state: focus, 25 minutes, not running.
assert.equal(timer.left(timer.load(), cfg), 25 * 60);

// Start, 10 minutes pass, pause: 15 minutes left and it stays there.
timer.pomo.toggle(cfg);
now += 10 * 60e3;
timer.pomo.toggle(cfg);
now += 60 * 60e3;
assert.equal(timer.left(timer.load(), cfg), 15 * 60);
assert.equal(timer.tick(cfg), null);

// Resume and let it run out: tick reports the finished phase once, then a short break is next.
timer.pomo.toggle(cfg);
now += 15 * 60e3;
assert.equal(timer.tick(cfg), 'focus');
assert.equal(timer.tick(cfg), null);
let s = timer.load();
assert.equal(s.phase, 'short');
assert.equal(s.running, false);
assert.equal(timer.left(s, cfg), 5 * 60);

// After `cycles` focus sessions the break is long.
timer.pomo.skip(cfg);                       // short -> focus
timer.pomo.skip(cfg);                       // focus (2nd) -> long
assert.equal(timer.load().phase, 'long');

// Stopwatch: runs, pauses, keeps laps; reset leaves the pomodoro alone.
timer.sw.toggle();
now += 4500;
timer.sw.lap();
now += 500;
timer.sw.toggle();
now += 99e3;
s = timer.load();
assert.equal(timer.swElapsed(s), 5);
assert.deepEqual(s.sw.laps, [4.5]);
timer.sw.reset();
assert.equal(timer.swElapsed(timer.load()), 0);
assert.equal(timer.load().phase, 'long');

assert.equal(timer.clock(65), '1:05');
assert.equal(timer.clock(3725), '1:02:05');
console.log('timer ok');
