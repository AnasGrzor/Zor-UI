// Run: node tests/barlayout.test.mjs
import assert from 'node:assert/strict';
import { BAR_MODULES, barLayout, moveModule, placeModule } from '../zebar/zor-ui/lib/barlayout.js';

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
// Drag and drop: to another side at a position, or to the top/bottom of its own side.
const side = (l, sd) => l.filter(m => m[1] === sd).map(m => m[0]);
const p1 = placeModule(d, 'tray', 'left', 1);
assert.deepEqual(side(p1, 'left'), ['launcher', 'tray', 'window']);
assert.ok(!side(p1, 'right').includes('tray'));
assert.equal(p1.length, d.length);
assert.deepEqual(side(placeModule(d, 'weather', 'center', 0), 'center')[0], 'weather');
assert.deepEqual(side(placeModule(d, 'resources', 'center', 99), 'center').at(-1), 'resources');
assert.deepEqual(side(placeModule(d, 'media', 'hidden', 0), 'hidden'), ['media', 'netspeed']);
console.log('barlayout ok');
