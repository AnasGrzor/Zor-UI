//! The bottom card of the right sidebar: tab row plus the notifications, calendar, to-do and timer pages.

use super::{data::*, SidebarRight, ID_TAB};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, Font, Gfx, R},
    hub, menu::Item, proc, ui,
};
use serde_json::{json, Value};

const ID_CLEAR: u32 = 211;
const ID_CENTER: u32 = 210;
const ID_CAL: u32 = 220; // prev, next
const ID_ADD: u32 = 231;
const ID_TODO_CHK: u32 = 400;
const ID_TODO_DEL: u32 = 500;
const ID_DISMISS: u32 = 600;
const ID_GROUP: u32 = 700; // a notification card's body, for its right-click menu
const ID_MODE: u32 = 240; // pomodoro, stopwatch
const ID_T: u32 = 242; // reset, toggle, skip / lap

/// Wrapped text height snapped to whole CSS lines (`line` px each, at most `max` of them).
fn lines_h(g: &Gfx, s: &str, f: Font, w: f32, line: f32, max: usize) -> f32 {
    if s.is_empty() { return 0.0; }
    let n = (g.measure_wrapped(s, f, w) / (f.size * ui::LH)).round().max(1.0) as usize;
    n.min(max) as f32 * line
}

/// A filled round-rect button with an icon (the timer's 56 px play button).
fn big_btn(c: &mut Canvas, cx: &Ctx, r: R, id: u32, icon: &str) {
    c.rect(r, 23.0, cx.theme.c("primary"));
    if c.hit(r, id) { c.rect(r, 23.0, alpha(cx.theme.c("onPrimary"), 0.08)); }
    c.icon(icon, r, ico(28.0, true), cx.theme.c("onPrimary"));
}

fn empty(c: &mut Canvas, cx: &Ctx, r: R, icon: &str, text: &str) {
    let y = r.y + (r.h - 67.0) / 2.0;
    c.icon(icon, R::new(r.x, y, r.w, 44.0), ico(44.0, false), cx.theme.c("onSurfaceVariant"));
    c.text(text, R::new(r.x, y + 50.0, r.w, 17.0), txt(14.0, 400), cx.theme.c("onSurfaceVariant"), Align::Center);
}

fn chip_w(g: &Gfx, label: &str) -> f32 { 14.0 + 24.0 + g.measure(label, txt(13.0, 400)).0.ceil() + 14.0 }

struct Row { h: f32, th: f32, bh: f32, time: String, tw: f32 }

impl SidebarRight {
    pub(super) fn timer_ticking(&self) -> bool {
        if self.tab != 3 { return false; }
        let t = hub::timer();
        (if self.sw_mode { &t["sw"]["running"] } else { &t["running"] }).as_bool() == Some(true)
    }

    pub(super) fn wheel_tab(&mut self, d: i32) {
        match self.tab {
            0 => self.sc_notif.wheel(d),
            1 => self.sc_page.wheel(d),
            2 => self.sc_todo.wheel(d),
            _ => if self.sw_mode { self.sc_laps.wheel(d) } else { self.sc_page.wheel(d) },
        }
    }

    pub(super) fn add_todo(&mut self) {
        let text = self.todo.text.trim().to_string();
        if text.is_empty() { return; }
        let mut t = todos();
        t.push(json!({ "text": text, "done": false }));
        save_todos(&t);
        self.todo.set("");
    }

    /// Right-click menu of notification group `id - ID_GROUP` (or its close button).
    pub(super) fn notif_menu(&self, id: u32) -> Vec<Item> {
        let gi = match id { i if (ID_GROUP..ID_GROUP + 100).contains(&i) => i - ID_GROUP, i if (ID_DISMISS..ID_DISMISS + 100).contains(&i) => i - ID_DISMISS, _ => return vec![] };
        let s = lock(&self.sh);
        let Some(g) = group(&s.notifs).into_iter().nth(gi as usize) else { return vec![] };
        let text = g.items.iter().map(|n| if n.body.is_empty() { n.title.clone() } else { format!("{}
{}", n.title, n.body) }).collect::<Vec<_>>().join("

");
        let ids: Vec<String> = s.notifs.iter().filter(|n| n.app == g.app).map(|n| n.id.clone()).collect();
        let app = g.app.clone();
        let (sh, cfg) = (self.sh.clone(), self.cfg.clone());
        vec![
            Item::new("content_copy", "Copy text", move |_| crate::util::clipboard_set(&text)),
            Item::new("close", "Dismiss", move |_| {
                lock(&sh).notifs.retain(|n| n.app != app);
                if let Some(cfg) = cfg { std::thread::spawn(move || { let a: Vec<&str> = ids.iter().map(String::as_str).collect(); proc::bridge(&cfg, "notifications-dismiss", &a); }); }
            }),
            Item::new("clear_all", "Clear all", { let (sh, cfg) = (self.sh.clone(), self.cfg.clone()); move |_| {
                lock(&sh).notifs.clear();
                if let Some(cfg) = cfg { std::thread::spawn(move || { proc::bridge(&cfg, "notifications-dismiss", &["all"]); }); }
            } }),
        ]
    }

    pub(super) fn page_click(&mut self, id: u32, cx: &mut Ctx) {
        let cfg = &cx.cfg;
        let now = hub::now_ms();
        match id {
            ID_CENTER => proc::run_bg(cfg, "toggles", &["-Action", "actioncenter"]),
            ID_CLEAR => {
                lock(&self.sh).notifs.clear();
                if let Some(cfg) = self.cfg.clone() { std::thread::spawn(move || { proc::bridge(&cfg, "notifications-dismiss", &["all"]); }); }
            }
            i if (ID_DISMISS..ID_DISMISS + 100).contains(&i) => {
                let ids: Vec<String> = {
                    let mut s = lock(&self.sh);
                    let Some(app) = group(&s.notifs).get((i - ID_DISMISS) as usize).map(|g| g.app.clone()) else { return };
                    let ids = s.notifs.iter().filter(|n| n.app == app).map(|n| n.id.clone()).collect();
                    s.notifs.retain(|n| n.app != app);
                    ids
                };
                if let Some(cfg) = self.cfg.clone() {
                    std::thread::spawn(move || { let a: Vec<&str> = ids.iter().map(String::as_str).collect(); proc::bridge(&cfg, "notifications-dismiss", &a); });
                }
            }
            i if i == ID_CAL || i == ID_CAL + 1 => self.cal = shift_month(self.cal, if i == ID_CAL { -1 } else { 1 }),
            ID_ADD => self.add_todo(),
            i if (ID_TODO_CHK..ID_TODO_CHK + 100).contains(&i) => {
                let mut t = todos();
                if let Some(x) = t.get_mut((i - ID_TODO_CHK) as usize) { x["done"] = json!(x["done"].as_bool() != Some(true)); save_todos(&t); }
            }
            i if (ID_TODO_DEL..ID_TODO_DEL + 100).contains(&i) => {
                let mut t = todos();
                if ((i - ID_TODO_DEL) as usize) < t.len() { t.remove((i - ID_TODO_DEL) as usize); save_todos(&t); }
            }
            i if i == ID_MODE || i == ID_MODE + 1 => {
                self.sw_mode = i == ID_MODE + 1;
                cx.state.update(|v| v["timerMode"] = json!(if self.sw_mode { "sw" } else { "pomo" }));
            }
            i if (ID_T..ID_T + 3).contains(&i) => {
                let k = i - ID_T;
                let cfg = &cx.cfg;
                match (self.sw_mode, k) {
                    (false, 0) => edit_timer(pomo_reset),
                    (false, 1) => edit_timer(|t| pomo_toggle(t, cfg, now)),
                    (false, _) => edit_timer(|t| pomo_skip(t, cfg)),
                    (true, 0) => edit_timer(sw_reset),
                    (true, 1) => edit_timer(|t| sw_toggle(t, now)),
                    (true, _) => edit_timer(|t| sw_lap(t, now)),
                }
            }
            _ => {}
        }
    }

    /// The card (`.card` layer1, radius 23): tab row and the selected page.
    pub(super) fn paint_bottom(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, r: R) {
        c.rect(r, 23.0, cx.theme.c("surfaceContainerLow"));
        c.clip_round(r, 23.0, |c| {
            self.paint_tabs(c, cx, s, r);
            let pr = R::new(r.x + 12.0, r.y + 44.0 + 12.0, r.w - 24.0, (r.h - 44.0 - 24.0).max(0.0));
            match self.tab {
                0 => self.paint_notifs(c, cx, s, pr),
                1 => self.paint_cal(c, cx, pr),
                2 => self.paint_todo(c, cx, pr),
                _ => self.paint_timer(c, cx, pr),
            }
        });
    }

    fn paint_tabs(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, r: R) {
        let t = &cx.theme;
        let tabs = [("notifications", "Notifications"), ("calendar_month", "Calendar"), ("done_outline", "To Do"), ("timer", "Timer")];
        let badge = (!s.notifs.is_empty()).then(|| s.notifs.len().to_string());
        let bw = badge.as_ref().map_or(0.0, |b| (c.g.measure(b, txt(11.0, 400)).0.ceil() + 10.0).max(18.0));
        let content: Vec<f32> = tabs.iter().enumerate().map(|(i, (_, l))| 18.0 + 6.0 + c.g.measure(l, txt(13.0, 400)).0.ceil() + if i == 0 && bw > 0.0 { 6.0 + bw } else { 0.0 }).collect();
        let free = (r.w - 16.0 - 12.0 - content.iter().map(|w| w + 20.0).sum::<f32>()) / 4.0;
        let mut x = r.x + 8.0;
        for (i, (icon, label)) in tabs.iter().enumerate() {
            let w = content[i] + 20.0 + free;
            let b = R::new(x, r.y + 8.0, w.max(0.0), 36.0);
            x += b.w + 4.0;
            let on = i == self.tab;
            let fg = if on { t.c("onSecondaryContainer") } else { t.c("onSurfaceVariant") };
            if on { c.rect(b, 18.0, t.c("secondaryContainer")); }
            c.hit(b, ID_TAB + i as u32);
            let mut tx = b.x + (b.w - content[i]) / 2.0;
            c.icon(icon, R::new(tx, b.y, 18.0, 36.0), ico(18.0, false), fg);
            tx += 24.0;
            let tw = c.g.measure(label, txt(13.0, 400)).0.ceil();
            c.text(label, R::new(tx, b.y, tw + 2.0, 36.0), txt(13.0, 400), fg, Align::Left);
            if let (0, Some(n)) = (i, &badge) {
                let br = R::new(tx + tw + 6.0, b.y + 9.0, bw, 18.0);
                c.rect(br, 9.0, t.c("primary"));
                c.text(n, br, txt(11.0, 400), t.c("onPrimary"), Align::Center);
            }
        }
    }

    // -------------------------------------------------------------- notifications

    fn paint_notifs(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, pr: R) {
        let t = &cx.theme;
        let foot = pr.y + pr.h - 32.0;
        ui::chip(c, cx, pr.x, foot, ID_CENTER, Some("open_in_new"), "Notification center", false);
        let cw = chip_w(c.g, "Clear");
        ui::chip(c, cx, pr.x + pr.w - cw, foot, ID_CLEAR, Some("clear_all"), "Clear", false);
        let list = R::new(pr.x, pr.y, pr.w, (foot - 8.0 - pr.y).max(0.0));
        if !s.notif_err.is_empty() {
            let h = c.g.measure_wrapped(&s.notif_err, txt(13.0, 400), list.w - 24.0) + 20.0;
            c.rect(R::new(list.x, list.y, list.w, h), 17.0, alpha(t.c("error"), 0.18));
            c.para(&s.notif_err, R::new(list.x + 12.0, list.y + 10.0, list.w - 24.0, h - 20.0), txt(13.0, 400), t.c("onSurface"), Align::Left);
            return;
        }
        if s.notifs.is_empty() { return empty(c, cx, list, "notifications_off", "Nothing"); }
        let groups = group(&s.notifs);
        // layout: per group the row heights, so the scroll range is known before drawing
        let lh = 13.0 * 1.35;
        let layout: Vec<(f32, Vec<Row>)> = groups.iter().map(|g| {
            let rows: Vec<Row> = g.items.iter().take(5).enumerate().map(|(i, n)| {
                let time = ago_text(n.time);
                let tw = c.g.measure(&time, txt(11.0, 400)).0.ceil();
                let w = list.w - 20.0 - 4.0 - 8.0 - tw;
                let th = lines_h(c.g, &n.title, txt(13.0, 500), w, lh, 4);
                let bh = lines_h(c.g, &n.body, txt(13.0, 400), w, lh, 4);
                Row { h: 12.0 + th + bh + if i > 0 { 1.0 } else { 0.0 }, th, bh, time, tw }
            }).collect();
            (8.0 + 24.0 + 4.0 + rows.iter().map(|r| r.h).sum::<f32>() + if g.items.len() > 5 { 4.0 + 14.4 } else { 0.0 } + 8.0, rows)
        }).collect();
        let content = layout.iter().map(|l| l.0).sum::<f32>() + 6.0 * (layout.len() - 1) as f32;
        let mut sc = std::mem::take(&mut self.sc_notif);
        sc.region(c, cx, list, content, |c, top| {
            let mut y = top;
            for (gi, (g, (gh, rows))) in groups.iter().zip(&layout).enumerate() {
                let (gy, next) = (y, y + gh + 6.0);
                y = next;
                if gy + gh < list.y || gy > list.y + list.h { continue; }
                let card = R::new(list.x, gy, list.w, *gh);
                c.rect(card, 17.0, t.c("surfaceContainer"));
                let letter = g.app.chars().next().map(|ch| ch.to_string()).unwrap_or_default();
                c.circle(card.x + 21.0, gy + 20.0, 11.0, t.c("secondaryContainer"));
                c.text(&letter, R::new(card.x + 10.0, gy + 9.0, 22.0, 22.0), txt(11.0, 600), t.c("onSecondaryContainer"), Align::Center);
                c.text(&g.app, R::new(card.x + 38.0, gy + 8.0, card.w - 38.0 - 40.0, 24.0), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
                let (top, bot) = (card.y.max(list.y), (card.y + card.h).min(list.y + list.h));
                c.hit(R::new(card.x, top, card.w, bot - top), ID_GROUP + gi as u32);
                let x = R::new(card.x + card.w - 34.0, gy + 8.0, 24.0, 24.0);
                if c.hit(x, ID_DISMISS + gi as u32) { c.circle(x.x + 12.0, x.y + 12.0, 12.0, alpha(t.c("onSurface"), 0.08)); }
                c.icon("close", x, ico(16.0, false), t.c("onSurfaceVariant"));
                let mut ry = gy + 36.0;
                for (n, row) in g.items.iter().zip(rows) {
                    if row.h > 0.0 && ry > gy + 36.0 { c.rect(R::new(card.x + 10.0, ry, card.w - 20.0, 1.0), 0.0, alpha(t.c("outlineVariant"), 0.35)); }
                    let top = ry + if ry > gy + 36.0 { 1.0 } else { 0.0 } + 6.0;
                    let w = card.w - 20.0 - 4.0 - 8.0 - row.tw;
                    c.para(&n.title, R::new(card.x + 12.0, top, w, row.th), txt(13.0, 500), t.c("onSurface"), Align::Left);
                    if row.bh > 0.0 { c.para(&n.body, R::new(card.x + 12.0, top + row.th, w, row.bh), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left); }
                    c.text(&row.time, R::new(card.x + card.w - 12.0 - row.tw, top, row.tw + 2.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Right);
                    ry += row.h;
                }
                if g.items.len() > 5 {
                    c.text(&format!("+{} more", g.items.len() - 5), R::new(card.x + 12.0, ry + 4.0, card.w - 24.0, 14.4), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left);
                }
            }
        });
        self.sc_notif = sc;
    }

    // -------------------------------------------------------------- calendar

    fn paint_cal(&mut self, c: &mut Canvas, cx: &Ctx, pr: R) {
        let t = &cx.theme;
        let today = crate::util::now();
        let cells = month_grid(self.cal.0, self.cal.1, (today.wYear as i32, today.wMonth as u32, today.wDay as u32));
        let rows = cells.len() / 7;
        let content = 36.0 + 6.0 + 26.4 + rows as f32 * 36.0;
        let first = windows::Win32::Foundation::SYSTEMTIME { wYear: self.cal.0 as u16, wMonth: self.cal.1 as u16, wDay: 1, ..Default::default() };
        let month = crate::util::date_fmt(&first, "MMMM yyyy");
        let mut sc = std::mem::take(&mut self.sc_page);
        sc.region(c, cx, pr, content, |c, top| {
            let cr = R::new(pr.x, top + 2.0, 28.0 + c.g.measure(&month, txt(13.0, 500)).0.ceil(), 32.0);
            c.rect(cr, 16.0, t.c("secondaryContainer"));
            c.text(&month, R::new(cr.x + 14.0, cr.y, cr.w - 28.0, 32.0), txt(13.0, 500), t.c("onSecondaryContainer"), Align::Left);
            ui::icon_btn(c, cx, R::new(pr.x + pr.w - 72.0, top, 36.0, 36.0), ID_CAL, "chevron_left", ui::IconKind::Plain, 20.0, false);
            ui::icon_btn(c, cx, R::new(pr.x + pr.w - 36.0, top, 36.0, 36.0), ID_CAL + 1, "chevron_right", ui::IconKind::Plain, 20.0, false);
            let colw = (pr.w - 12.0) / 7.0;
            let gy = top + 42.0;
            for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
                c.text(d, R::new(pr.x + i as f32 * (colw + 2.0), gy, colw, 26.4), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Center);
            }
            for (i, cell) in cells.iter().enumerate() {
                let (col, row) = ((i % 7) as f32, (i / 7) as f32);
                let (x, y) = (pr.x + col * (colw + 2.0), gy + 28.4 + row * 36.0);
                let r = R::new(x + (colw - 34.0) / 2.0, y, 34.0, 34.0);
                if cell.today { c.circle(r.x + 17.0, r.y + 17.0, 17.0, t.c("primary")); }
                let (col, wt) = if cell.today { (t.c("onPrimary"), 600) } else if cell.out { (alpha(t.c("outline"), 0.6), 400) } else { (t.c("onSurface"), 400) };
                c.text(&cell.d.to_string(), r, txt(13.0, wt), col, Align::Center);
            }
        });
        self.sc_page = sc;
    }

    // -------------------------------------------------------------- to-do

    fn paint_todo(&mut self, c: &mut Canvas, cx: &Ctx, pr: R) {
        let t = &cx.theme;
        let add = pr.y + pr.h - 40.0;
        let list = R::new(pr.x, pr.y, pr.w, (add - 8.0 - pr.y).max(0.0));
        let items = todos();
        if items.is_empty() { empty(c, cx, list, "checklist", "Nothing to do"); } else {
            let tw = pr.w - 98.0;
            let hs: Vec<f32> = items.iter().map(|x| (c.g.measure_wrapped(x["text"].as_str().unwrap_or(""), txt(13.5, 400), tw) + 12.0).max(44.0)).collect();
            let content = hs.iter().sum::<f32>() + 6.0 * (hs.len() - 1) as f32;
            let mut sc = std::mem::take(&mut self.sc_todo);
            sc.region(c, cx, list, content, |c, top| {
                let mut y = top;
                for (i, (x, h)) in items.iter().zip(&hs).enumerate().take(100) {
                    let ry = y;
                    y += h + 6.0;
                    if ry + h < list.y || ry > list.y + list.h { continue; }
                    let done = x["done"].as_bool() == Some(true);
                    let text = x["text"].as_str().unwrap_or("");
                    c.rect(R::new(list.x, ry, list.w, *h), 17.0, t.c("surfaceContainer"));
                    let ck = R::new(list.x + 12.0, ry + (h - 22.0) / 2.0, 22.0, 22.0);
                    if done {
                        c.circle(ck.x + 11.0, ck.y + 11.0, 11.0, t.c("primary"));
                        c.icon("check", ck, ico(15.0, false), t.c("onPrimary"));
                    } else {
                        c.circle(ck.x + 11.0, ck.y + 11.0, 11.0, t.c("outline"));
                        c.circle(ck.x + 11.0, ck.y + 11.0, 9.0, t.c("surfaceContainer"));
                    }
                    c.hit(R::new(ck.x - 4.0, ry, 30.0, *h), ID_TODO_CHK + i as u32);
                    let tr = R::new(list.x + 44.0, ry + 6.0, tw, h - 12.0);
                    let col = if done { t.c("onSurfaceVariant") } else { t.c("onSurface") };
                    c.para(text, tr, txt(13.5, 400), col, Align::Left);
                    // shortcut: the strike-through is drawn for one-line tasks only
                    if done && *h <= 44.0 { let w = c.g.measure(text, txt(13.5, 400)).0.min(tw); c.line((tr.x, ry + h / 2.0), (tr.x + w, ry + h / 2.0), 1.2, col); }
                    ui::icon_btn(c, cx, R::new(list.x + list.w - 44.0, ry + (h - 36.0) / 2.0, 36.0, 36.0), ID_TODO_DEL + i as u32, "delete", ui::IconKind::Plain, 20.0, false);
                }
            });
            self.sc_todo = sc;
        }
        let input = R::new(pr.x, add, pr.w - 42.0, 40.0);
        c.rect(input, 20.0, if self.todo_focus { t.c("primary") } else { t.c("outlineVariant") });
        c.rect(R::new(input.x + 1.0, input.y + 1.0, input.w - 2.0, 38.0), 19.0, t.c("surfaceContainer"));
        c.hit(input, 230);
        self.todo.draw(c, cx, R::new(input.x + 14.0, input.y, input.w - 28.0, 40.0), txt(14.0, 400), "Add a task", self.todo_focus);
        ui::icon_btn(c, cx, R::new(pr.x + pr.w - 36.0, add, 36.0, 36.0), ID_ADD, "add", ui::IconKind::Filled, 20.0, false);
    }

    // -------------------------------------------------------------- timer

    fn paint_timer(&mut self, c: &mut Canvas, cx: &Ctx, pr: R) {
        let t = &cx.theme;
        let st = hub::timer();
        let now = hub::now_ms();
        let mid = pr.x + pr.w / 2.0;
        let sw = self.sw_mode;
        let modes = |c: &mut Canvas, y: f32| {
            let (a, b) = (chip_w(c.g, "Pomodoro") - 24.0, chip_w(c.g, "Stopwatch") - 24.0);
            let x = mid - (a + b + 6.0) / 2.0;
            ui::chip(c, cx, x, y, ID_MODE, None, "Pomodoro", !sw);
            ui::chip(c, cx, x + a + 6.0, y, ID_MODE + 1, None, "Stopwatch", sw);
        };
        let ctl = |c: &mut Canvas, y: f32, run: bool, lap: bool| {
            let x = mid - 72.0;
            ui::icon_btn(c, cx, R::new(x, y + 10.0, 36.0, 36.0), ID_T, "replay", ui::IconKind::Tonal, 20.0, false);
            big_btn(c, cx, R::new(x + 44.0, y, 56.0, 56.0), ID_T + 1, if run { "pause" } else { "play_arrow" });
            let r = R::new(x + 108.0, y + 10.0, 36.0, 36.0);
            if sw && !lap { c.rect(r, 18.0, alpha(t.c("secondaryContainer"), 0.4)); c.icon("flag", r, ico(20.0, false), alpha(t.c("onSecondaryContainer"), 0.4)); }
            else { ui::icon_btn(c, cx, r, ID_T + 2, if sw { "flag" } else { "skip_next" }, ui::IconKind::Tonal, 20.0, false); }
        };
        if !sw {
            let left = left_at(&st, &cx.cfg, now);
            let frac = (left / total(&st, &cx.cfg)).clamp(0.0, 1.0) as f32;
            let label = pomo_label(&st, &cx.cfg);
            let mut sc = std::mem::take(&mut self.sc_page);
            sc.region(c, cx, pr, 316.0, |c, top| {
                modes(c, top);
                let cy = top + 32.0 + 12.0 + 4.0 + 100.0;
                c.arc(mid, cy, 95.0, 1.0, 10.0, t.c("surfaceContainerHigh"));
                c.arc(mid, cy, 95.0, frac, 10.0, t.c("primary"));
                c.text(&hub::clock(left as f64), R::new(mid - 90.0, cy - 33.0, 180.0, 50.0), txt(40.0, 500), t.c("onSurface"), Align::Center);
                c.text(&label, R::new(mid - 90.0, cy + 17.0, 180.0, 16.3), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Center);
                ctl(c, cy + 100.0 + 12.0, st["running"].as_bool() == Some(true), true);
            });
            self.sc_page = sc;
            return;
        }
        modes(c, pr.y);
        let e = sw_elapsed_at(&st, now);
        let running = st["sw"]["running"].as_bool() == Some(true);
        let big = pr.y + 32.0 + 12.0 + 28.0;
        c.text(&sw_text(e), R::new(pr.x, big, pr.w, 55.0), txt(44.0, 500), t.c("onSurface"), Align::Center);
        let cy = big + 55.0 + 8.0 + 12.0;
        ctl(c, cy, running, running);
        let laps: Vec<f64> = st["sw"]["laps"].as_array().map(|l| l.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let list = R::new(pr.x, cy + 56.0 + 12.0, pr.w, (pr.y + pr.h - cy - 68.0).max(0.0));
        let content = if laps.is_empty() { 0.0 } else { laps.len() as f32 * 32.3 - 4.0 };
        let mut sc = std::mem::take(&mut self.sc_laps);
        sc.region(c, cx, list, content, |c, top| {
            for (i, l) in laps.iter().enumerate() {
                let y = top + i as f32 * 32.3;
                if y + 28.3 < list.y || y > list.y + list.h { continue; }
                let r = R::new(list.x, y, list.w, 28.3);
                c.rect(r, 12.0, t.c("surfaceContainer"));
                c.text(&format!("Lap {}", laps.len() - i), R::new(r.x + 12.0, r.y, 200.0, r.h), txt(13.0, 400), t.c("onSurface"), Align::Left);
                c.text(&sw_text(*l), R::new(r.x + r.w - 212.0, r.y, 200.0, r.h), txt(13.0, 400), t.c("onSurface"), Align::Right);
            }
        });
        self.sc_laps = sc;
    }
}
