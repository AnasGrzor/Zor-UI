// Football tab (zor-ui's Football page): fixtures, live scores, win/draw/loss
// probabilities and league tables, from end4's own scripts/football.py (ESPN data).
import { $, esc, icon, bridge, config } from './core.js';

export async function mountFootball(root) {
  const cfg = await config();
  let data = null, league = 'all', view = 'matches', table = null, open = null, loading = false, error = '';
  let liveTimer = 0;

  root.innerHTML = `
    <div class="fb-top">
      <div class="chips scroll-x" id="fb-leagues"></div>
      <div class="fb-views">
        <button class="chip sel" data-view="matches">${icon('sports_soccer')}Matches</button>
        <button class="chip" data-view="table">${icon('format_list_numbered')}Table</button>
        <button class="icon-btn" id="fb-refresh" title="Refresh">${icon('refresh')}</button>
      </div>
    </div>
    <div class="fb-body scroll" id="fb-body"></div>`;
  const body = $('#fb-body', root);

  const leagues = () => (data?.leagues || []).filter(l => !cfg.football.leagues?.length || cfg.football.leagues.includes(l.id));
  const time = d => new Date(d).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: !cfg.use24h });
  const day = d => {
    const x = new Date(d), t = new Date();
    const diff = Math.round((new Date(x.toDateString()) - new Date(t.toDateString())) / 864e5);
    return diff === 0 ? 'Today' : diff === 1 ? 'Tomorrow' : diff === -1 ? 'Yesterday' : x.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'short' });
  };
  const pct = v => `${Math.round((v || 0) * 100)}%`;

  function renderLeagues() {
    const ls = leagues();
    $('#fb-leagues', root).innerHTML = [['all', 'All']].concat(ls.map(l => [l.id, l.name]))
      .map(([id, n]) => `<button class="chip${id === league ? ' sel' : ''}" data-league="${id}">${esc(n)}</button>`).join('');
  }

  function team(t, side) {
    return `<div class="fb-team ${side}">${t.logo ? `<img src="${esc(t.logo)}" alt="" />` : icon('shield')}<span class="ellipsis">${esc(t.name)}</span></div>`;
  }

  function renderMatches() {
    let ms = (data?.matches || []).filter(m => league === 'all' ? leagues().some(l => l.id === m.league.id) : m.league.id === league);
    if (!ms.length) return `<div class="empty muted">${icon('sports_soccer')}No matches in the next week</div>`;
    let html = '', lastDay = '';
    for (const m of ms) {
      const d = day(m.date);
      if (d !== lastDay) { html += `<div class="fb-day">${esc(d)}</div>`; lastDay = d; }
      const live = m.state === 'in', done = m.state === 'post';
      const mid = live || done
        ? `<div class="fb-score${live ? ' live' : ''}">${esc(m.home.score ?? 0)} – ${esc(m.away.score ?? 0)}<small>${esc(m.detail)}</small></div>`
        : `<div class="fb-score pre">${time(m.date)}<small>${esc(m.league.name)}</small></div>`;
      const p = m.probs;
      html += `<div class="fb-match${open === m.id ? ' open' : ''}" data-match="${esc(m.id)}">
        <div class="fb-row">${team(m.home, 'home')}${mid}${team(m.away, 'away')}</div>
        ${p ? `<div class="fb-prob"><i style="flex:${p.home}"></i><i style="flex:${p.draw}"></i><i style="flex:${p.away}"></i></div>
               <div class="fb-prob-l"><span>${pct(p.home)}</span><span>Draw ${pct(p.draw)}</span><span>${pct(p.away)}</span></div>` : ''}
        ${open === m.id && p ? `<div class="fb-more">
            <div><b>${m.expected.home} – ${m.expected.away}</b><span>Expected goals</span></div>
            <div><b>${pct(p.o15)} / ${pct(p.o25)}</b><span>Over 1.5 / 2.5</span></div>
            <div><b>${m.expected.corners}</b><span>Expected corners</span></div>
            ${m.lowData ? `<div class="muted" style="grid-column:1/-1">Early season: few games played, estimates are rough.</div>` : ''}
          </div>` : ''}
      </div>`;
    }
    return html;
  }

  function renderTable() {
    if (league === 'all') return `<div class="empty muted">${icon('format_list_numbered')}Pick a league above to see its table</div>`;
    if (!table || table.league !== league) return '<div class="spinner"></div>';
    if (!table.groups?.length) return `<div class="empty muted">No table available</div>`;
    return table.groups.map(g => `
      ${table.groups.length > 1 ? `<div class="fb-day">${esc(g.name)}</div>` : ''}
      <table class="fb-table"><tr><th>#</th><th></th><th>P</th><th>W</th><th>D</th><th>L</th><th>GD</th><th>Pts</th></tr>
      ${g.rows.map(r => `<tr><td>${r.rank}</td><td class="t">${r.logo ? `<img src="${esc(r.logo)}" alt=""/>` : ''}<span class="ellipsis">${esc(r.name)}</span></td>
        <td>${r.p}</td><td>${r.w}</td><td>${r.d}</td><td>${r.l}</td><td>${r.gd > 0 ? '+' : ''}${r.gd}</td><td><b>${r.pts}</b></td></tr>`).join('')}</table>`).join('');
  }

  function render() {
    renderLeagues();
    body.innerHTML = error ? `<div class="err">${esc(error)}</div>` : loading && !data ? '<div class="spinner"></div>' : view === 'table' ? renderTable() : renderMatches();
  }

  async function load() {
    loading = true; render();
    const r = await bridge('football');
    loading = false;
    if (r.error) error = r.error; else { data = r; error = ''; }
    render();
    scheduleLive();
  }
  async function loadTable() {
    if (league === 'all') return render();
    table = null; render();
    table = await bridge('football', '--standings', league);
    render();
  }
  function scheduleLive() {
    clearTimeout(liveTimer);
    if (!data?.matches?.some(m => m.state === 'in')) return;
    liveTimer = setTimeout(async () => {
      const r = await bridge('football', '--live');
      for (const u of r.updates || []) {
        const m = data.matches.find(x => x.id === u.id);
        if (m) Object.assign(m, { state: u.state, detail: u.detail, home: { ...m.home, ...u.home }, away: { ...m.away, ...u.away } });
      }
      render(); scheduleLive();
    }, 60000);
  }

  root.addEventListener('click', e => {
    const l = e.target.closest('[data-league]');
    if (l) { league = l.dataset.league; view === 'table' ? loadTable() : render(); return; }
    const v = e.target.closest('[data-view]');
    if (v) {
      view = v.dataset.view;
      root.querySelectorAll('[data-view]').forEach(b => b.classList.toggle('sel', b === v));
      view === 'table' ? loadTable() : render();
      return;
    }
    if (e.target.closest('#fb-refresh')) return view === 'table' ? loadTable() : load();
    const m = e.target.closest('[data-match]');
    if (m) { open = open === m.dataset.match ? null : m.dataset.match; render(); }
  });

  let started = false;
  return { focus: () => { if (!started) { started = true; load(); } } };
}
