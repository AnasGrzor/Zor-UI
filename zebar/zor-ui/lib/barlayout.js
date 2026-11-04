// Zor UI - which bar modules show, on which side, in what order (Settings > Bar).
// No imports, so tests/barlayout.test.mjs can run it in Node.

// [id, default side, label, icon], in default order. Sides: left, center, right, hidden.
export const BAR_MODULES = [
  ['launcher', 'left', 'Sidebar button', 'auto_awesome'], ['window', 'left', 'Active window', 'web_asset'],
  ['resources', 'center', 'CPU & RAM', 'memory'], ['media', 'center', 'Media', 'music_note'],
  ['island', 'center', 'Dynamic Island', 'pill'],
  ['workspaces', 'center', 'Workspaces', 'grid_view'], ['clock', 'center', 'Clock', 'schedule'],
  ['utils', 'center', 'Utility buttons', 'build'], ['weather', 'center', 'Weather', 'partly_cloudy_day'],
  ['wm', 'right', 'Tiling status', 'view_quilt'], ['netspeed', 'hidden', 'Network speed', 'swap_vert'],
  ['tray', 'right', 'System tray', 'apps'], ['system', 'right', 'Quick settings', 'tune'],
];
export const SIDES = ['left', 'center', 'right', 'hidden'];

/**
 * The saved layout ([[id, side], ...] from config bar.modules) with unknown ids dropped and any
 * module it lacks (e.g. added in an update) appended with its default side. netSpeed: the old
 * showNetSpeed switch, which put the network speed on the right.
 */
export function barLayout(saved, { netSpeed = false } = {}) {
  const known = new Map(BAR_MODULES.map(m => [m[0], m[1]]));
  const list = (Array.isArray(saved) ? saved : [])
    .filter(m => Array.isArray(m) && known.has(m[0]) && SIDES.includes(m[1]))
    .filter((m, i, all) => all.findIndex(x => x[0] === m[0]) === i)
    .map(([id, side]) => [id, side]);
  for (const [id, side] of known)
    if (!list.some(m => m[0] === id)) list.push([id, id === 'netspeed' && netSpeed ? 'right' : side]);
  return list;
}

/** Moves a module one step (dir -1 / 1) among the modules on its own side. */
export function moveModule(list, id, dir) {
  const out = list.map(m => [...m]);
  const i = out.findIndex(m => m[0] === id);
  if (i < 0) return out;
  let j = i + dir;
  while (j >= 0 && j < out.length && out[j][1] !== out[i][1]) j += dir;
  if (j >= 0 && j < out.length) [out[i], out[j]] = [out[j], out[i]];
  return out;
}

/** Puts a module on `side` at position `index` among that side's modules (drag and drop). */
export function placeModule(list, id, side, index) {
  const rest = list.filter(m => m[0] !== id).map(m => [...m]);
  const bySide = SIDES.map(sd => rest.filter(m => m[1] === sd));
  const col = bySide[SIDES.indexOf(side)];
  col.splice(Math.max(0, Math.min(index, col.length)), 0, [id, side]);
  return bySide.flat();
}
