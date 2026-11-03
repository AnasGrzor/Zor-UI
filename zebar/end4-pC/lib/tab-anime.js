// Anime / online wallpapers tab (end4's sidebarLeft/Anime + online wallpapers):
// browse booru sites and Wallhaven, open posts, or set an image as wallpaper.
import { $, esc, icon, bridge, config, openUrl } from './core.js';

const SOURCES = [
  ['yandere', 'yande.re'], ['konachan', 'Konachan'], ['danbooru', 'Danbooru'], ['wallhaven', 'Wallhaven'],
];

export async function mountAnime(root) {
  const cfg = await config();
  let source = cfg.booru.source === 'waifuim' ? 'wallhaven' : cfg.booru.source || 'yandere', tags = '', page = 1, items = [], loading = false, error = '';

  root.innerHTML = `
    <div class="chips" id="an-src">${SOURCES.map(([id, n]) => `<button class="chip${id === source ? ' sel' : ''}" data-src="${id}">${n}</button>`).join('')}</div>
    <form id="an-form" class="an-search"><span class="ms">search</span><input id="an-tags" placeholder="Tags, e.g. landscape scenery" autocomplete="off" />
      ${cfg.booru.allowNsfw ? '<span class="chip nsfw" title="NSFW allowed in Settings">18+</span>' : ''}</form>
    <div class="an-grid scroll" id="an-grid"></div>`;
  const grid = $('#an-grid', root);

  function render() {
    const cards = items.map((it, i) => `
      <div class="an-item" style="aspect-ratio:${it.w && it.h ? `${it.w}/${it.h}` : '1'}">
        <img loading="lazy" src="${esc(it.preview)}" alt="" referrerpolicy="no-referrer" />
        <div class="an-actions">
          <button title="Open post" data-open="${i}">${icon('open_in_new')}</button>
          <button title="Download to Pictures\\Wallpapers" data-dl="${i}">${icon('download')}</button>
          <button title="Set as wallpaper and theme" data-wall="${i}">${icon('wallpaper')}</button>
        </div>
      </div>`).join('');
    grid.innerHTML = `<div class="an-cols">${cards}</div>
      ${error ? `<div class="err">${esc(error)}</div>` : ''}
      ${loading ? '<div class="spinner"></div>' : items.length ? '<button class="chip an-more" id="an-more">Load more</button>' : !error ? `<div class="empty muted">${icon('image_search')}Nothing found</div>` : ''}`;
  }

  async function load(reset) {
    if (loading) return;
    if (reset) { page = 1; items = []; grid.scrollTop = 0; }
    loading = true; error = ''; render();
    const r = await bridge('booru', source, page, ...tags.split(/\s+/).filter(Boolean));
    loading = false;
    if (r.error) error = r.error;
    else { items = items.concat(r.items || []); page += 1; }
    render();
  }

  async function act(btn, i, apply) {
    const it = items[i];
    btn.innerHTML = '<span class="spinner small"></span>';
    const r = await bridge('download-wallpaper', it.file, ...(apply ? ['--apply'] : []));
    btn.innerHTML = icon(r.ok ? 'check' : 'error');
    btn.title = r.ok ? r.path : r.error;
    setTimeout(() => (btn.innerHTML = icon(apply ? 'wallpaper' : 'download')), 2500);
  }

  $('#an-src', root).addEventListener('click', e => {
    const b = e.target.closest('[data-src]');
    if (!b) return;
    source = b.dataset.src;
    root.querySelectorAll('#an-src .chip').forEach(c => c.classList.toggle('sel', c === b));
    bridge('config-set', 'booru.source', JSON.stringify(source));
    load(true);
  });
  $('#an-form', root).addEventListener('submit', e => { e.preventDefault(); tags = $('#an-tags', root).value.trim(); load(true); });
  grid.addEventListener('click', e => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.id === 'an-more') return load(false);
    if (b.dataset.open) return openUrl(items[b.dataset.open].page);
    if (b.dataset.dl) return act(b, +b.dataset.dl, false);
    if (b.dataset.wall) return act(b, +b.dataset.wall, true);
  });
  grid.addEventListener('scroll', () => {
    if (!loading && items.length && grid.scrollTop + grid.clientHeight > grid.scrollHeight - 300) load(false);
  });

  let started = false;
  return { focus: () => { if (!started) { started = true; load(true); } $('#an-tags', root).focus(); } };
}
