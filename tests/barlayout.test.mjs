// Run: node tests/barlayout.test.mjs
import assert from 'node:assert/strict';
import { BAR_MODULES, barLayout, moveModule } from '../zebar/end4-pC/lib/barlayout.js';

// Nothing saved: the defaults, in order.
assert.deepEqual(barLayout(undefined), BAR_MODULES.map(m => [m[0], m[1]]));
// The old "Network speed" switch moves it to the right.
assert.deepEqual(barLayout(undefined, { netSpeed: true }).find(m => m[0] === 'netspeed'), ['netspeed', 'right']);

// A saved layout keeps its order and sides; junk and duplicates are dropped; missing modules are appended.
const saved = [['clock', 'left'], ['bogus', 'left'], ['clock', 'right'], ['tray', 'nowhere'], ['media', 'hidden']];
const l = barLayout(saved);
assert.deepEqual(l.slice(0, 2), [['clock', 'left'], ['media', 'hidden']]);
assert.equal(l.length, BAR_MODULES.length);
assert.deepEqual(l.find(m => m[0] === 'tray'), ['tray', 'right']);

// Moving skips modules on other sides and stops at the ends.
const d = barLayout(undefined);
const up = moveModule(d, 'weather', -1);   // center: ... clock, utils, weather -> ... clock, weather, utils
assert.deepEqual(up.filter(m => m[1] === 'center').map(m => m[0]).slice(-2), ['weather', 'utils']);
const first = moveModule(d, 'launcher', -1);
assert.deepEqual(first, d);
// The window's left neighbour is the launcher; moving it up swaps them.
assert.deepEqual(moveModule(d, 'window', -1).slice(0, 2), [['window', 'left'], ['launcher', 'left']]);
console.log('barlayout ok');
