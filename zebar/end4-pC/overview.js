import { zebar, $, $$, esc, icon, setHTML, config, watchTheme, popupBehaviour, prettyApp, hashSlot, openOnTargetMonitor, run } from './lib/core.js';

if ((await config()).native?.['overview']) { zebar.currentWidget().window.tauri.close().catch(() => {}); await new Promise(() => {}); }   // drawn by native/end4 instead
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

// ---------------------------------------------------------------- clipboard history (Windows' Win+V list)
let clip = null;   // null = not loaded yet
const clipMode = () => query.startsWith(';');
const ago = t => {
  const s = Date.now() / 1000 - t;
  return s < 60 ? 'now' : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
};
function renderClip() {
  $('#grid').classList.add('list');
  setHTML($('#hint'), 'Enter or click copies it back · arrows to move · Esc to close');
  if (clip === null) {
    clip = { loading: true };
    run('toggles', '-Action', 'clipboard').then(r => { clip = r; render(); });
  }
  if (clip.loading) return setHTML($('#grid'), '<div class="spinner"></div>');
  if (clip.error) return setHTML($('#grid'), `<div class="err">${esc(clip.error)}</div>`);
  const q = query.slice(1).trim();
  flat = (clip.items || []).filter(c => (c.text ? c.text.toLowerCase().includes(q) : !q));
  if (sel >= flat.length) sel = Math.max(0, flat.length - 1);
  setHTML($('#grid'), flat.length ? flat.map((c, k) => `<button class="clip${k === sel ? ' sel' : ''}" data-clip="${c.i}">
    ${icon(c.text ? 'notes' : 'image')}<span class="tx">${c.text ? esc(c.text) : 'Image'}</span><time>${ago(c.time)}</time></button>`).join('')
    : `<div class="empty-ws">${clip.items?.length ? 'Nothing matches' : 'Clipboard history is empty'}</div>`);
}
async function pickClip(i) {
  await run('toggles', '-Action', 'clipboard-set', '-Value', i);
  close();
}

function render() {
  if (clipMode()) return renderClip();
  $('#grid').classList.remove('list');
  setHTML($('#hint'), 'Enter to focus · arrows to move · drag a window onto a workspace to move it · Esc to close');
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
        return `<button class="win${i === sel ? ' sel' : ''}${w.hasFocus ? ' focused' : ''}" data-win="${w.id}" data-i="${i}">
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
  if (e.key === 'Enter' && flat[sel]) { e.preventDefault(); clipMode() ? pickClip(flat[sel].i) : focusWindow(flat[sel].id); }
  else if (e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey)) { e.preventDefault(); sel = Math.min(flat.length - 1, sel + 1); render(); }
  else if (e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey)) { e.preventDefault(); sel = Math.max(0, sel - 1); render(); }
  else if (/^[0-9]$/.test(e.key) && e.altKey) { wm?.runCommand(`focus --workspace ${e.key === '0' ? 10 : e.key}`); close(); }
});

$('#clip-btn').addEventListener('click', e => {
  e.preventDefault();
  $('#q').value = ';'; query = ';'; sel = 0; render(); $('#q').focus();
});
$('#grid').addEventListener('click', e => {
  const c = e.target.closest('[data-clip]');
  if (c) return pickClip(c.dataset.clip);
  const w = e.target.closest('[data-win]');
  if (w) return focusWindow(w.dataset.win);
  const head = e.target.closest('[data-focus-ws]');
  if (head) { wm?.runCommand(`focus --workspace ${head.dataset.focusWs}`); close(); }
});
$('#grid').addEventListener('auxclick', e => {
  const w = e.target.closest('[data-win]');
  if (w && e.button === 1) wm?.runCommand('close', w.dataset.win); // middle-click closes, like end4
});

// Drag a window onto another workspace to move it. Pointer events, not HTML drag and drop: Tauri
// handles file drops on Windows, which keeps the webview's dragover/drop events from firing.
let drag = null, dragged = false;   // drag: { id, x, y, ghost }
const wsAt = e => document.elementFromPoint(e.clientX, e.clientY)?.closest('[data-ws]');
$('#grid').addEventListener('pointerdown', e => {
  const w = e.target.closest('[data-win]');
  if (!w || e.button !== 0) return;
  drag = { id: w.dataset.win, el: w, x: e.clientX, y: e.clientY, ghost: null };
  w.setPointerCapture(e.pointerId);
});
document.addEventListener('pointermove', e => {
  if (!drag) return;
  if (!drag.ghost) {
    if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 6) return;
    drag.ghost = drag.el.cloneNode(true);
    Object.assign(drag.ghost.style, { position: 'fixed', left: '0', top: '0', width: `${drag.el.offsetWidth}px`, pointerEvents: 'none', zIndex: 1000, opacity: 0.9 });
    document.body.append(drag.ghost);
    drag.el.classList.add('dragging');
  }
  drag.ghost.style.transform = `translate(${e.clientX - 30}px, ${e.clientY - 20}px)`;
  const ws = wsAt(e);
  $$('.drop').forEach(x => x !== ws && x.classList.remove('drop'));
  ws?.classList.add('drop');
});
document.addEventListener('pointerup', e => {
  if (!drag) return;
  const d = drag, ws = d.ghost && wsAt(e);
  drag = null;
  d.ghost?.remove();
  $$('.dragging, .drop').forEach(x => x.classList.remove('dragging', 'drop'));
  if (!d.ghost) return;               // a plain click: focus the window as usual
  dragged = true;                     // swallow the click that follows the drag
  setTimeout(() => (dragged = false), 0);
  if (ws) wm?.runCommand(`move --workspace ${ws.dataset.ws}`, d.id);
});
document.addEventListener('click', e => { if (dragged) { e.stopPropagation(); e.preventDefault(); } }, true);
