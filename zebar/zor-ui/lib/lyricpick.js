// Zor UI - picks the right lrclib search result for what's playing.
// No imports, so tests/lyricpick.test.mjs can run it in Node.

const norm = s => String(s || '').toLowerCase().normalize('NFKD').replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
const words = s => norm(s).split(' ').filter(w => w.length > 2);

/**
 * list: lrclib /api/search results. Returns { item, synced } or null.
 * 1. A result within 5 s of the track's length (synced if it has timings).
 * 2. Otherwise one whose title appears in the playing title and whose artist shares a word with
 *    the playing artist or title, as plain lyrics only: video uploads run longer than the song
 *    (intros, outros), so its timings wouldn't line up.
 */
export function pickLyrics(list, { dur = 0, title = '', artist = '' } = {}) {
  const items = (Array.isArray(list) ? list : []).filter(x => x && (x.syncedLyrics || x.plainLyrics));
  const near = items.filter(x => !(dur > 0) || Math.abs(x.duration - dur) <= 5);
  const best = near.find(x => x.syncedLyrics) || near.find(x => x.plainLyrics);
  if (best) return { item: best, synced: !!best.syncedLyrics };
  const t = norm(title), who = words(`${artist} ${title}`);
  const same = items.find(x => {
    const xt = norm(x.trackName);
    return xt && (xt === t || ` ${t} `.includes(` ${xt} `)) && words(x.artistName).some(w => who.includes(w));
  });
  return same ? { item: same, synced: false } : null;
}

/** Lyrics text without timings (plainLyrics, or syncedLyrics with the [mm:ss] tags removed). */
export const plainText = item => (item.plainLyrics || item.syncedLyrics.replace(/^\[[\d:.]+\]\s?/gm, '')).split('\n');
