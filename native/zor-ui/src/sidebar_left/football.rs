//! Football tab (tab-football.js): league chips, fixtures / live scores with win-draw-loss bars, league tables.
//! Data comes from the bridge `football` command (end4's football.py, ESPN).

use super::{call, dots, err_box, img::Imgs, Inbox, Tab};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    hub, util,
    ui::{self, IconKind, Scroll},
    win::Input,
};
use serde_json::Value;

const VIEW0: u32 = 501; // 501 matches, 502 table
const REFRESH: u32 = 503;
const LGROW: u32 = 509;
const LG0: u32 = 510; // + index: 0 = All
const BODY: u32 = 590;
const MATCH0: u32 = 2000;

struct Team { name: String, logo: String, score: String }
struct Probs { home: f32, draw: f32, away: f32, o15: f32, o25: f32 }
struct M {
    id: String, league_id: String, league_name: String, date: i64, state: String, detail: String, home: Team, away: Team,
    probs: Option<Probs>, exp: Option<(f64, f64, f64)>, low: bool,
}
struct Row { rank: i64, name: String, logo: String, nums: [i64; 5], pts: i64 }

enum Msg { Data(Value), Table(String, Value), Live(Value) }

pub struct Football {
    leagues: Vec<(String, String)>,
    matches: Vec<M>,
    league: String,
    table_view: bool,
    table: Option<(String, Vec<(String, Vec<Row>)>)>,
    table_loading: bool,
    open: Option<String>,
    loading: bool,
    loaded: bool,
    error: String,
    next_live: f64,
    started: bool,
    scroll: Scroll,
    dims: (f32, f32),
    hscroll: f32,
    hdims: (f32, f32),
    imgs: Imgs,
    inbox: Inbox<Msg>,
}

// ------------------------------------------------------------------ dates

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// "2026-11-04T15:30Z" (seconds optional) -> unix seconds.
fn parse_iso(s: &str) -> Option<i64> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|p| p.parse::<f64>().ok());
    let (h, mi) = (t.next()??, t.next()??);
    Some(days_from_civil(y, m, day) * 86_400 + (h * 3600.0 + mi * 60.0) as i64)
}

/// (local day number, hour, minute) of a unix time.
fn local(secs: i64) -> (i64, u16, u16) {
    let l = secs + util::local_offset() as i64 * 60;
    (l.div_euclid(86_400), (l.rem_euclid(86_400) / 3600) as u16, (l.rem_euclid(3600) / 60) as u16)
}

fn day_label(day: i64, today: i64) -> String {
    match day - today {
        0 => "Today".into(), 1 => "Tomorrow".into(), -1 => "Yesterday".into(),
        _ => {
            let (y, m, d) = civil_from_days(day);
            let st = windows::Win32::Foundation::SYSTEMTIME { wYear: y as u16, wMonth: m as u16, wDay: d as u16, wDayOfWeek: ((day + 4).rem_euclid(7)) as u16, ..Default::default() };
            util::date_fmt(&st, "dddd, d MMM")
        }
    }
}

fn today() -> i64 { let n = util::now(); days_from_civil(n.wYear as i64, n.wMonth as i64, n.wDay as i64) }

// ------------------------------------------------------------------ data

fn team(v: &Value) -> Team {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    Team { name: s("name"), logo: s("logo"), score: v["score"].as_str().map(String::from).or_else(|| v["score"].as_i64().map(|n| n.to_string())).unwrap_or_else(|| "0".into()) }
}

fn parse_match(m: &Value) -> Option<M> {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    let f = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let p = &m["probs"];
    Some(M {
        id: s("id"), league_id: m["league"]["id"].as_str()?.into(), league_name: m["league"]["name"].as_str().unwrap_or("").into(),
        date: parse_iso(m["date"].as_str()?)?, state: s("state"), detail: s("detail"), home: team(&m["home"]), away: team(&m["away"]),
        probs: p.is_object().then(|| Probs { home: f(p, "home") as f32, draw: f(p, "draw") as f32, away: f(p, "away") as f32, o15: f(p, "o15") as f32, o25: f(p, "o25") as f32 }),
        exp: m["expected"].is_object().then(|| (f(&m["expected"], "home"), f(&m["expected"], "away"), f(&m["expected"], "corners"))),
        low: m["lowData"].as_bool().unwrap_or(false),
    })
}

fn parse_table(v: &Value) -> Vec<(String, Vec<Row>)> {
    v["groups"].as_array().into_iter().flatten().map(|g| {
        let rows = g["rows"].as_array().into_iter().flatten().map(|r| {
            let n = |k: &str| r[k].as_i64().unwrap_or(0);
            Row { rank: n("rank"), name: r["name"].as_str().unwrap_or("").into(), logo: r["logo"].as_str().unwrap_or("").into(), nums: [n("p"), n("w"), n("d"), n("l"), n("gd")], pts: n("pts") }
        }).collect();
        (g["name"].as_str().unwrap_or("").to_string(), rows)
    }).collect()
}

fn pct(v: f32) -> String { format!("{}%", (v * 100.0).round()) }

/// What the list shows, with heights: day headings and match cards, or table groups.
enum Item { Day(String), Match(usize, f32), Group(String), Head, Line(usize, usize) }

impl Football {
    pub fn new() -> Football {
        Football { leagues: vec![], matches: vec![], league: "all".into(), table_view: false, table: None, table_loading: false, open: None, loading: false, loaded: false,
            error: String::new(), next_live: 0.0, started: false, scroll: Scroll::default(), dims: (0.0, 0.0), hscroll: 0.0, hdims: (0.0, 0.0), imgs: Imgs::default(), inbox: Inbox::new() }
    }

    fn load(&mut self, cx: &Ctx) {
        self.loading = true;
        let inbox = self.inbox.clone();
        call(cx, "football", vec![], move |v| inbox.push(Msg::Data(v)));
    }

    fn load_table(&mut self, cx: &Ctx) {
        if self.league == "all" { return; }
        self.table = None;
        self.table_loading = true;
        let (league, inbox) = (self.league.clone(), self.inbox.clone());
        call(cx, "football", vec!["--standings".into(), league.clone()], move |v| inbox.push(Msg::Table(league, v)));
    }

    fn schedule_live(&mut self) {
        self.next_live = if self.matches.iter().any(|m| m.state == "in") { hub::now_ms() + 60_000.0 } else { 0.0 };
    }

    fn drain(&mut self, cx: &Ctx) {
        for m in self.inbox.take() {
            match m {
                Msg::Data(v) => {
                    self.loading = false;
                    if v.is_null() { self.error = "No answer from the bridge.".into(); continue; }
                    if let Some(e) = v["error"].as_str() { self.error = e.into(); continue; }
                    self.error.clear();
                    self.loaded = true;
                    let allowed = cx.cfg.strs("football.leagues");
                    self.leagues = v["leagues"].as_array().into_iter().flatten().filter_map(|l| Some((l["id"].as_str()?.to_string(), l["name"].as_str()?.to_string())))
                        .filter(|l| allowed.is_empty() || allowed.contains(&l.0)).collect();
                    self.matches = v["matches"].as_array().into_iter().flatten().filter_map(parse_match).collect();
                    self.schedule_live();
                }
                Msg::Table(league, v) if league == self.league => {
                    self.table_loading = false;
                    self.table = Some((league, parse_table(&v)));
                }
                Msg::Live(v) => {
                    for u in v["updates"].as_array().into_iter().flatten() {
                        let Some(m) = self.matches.iter_mut().find(|m| Some(m.id.as_str()) == u["id"].as_str()) else { continue };
                        if let Some(s) = u["state"].as_str() { m.state = s.into(); }
                        if let Some(s) = u["detail"].as_str() { m.detail = s.into(); }
                        for (t, k) in [(&mut m.home, "home"), (&mut m.away, "away")] {
                            if let Some(s) = u[k]["score"].as_str().map(String::from).or_else(|| u[k]["score"].as_i64().map(|n| n.to_string())) { t.score = s; }
                        }
                    }
                    self.schedule_live();
                }
                _ => {}
            }
        }
    }

    /// Matches of the chosen league (or all configured ones).
    fn visible(&self) -> Vec<usize> {
        self.matches.iter().enumerate()
            .filter(|(_, m)| if self.league == "all" { self.leagues.iter().any(|l| l.0 == m.league_id) } else { m.league_id == self.league })
            .map(|(i, _)| i).collect()
    }

    fn items(&self, c: &Canvas) -> Vec<Item> {
        let mut out = vec![];
        if self.table_view {
            let Some((_, groups)) = &self.table else { return out };
            for (gi, (name, rows)) in groups.iter().enumerate() {
                if groups.len() > 1 { out.push(Item::Group(name.clone())); }
                out.push(Item::Head);
                out.extend((0..rows.len()).map(|ri| Item::Line(gi, ri)));
            }
            return out;
        }
        let today = today();
        let mut last = i64::MIN;
        for i in self.visible() {
            let m = &self.matches[i];
            let day = local(m.date).0;
            if day != last { out.push(Item::Day(day_label(day, today))); last = day; }
            let open = self.open.as_deref() == Some(m.id.as_str());
            let mut h = 10.0 + 33.0 + 10.0;
            if m.probs.is_some() { h += 31.0; }
            if open && m.probs.is_some() { h += 10.0 + 50.0 + if m.low { 8.0 + c.g.measure_wrapped("Early season: few games played, estimates are rough.", txt(11.5, 400), 400.0).max(14.0) + 8.0 } else { 0.0 }; }
            out.push(Item::Match(i, h));
        }
        out
    }

    fn team_row(&mut self, c: &mut Canvas, cx: &Ctx, t: &Team, r: R, away: bool) {
        let (logo_x, text) = if away { (r.x + r.w - 24.0, R::new(r.x, r.y, r.w - 32.0, r.h)) } else { (r.x, R::new(r.x + 32.0, r.y, r.w - 32.0, r.h)) };
        let lr = R::new(logo_x, r.y + (r.h - 24.0) / 2.0, 24.0, 24.0);
        let s = c.scale;
        match (!t.logo.is_empty()).then(|| self.imgs.get(c.g, &t.logo, (24.0 * s) as u32, (24.0 * s) as u32)).flatten() {
            Some(p) => c.image(p, lr, 1.0),
            None => c.icon("shield", lr, ico(20.0, false), cx.theme.c("onSurfaceVariant")),
        }
        c.text(&t.name, text, txt(13.0, 400), cx.theme.c("onSurface"), if away { Align::Right } else { Align::Left });
    }
}

impl Tab for Football {
    // BODY: the match list's scroll area; LGROW: the league chip row's background
    fn hand(&self, id: u32) -> bool { !matches!(id, 0 | BODY | LGROW) }

    fn show(&mut self, cx: &Ctx) {
        if self.started { return; }
        self.started = true;
        self.load(cx);
    }

    fn frame_ms(&self) -> u32 { if self.loading || self.table_loading || self.imgs.busy() { 100 } else { 0 } }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        self.drain(cx);
        self.imgs.begin();
        if self.next_live > 0.0 && hub::now_ms() >= self.next_live {
            self.next_live = 0.0;
            let inbox = self.inbox.clone();
            call(cx, "football", vec!["--live".into()], move |v| inbox.push(Msg::Live(v)));
        }
        let t = &cx.theme;

        // league chips (a row that scrolls sideways)
        let row = R::new(r.x, r.y, r.w, 32.0);
        c.hit(row, LGROW);
        let chips: Vec<(String, String)> = std::iter::once(("all".to_string(), "All".to_string())).chain(self.leagues.iter().cloned()).collect();
        let total: f32 = chips.iter().map(|(_, n)| c.g.measure(n, txt(13.0, 400)).0.ceil() + 28.0 + 6.0).sum::<f32>() - 6.0;
        self.hdims = (total, r.w);
        self.hscroll = self.hscroll.clamp(0.0, (total - r.w).max(0.0));
        let hs = self.hscroll;
        let (league, ids): (String, Vec<u32>) = (self.league.clone(), (0..chips.len() as u32).collect());
        c.clip_round(row, 0.0, |c| {
            let mut x = r.x - hs;
            for ((id, name), n) in chips.iter().zip(ids) {
                let cr = ui::chip(c, cx, x, r.y, LG0 + n, None, name, *id == league);
                x += cr.w + 6.0;
            }
        });
        // view chips + refresh
        let vy = r.y + 38.0;
        let m = ui::chip(c, cx, r.x, vy, VIEW0, Some("sports_soccer"), "Matches", !self.table_view);
        ui::chip(c, cx, r.x + m.w + 6.0, vy, VIEW0 + 1, Some("format_list_numbered"), "Table", self.table_view);
        ui::icon_btn(c, cx, R::new(r.x + r.w - 36.0, vy - 2.0, 36.0, 36.0), REFRESH, "refresh", IconKind::Plain, 24.0, false);

        // list
        let view = R::new(r.x, r.y + 80.0, r.w, (r.h - 80.0).max(40.0));
        c.hit(view, BODY);
        let msg_note = |c: &mut Canvas, icon: &str, text: &str| {
            c.icon(icon, R::new(view.x, view.y + view.h / 2.0 - 50.0, view.w, 48.0), ico(44.0, false), t.c("onSurfaceVariant"));
            c.text(text, R::new(view.x, view.y + view.h / 2.0 + 4.0, view.w, 22.0), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Center);
        };
        if !self.error.is_empty() { err_box(c, cx, R::new(view.x, view.y, view.w, 0.0), &self.error); return; }
        if self.loading && !self.loaded { dots(c, cx, view.x + view.w / 2.0, view.y + 40.0); return; }
        if self.table_view {
            if self.league == "all" { return msg_note(c, "format_list_numbered", "Pick a league above to see its table"); }
            if self.table_loading || self.table.is_none() { dots(c, cx, view.x + view.w / 2.0, view.y + 40.0); return; }
            if self.table.as_ref().is_some_and(|t| t.1.is_empty()) { return msg_note(c, "format_list_numbered", "No table available"); }
        } else if self.visible().is_empty() { return msg_note(c, "sports_soccer", "No matches in the next week"); }

        let items = self.items(c);
        let heights: Vec<f32> = items.iter().map(|i| match i { Item::Day(_) => 25.0, Item::Match(_, h) => h + 6.0, Item::Group(_) => 25.0, Item::Head => 24.0, Item::Line(..) => 29.0 }).collect();
        let content: f32 = heights.iter().sum();
        self.dims = (content, view.h);
        let (hover, now_24) = (c.hover, cx.cfg.get("use24h").as_bool() != Some(false));
        let w = view.w - 8.0;
        let mut y_at = 0.0;
        let mut calls: Vec<(Item, f32)> = vec![];
        for (it, h) in items.into_iter().zip(&heights) { calls.push((it, y_at)); y_at += h; }
        let mut scroll = std::mem::take(&mut self.scroll);
        let me = &mut *self;
        scroll.region(c, cx, view, content, |c, top| {
            for (it, y) in &calls {
                let y = top + y;
                if y + 80.0 < view.y - 100.0 || y > view.y + view.h { continue; }
                match it {
                    Item::Day(d) | Item::Group(d) => c.text(d, R::new(view.x + 4.0, y + 8.0, w, 16.0), txt(12.0, 500), t.c("primary"), Align::Left),
                    Item::Match(i, h) => me.match_card(c, cx, R::new(view.x, y, w, *h), *i, hover, now_24),
                    Item::Head => {
                        for (k, (label, cw, left)) in Self::cols(w).iter().enumerate() { let _ = k; c.text(label, R::new(view.x + left, y, *cw, 24.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Center); }
                    }
                    Item::Line(g, ri) => me.table_line(c, cx, view.x, y, w, *g, *ri),
                }
            }
        });
        self.scroll = scroll;
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        match ev {
            Input::Click(id, _) if (LG0..LG0 + 40).contains(id) => {
                let i = (*id - LG0) as usize;
                self.league = if i == 0 { "all".into() } else { self.leagues.get(i - 1).map_or("all".into(), |l| l.0.clone()) };
                self.scroll.y = 0.0;
                if self.table_view { self.load_table(cx); }
            }
            Input::Click(id, _) if *id == VIEW0 || *id == VIEW0 + 1 => {
                self.table_view = *id == VIEW0 + 1;
                self.scroll.y = 0.0;
                if self.table_view { self.load_table(cx); }
            }
            Input::Click(REFRESH, _) => if self.table_view { self.load_table(cx) } else { self.load(cx) },
            Input::Click(id, _) if *id >= MATCH0 => {
                if let Some(m) = self.matches.get((*id - MATCH0) as usize) { self.open = if self.open.as_deref() == Some(m.id.as_str()) { None } else { Some(m.id.clone()) }; }
            }
            Input::Wheel(id, d) if *id == LGROW || (LG0..LG0 + 40).contains(id) => { self.hscroll -= *d as f32 / 120.0 * 60.0; self.hscroll = self.hscroll.clamp(0.0, (self.hdims.0 - self.hdims.1).max(0.0)); }
            Input::Wheel(_, d) => self.scroll.wheel(*d, self.dims.0, self.dims.1),
            _ => {}
        }
    }
}

impl Football {
    /// Table columns: (label, width, left offset) for a list `w` wide.
    fn cols(w: f32) -> [(&'static str, f32, f32); 8] {
        let team = (w - 24.0 - 4.0 * 30.0 - 40.0 - 40.0).max(60.0);
        let mut x = 0.0;
        let mut out = [("", 0.0, 0.0); 8];
        for (i, (l, cw)) in [("#", 24.0), ("", team), ("P", 30.0), ("W", 30.0), ("D", 30.0), ("L", 30.0), ("GD", 40.0), ("Pts", 40.0)].into_iter().enumerate() { out[i] = (l, cw, x); x += cw; }
        out
    }

    fn match_card(&mut self, c: &mut Canvas, cx: &Ctx, r: R, i: usize, hover: u32, h24: bool) {
        let t = &cx.theme;
        let (open, id) = (self.open.as_deref() == Some(self.matches[i].id.as_str()), MATCH0 + i as u32);
        let over = c.hit(r, id) || hover == id;
        c.rect(r, 17.0, t.c("surfaceContainerLow"));
        if over { c.rect(r, 17.0, alpha(t.c("onSurface"), 0.05)); }
        let m = &self.matches[i];
        let (live, done) = (m.state == "in", m.state == "post");
        let inner = R::new(r.x + 12.0, r.y + 10.0, r.w - 24.0, 33.0);
        let mid = 72.0;
        let side = (inner.w - mid - 16.0) / 2.0;
        let (home, away) = (Team { name: m.home.name.clone(), logo: m.home.logo.clone(), score: String::new() }, Team { name: m.away.name.clone(), logo: m.away.logo.clone(), score: String::new() });
        let mc = R::new(inner.x + side + 8.0, inner.y, mid, inner.h);
        if live || done {
            c.text(&format!("{} \u{2013} {}", m.home.score, m.away.score), R::new(mc.x, mc.y, mc.w, 20.0), txt(16.0, 600), if live { t.c("primary") } else { t.c("onSurface") }, Align::Center);
            c.text(&m.detail, R::new(mc.x - 8.0, mc.y + 20.0, mc.w + 16.0, 13.0), txt(10.5, 400), t.c("onSurfaceVariant"), Align::Center);
        } else {
            let (_, hh, mm) = local(m.date);
            c.text(&util::clock(hh, mm, h24), R::new(mc.x, mc.y, mc.w, 20.0), txt(14.0, 500), t.c("onSurface"), Align::Center);
            c.text(&m.league_name, R::new(mc.x - 8.0, mc.y + 20.0, mc.w + 16.0, 13.0), txt(10.5, 400), t.c("onSurfaceVariant"), Align::Center);
        }
        let (probs, exp, low) = (m.probs.as_ref().map(|p| (p.home, p.draw, p.away, p.o15, p.o25)), m.exp, m.low);
        self.team_row(c, cx, &home, R::new(inner.x, inner.y, side, inner.h), false);
        self.team_row(c, cx, &away, R::new(inner.x + side + 16.0 + mid, inner.y, side, inner.h), true);
        let Some((ph, pd, pa, o15, o25)) = probs else { return };
        let by = inner.y + inner.h + 8.0;
        let tot = (ph + pd + pa).max(0.0001);
        let bw = inner.w - 4.0;
        let mut x = inner.x;
        c.clip_round(R::new(inner.x, by, inner.w, 6.0), 3.0, |c| {
            for (v, col) in [(ph, "primary"), (pd, "outlineVariant"), (pa, "tertiary")] {
                let w = bw * v / tot;
                c.rect(R::new(x, by, w, 6.0), 0.0, t.c(col));
                x += w + 2.0;
            }
        });
        let ly = by + 9.0;
        c.text(&pct(ph), R::new(inner.x, ly, 80.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
        c.text(&format!("Draw {}", pct(pd)), R::new(inner.x + inner.w / 2.0 - 60.0, ly, 120.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Center);
        c.text(&pct(pa), R::new(inner.x + inner.w - 80.0, ly, 80.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Right);
        if open {
            let (e, y) = (exp.unwrap_or_default(), ly + 14.0 + 10.0);
            let cw = (inner.w - 12.0) / 3.0;
            let cells = [(format!("{} \u{2013} {}", e.0, e.1), "Expected goals"), (format!("{} / {}", pct(o15), pct(o25)), "Over 1.5 / 2.5"), (format!("{}", e.2), "Expected corners")];
            for (k, (big, small)) in cells.iter().enumerate() {
                let cr = R::new(inner.x + k as f32 * (cw + 6.0), y, cw, 50.0);
                c.rect(cr, 12.0, t.c("surfaceContainer"));
                c.text(big, R::new(cr.x, cr.y + 8.0, cr.w, 18.0), txt(14.0, 500), t.c("onSurface"), Align::Center);
                c.text(small, R::new(cr.x, cr.y + 27.0, cr.w, 15.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Center);
            }
            if low { c.para("Early season: few games played, estimates are rough.", R::new(inner.x, y + 58.0, inner.w, 30.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left); }
        }
    }

    #[allow(clippy::too_many_arguments)] // the args mirror the callers; a signature refactor is out of scope
    fn table_line(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, w: f32, g: usize, ri: usize) {
        let t = &cx.theme;
        let Some(row) = self.table.as_ref().and_then(|t| t.1.get(g)).and_then(|g| g.1.get(ri)) else { return };
        let (name, logo, rank, nums, pts) = (row.name.clone(), row.logo.clone(), row.rank, row.nums, row.pts);
        c.rect(R::new(x, y, w, 1.0), 0.0, alpha(t.c("outlineVariant"), 0.4));
        let cols = Self::cols(w);
        let f = txt(12.5, 400);
        let fg = t.c("onSurface");
        c.text(&rank.to_string(), R::new(x + cols[0].2, y, cols[0].1, 29.0), f, fg, Align::Center);
        let s = c.scale;
        let lr = R::new(x + cols[1].2, y + 5.5, 18.0, 18.0);
        if !logo.is_empty() { if let Some(p) = self.imgs.get(c.g, &logo, (18.0 * s) as u32, (18.0 * s) as u32) { c.image(p, lr, 1.0); } }
        c.text(&name, R::new(x + cols[1].2 + 24.0, y, cols[1].1 - 24.0, 29.0), f, fg, Align::Left);
        for k in 0..5 {
            let txt_ = if k == 4 && nums[k] > 0 { format!("+{}", nums[k]) } else { nums[k].to_string() };
            c.text(&txt_, R::new(x + cols[2 + k].2, y, cols[2 + k].1, 29.0), f, fg, Align::Center);
        }
        c.text(&pts.to_string(), R::new(x + cols[7].2, y, cols[7].1, 29.0), txt(12.5, 700), fg, Align::Center);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(parse_iso("1970-01-02T00:00Z"), Some(86_400));
        assert_eq!(parse_iso("2026-11-04T15:30Z"), Some(days_from_civil(2026, 11, 4) * 86_400 + 15 * 3600 + 1800));
        assert_eq!(civil_from_days(days_from_civil(2026, 11, 4)), (2026, 11, 4));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(parse_iso("nonsense"), None);
        assert_eq!((day_label(10, 10), day_label(11, 10), day_label(9, 10)), ("Today".into(), "Tomorrow".into(), "Yesterday".into()));
    }

    #[test]
    fn match_json() {
        let v: Value = serde_json::from_str(r#"{"id":"1","league":{"id":"esp.1","name":"LaLiga"},"date":"2026-11-04T15:30Z","state":"pre","detail":"Scheduled",
            "home":{"name":"A","logo":"","score":"0"},"away":{"name":"B","logo":"","score":"2"},"probs":{"home":0.5,"draw":0.25,"away":0.25,"o15":0.8,"o25":0.6},"expected":{"home":1.5,"away":1.0,"corners":10.2}}"#).unwrap();
        let m = parse_match(&v).unwrap();
        assert_eq!((m.away.score.as_str(), m.league_id.as_str()), ("2", "esp.1"));
        assert_eq!(pct(m.probs.unwrap().home), "50%");
    }
}
