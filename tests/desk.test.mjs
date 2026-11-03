// Run: node tests/desk.test.mjs
import assert from 'node:assert/strict';
import { defaultSlot, onScreen } from '../zebar/end4-pC/lib/desk.js';

// Cards stack top to bottom in list order under the bar.
assert.deepEqual(defaultSlot(['clock', 'weather'], 'clock', 1080), { x: 16, y: 72 });
assert.deepEqual(defaultSlot(['clock', 'weather'], 'weather', 1080), { x: 16, y: 72 + 250 + 10 });

// A card that would run off the bottom starts a new column.
const all = ['clock', 'weather', 'week', 'greet', 'media', 'calendar', 'todo', 'notes'];
const notes = defaultSlot(all, 'notes', 1080);
assert.equal(notes.x, 16 + 310 + 10);
assert.equal(notes.y, 72 + 150 + 10);   // after todo, the first card of the 2nd column

// Positions on either monitor count; one left behind by an unplugged monitor doesn't.
const monitors = [
  { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 } },
  { position: { x: -1920, y: 0 }, size: { width: 1920, height: 1080 } },
];
assert.ok(onScreen({ x: 16, y: 72 }, monitors));
assert.ok(onScreen({ x: -1800, y: 500 }, monitors));
assert.ok(!onScreen({ x: 2500, y: 72 }, monitors));
assert.ok(!onScreen({ x: 16, y: 1070 }, monitors));
console.log('desk ok');
