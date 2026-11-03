// end4-pC for Windows - dock (end4's Dock): pinned apps, then every other app with an open window.
import { zebar, $, esc, setHTML, config, watchTheme, bridge, prettyApp, reloadOnConfigChange } from './lib/core.js';

const cfg = await config();
if (!cfg.dock.enabled) { zebar.currentWidget().window.tauri.close(); await new Promise(() => {}); }
reloadOnConfigChange();   // pinning saves config.json, which reloads the dock
await watchTheme();

const providers = zebar.createProviderGroup({ glazewm: { type: 'glazewm' } });
const windowsOf = node => (node?.children || []).flatMap(c => (c.type === 'window' ? [c] : windowsOf(c)));
const exes = {};            // app -> exe path, for pinning
const asked = new Set();    // apps whose icon was already requested
let iconStamp = Date.now();

function apps() {
  const wins = (providers.outputMap.glazewm?.allWorkspaces || []).flatMap(windowsOf);
  const groups = new Map(cfg.dock.pinned.map(p => [p.name, { name: p.name, pinned: true, wins: [] }]));
  for (const w of wins) {
    if (!w.processName) continue;
    if (!groups.has(w.processName)) groups.set(w.processName, { name: w.processName, pinned: false, wins: [] });
    groups.get(w.processName).wins.push(w);
  }
  return [...groups.values()];
}

function render() {
  const list = apps();
  const missing = list.map(a => a.name).filter(n => !asked.has(n));
  if (missing.length) {
    missing.forEach(n => asked.add(n));
    bridge('app-icons', ...missing).then(r => { Object.assign(exes, r.exes || {}); iconStamp = Date.now(); render(); });
  }
  const item = a => `<button class="app${a.wins.length ? ' running' : ''}${a.wins.some(w => w.hasFocus) ? ' focused' : ''}" data-app="${esc(a.name)}"
    title="${esc(prettyApp(a.name))}${a.wins.length > 1 ? ` (${a.wins.length} windows)` : ''} · right-click to ${a.pinned ? 'unpin' : 'pin'}">
    <img src="./icons/${encodeURIComponent(a.name)}.png?t=${iconStamp}" alt="" /><span class="letter">${esc(prettyApp(a.name).charAt(0))}</span></button>`;
  const pinned = list.filter(a => a.pinned), rest = list.filter(a => !a.pinned);
  setHTML($('#dock'), pinned.map(item).join('') + (pinned.length && rest.length ? '<span class="sep"></span>' : '') + rest.map(item).join(''));
}
// Apps without an icon file show their first letter instead.
document.addEventListener('error', e => e.target.tagName === 'IMG' && e.target.remove(), true);

$('#dock').addEventListener('click', e => {
  const name = e.target.closest('[data-app]')?.dataset.app;
  const a = name && apps().find(x => x.name === name);
  if (!a) return;
  if (!a.wins.length) return bridge('launch', name);
  const next = a.wins[(a.wins.findIndex(w => w.hasFocus) + 1) % a.wins.length];   // clicking again cycles its windows
  providers.outputMap.glazewm?.runCommand(`focus --container-id ${next.id}`);
});
$('#dock').addEventListener('contextmenu', e => {
  e.preventDefault();
  const name = e.target.closest('[data-app]')?.dataset.app;
  if (!name) return;
  const pinned = cfg.dock.pinned.filter(p => p.name !== name);
  if (pinned.length === cfg.dock.pinned.length) {
    if (!exes[name]) return;   // exe not found (e.g. an elevated app): can't relaunch it later
    pinned.push({ name, path: exes[name] });
  }
  bridge('config-set', 'dock.pinned', JSON.stringify(pinned));
});

providers.onOutput(render);
render();
