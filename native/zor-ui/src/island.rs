//! The Dynamic Island panel (island-panel.js / .html): now playing with wave seek and lyrics, a recording,
//! the timer or stopwatch, the idle clock + weather, AI agents and the latest notifications.
//! Sizes itself from its sections (400 wide + 12 px of shadow room); the app keeps it centred under the island.

use crate::{
    app::{self, Action, Ctx},
    conf::Config,
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, Pixmap, R},
    hub, lyrics::{self, Lyr, Lyrics, Track}, media, ui, util, weather,
    win::{Btn, Input, Phase, View},
};
use serde_json::{json, Value};
use std::cell::Cell;
use windows::Win32::{Foundation::{FILETIME, SYSTEMTIME}, System::Time::{FileTimeToSystemTime, SystemTimeToFileTime}};
use std::sync::{Arc, Mutex};

const W: f32 = crate::tokens::ISLAND_W; // the panel; the window adds 12 px each side for the shadow
const PAD: f32 = 15.0; // 1 px border + 14 px padding
const SEP: f32 = 25.0; // gap 12 + border 1 + padding 12 between sections
const LYR_H: f32 = 70.0;
const ROW: f32 = 13.0 * ui::LH + 4.0; // one lyric line
const PREV: u32 = 1;
const PLAY: u32 = 2;
const NEXT: u32 = 3;
const SEEK: u32 = 4;
const REC_STOP: u32 = 5;
const TIMER_A: u32 = 6;
const TIMER_B: u32 = 7;
const CLEAR: u32 = 8;
const LYRICS: u32 = 9;
/// Most lines the buddy's question shows before it scrolls.
const ASK_MAX_LINES: f32 = 8.0;
const ALLOW: u32 = 10;
const DENY: u32 = 11;
const AGENT_CLEAR: u32 = 12;
/// Allow and Deny for a Claude Code permission prompt (permit.rs); the same block as the buddy's question.
const PERMIT_ALLOW: u32 = 17;
const PERMIT_DENY: u32 = 18;
/// The open notification's buttons and its body (the list rows are NOTIF + index).
const NOTIF_BACK: u32 = 130;
const NOTIF_OPEN: u32 = 131;
const NOTIF_DISMISS: u32 = 132;
const NOTIF_BODY: u32 = 133;
const ASK_Q: u32 = 13;
const ASK_BOX: u32 = 14;
const ASK_SEND: u32 = 15;
const ASK_MIC: u32 = 16;
const ASK_BOX_H: f32 = 36.0;
const NOTIF: u32 = 100;

/// Ask: the buddy's question. Permit: what Claude Code wants to do (the answer finds its session itself).
enum Sec { Ask(String), Permit(String, String),Media, Rec, Timer, Idle, Agents(Vec<Value>), Notifs(Vec<Value>), NotifDetail(Value) }

pub struct Island {
    notifs: Arc<Mutex<Vec<Value>>>,
    asked: bool,
    art: Option<((std::sync::Arc<Vec<u8>>, u32), Pixmap)>,
    /// Lyrics slide: current offset, and the track it belongs to (a new track jumps instead of sliding).
    ly: f32,
    ly_key: String,
    scroll: ui::Scroll,
    plain_h: f32,
    seek: R,
    scrub: ui::Scrub,
    /// What the two timer buttons do: (stopwatch, done) from the last paint.
    tkind: (bool, bool),
    /// frame_ms: 50 while the wave moves, 100 while the recording dot pulses.
    frames: Cell<u32>,
    /// The reply box under the buddy's question, whether it has the keyboard, and the question's scroll and full height.
    reply: ui::TextBox,
    reply_focus: bool,
    ask_scroll: ui::Scroll,
    ask_full: f32,
    /// The notification opened in the panel (its id), and its body's scroll and full height.
    notif_open: Option<Value>,
    body_scroll: ui::Scroll,
    body_full: f32,
}

impl Island {
    /// `area`: unused, the panel sizes itself.
    pub fn new(_area: (f32, f32)) -> Island {
        Island { notifs: Default::default(), asked: false, art: None, ly: 0.0, ly_key: String::new(), scroll: Default::default(), plain_h: 0.0,
            seek: R::new(0.0, 0.0, 1.0, 22.0), scrub: Default::default(), tkind: (false, false), frames: Cell::new(0),
            reply: ui::TextBox::new(false), reply_focus: false, ask_scroll: Default::default(), ask_full: 0.0,
            notif_open: None, body_scroll: Default::default(), body_full: 0.0 }
    }
}

/// Height the question gets: all of it, from one line up to `ASK_MAX_LINES` (it scrolls past that).
fn ask_h(full: f32) -> f32 { full.clamp(lh(13.0), lh(13.0) * ASK_MAX_LINES) }

/// Height of the whole ask section for a question that wraps to `full` px.
fn ask_section_h(full: f32) -> f32 { 16.0 + 10.0 + ask_h(full) + 12.0 + ASK_BOX_H + 10.0 + 32.0 }

/// island-panel.js `ago`: "now", "5m", "3h", "2d".
pub fn ago(now: f64, t: f64) -> String {
    let s = now - t;
    if s < 60.0 { "now".into() } else if s < 3600.0 { format!("{}m", (s / 60.0) as u64) } else if s < 86400.0 { format!("{}h", (s / 3600.0) as u64) } else { format!("{}d", (s / 86400.0) as u64) }
}

fn dp(cx: &Ctx, k: &str) -> bool { cx.cfg.get(&format!("island.panel.{k}")).as_bool() != Some(false) }
fn lh(size: f32) -> f32 { size * ui::LH }
fn s(v: &Value, k: &str) -> String { v[k].as_str().unwrap_or("").to_string() }

/// "resets 14:05", or with the weekday when more than a day away.
fn resets(cx: &Ctx, at: f64) -> String {
    if at <= 0.0 { return String::new(); }
    let now = util::now();
    let diff = at - hub::now_ms() / 1000.0;
    let st = add_secs(&now, diff as i64);
    let hm = util::clock(st.wHour, st.wMinute, cx.cfg.bool("use24h"));
    if diff < 86400.0 { format!("resets {hm}") } else { format!("resets {} {hm}", util::date_fmt(&st, "ddd")) }
}

fn usage_name(k: &str) -> &str { match k { "five_hour" => "5-hour", "seven_day" => "Weekly", "spend_limit" => "Spend", o => o } }

fn phase_name(p: &str) -> &'static str { match p { "short" => "Short break", "long" => "Long break", _ => "Focus" } }

/// timer.pomo.toggle: pause with the time left, or run until now + left.
fn pomo_toggle(t: &mut Value, left: f64, now: f64) {
    if t["running"].as_bool() == Some(true) { t["left"] = json!(left); t["running"] = json!(false); }
    else { t["endsAt"] = json!(now + left * 1000.0); t["left"] = Value::Null; t["running"] = json!(true); }
}

fn sw_toggle(t: &mut Value, now: f64) {
    let sw = &mut t["sw"];
    if sw["running"].as_bool() == Some(true) { sw["acc"] = json!(sw["acc"].as_f64().unwrap_or(0.0) + now - sw["start"].as_f64().unwrap_or(0.0)); sw["running"] = json!(false); }
    else { sw["start"] = json!(now); sw["running"] = json!(true); }
}

fn timer_act(cx: &Ctx, act: &str, sw: bool) {
    let (mut t, now) = (hub::timer(), hub::now_ms());
    match (act, sw) {
        ("toggle", false) => { let l = hub::left(&t, &cx.cfg); pomo_toggle(&mut t, l, now) }
        ("reset", false) => t = json!({ "phase": "focus", "cycle": 0, "running": false, "endsAt": 0, "left": null, "sw": t["sw"].clone() }),
        ("toggle", true) => sw_toggle(&mut t, now),
        ("reset", true) => t["sw"] = json!({ "running": false, "start": 0, "acc": 0, "laps": [] }),
        ("next", _) => { hub::set("zor-ui.timerDone", None); let l = hub::left(&t, &cx.cfg); pomo_toggle(&mut t, l, now) }
        _ => return,
    }
    hub::set("zor-ui.timer", Some(t.to_string()));
}

/// `st` plus `secs` (calendar arithmetic through FILETIME, like util::add_days).
fn add_secs(st: &SYSTEMTIME, secs: i64) -> SYSTEMTIME {
    unsafe {
        let mut ft = FILETIME::default();
        let _ = SystemTimeToFileTime(st, &mut ft);
        let t = ((ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64) as i64 + secs * 10_000_000;
        let mut out = SYSTEMTIME::default();
        let _ = FileTimeToSystemTime(&FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 }, &mut out);
        out
    }
}

fn chip_w(c: &Canvas, label: &str, icon: bool) -> f32 { 14.0 + if icon { 24.0 } else { 0.0 } + c.g.measure(label, txt(13.0, 400)).0.ceil() + 14.0 }

impl Island {
    fn plan(&self, g: &Gfx, cx: &Ctx) -> Vec<(Sec, f32)> {
        let mut v = vec![];
        // the buddy wants an answer before he drives (drive.rs): first, always
        if let Some(q) = crate::drive::pending() { let h = ask_section_h(g.measure_wrapped(&q, txt(13.0, 400), W - 2.0 * PAD)); v.push((Sec::Ask(q), h)); }
        if let Some((label, _, q)) = crate::bridge::permit::pending() { let h = ask_section_h(g.measure_wrapped(&q, txt(13.0, 400), W - 2.0 * PAD)); v.push((Sec::Permit(format!("{label} is asking"), q), h)); }
        let m = cx.media.as_ref().filter(|m| !m.title.is_empty());
        let ly = Self::lyrics_h(cx);
        if dp(cx, "media") && m.is_some() { v.push((Sec::Media, 92.0 + 22.0 + 60.0 + 20.0 + if ly > 0.0 { 10.0 + ly } else { 0.0 })); }
        if dp(cx, "recording") && hub::get("zor-ui.rec").and_then(|r| r.parse::<f64>().ok()).is_some_and(|r| r > 0.0) { v.push((Sec::Rec, 44.0)); }
        let t = hub::timer();
        let done = hub::get("zor-ui.timerDone").is_some_and(|d| !d.is_empty());
        if dp(cx, "timer") && (done || t["running"].as_bool() == Some(true) || t["sw"]["running"].as_bool() == Some(true)) { v.push((Sec::Timer, 16.0 + 10.0 + 44.0)); }
        if dp(cx, "clock") && v.is_empty() { v.push((Sec::Idle, 72.0)); }
        if dp(cx, "agents") {
            let list = cx.agents.clone();
            if !list.is_empty() {
                let h = 16.0 + list.iter().map(|a| 10.0 + 18.0 + if a["state"] == "waiting" { 6.0 + lh(12.5) } else { 0.0 }
                    + a["usage"].as_object().map_or(0.0, |u| u.len() as f32 * (6.0 + lh(12.0)))).sum::<f32>();
                v.push((Sec::Agents(list), h));
            }
        }
        if dp(cx, "notifications") {
            // a clicked notification opens in place of the list
            if let Some(n) = self.notif_open.clone() {
                let (title_h, body_full) = Self::notif_text_h(g, &n);
                let body = if s(&n, "body").is_empty() { 0.0 } else { ask_h(body_full) + 12.0 };
                v.push((Sec::NotifDetail(n), 26.0 + title_h + 8.0 + body + 32.0));
            } else {
                let n = self.notifs.lock().map(|n| n.clone()).unwrap_or_default();
                let count = cx.cfg.get("island.panel.notifCount").as_u64().unwrap_or(3) as usize;
                let n: Vec<Value> = n.into_iter().take(count).collect();
                let h = if n.is_empty() { 16.0 + 10.0 + lh(12.5) } else {
                    22.0 + n.iter().map(|x| 10.0 + 16.0 + lh(13.0) * if s(x, "body").is_empty() { 1.0 } else { 2.0 }).sum::<f32>()
                };
                v.push((Sec::Notifs(n), h));
            }
        }
        v
    }

    /// Height of the lyrics area under the controls: 0 when off or hidden, one line for the "No lyrics found" note, else the full area.
    fn lyrics_h(cx: &Ctx) -> f32 {
        let Some(m) = cx.media.as_ref().filter(|m| !m.title.is_empty()) else { return 0.0 };
        if !(cx.cfg.bool("lyrics") && dp(cx, "lyrics")) { return 0.0; }
        Self::lyrics_h_for(&lyrics::get(&Track { title: &m.title, artist: &m.artist, album: &m.album, dur: m.duration }), m)
    }

    fn lyrics_h_for(l: &Lyr, m: &media::Media) -> f32 {
        match l { Lyr::Missing if l.hidden_for(m) => 0.0, Lyr::Missing => ROW, _ => LYR_H }
    }

    fn content_h(plan: &[(Sec, f32)]) -> f32 {
        if plan.is_empty() { return 16.0; }
        plan.iter().map(|p| p.1).sum::<f32>() + SEP * (plan.len() - 1) as f32
    }

    fn art(&mut self, g: &Gfx, m: &media::Media, px: u32) -> Option<Pixmap> {
        let b = m.art.as_ref()?;
        // holding the Arc keeps its address from being reused by the next cover
        if !self.art.as_ref().is_some_and(|a| std::sync::Arc::ptr_eq(&a.0 .0, b) && a.0 .1 == px) {
            self.art = Some(((b.clone(), px), g.load_image_bytes(b, px, px)?));
        }
        self.art.as_ref().map(|a| a.1.clone())
    }

    fn now_playing(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, lyr: f32) {
        let (t, Some(m)) = (&cx.theme, cx.media.as_ref()) else { return };
        let (w, pos) = (W - 2.0 * PAD, self.scrub.get().filter(|_| m.duration > 0.0).map_or(m.now(), |v| v as f64 * m.duration));
        let art = R::new(x, y, 92.0, 92.0);
        c.rect(art, 20.0, t.c("secondaryContainer"));
        match self.art(c.g, m, (92.0 * c.scale).round() as u32) {
            Some(p) => c.clip_round(art, 20.0, |c| c.image(&p, art, 1.0)),
            None => c.icon("music_note", art, ico(36.0, true), t.c("onSecondaryContainer")),
        }
        let ty = y + (92.0 - lh(17.0) - lh(13.0)) / 2.0;
        c.text(&m.title, R::new(x + 106.0, ty, w - 106.0, lh(17.0)), txt(17.0, 500), t.c("onSurface"), Align::Left);
        c.text(&m.artist, R::new(x + 106.0, ty + lh(17.0), w - 106.0, lh(13.0)), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left);
        // seek row
        let sy = y + 102.0;
        let (ps, ds) = (hub::clock(pos), if m.duration > 0.0 { hub::clock(m.duration) } else { String::new() });
        let (pw, dw) = (c.g.measure(&ps, txt(11.5, 400)).0.ceil(), c.g.measure(&ds, txt(11.5, 400)).0.ceil());
        let small = txt(11.5, 400);
        c.text(&ps, R::new(x, sy, pw + 2.0, 22.0), small, t.c("onSurfaceVariant"), Align::Left);
        c.text(&ds, R::new(x + w - dw - 2.0, sy, dw + 2.0, 22.0), small, t.c("onSurfaceVariant"), Align::Right);
        self.seek = R::new(x + pw + 10.0, sy, (w - pw - dw - 20.0).max(10.0), 22.0);
        ui::wave(c, self.seek, SEEK, if m.duration > 0.0 { (pos / m.duration) as f32 } else { 0.0 }, m.playing, t.c("primary"));
        // controls
        let cy = y + 134.0;
        let cx0 = x + (w - 168.0) / 2.0;
        ui::icon_btn(c, cx, R::new(cx0, cy + 8.0, crate::tokens::TRANSPORT_BTN, crate::tokens::TRANSPORT_BTN), PREV, "skip_previous", ui::IconKind::Plain, 26.0, true);
        let big = R::new(cx0 + 54.0, cy, 60.0, 60.0);
        c.rect(big, 20.0, ui::accent(cx));
        if c.hit(big, PLAY) { c.rect(big, 20.0, alpha(t.c("onPrimary"), crate::tokens::HOVER_A)); }
        c.icon(if m.playing { "pause" } else { "play_arrow" }, big, ico(30.0, true), t.c("onPrimary"));
        ui::icon_btn(c, cx, R::new(cx0 + 124.0, cy + 8.0, crate::tokens::TRANSPORT_BTN, crate::tokens::TRANSPORT_BTN), NEXT, "skip_next", ui::IconKind::Plain, 26.0, true);
        if lyr > 0.0 { self.lyrics(c, cx, R::new(x, cy + 70.0, w, lyr), m, pos); }
    }

    /// renderLyrics(): the current synced line stays centred (sliding), the edges fade; plain lyrics scroll.
    fn lyrics(&mut self, c: &mut Canvas, cx: &Ctx, r: R, m: &media::Media, pos: f64) {
        let t = &cx.theme;
        let msg = |c: &mut Canvas, s: &str| c.text(s, r, txt(12.5, 400), t.c("onSurfaceVariant"), Align::Center);
        let found = match lyrics::get(&Track { title: &m.title, artist: &m.artist, album: &m.album, dur: m.duration }) {
            Lyr::Searching => return msg(c, "Searching lyrics…"),
            l @ Lyr::Missing if l.hidden_for(m) => return,
            Lyr::Missing => return msg(c, "No lyrics found"),
            Lyr::Found(l) => l,
        };
        match &*found {
            Lyrics::Plain(lines) => {
                self.plain_h = lines.len() as f32 * ROW;
                c.hit(r, LYRICS);
                self.scroll.region(c, cx, r, self.plain_h, |c, top| {
                    for (i, l) in lines.iter().enumerate() {
                        let y = top + i as f32 * ROW;
                        if y + ROW >= r.y && y <= r.y + r.h { c.text(l, R::new(r.x + 8.0, y, r.w - 16.0, ROW), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Center); }
                    }
                });
            }
            Lyrics::Synced(v) => {
                let idx = lyrics::current_index(v, pos);
                let target = r.h / 2.0 - (idx.unwrap_or(0) as f32 * ROW + ROW / 2.0);
                let key = format!("{}|{}", m.title, m.artist);
                if self.ly_key != key { self.ly_key = key; self.ly = target; } else { self.ly += (target - self.ly) * 0.3; }
                c.clip_round(r, 0.0, |c| for (i, (_, l)) in v.iter().enumerate() {
                    let y = r.y + self.ly + i as f32 * ROW;
                    if y + ROW < r.y || y > r.y + r.h { continue; }
                    let u = (y + ROW / 2.0 - r.y) / r.h; // the CSS mask: transparent -> opaque over 30% at each end
                    let fade = (u / 0.3).clamp(0.0, 1.0) * ((1.0 - u) / 0.3).clamp(0.0, 1.0);
                    let l = if l.is_empty() { "♪" } else { l };
                    let cur = idx == Some(i);
                    let (f, col) = if cur { (txt(13.0 * 1.12, 500), t.c("onSurface")) } else { (txt(13.0, 400), alpha(t.c("onSurfaceVariant"), 0.5)) };
                    c.text(l, R::new(r.x + 8.0, y, r.w - 16.0, ROW), f, alpha(col, fade), Align::Center);
                });
            }
        }
    }

    /// The buddy's question, or a Claude Code permission (`permit`), with Allow, Deny and a box to answer in words.
    /// Answering is the only way a take-over starts or a risky click goes through.
    fn ask(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, q: &str, permit: Option<&str>) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        let (icon, head) = match permit { Some(h) => ("terminal", h), None => ("smart_toy", "The buddy is asking") };
        c.icon(icon, R::new(x, y, 16.0, 16.0), ico(16.0, false), t.c("tertiary"));
        c.text(head, R::new(x + 22.0, y, w - 22.0, 16.0), txt(12.0, 500), t.c("tertiary"), Align::Left);
        let y = y + 26.0;
        let f = txt(13.0, 400);
        self.ask_full = c.g.measure_wrapped(q, f, w);
        let qr = R::new(x, y, w, ask_h(self.ask_full));
        c.hit(qr, ASK_Q);
        let full = self.ask_full;
        self.ask_scroll.region(c, cx, qr, full, |c, top| c.para(q, R::new(qr.x, top, qr.w - 8.0, full), f, t.c("onSurface"), Align::Left));
        let y = y + qr.h + 12.0;
        let br = R::new(x, y, w, ASK_BOX_H);
        c.rect(br, ASK_BOX_H / 2.0, t.c("surfaceContainer"));
        c.hit(br, ASK_BOX);
        self.reply.draw(c, cx, R::new(br.x + 14.0, br.y, br.w - 14.0 - 72.0, br.h), f, "Type an answer, or tap the mic", self.reply_focus);
        let kind = if crate::pill::active() { ui::IconKind::Filled } else { ui::IconKind::Plain }; // filled while it listens
        ui::icon_btn(c, cx, R::new(br.x + br.w - 70.0, br.y + 2.0, 32.0, 32.0), ASK_MIC, "mic", kind, 18.0, false);
        ui::icon_btn(c, cx, R::new(br.x + br.w - 34.0, br.y + 2.0, 32.0, 32.0), ASK_SEND, "send", ui::IconKind::Plain, 18.0, false);
        let y = y + ASK_BOX_H + 10.0;
        let (la, ld) = ("Allow", "Deny");
        let (wa, wd) = (chip_w(c, la, true), chip_w(c, ld, true));
        let (allow, deny) = if permit.is_some() { (PERMIT_ALLOW, PERMIT_DENY) } else { (ALLOW, DENY) };
        ui::chip(c, cx, x + w - wa - 8.0 - wd, y, allow, Some("check"), la, true);
        ui::chip(c, cx, x + w - wd, y, deny, Some("close"), ld, false);
    }

    fn rec(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        let since = hub::get("zor-ui.rec").and_then(|r| r.parse::<f64>().ok()).unwrap_or(0.0);
        let pulse = 0.25 + 0.75 * (0.5 + 0.5 * (hub::now_ms() / 900.0 * std::f64::consts::PI).cos()) as f32;
        c.circle(x + 5.0, y + 28.0, 5.0, alpha(t.c("error"), pulse));
        c.text(&hub::clock((hub::now_ms() - since) / 1000.0), R::new(x + 18.0, y, 220.0, 44.0), txt(44.0, 500), t.c("error"), Align::Left);
        let l = "Stop and show";
        ui::chip(c, cx, x + w - chip_w(c, l, true), y + 8.0, REC_STOP, Some("stop_circle"), l, false);
    }

    fn timer(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        let (tm, done) = (hub::timer(), hub::get("zor-ui.timerDone").unwrap_or_default());
        let running = tm["running"].as_bool() == Some(true);
        let sw = done.is_empty() && !running && tm["sw"]["running"].as_bool() == Some(true);
        let phase = tm["phase"].as_str().unwrap_or("focus");
        let title = if !done.is_empty() { if done == "focus" { "Focus done: break time" } else { "Break over: focus time" } } else if sw { "Stopwatch" } else { phase_name(phase) };
        c.icon(if sw { "timer_play" } else { "timer" }, R::new(x, y, 16.0, 16.0), ico(16.0, false), t.c("onSurfaceVariant"));
        c.text(title, R::new(x + 22.0, y, w - 22.0, 16.0), txt(12.0, 500), t.c("onSurfaceVariant"), Align::Left);
        let y = y + 26.0;
        let secs = if sw { hub::sw_elapsed(&tm) } else { hub::left(&tm, &cx.cfg) };
        c.text(&hub::clock(secs), R::new(x, y, 220.0, 44.0), txt(44.0, 500), t.c("onSurface"), Align::Left);
        self.tkind = (sw, !done.is_empty());
        if !done.is_empty() {
            let l = format!("Start {}", phase_name(phase).to_lowercase());
            ui::chip(c, cx, x + w - chip_w(c, &l, true), y + 8.0, TIMER_A, Some("play_arrow"), &l, false);
        } else {
            let (la, lb) = (if sw || running { "Pause" } else { "Resume" }, "Reset");
            let (wa, wb) = (chip_w(c, la, true), chip_w(c, lb, true));
            ui::chip(c, cx, x + w - wa - 8.0 - wb, y + 8.0, TIMER_A, Some(if sw || running { "pause" } else { "play_arrow" }), la, false);
            ui::chip(c, cx, x + w - wb, y + 8.0, TIMER_B, Some("replay"), lb, false);
        }
    }

    fn idle(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        let now = util::now();
        c.text(&util::clock(now.wHour, now.wMinute, cx.cfg.bool("use24h")), R::new(x, y + 6.0, 220.0, 44.0), txt(44.0, 500), t.c("onSurface"), Align::Left);
        c.text(&util::date_fmt(&now, "dddd, d MMMM"), R::new(x, y + 50.0, 220.0, lh(13.0)), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left);
        if let Some(wx) = &cx.weather {
            let rx = R::new(x + 190.0, y, w - 190.0, 34.0);
            c.icon(weather::icon(wx.status), R::new(rx.x, rx.y, rx.w, 34.0), ico(34.0, true), t.c("primary"));
            // the icon is block-level and right-aligned: place it at the right edge
            c.text(&weather::temp(wx, cx.cfg.str("tempUnit")), R::new(rx.x, y + 34.0, rx.w, 22.0), txt(18.0, 500), t.c("onSurface"), Align::Right);
            let line = format!("{}{}", weather::text(wx.status), if wx.city.is_empty() { String::new() } else { format!(" · {}", wx.city) });
            c.text(&line, R::new(rx.x, y + 56.0, rx.w, lh(13.0)), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Right);
        }
    }

    fn agents(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, list: &[Value]) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        c.icon("smart_toy", R::new(x, y, 16.0, 16.0), ico(16.0, false), t.c("onSurfaceVariant"));
        c.text("AI agents", R::new(x + 22.0, y, w - 22.0, 16.0), txt(12.0, 500), t.c("onSurfaceVariant"), Align::Left);
        if list.iter().any(|a| a["state"] == "waiting") {
            let tw = c.g.measure("Dismiss", txt(12.0, 500)).0.ceil();
            let b = R::new(x + w - (9.0 + 16.0 + 4.0 + tw + 9.0), y - 3.0, 9.0 + 16.0 + 4.0 + tw + 9.0, 22.0);
            if c.hit(b, AGENT_CLEAR) { c.rect(b, 11.0, t.c("surfaceContainer")); }
            c.icon("clear_all", R::new(b.x + 9.0, b.y + 3.0, 16.0, 16.0), ico(16.0, false), t.c("primary"));
            c.text("Dismiss", R::new(b.x + 29.0, b.y, tw + 4.0, 22.0), txt(12.0, 500), t.c("primary"), Align::Left);
        }
        let mut y = y + 26.0;
        for a in list {
            let state = s(a, "state");
            let (label, model) = (s(a, "label"), s(a, "model"));
            let (pill, bg, fg) = match state.as_str() {
                "waiting" => ("Needs you", t.c("tertiaryContainer"), t.c("onTertiaryContainer")),
                "working" => ("Working", t.c("secondaryContainer"), t.c("onSecondaryContainer")),
                _ => ("Idle", t.c("surfaceContainerHigh"), t.c("onSurface")),
            };
            let pw = c.g.measure(pill, txt(11.0, 400)).0.ceil() + 16.0;
            let lw = c.g.measure(&label, txt(13.0, 500)).0.ceil() + 2.0;
            c.text(&label, R::new(x, y, lw.min(w - pw - 8.0), 18.0), txt(13.0, 500), t.c("onSurface"), Align::Left);
            if !model.is_empty() { c.text(&model, R::new(x + lw + 8.0, y, (w - lw - pw - 16.0).max(0.0), 18.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left); }
            c.rect(R::new(x + w - pw, y, pw, 18.0), 9.0, bg);
            c.text(pill, R::new(x + w - pw, y, pw, 18.0), txt(11.0, 400), fg, Align::Center);
            y += 18.0;
            if state == "waiting" {
                let msg = a["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("Needs your input");
                c.text(msg, R::new(x, y + 6.0, w, lh(12.5)), txt(12.5, 400), t.c("tertiary"), Align::Left);
                y += 6.0 + lh(12.5);
            }
            for (k, u) in a["usage"].as_object().into_iter().flatten() {
                y += 6.0;
                let used = u["used_percentage"].as_f64().unwrap_or(0.0);
                let rs = resets(cx, u["resets_at"].as_f64().unwrap_or(0.0));
                let rw = c.g.measure(&rs, txt(11.0, 400)).0.ceil();
                let h = lh(12.0);
                c.text(usage_name(k), R::new(x, y, 56.0, h), txt(12.0, 400), t.c("onSurface"), Align::Left);
                let bar = R::new(x + 64.0, y + (h - 6.0) / 2.0, (w - 64.0 - 38.0 - rw - 24.0).max(10.0), 6.0);
                c.rect(bar, 3.0, alpha(t.c("primary"), 0.18));
                c.rect(R::new(bar.x, bar.y, bar.w * (used.min(100.0) / 100.0) as f32, 6.0), 3.0, t.c("primary"));
                c.text(&format!("{}%", used.round()), R::new(bar.x + bar.w + 8.0, y, 38.0, h), txt(12.0, 500), t.c("onSurface"), Align::Right);
                c.text(&rs, R::new(x + w - rw - 2.0, y, rw + 4.0, h), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Right);
                y += h;
            }
            y += 10.0;
        }
    }

    fn notifs(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, list: &[Value]) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        let hh = if list.is_empty() { 16.0 } else { 22.0 };
        c.icon("notifications", R::new(x, y + (hh - 16.0) / 2.0, 16.0, 16.0), ico(16.0, false), t.c("onSurfaceVariant"));
        c.text("Notifications", R::new(x + 22.0, y, w - 22.0, hh), txt(12.0, 500), t.c("onSurfaceVariant"), Align::Left);
        if !list.is_empty() {
            let tw = c.g.measure("Clear all", txt(12.0, 500)).0.ceil();
            let b = R::new(x + w - (9.0 + 16.0 + 4.0 + tw + 9.0), y, 9.0 + 16.0 + 4.0 + tw + 9.0, 22.0);
            if c.hit(b, CLEAR) { c.rect(b, 11.0, t.c("surfaceContainer")); }
            c.icon("clear_all", R::new(b.x + 9.0, b.y + 3.0, 16.0, 16.0), ico(16.0, false), t.c("primary"));
            c.text("Clear all", R::new(b.x + 29.0, b.y, tw + 4.0, 22.0), txt(12.0, 500), t.c("primary"), Align::Left);
        }
        let mut y = y + hh + 10.0;
        if list.is_empty() { return c.text("Nothing new", R::new(x, y, w, lh(12.5)), txt(12.5, 400), t.c("onSurfaceVariant"), Align::Left); }
        let now = hub::now_ms() / 1000.0;
        for (i, n) in list.iter().enumerate() {
            let body = s(n, "body");
            let h = 16.0 + lh(13.0) * if body.is_empty() { 1.0 } else { 2.0 };
            let r = R::new(x, y, w, h);
            let over = c.hit(r, NOTIF + i as u32);
            c.rect(r, 14.0, t.c(if over { "surfaceContainer" } else { "surfaceContainerLow" }));
            let ago = ago(now, n["time"].as_f64().unwrap_or(now));
            let aw = c.g.measure(&ago, txt(11.0, 400)).0.ceil();
            c.text(&ago, R::new(r.x + w - 10.0 - aw - 2.0, r.y + 8.0, aw + 4.0, lh(11.0)), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Right);
            let tw = w - 20.0 - aw - 10.0;
            c.text(&format!("{} · {}", s(n, "app"), s(n, "title")), R::new(r.x + 10.0, r.y + 8.0, tw, lh(13.0)), txt(13.0, 500), t.c("onSurface"), Align::Left);
            if !body.is_empty() { c.text(&body, R::new(r.x + 10.0, r.y + 8.0 + lh(13.0), tw, lh(13.0)), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left); }
            y += h + 10.0;
        }
    }

    /// An opened notification: its app and age, the title, the body (scrolls past a few lines), then Back, Open and Dismiss.
    fn notif_detail(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, n: &Value) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        let (title, body) = (s(n, "title"), s(n, "body"));
        let (title_h, body_full) = Self::notif_text_h(c.g, n);
        self.body_full = body_full;
        let now = hub::now_ms() / 1000.0;
        let app = s(n, "app");
        let ago = ago(now, n["time"].as_f64().unwrap_or(now));
        let aw = c.g.measure(&ago, txt(11.0, 400)).0.ceil();
        c.icon("notifications", R::new(x, y, 16.0, 16.0), ico(16.0, false), t.c("tertiary"));
        c.text(&app, R::new(x + 22.0, y, w - 22.0 - aw - 8.0, 16.0), txt(12.0, 500), t.c("tertiary"), Align::Left);
        c.text(&ago, R::new(x + w - aw, y, aw, 16.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Right);
        let y = y + 26.0;
        c.para(&title, R::new(x, y, w, title_h), txt(13.0, 500), t.c("onSurface"), Align::Left);
        let mut y = y + title_h + 8.0;
        if !body.is_empty() {
            let br = R::new(x, y, w, ask_h(body_full));
            c.hit(br, NOTIF_BODY);
            let f = txt(13.0, 400);
            self.body_scroll.region(c, cx, br, body_full, |c, top| c.para(&body, R::new(br.x, top, br.w - 8.0, body_full), f, t.c("onSurface"), Align::Left));
            y += br.h + 12.0;
        }
        // Back on the left; Open (the app that sent it, if it names one) and Dismiss on the right
        let (ld, lo) = ("Dismiss", "Open");
        let wd = chip_w(c, ld, false);
        ui::chip(c, cx, x, y, NOTIF_BACK, Some("arrow_back"), "Back", false);
        ui::chip(c, cx, x + w - wd, y, NOTIF_DISMISS, None, ld, false);
        if !s(n, "open").is_empty() {
            let wo = chip_w(c, lo, true);
            ui::chip(c, cx, x + w - wd - 8.0 - wo, y, NOTIF_OPEN, Some("open_in_new"), lo, true);
        }
    }

    /// Height of a notification's title and body when they wrap to the panel width.
    fn notif_text_h(g: &Gfx, n: &Value) -> (f32, f32) {
        let w = W - 2.0 * PAD;
        (g.measure_wrapped(&s(n, "title"), txt(13.0, 500), w), g.measure_wrapped(&s(n, "body"), txt(13.0, 400), w - 8.0))
    }

    /// The app target of the open notification (what Open launches), "" if none.
    fn open_target(&self) -> String { self.notif_open.as_ref().map(|n| s(n, "open")).unwrap_or_default() }

    /// A banner click leaves the newest notification in "zor-ui.notifOpen" (see bar.rs); it opens here, so you see what it said.
    fn take_banner_notif(&mut self) {
        let Some(t) = hub::get("zor-ui.notifOpen") else { return };
        hub::set("zor-ui.notifOpen", None);
        if let Ok(n) = serde_json::from_str::<Value>(&t) { self.notif_open = Some(n); self.body_scroll = Default::default(); }
    }

    fn fetch_notifs(&mut self, cx: &Ctx) {
        if self.asked || !dp(cx, "notifications") { return; }
        self.asked = true;
        let (cfg, slot) = (Config(cx.cfg.0.clone()), self.notifs.clone());
        std::thread::spawn(move || {
            let r = crate::proc::bridge(&cfg, "notifications", &[]);
            if let (Some(items), Ok(mut n)) = (r["items"].as_array(), slot.lock()) { *n = items.clone(); }
            app::post(Box::new(|_| {})); // repaint
        });
    }
}

impl View for Island {
    fn size(&self, g: &Gfx, cx: &Ctx, _: f32) -> (f32, f32) {
        (W + 24.0, Self::content_h(&self.plan(g, cx)) + 2.0 * PAD + 24.0)
    }
    fn key(&self, _: &Ctx) -> String { format!("{}", (hub::now_ms() / 1000.0) as u64) }
    fn frame_ms(&self) -> u32 {
        self.frames.get()
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.take_banner_notif();
        self.fetch_notifs(cx);
        let plan = self.plan(c.g, cx);
        let (h, x) = (Self::content_h(&plan) + 2.0 * PAD, 12.0 + PAD);
        ui::panel(c, cx, R::new(12.0, 12.0, W, h), 28.0);
        let lyr = Self::lyrics_h(cx);
        let mut y = 12.0 + PAD;
        let asking = |p: &(Sec, f32)| matches!(p.0, Sec::Ask(_) | Sec::Permit(..));
        if !plan.iter().any(asking) && !self.reply.text.is_empty() { self.reply.set(""); self.reply_focus = false; } // answered by voice: the box starts empty next time
        self.frames.set(if plan.iter().any(|p| matches!(p.0, Sec::Media)) && cx.media.as_ref().is_some_and(|m| m.playing) { 50 } else if plan.iter().any(|p| matches!(p.0, Sec::Rec)) { 100 }
            else if (self.reply_focus || crate::pill::active()) && plan.iter().any(asking) { 500 } else { 0 }); // the reply caret blinks, the mic fills while listening
        if plan.is_empty() {
            c.text("Nothing to show. Pick sections in Settings > Dynamic Island", R::new(x, y, W - 2.0 * PAD, 16.0), txt(12.5, 400), cx.theme.c("onSurfaceVariant"), Align::Left);
        }
        for (i, (sec, sh)) in plan.iter().enumerate() {
            if i > 0 {
                c.rect(R::new(x, y - 12.5, W - 2.0 * PAD, 1.0), 0.0, alpha(cx.theme.c("outlineVariant"), 0.4));
            }
            match sec {
                Sec::Ask(q) => self.ask(c, cx, x, y, q, None),
                Sec::Permit(head, q) => self.ask(c, cx, x, y, q, Some(head.as_str())),
                Sec::Media => self.now_playing(c, cx, x, y, lyr),
                Sec::Rec => self.rec(c, cx, x, y),
                Sec::Timer => self.timer(c, cx, x, y),
                Sec::Idle => self.idle(c, cx, x, y),
                Sec::Agents(l) => self.agents(c, cx, x, y, l),
                Sec::Notifs(l) => self.notifs(c, cx, x, y, l),
                Sec::NotifDetail(n) => self.notif_detail(c, cx, x, y, n),
            }
            y += sh + SEP;
        }
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        let id = cx.media.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        match ev {
            Input::Click(i, Btn::Left) => match i {
                ALLOW | DENY => {
                    crate::drive::answer(i == ALLOW);
                    cx.actions.push(Action::Close("island-panel".into()));
                }
                PERMIT_ALLOW | PERMIT_DENY => {
                    if let Some((_, session, _)) = crate::bridge::permit::pending() { crate::bridge::permit::decide(&session, i == PERMIT_ALLOW, ""); }
                    cx.actions.push(Action::Close("island-panel".into()));
                }
                ASK_SEND => self.send_reply(cx),
                // the same recording as the hotkey; while a question waits, what you say answers it (assist::listen)
                ASK_MIC => cx.actions.push(Action::Command("buddy listen".into())),
                PREV => media::control_of(&id, "previous"),
                PLAY => media::control_of(&id, "togglePlayPause"),
                NEXT => media::control_of(&id, "next"),
                REC_STOP => {
                    hub::set("zor-ui.rec", None);
                    let cfg = Config(cx.cfg.0.clone());
                    std::thread::spawn(move || { crate::proc::bridge(&cfg, "record", &["stop"]); });
                    cx.actions.push(Action::Close("island-panel".into()));
                }
                TIMER_A => timer_act(cx, if self.tkind.1 { "next" } else { "toggle" }, self.tkind.0),
                TIMER_B => timer_act(cx, "reset", self.tkind.0),
                AGENT_CLEAR => {
                    let cfg = Config(cx.cfg.0.clone());
                    std::thread::spawn(move || { crate::proc::bridge(&cfg, "agents-dismiss", &[]); });
                }
                CLEAR => {
                    let ids: Vec<String> = self.notifs.lock().map(|mut n| n.drain(..).filter_map(|i| i["id"].as_str().map(String::from)).collect()).unwrap_or_default();
                    let cfg = Config(cx.cfg.0.clone());
                    std::thread::spawn(move || { let a: Vec<&str> = ids.iter().map(String::as_str).collect(); crate::proc::bridge(&cfg, "notifications-dismiss", &a); });
                }
                NOTIF_BACK => self.notif_open = None,
                NOTIF_OPEN => {
                    let open = self.open_target();
                    if !open.is_empty() { crate::bridge::sys::open_and_point(&open); }
                    cx.actions.push(Action::Close("island-panel".into()));
                }
                NOTIF_DISMISS => {
                    if let Some(n) = self.notif_open.take() {
                        let id = s(&n, "id");
                        if let Ok(mut l) = self.notifs.lock() { l.retain(|x| x["id"].as_str() != Some(id.as_str())); }
                        let cfg = Config(cx.cfg.0.clone());
                        std::thread::spawn(move || { crate::proc::bridge(&cfg, "notifications-dismiss", &[id.as_str()]); });
                    }
                }
                i if i >= NOTIF => {
                    self.notif_open = self.notifs.lock().ok().and_then(|n| n.get((i - NOTIF) as usize).cloned());
                    self.body_scroll = Default::default();
                }
                _ => {}
            },
            Input::Drag(SEEK, x, _, ph) => {
                if let Some(m) = cx.media.as_ref().filter(|m| m.duration > 0.0) {
                    let v = ui::slider_value(self.seek, x);
                    self.scrub.set(v, ph == Phase::End);
                    if ph == Phase::End { media::seek(&m.id, v as f64 * m.duration); }
                }
            }
            Input::Wheel(LYRICS, d) => self.scroll.wheel(d, self.plain_h, LYR_H),
            Input::Wheel(ASK_Q, d) => self.ask_scroll.wheel(d, self.ask_full, lh(13.0) * ASK_MAX_LINES),
            Input::Wheel(NOTIF_BODY, d) => self.body_scroll.wheel(d, self.body_full, lh(13.0) * ASK_MAX_LINES),
            Input::Drag(ASK_BOX, x, y, Phase::Start) => { self.reply.press(x, y); self.reply_focus = true; }
            Input::Focus(f) => self.reply_focus = f && self.reply_focus,
            Input::Key { vk, ctrl, shift, alt } if self.reply_focus => {
                if self.reply.key(vk, ctrl, shift, alt) == ui::Edit::Submit { self.send_reply(cx); }
            }
            Input::Char(ch) if self.reply_focus => { self.reply.ch(ch); }
            _ => {}
        }
    }
}

impl Island {
    /// Sends what you typed to the buddy as your answer, and closes the panel. An empty box sends nothing.
    fn send_reply(&mut self, cx: &mut Ctx) {
        if self.reply.text.trim().is_empty() { return; }
        // the buddy's question first (it shows first); otherwise the words answer the Claude Code permission
        if crate::drive::waiting() || !crate::bridge::permit::answer_with(&self.reply.text) { crate::drive::reply(&self.reply.text); }
        self.reply.set("");
        self.reply_focus = false;
        cx.actions.push(Action::Close("island-panel".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_area_shrinks_to_the_note_when_there_are_no_lyrics() {
        let player = media::Media { id: "Spotify".into(), ..Default::default() };
        let tab = media::Media { id: "chrome".into(), ..Default::default() };
        assert_eq!(Island::lyrics_h_for(&Lyr::Missing, &player), ROW); // the note only
        assert_eq!(Island::lyrics_h_for(&Lyr::Missing, &tab), 0.0); // browser tab: nothing at all
        assert_eq!(Island::lyrics_h_for(&Lyr::Searching, &player), LYR_H);
        assert_eq!(Island::lyrics_h_for(&Lyr::Found(std::sync::Arc::new(Lyrics::Plain(vec![]))), &player), LYR_H);
    }

    #[test]
    fn the_question_gets_all_its_lines_up_to_the_cap() {
        let one = lh(13.0);
        assert_eq!(ask_h(0.0), one, "an empty question still keeps one line");
        assert_eq!(ask_h(one * 3.0), one * 3.0, "a long question is not cut at four lines any more");
        assert_eq!(ask_h(one * 30.0), one * ASK_MAX_LINES, "past the cap it scrolls");
        assert_eq!(ask_section_h(one * 30.0) - ask_section_h(one * ASK_MAX_LINES), 0.0);
        assert!(ask_section_h(one * 3.0) > ask_section_h(one), "the panel grows with the question");
    }

    #[test]
    fn the_mic_button_starts_the_same_listening_as_the_hotkey() {
        let mut cx = Ctx::new(false);
        Island::new((0.0, 0.0)).input(Input::Click(ASK_MIC, Btn::Left), &mut cx);
        assert!(matches!(cx.actions.as_slice(), [Action::Command(c)] if c == "buddy listen"), "{}", cx.actions.len());
    }

    #[test]
    fn ago_units() {
        assert_eq!(ago(1000.0, 980.0), "now");
        assert_eq!(ago(1000.0, 1000.0 - 125.0), "2m");
        assert_eq!(ago(100000.0, 100000.0 - 7300.0), "2h");
        assert_eq!(ago(1_000_000.0, 1_000_000.0 - 200_000.0), "2d");
    }

    #[test]
    fn pomodoro_toggle() {
        let mut t = json!({ "running": false, "left": null });
        pomo_toggle(&mut t, 90.0, 1000.0);
        assert_eq!((t["running"].clone(), t["endsAt"].as_f64()), (json!(true), Some(91000.0)));
        pomo_toggle(&mut t, 60.0, 31000.0);
        assert_eq!((t["running"].clone(), t["left"].as_f64()), (json!(false), Some(60.0)));
    }

    #[test]
    fn stopwatch_accumulates() {
        let mut t = json!({ "sw": { "running": false, "start": 0, "acc": 500.0 } });
        sw_toggle(&mut t, 1000.0);
        sw_toggle(&mut t, 3000.0);
        assert_eq!(t["sw"]["acc"].as_f64(), Some(2500.0));
    }
}
