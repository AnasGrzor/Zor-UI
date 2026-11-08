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

const W: f32 = 400.0; // the panel; the window adds 12 px each side for the shadow
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
/// Lines the buddy's question gets.
const ASK_LINES: f32 = 4.0;
const ALLOW: u32 = 10;
const DENY: u32 = 11;
const AGENT_CLEAR: u32 = 12;
const NOTIF: u32 = 100;

enum Sec { Ask(String), Media, Rec, Timer, Idle, Agents(Vec<Value>), Notifs(Vec<Value>) }

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
}

impl Island {
    /// `area`: unused, the panel sizes itself.
    pub fn new(_area: (f32, f32)) -> Island {
        Island { notifs: Default::default(), asked: false, art: None, ly: 0.0, ly_key: String::new(), scroll: Default::default(), plain_h: 0.0,
            seek: R::new(0.0, 0.0, 1.0, 22.0), scrub: Default::default(), tkind: (false, false), frames: Cell::new(0) }
    }
}

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
    fn plan(&self, cx: &Ctx) -> Vec<(Sec, f32)> {
        let mut v = vec![];
        // the buddy wants a yes or no before he drives (drive.rs): first, always
        if let Some(q) = crate::drive::pending() { v.push((Sec::Ask(q), 16.0 + 10.0 + lh(13.0) * ASK_LINES + 12.0 + 32.0)); }
        let m = cx.media.as_ref().filter(|m| !m.title.is_empty());
        let ly = Self::lyrics_h(cx);
        if dp(cx, "media") && m.is_some() { v.push((Sec::Media, 92.0 + 22.0 + 60.0 + 20.0 + if ly > 0.0 { 10.0 + ly } else { 0.0 })); }
        if dp(cx, "recording") && hub::get("zor-ui.rec").and_then(|r| r.parse::<f64>().ok()).is_some_and(|r| r > 0.0) { v.push((Sec::Rec, 44.0)); }
        let t = hub::timer();
        let done = hub::get("zor-ui.timerDone").is_some_and(|d| !d.is_empty());
        if dp(cx, "timer") && (done || t["running"].as_bool() == Some(true) || t["sw"]["running"].as_bool() == Some(true)) { v.push((Sec::Timer, 16.0 + 10.0 + 44.0)); }
        if dp(cx, "clock") && v.is_empty() { v.push((Sec::Idle, 72.0)); }
        if dp(cx, "agents") {
            let list = hub::get_json("zor-ui.agents").as_array().cloned().unwrap_or_default();
            if !list.is_empty() {
                let h = 16.0 + list.iter().map(|a| 10.0 + 18.0 + if a["state"] == "waiting" { 6.0 + lh(12.5) } else { 0.0 }
                    + a["usage"].as_object().map_or(0.0, |u| u.len() as f32 * (6.0 + lh(12.0)))).sum::<f32>();
                v.push((Sec::Agents(list), h));
            }
        }
        if dp(cx, "notifications") {
            let n = self.notifs.lock().map(|n| n.clone()).unwrap_or_default();
            let count = cx.cfg.get("island.panel.notifCount").as_u64().unwrap_or(3) as usize;
            let n: Vec<Value> = n.into_iter().take(count).collect();
            let h = if n.is_empty() { 16.0 + 10.0 + lh(12.5) } else {
                22.0 + n.iter().map(|x| 10.0 + 16.0 + lh(13.0) * if s(x, "body").is_empty() { 1.0 } else { 2.0 }).sum::<f32>()
            };
            v.push((Sec::Notifs(n), h));
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
        ui::icon_btn(c, cx, R::new(cx0, cy + 8.0, 44.0, 44.0), PREV, "skip_previous", ui::IconKind::Plain, 26.0, true);
        let big = R::new(cx0 + 54.0, cy, 60.0, 60.0);
        c.rect(big, 20.0, t.c("primary"));
        if c.hit(big, PLAY) { c.rect(big, 20.0, alpha(t.c("onPrimary"), 0.08)); }
        c.icon(if m.playing { "pause" } else { "play_arrow" }, big, ico(30.0, true), t.c("onPrimary"));
        ui::icon_btn(c, cx, R::new(cx0 + 124.0, cy + 8.0, 44.0, 44.0), NEXT, "skip_next", ui::IconKind::Plain, 26.0, true);
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

    /// The buddy's question with Allow and Deny. Answering is the only way a take-over starts or a risky click goes through.
    fn ask(&mut self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, q: &str) {
        let (t, w) = (&cx.theme, W - 2.0 * PAD);
        c.icon("smart_toy", R::new(x, y, 16.0, 16.0), ico(16.0, false), t.c("tertiary"));
        c.text("The buddy is asking", R::new(x + 22.0, y, w - 22.0, 16.0), txt(12.0, 500), t.c("tertiary"), Align::Left);
        let y = y + 26.0;
        c.para(q, R::new(x, y, w, lh(13.0) * ASK_LINES), txt(13.0, 400), t.c("onSurface"), Align::Left);
        let y = y + lh(13.0) * ASK_LINES + 12.0;
        let (la, ld) = ("Allow", "Deny");
        let (wa, wd) = (chip_w(c, la, true), chip_w(c, ld, true));
        ui::chip(c, cx, x + w - wa - 8.0 - wd, y, ALLOW, Some("check"), la, true);
        ui::chip(c, cx, x + w - wd, y, DENY, Some("close"), ld, false);
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
    fn size(&self, _: &Gfx, cx: &Ctx, _: f32) -> (f32, f32) {
        (W + 24.0, Self::content_h(&self.plan(cx)) + 2.0 * PAD + 24.0)
    }
    fn key(&self, _: &Ctx) -> String { format!("{}", (hub::now_ms() / 1000.0) as u64) }
    fn frame_ms(&self) -> u32 {
        self.frames.get()
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.fetch_notifs(cx);
        let plan = self.plan(cx);
        let (h, x) = (Self::content_h(&plan) + 2.0 * PAD, 12.0 + PAD);
        ui::panel(c, cx, R::new(12.0, 12.0, W, h), 28.0);
        let lyr = Self::lyrics_h(cx);
        let mut y = 12.0 + PAD;
        self.frames.set(if plan.iter().any(|p| matches!(p.0, Sec::Media)) && cx.media.as_ref().is_some_and(|m| m.playing) { 50 } else if plan.iter().any(|p| matches!(p.0, Sec::Rec)) { 100 } else { 0 });
        if plan.is_empty() {
            c.text("Nothing to show. Pick sections in Settings > Dynamic Island", R::new(x, y, W - 2.0 * PAD, 16.0), txt(12.5, 400), cx.theme.c("onSurfaceVariant"), Align::Left);
        }
        for (i, (sec, sh)) in plan.iter().enumerate() {
            if i > 0 {
                c.rect(R::new(x, y - 12.5, W - 2.0 * PAD, 1.0), 0.0, alpha(cx.theme.c("outlineVariant"), 0.4));
            }
            match sec {
                Sec::Ask(q) => self.ask(c, cx, x, y, q),
                Sec::Media => self.now_playing(c, cx, x, y, lyr),
                Sec::Rec => self.rec(c, cx, x, y),
                Sec::Timer => self.timer(c, cx, x, y),
                Sec::Idle => self.idle(c, cx, x, y),
                Sec::Agents(l) => self.agents(c, cx, x, y, l),
                Sec::Notifs(l) => self.notifs(c, cx, x, y, l),
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
                i if i >= NOTIF => {
                    hub::set("zor-ui.rightTab", Some("notif".into()));
                    // the sidebar scrolls to this notification (see sidebar_right/pages.rs FOCUS)
                    hub::set("zor-ui.notifFocus", self.notifs.lock().ok().and_then(|n| n.get((i - NOTIF) as usize).and_then(|n| n["id"].as_str().map(|id| format!("{}:{id}", hub::now_ms() as u64)))));
                    cx.actions.push(Action::open("sidebar-right"));
                    cx.actions.push(Action::Close("island-panel".into()));
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
            _ => {}
        }
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
