import { zebar, $, $$, esc, icon, setHTML, config, watchTheme, popupBehaviour, prettyApp, hashSlot, openOnTargetMonitor } from './lib/core.js';

await openOnTargetMonitor();
const cfg = await config();
await watchTheme();
const close = popupBehaviour();

const providers = zebar.createProviderGroup({ glazewm: { type: 'glazewm' } });
let wm = providers.outputMap.glazewm;
let query = '';
let sel = 0;
let flat = [];  // visible windows in display order, for keyboard navigation

const windowsOf = node => (node?.children || []).flatMap(c => (c.type === 'window' ? [c] : windowsOf(c)));
const matches = w => !query || `${w.title} ${w.processName}`.toLowerCase().includes(query);

function render() {
  wm = providers.outputMap.glazewm;
  if (!wm) return setHTML($('#grid'), `<div class="empty-ws">GlazeWM is not running</div>`);
  const byName = Object.fromEntries((wm.allWorkspaces || []).map(w => [w.name, w]));
  const names = [...new Set([...Array.from({ length: cfg.workspaces }, (_, i) => String(i + 1)), ...Object.keys(byName)])];
  const current = wm.focusedWorkspace?.name;
  flat = [];
  let html = '';
  for (const name of names) {
    const ws = byName[name];
    const wins = windowsOf(ws).filter(matches);
    if (query && !wins.length) continue;
    html += `<div class="ws${name === current ? ' current' : ''}" data-ws="${esc(name)}">
      <div class="ws-head" data-focus-ws="${esc(name)}"><b>${esc(ws?.displayName || name)}</b>${ws ? `${windowsOf(ws).length} window${windowsOf(ws).length === 1 ? '' : 's'}` : 'Empty'}</div>
      <div class="wins">${wins.map(w => {
        const i = flat.push(w) - 1;
        const app = prettyApp(w.processName);
        return `<button class="win${i === sel ? ' sel' : ''}${w.hasFocus ? ' focused' : ''}" draggable="true" data-win="${w.id}" data-i="${i}">
          <span class="app s${hashSlot(w.processName)}">${esc(app.charAt(0))}</span>
          <span class="tx"><b class="ellipsis">${esc(app)}</b><span class="ellipsis">${esc(w.title)}</span></span>
          ${w.state?.type === 'floating' ? `<span class="badge">${icon('picture_in_picture')}</span>` : ''}
        </button>`;
      }).join('')}</div>
    </div>`;
  }
  if (sel >= flat.length) sel = Math.max(0, flat.length - 1);
  setHTML($('#grid'), html || `<div class="empty-ws">No windows match “${esc(query)}”</div>`);
}

async function focusWindow(id) {
  await wm?.runCommand(`focus --container-id ${id}`);
  close();
}

providers.onOutput(render);
render();

// ---------------------------------------------------------------- input
$('#q').focus();
$('#q').addEventListener('input', e => { query = e.target.value.trim().toLowerCase(); sel = 0; render(); });
window.addEventListener('keydown', e => {
  if (e.key === 'Enter' && flat[sel]) { e.preventDefault(); focusWindow(flat[sel].id); }
  else if (e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey)) { e.preventDefault(); sel = Math.min(flat.length - 1, sel + 1); render(); }
  else if (e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey)) { e.preventDefault(); sel = Math.max(0, sel - 1); render(); }
  else if (/^[0-9]$/.test(e.key) && e.altKey) { wm?.runCommand(`focus --workspace ${e.key === '0' ? 10 : e.key}`); close(); }
});

$('#grid').addEventListener('click', e => {
  const w = e.target.closest('[data-win]');
  if (w) return focusWindow(w.dataset.win);
  const head = e.target.closest('[data-focus-ws]');
  if (head) { wm?.runCommand(`focus --workspace ${head.dataset.focusWs}`); close(); }
});
$('#grid').addEventListener('auxclick', e => {
  const w = e.target.closest('[data-win]');
  if (w && e.button === 1) wm?.runCommand('close', w.dataset.win); // middle-click closes, like end4
});

// drag a window onto another workspace to move it
let dragId = null;
$('#grid').addEventListener('dragstart', e => {
  const w = e.target.closest('[data-win]');
  if (!w) return;
  dragId = w.dataset.win;
  w.classList.add('dragging');
  e.dataTransfer.effectAllowed = 'move';
});
$('#grid').addEventListener('dragend', () => { dragId = null; $$('.dragging, .drop').forEach(x => x.classList.remove('dragging', 'drop')); });
$('#grid').addEventListener('dragover', e => {
  const ws = e.target.closest('[data-ws]');
  if (!ws || !dragId) return;
  e.preventDefault();
  $$('.drop').forEach(x => x !== ws && x.classList.remove('drop'));
  ws.classList.add('drop');
});
$('#grid').addEventListener('drop', e => {
  const ws = e.target.closest('[data-ws]');
  if (ws && dragId) { e.preventDefault(); wm?.runCommand(`move --workspace ${ws.dataset.ws}`, dragId); }
});
