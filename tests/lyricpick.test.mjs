// Run: node tests/lyricpick.test.mjs
import assert from 'node:assert/strict';
import { pickLyrics, plainText } from '../zebar/end4-pC/lib/lyricpick.js';

const song = (trackName, artistName, duration, synced = true) => ({
  trackName, artistName, duration, plainLyrics: 'line one\nline two', syncedLyrics: synced ? '[00:01.00] line one\n[00:03.50] line two' : null,
});

// Same length: synced lyrics win.
assert.deepEqual(pickLyrics([song('Tum Hi Ho', 'Arijit Singh', 261)], { dur: 262, title: 'Tum Hi Ho', artist: 'Arijit Singh' }),
  { item: song('Tum Hi Ho', 'Arijit Singh', 261), synced: true });

// A YouTube video 40 s longer than the song: same song, plain lyrics only.
const yt = { dur: 301, title: 'Teriyaan - Asim Azhar & Aima Baig (Official Video)', artist: 'Asim Azhar' };
const r = pickLyrics([song('Teriyaan Yaadan', 'Sharry Nexus', 220), song('Teriyaan', 'Asim Azhar, Aima Baig', 261)], yt);
assert.equal(r.item.artistName, 'Asim Azhar, Aima Baig');
assert.equal(r.synced, false);

// The artist only in the title (a label channel uploaded it) still counts.
assert.ok(pickLyrics([song('Teriyaan', 'Asim Azhar', 261)], { ...yt, artist: 'T-Series' }));

// A different song with a similar name and another artist is rejected.
assert.equal(pickLyrics([song('Teriyaan Yaadan', 'Sharry Nexus', 220)], yt), null);
// A title word alone (e.g. "Love" inside "Lovely") isn't a match.
assert.equal(pickLyrics([song('Love', 'Billie Eilish', 100)], { dur: 200, title: 'Lovely', artist: 'Billie Eilish' }), null);

assert.deepEqual(plainText({ syncedLyrics: '[00:01.00] line one\n[00:03.50] line two' }), ['line one', 'line two']);
console.log('lyricpick ok');
