//! The bar (bar.html / bar.js) and its hover popups (bar-popup.html), one bar per monitor.

use crate::{
    app::{Action, Ctx},
    gfx::{alpha, ico, txt, Align, Canvas, Color, Font, Gfx, Pixmap, R},
    hub, media, proc, sys,
    util::{self, date_fmt},
    weather,
    win::{Btn, Input, View},
};
use serde_json::Value;
use std::{collections::HashMap, time::Instant};

const LH: f32 = 1.252;
const BAR_H: f32 = 56.0;
const BAR_W: f32 = 68.0; // a bar on the left or right edge
const GH: f32 = 34.0; // .group height

/// Which monitor edge the bar sits on (config "bar.position").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge { Top, Bottom, Left, Right }

impl Edge {
    pub fn parse(s: &str) -> Edge { match s { "bottom" => Edge::Bottom, "left" => Edge::Left, "right" => Edge::Right, _ => Edge::Top } }
    pub fn vertical(self) -> bool { matches!(self, Edge::Left | Edge::Right) }
    /// How much of the monitor the bar takes, in CSS px.
    pub fn thick(self) -> f32 { if self.vertical() { BAR_W } else { BAR_H } }
    pub fn abe(self) -> u32 {
        use windows::Win32::UI::Shell::{ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP};
        match self { Edge::Top => ABE_TOP, Edge::Bottom => ABE_BOTTOM, Edge::Left => ABE_LEFT, Edge::Right => ABE_RIGHT }
    }
}

/// The `h` px thick strip along `edge` of `area`.
pub fn strip(area: windows::Win32::Foundation::RECT, edge: Edge, h: i32) -> windows::Win32::Foundation::RECT {
    let mut r = area;
    match edge { Edge::Top => r.bottom = r.top + h, Edge::Bottom => r.top = r.bottom - h, Edge::Left => r.right = r.left + h, Edge::Right => r.left = r.right - h }
    r
}

/// Where a popup `size` px big goes next to the bar group `r` (CSS px in a bar window at `bar`): under it, over it, or beside it.
pub fn popup_spot(edge: Edge, bar: (i32, i32), r: R, s: f32, size: (i32, i32)) -> (i32, i32) {
    let at = |v: f32, o: i32| (o as f32 + v * s).round() as i32;
    let (cx, cy) = (at(r.x + r.w / 2.0, bar.0), at(r.y + r.h / 2.0, bar.1));
    let gap = (4.0 * s).round() as i32;
    match edge {
        Edge::Top => ((cx - size.0 / 2).max(bar.0), at(r.y + r.h, bar.1) + gap),
        Edge::Bottom => ((cx - size.0 / 2).max(bar.0), at(r.y, bar.1) - gap - size.1),
        Edge::Left => (at(r.x + r.w, bar.0) + gap, (cy - size.1 / 2).max(bar.1)),
        Edge::Right => (at(r.x, bar.0) - gap - size.0, (cy - size.1 / 2).max(bar.1)),
    }
}

/// The point a panel hangs from (physical px): the middle of the group's face that looks into the screen.
pub fn anchor(edge: Edge, bar: (i32, i32), r: R, s: f32) -> (i32, i32) {
    let at = |v: f32, o: i32| (o as f32 + v * s).round() as i32;
    let gap = 2.0;
    match edge {
        Edge::Top => (at(r.x + r.w / 2.0, bar.0), at(r.y + r.h + gap, bar.1)),
        Edge::Bottom => (at(r.x + r.w / 2.0, bar.0), at(r.y - gap, bar.1)),
        Edge::Left => (at(r.x + r.w + gap, bar.0), at(r.y + r.h / 2.0, bar.1)),
        Edge::Right => (at(r.x - gap, bar.0), at(r.y + r.h / 2.0, bar.1)),
    }
}

// Clickable spots (hit ids). Lists (workspaces, tray, ...) add the item index.
pub const LAUNCHER: u32 = 1; const WINDOW: u32 = 2; pub const RES: u32 = 3; const MEDIA: u32 = 4; const ISLAND: u32 = 5;
pub const CLOCK: u32 = 7; const TIMER: u32 = 8; const REC: u32 = 9; pub const WEATHER: u32 = 11; pub const NET: u32 = 13; pub const SYS: u32 = 15;
const WS: u32 = 0x100; const UTIL: u32 = 0x200; const WMC: u32 = 0x300; const TRAY: u32 = 0x400; const BADGE: u32 = 0x500; const CTL: u32 = 0x600;
/// Groups with a hover popup (POPUPS in bar.js).
pub const POPUPS: [u32; 5] = [CLOCK, RES, WEATHER, NET, SYS];

/// barLayout() in lib/barlayout.js: [(id, side)] in order, unknown ids dropped, missing ones added.
pub fn layout(cfg: &crate::conf::Config) -> Vec<(String, String)> {
    const MODULES: [(&str, &str); 13] = [("launcher", "left"), ("window", "left"), ("resources", "center"), ("media", "center"), ("island", "center"),
        ("workspaces", "center"), ("clock", "center"), ("utils", "center"), ("weather", "center"), ("wm", "right"), ("netspeed", "hidden"), ("tray", "right"), ("system", "right")];
    let sides = ["left", "center", "right", "hidden"];
    let mut list: Vec<(String, String)> = vec![];
    for m in cfg.get("bar.modules").as_array().into_iter().flatten() {
        let (Some(id), Some(side)) = (m[0].as_str(), m[1].as_str()) else { continue };
        if MODULES.iter().any(|k| k.0 == id) && sides.contains(&side) && !list.iter().any(|x| x.0 == id) { list.push((id.into(), side.into())); }
    }
    for (id, side) in MODULES {
        if !list.iter().any(|x| x.0 == id) {
            let side = if id == "netspeed" && cfg.bool("showNetSpeed") { "right" } else { side };
            list.push((id.into(), side.into()));
        }
    }
    list
}

/// prettyApp() in core.js.
pub fn pretty_app(name: &str) -> String {
    let n = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    match n {
        "msedge" => "Microsoft Edge".into(), "explorer" => "File Explorer".into(), "WindowsTerminal" => "Terminal".into(),
        "Code" => "Visual Studio Code".into(), "chrome" => "Google Chrome".into(), "firefox" => "Firefox".into(), "ApplicationFrameHost" => "App".into(),
        _ => { let mut c = n.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }
    }
}

/// The workspaces of `1..=n` one monitor shows: the first `main_count` on the main monitor, the rest on the other.
pub fn split_workspaces(n: u32, main_count: u32, main: bool) -> Vec<u32> {
    (1..=n).filter(|&i| (i <= main_count) == main).collect()
}

/// volumeIcon() in core.js.
pub fn volume_icon(a: Option<&media::Audio>) -> &'static str {
    match a {
        None => "volume_off",
        Some(a) if a.muted || a.volume < 0.5 => "volume_off",
        Some(a) if a.volume < 34.0 => "volume_mute",
        Some(a) if a.volume < 67.0 => "volume_down",
        _ => "volume_up",
    }
}

/// CSS cubic-bezier(x1, y1, x2, y2) at progress x.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let f = |a: f32, b: f32, t: f32| 3.0 * a * t * (1.0 - t).powi(2) + 3.0 * b * t * t * (1.0 - t) + t.powi(3);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 { let m = (lo + hi) / 2.0; if f(x1, x2, m) < x { lo = m } else { hi = m } }
    f(y1, y2, (lo + hi) / 2.0)
}

/// The island's width spring (transition: width 0.4s cubic-bezier(0.34, 1.35, 0.64, 1)).
#[derive(Default)]
struct Spring { from: f32, to: f32, t0: Option<Instant> }
impl Spring {
    fn value(&self) -> f32 {
        let Some(t0) = self.t0 else { return self.to };
        let x = (t0.elapsed().as_secs_f32() / 0.4).min(1.0);
        self.from + (self.to - self.from) * bezier(0.34, 1.35, 0.64, 1.0, x)
    }
    fn moving(&self) -> bool { self.t0.is_some_and(|t| t.elapsed().as_secs_f32() < 0.4) }
    fn set(&mut self, to: f32) {
        if (to - self.to).abs() < 0.5 { return; }
        self.from = if self.to == 0.0 { to } else { self.value() };
        self.to = to;
        self.t0 = (self.from != to).then(Instant::now);
    }
}

/// What a group holds, laid out left to right.
enum El {
    Ico(&'static str, f32, bool, Color),
    IcoS(String, f32, bool, Color),
    Txt(String, Font, Color, f32), // max width
    Ring { size: f32, value: f32, icon: &'static str, isize: f32, fill: bool, color: Color, art: Option<Pixmap> },
    Btn { id: u32, w: f32, h: f32, label: Box<El>, bg: Option<Color>, round: f32 },
    Row(Vec<El>, f32), // inline items with a gap (a chip's icon + text)
    Dot(f32, Color),
    Meter(f32, f32), // width, value 0-100 (.osd-bar)
    Img(u32, Pixmap, f32), // hit id, picture, size
    Col(Vec<(String, Font, Color)>, f32), // stacked lines (active window, net speed), max width
}

struct Pill { id: u32, els: Vec<El>, pad: (f32, f32), gap: f32, bg: Option<Color>, hover: bool, fixed: Option<f32>, kind: Kind }
#[derive(Clone, Copy, PartialEq)]
enum Kind { Group, Plain, Aw }

pub struct Bar {
    /// This bar's monitor (physical px): x, y, width, height.
    pub mon: (i32, i32, i32, i32),
    pub edge: Edge,
    /// Top-left of the bar window (physical px).
    pub origin: (i32, i32),
    island: Spring,
    di_hover: bool,
    di_focus: String,
    di_track: String,
    di_track_until: f64,
    di_scroll_at: f64,
    art: Option<((std::sync::Arc<Vec<u8>>, u32), Pixmap)>,
    tray_px: HashMap<String, (usize, Pixmap)>,
    pills: Vec<(u32, R)>,
    hover: u32,
    ws_names: Vec<String>,
    wm_cmds: Vec<String>,
    tray_ids: Vec<String>,
    utils: Vec<String>,
    /// A pulsing dot is showing (recording, an agent waiting).
    pulsing: bool,
}

impl Bar {
    pub fn new(mon: (i32, i32, i32, i32), edge: Edge, origin: (i32, i32)) -> Bar {
        Bar { mon, edge, origin, island: Spring::default(), di_hover: false, di_focus: String::new(), di_track: String::new(), di_track_until: 0.0, di_scroll_at: 0.0,
            art: None, tray_px: HashMap::new(), pills: vec![], hover: 0, ws_names: vec![], wm_cmds: vec![], tray_ids: vec![], utils: vec![], pulsing: false }
    }

    fn monitor<'a>(&self, cx: &'a Ctx) -> Option<&'a crate::glaze::Monitor> { cx.wm.monitor_at(self.mon.0, self.mon.1) }

    /// myWorkspaces() in bar.js: the first `workspacesMain` (default 5) on the main monitor and the rest on the other when there are two.
    fn my_workspaces(&self, cx: &Ctx) -> Vec<u32> {
        let n = cx.cfg.get("workspaces").as_u64().unwrap_or(10) as u32;
        if cx.wm.monitors.len() < 2 { return (1..=n).collect(); }
        let main = self.monitor(cx).map_or(true, |m| m.x == 0 && m.y == 0);
        split_workspaces(n, cx.cfg.get("workspacesMain").as_u64().unwrap_or(5) as u32, main)
    }

    /// Opens a popup on this bar's monitor (openHere).
    fn open_here(&self, cx: &mut Ctx, name: &str) {
        cx.actions.push(Action::Open { name: name.into(), mon: Some((self.mon.0, self.mon.1)), at: None });
    }

    fn art(&mut self, g: &Gfx, cx: &Ctx, px: u32) -> Option<Pixmap> {
        // keyed on the bytes, not the title: Windows can publish a track's cover after its title
        let b = cx.media.as_ref()?.art.as_ref()?;
        // holding the Arc keeps its address from being reused by the next cover
        if !self.art.as_ref().is_some_and(|a| std::sync::Arc::ptr_eq(&a.0 .0, b) && a.0 .1 == px) {
            self.art = Some(((b.clone(), px), g.load_image_bytes(b, px, px)?));
        }
        self.art.as_ref().map(|a| a.1.clone())
    }

    fn tray_icon(&mut self, i: &crate::tray::Icon, px: u32) -> Option<Pixmap> {
        let rgba = i.rgba.as_ref()?;
        let tag = std::sync::Arc::as_ptr(rgba) as usize ^ px as usize;
        if self.tray_px.get(&i.id).map(|e| e.0) != Some(tag) {
            self.tray_px.insert(i.id.clone(), (tag, rgba_to_pixmap(&rgba.0, rgba.1, rgba.2, px)?));
        }
        self.tray_px.get(&i.id).map(|e| e.1.clone())
    }
}

/// Straight RGBA icon -> premultiplied BGRA pixmap scaled to `px`.
fn rgba_to_pixmap(rgba: &[u8], w: u32, h: u32, px: u32) -> Option<Pixmap> {
    let mut src = Pixmap::new(w, h)?;
    for (d, s) in src.data_mut().chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = (s[2] as u32 * a / 255) as u8; d[1] = (s[1] as u32 * a / 255) as u8; d[2] = (s[0] as u32 * a / 255) as u8; d[3] = s[3];
    }
    let mut out = Pixmap::new(px, px)?;
    let t = tiny_skia::Transform::from_scale(px as f32 / w as f32, px as f32 / h as f32);
    out.draw_pixmap(0, 0, src.as_ref(), &tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() }, t, None);
    Some(out)
}

fn el_w(g: &Gfx, e: &El) -> f32 {
    match e {
        El::Ico(_, s, ..) | El::IcoS(_, s, ..) => *s,
        El::Txt(t, f, _, max) => g.measure(t, *f).0.min(*max).ceil(),
        El::Ring { size, .. } => *size,
        El::Btn { w, .. } => *w,
        El::Row(items, gap) => items.iter().map(|e| el_w(g, e)).sum::<f32>() + gap * items.len().saturating_sub(1) as f32,
        El::Dot(s, _) => *s,
        El::Meter(w, _) => *w,
        El::Img(_, _, s) => *s + 6.0,
        El::Col(lines, max) => lines.iter().map(|(t, f, _)| g.measure(t, *f).0).fold(0.0, f32::max).min(*max).ceil(),
    }
}

fn draw_el(c: &mut Canvas, cx: &Ctx, e: &El, x: f32, cy: f32, hover: u32) {
    let t = &cx.theme;
    let w = el_w(c.g, e);
    match e {
        El::Ico(n, s, fill, col) => c.icon(n, R::new(x, cy - s / 2.0, *s, *s), ico(*s, *fill), *col),
        El::IcoS(n, s, fill, col) => c.icon(n, R::new(x, cy - s / 2.0, *s, *s), ico(*s, *fill), *col),
        El::Txt(s, f, col, _) => c.text(s, R::new(x, cy - 15.0, w + 1.0, 30.0), *f, *col, Align::Left),
        El::Ring { size, value, icon, isize, fill, color, art } => {
            let (rx, r) = (x + size / 2.0, (size - 2.5) / 2.0);
            c.arc(rx, cy, r, 1.0, 2.5, t.c("secondaryContainer"));
            c.arc(rx, cy, r, *value, 2.5, t.c("primary"));
            match art {
                Some(img) => { let ir = R::new(x + 4.0, cy - size / 2.0 + 4.0, size - 8.0, size - 8.0); c.clip(crate::gfx::Shape::Circle.path(ir), |c| c.image(img, ir, 1.0)); }
                None => c.icon(icon, R::new(x, cy - isize / 2.0, *size, *isize), ico(*isize, *fill), *color),
            }
        }
        El::Btn { id, w, h, label, bg, round } => {
            let r = R::new(x, cy - h / 2.0, *w, *h);
            if let Some(b) = bg { c.rect(r, *round, *b); }
            if c.hit(r, *id) && hover == *id { c.rect(r, *round, alpha(t.c("onSurface"), 0.08)); }
            let lw = el_w(c.g, label);
            draw_el(c, cx, label, x + (w - lw) / 2.0, cy, hover);
        }
        El::Row(items, gap) => {
            let mut x = x;
            for e in items { draw_el(c, cx, e, x, cy, hover); x += el_w(c.g, e) + gap; }
        }
        El::Dot(s, col) => c.circle(x + s / 2.0, cy, s / 2.0, *col),
        El::Meter(w, v) => {
            let fg = t.c("onSecondaryContainer");
            c.rect(R::new(x, cy - 3.0, *w, 6.0), 3.0, alpha(fg, 0.2));
            c.rect(R::new(x, cy - 3.0, w * (v / 100.0).clamp(0.0, 1.0), 6.0), 3.0, t.c("primary"));
        }
        El::Img(id, img, s) => {
            let r = R::new(x, cy - s / 2.0 - 3.0, s + 6.0, s + 6.0);
            if c.hit(r, *id) && hover == *id { c.rect(r, 6.0, alpha(t.c("onSurface"), 0.08)); }
            c.image(img, R::new(x + 3.0, cy - s / 2.0, *s, *s), 1.0);
        }
        El::Col(lines, _) => {
            let h: f32 = lines.iter().map(|(_, f, _)| f.size * 1.15).sum();
            let mut y = cy - h / 2.0;
            for (s, f, col) in lines {
                c.text(s, R::new(x, y, w + 1.0, f.size * 1.15), *f, *col, Align::Left);
                y += f.size * 1.15;
            }
        }
    }
}

fn btn(id: u32, w: f32, icon: &'static str, size: f32, fill: bool, col: Color) -> El {
    El::Btn { id, w, h: w, label: Box::new(El::Ico(icon, size, fill, col)), bg: None, round: w / 2.0 }
}

/// New width for item `i` so a row of `widths` (with `gap`) fits `room`; None if it already fits.
/// Never below `min` (the row then overflows, like CSS min-width).
fn fit(widths: &[f32], i: usize, gap: f32, room: f32, min: f32) -> Option<f32> {
    let total = widths.iter().sum::<f32>() + gap * widths.len().saturating_sub(1) as f32;
    (total > room).then(|| (widths[i] - (total - room)).max(min))
}

fn pill_w(g: &Gfx, p: &Pill) -> f32 {
    if let Some(w) = p.fixed { return w; }
    let inner: f32 = p.els.iter().map(|e| el_w(g, e)).sum::<f32>() + p.gap * p.els.len().saturating_sub(1) as f32;
    p.pad.0 + inner + p.pad.1
}

impl Bar {
    /// timerChip() in bar.js: (done, icon, text); None when no timer runs.
    fn timer_chip(cx: &Ctx) -> Option<(bool, &'static str, String)> {
        if let Some(d) = hub::get("zor-ui.timerDone") { return Some((true, "alarm", if d == "focus" { "Break time".into() } else { "Focus time".into() })); }
        let t = hub::timer();
        if t["running"].as_bool() == Some(true) { return Some((false, if t["phase"] == "focus" { "timer" } else { "coffee" }, hub::clock(hub::left(&t, &cx.cfg)))); }
        if t["sw"]["running"].as_bool() == Some(true) { return Some((false, "timer_play", hub::clock(hub::sw_elapsed(&t)))); }
        None
    }

    fn rec_started() -> f64 { hub::get("zor-ui.rec").and_then(|v| v.parse().ok()).unwrap_or(0.0) }

    /// diProviders() in bar.js: what the island could show, most important first; .2 = wins outright.
    fn island_list(cx: &Ctx) -> Vec<(&'static str, bool)> {
        let di = |k: &str| cx.cfg.get(&format!("island.{k}")).as_bool() != Some(false);
        let agents = hub::get_json("end4.agents");
        let agents = agents.as_array().cloned().unwrap_or_default();
        let asks = di("agents") && agents.iter().any(|a| a["state"] == "waiting");
        let now = hub::now_ms();
        let mut l = vec![];
        if cx.notif.as_ref().is_some_and(|n| n.until > now) { l.push(("notification", true)); }
        if cx.osd.as_ref().is_some_and(|o| o.until > now) { l.push(("osd", true)); }
        if asks { l.push(("agent", false)); }
        if di("recording") && Self::rec_started() > 0.0 { l.push(("recording", false)); }
        if di("timer") && Self::timer_chip(cx).is_some() { l.push(("timer", false)); }
        if di("media") && cx.media.is_some() { l.push(("media", false)); }
        if di("agents") && di("agentWorking") && !asks && agents.iter().any(|a| a["state"] == "working") { l.push(("agent", false)); }
        l
    }

    /// The island's content and its state colours (renderIsland in bar.js).
    fn island(&mut self, c: &mut Canvas, cx: &Ctx, on_bar: &dyn Fn(&str) -> bool) -> Option<Pill> {
        let t = &cx.theme;
        let num = |k: &str, d: f64| cx.cfg.get(&format!("island.{k}")).as_f64().unwrap_or(d) as f32;
        let flag = |k: &str| cx.cfg.get(&format!("island.{k}")).as_bool() != Some(false);
        let list = Self::island_list(cx);
        let win = list.iter().find(|p| p.1).copied();
        let main = win.or_else(|| list.iter().find(|p| p.0 == self.di_focus).copied()).or_else(|| list.first().copied());
        let state = main.map_or("idle", |m| m.0);
        let key = cx.media.as_ref().map(|m| format!("{}|{}", m.title, m.artist)).unwrap_or_default();
        if key != self.di_track {
            if !self.di_track.is_empty() && !key.is_empty() { self.di_track_until = hub::now_ms() + num("trackSeconds", 3.0) as f64 * 1000.0; }
            self.di_track = key;
        }
        let (text_w, wide_w) = (num("textWidth", 170.0), num("wideWidth", 280.0));
        let mut fg = t.c("onSurface");
        let mut bg = None;
        let mut els = vec![];
        self.pulsing = state == "recording";
        match state {
            "notification" => {
                let n = cx.notif.as_ref()?;
                (bg, fg) = (Some(t.c("secondaryContainer")), t.c("onSecondaryContainer"));
                els.push(El::Ico("notifications", 18.0, true, fg));
                els.push(El::Txt(format!("{} · {}", n.app, n.text), txt(13.0, 400), fg, wide_w));
            }
            "agent" => {
                let agents = hub::get_json("end4.agents");
                let agents = agents.as_array().cloned().unwrap_or_default();
                let waiting: Vec<&Value> = agents.iter().filter(|a| a["state"] == "waiting").collect();
                let a = waiting.first().copied().or_else(|| agents.iter().find(|a| a["state"] == "working"))?;
                let s = |v: &Value| v.as_str().unwrap_or("").to_string();
                if !waiting.is_empty() {
                    self.pulsing = true;
                    (bg, fg) = (Some(t.c("tertiaryContainer")), t.c("onTertiaryContainer"));
                    let pulse = 0.25 + 0.75 * (0.5 + 0.5 * (hub::now_ms() as f32 / 900.0 * std::f32::consts::PI).cos());
                    els.push(El::Dot(9.0, alpha(fg, pulse)));
                    els.push(El::Ico("smart_toy", 18.0, true, fg));
                    let msg = a["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("Needs your input");
                    els.push(El::Txt(format!("{} · {msg}", s(&a["label"])), txt(13.0, 400), fg, wide_w));
                    let more = waiting.iter().map(|w| w["waiting"].as_i64().unwrap_or(1)).sum::<i64>() - 1;
                    if more > 0 { els.push(El::Txt(format!("+{more}"), txt(13.0, 400), fg, 60.0)); }
                } else {
                    let u = a["usage"].get("five_hour").or_else(|| a["usage"].as_object().and_then(|o| o.values().next())).cloned();
                    match &u {
                        Some(u) => els.push(El::Ring { size: 28.0, value: u["used_percentage"].as_f64().unwrap_or(0.0) as f32 / 100.0, icon: "smart_toy", isize: 14.0, fill: true, color: fg, art: None }),
                        None => els.push(El::Ico("smart_toy", 18.0, true, fg)),
                    }
                    let pct = u.map(|u| format!(" · {}%", u["used_percentage"].as_f64().unwrap_or(0.0).round())).unwrap_or_default();
                    els.push(El::Txt(format!("{}{pct}", s(&a["label"])), txt(13.0, 400), fg, text_w));
                }
            }
            "osd" => {
                let o = cx.osd.as_ref()?;
                (bg, fg) = (Some(t.c("secondaryContainer")), t.c("onSecondaryContainer"));
                els.push(El::Ico(o.icon, 18.0, true, fg));
                if let Some(v) = o.value { els.push(El::Meter(110.0, v)); }
                els.push(El::Txt(o.text.clone(), txt(13.0, 400), fg, 200.0));
            }
            "recording" => {
                (bg, fg) = (Some(t.c("error")), t.c("onError"));
                let pulse = 0.25 + 0.75 * (0.5 + 0.5 * (hub::now_ms() as f32 / 900.0 * std::f32::consts::PI).cos());
                els.push(El::Dot(9.0, alpha(fg, pulse)));
                els.push(El::Txt(format!("Recording {}", hub::clock((hub::now_ms() - Self::rec_started()) / 1000.0)), txt(13.0, 400), fg, 200.0));
            }
            "timer" => {
                let (done, icon, text) = Self::timer_chip(cx)?;
                if done { (bg, fg) = (Some(t.c("primary")), t.c("onPrimary")); }
                els.push(El::Ico(icon, 18.0, done, fg));
                els.push(El::Txt(text, txt(13.0, 400), fg, 200.0));
            }
            "media" => {
                let m = cx.media.clone()?;
                let wide = (flag("controls") && self.di_hover) || hub::now_ms() < self.di_track_until;
                let art = if flag("art") { self.art(c.g, cx, (20.0 * c.scale).round() as u32) } else { None };
                els.push(El::Ring { size: 28.0, value: if flag("progress") { m.progress() } else { 0.0 }, icon: "music_note", isize: 14.0, fill: true, color: fg, art });
                let title = if wide && !m.artist.is_empty() { format!("{} • {}", m.title, m.artist) } else { m.title.clone() };
                els.push(El::Txt(title, txt(13.0, 400), fg, if wide { wide_w } else { text_w }));
                if wide && flag("controls") {
                    els.push(btn(CTL, 26.0, "skip_previous", 18.0, true, fg));
                    els.push(btn(CTL + 1, 26.0, if m.playing { "pause" } else { "play_arrow" }, 18.0, true, fg));
                    els.push(btn(CTL + 2, 26.0, "skip_next", 18.0, true, fg));
                }
            }
            _ => {
                let st = util::now();
                let idle = cx.cfg.str("island.idle");
                let pick = if !idle.is_empty() && idle != "auto" { idle } else if !on_bar("clock") { "time" } else if cx.weather.is_some() && !on_bar("weather") { "weather" } else { "greeting" };
                match pick {
                    "time" => els.push(El::Txt(util::clock(st.wHour, st.wMinute, cx.cfg.get("use24h").as_bool() != Some(false)), txt(13.0, 400), fg, 200.0)),
                    "date" => { els.push(El::Ico("calendar_today", 18.0, false, fg)); els.push(El::Txt(date_fmt(&st, "ddd, MMM d"), txt(13.0, 400), fg, 200.0)); }
                    "weather" => if let Some(w) = &cx.weather { els.push(El::Ico(weather::icon(w.status), 18.0, false, fg)); els.push(El::Txt(weather::temp(w, cx.cfg.str("tempUnit")), txt(13.0, 400), fg, 200.0)); },
                    "none" => {}
                    _ => { els.push(El::Ico("waving_hand", 18.0, false, fg)); els.push(El::Txt(crate::desk::greeting(st.wHour).into(), txt(13.0, 400), fg, 200.0)); }
                }
                if flag("status") {
                    let a = cx.audio.as_ref();
                    for (on, i) in [(a.and_then(|a| a.mic_muted) == Some(true), "mic_off"), (a.is_some_and(|a| a.muted), "volume_off"), (cx.net.is_none(), "wifi_off")] {
                        if on { els.push(El::Ico(i, 16.0, false, t.c("onSurfaceVariant"))); }
                    }
                }
            }
        }
        // badges: the other things going on, click one to show it
        if win.is_none() && flag("badges") {
            for (i, (id, _)) in list.iter().enumerate() {
                if Some(*id) == main.map(|m| m.0) { continue; }
                let icon = match *id { "recording" => "screen_record", "timer" => "timer", "media" => "music_note", _ => "smart_toy" };
                els.push(El::Btn { id: BADGE + i as u32, w: 26.0, h: 26.0, label: Box::new(El::Ico(icon, 15.0, true, t.c("onSecondaryContainer"))), bg: Some(t.c("secondaryContainer")), round: 13.0 });
            }
        }
        if els.is_empty() { return None; }
        let mut p = Pill { id: ISLAND, els, pad: (9.0, 9.0), gap: 7.0, bg, hover: false, fixed: None, kind: Kind::Group };
        let target = pill_w(c.g, &p);
        self.island.set(target);
        p.fixed = Some(self.island.value().max(10.0));
        Some(p)
    }

    /// All groups for one side, in the configured order.
    /// `fit`: the width a group's content gets on a vertical bar (None on a horizontal one).
    fn pills(&mut self, c: &mut Canvas, cx: &Ctx, side: &str, fit: Option<f32>) -> Vec<Pill> {
        let t = &cx.theme;
        let v = fit.is_some();
        let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
        let lay = layout(&cx.cfg);
        let on_bar = |id: &str| lay.iter().any(|m| m.0 == id && m.1 != "hidden");
        let island_on = on_bar("island");
        let island_has = |k: &str| island_on && cx.cfg.get(&format!("island.{k}")).as_bool() != Some(false);
        let group = |id, els, pad: (f32, f32), gap| Pill { id, els, pad, gap, bg: None, hover: false, fixed: None, kind: Kind::Group };
        let mut out = vec![];
        for (id, _) in lay.iter().filter(|m| m.1 == side) {
            match id.as_str() {
                "launcher" => out.push(Pill { kind: Kind::Plain, ..group(LAUNCHER, vec![btn(LAUNCHER, 34.0, "auto_awesome", 22.0, true, t.c("primary"))], (0.0, 0.0), 0.0) }),
                "window" if v => {} // a title doesn't fit a side bar
                "window" => {
                    let (app, title) = match &cx.wm.window {
                        Some((title, process)) => (pretty_app(process), if title.is_empty() { pretty_app(process) } else { title.clone() }),
                        None => ("Desktop".into(), self.monitor(cx).and_then(|m| m.workspaces.iter().find(|w| w.displayed)).map_or("Windows".into(), |w| format!("Workspace {}", w.display))),
                    };
                    let max = c.w * cx.cfg.get("bar.titleWidth").as_f64().unwrap_or(34.0) as f32 / 100.0; // .aw-title max-width: 34vw
                    let pad = if cx.cfg.str("bar.style") == "islands" { (16.0, 16.0) } else { (6.0, 6.0) };
                    // one line, like the clock pill: "App • Title"
                    let text = if title == app { app } else { format!("{app} • {title}") };
                    out.push(Pill { kind: Kind::Aw, ..group(WINDOW, vec![El::Txt(text, txt(13.5, 400), fg, max)], pad, 0.0) });
                }
                "resources" => {
                    let ring = |v: f32, icon| El::Ring { size: 24.0, value: v, icon, isize: 13.0, fill: false, color: t.c("onSecondaryContainer"), art: None };
                    out.push(group(RES, vec![ring(cx.cpu, "memory"), El::Txt(format!("{}", (cx.cpu * 100.0).round()), txt(13.0, 400), fg, 40.0),
                        ring(cx.mem, "memory_alt"), El::Txt(format!("{}", (cx.mem * 100.0).round()), txt(13.0, 400), fg, 40.0)], (10.0, 10.0), 4.0));
                }
                "media" if !island_has("media") => {
                    if let Some(o) = cx.osd.as_ref().filter(|o| o.until > hub::now_ms()) {
                        let f = t.c("onSecondaryContainer");
                        let mut els = vec![El::Ico(o.icon, 20.0, true, f)];
                        if let Some(v) = o.value { els.push(El::Meter(110.0, v)); }
                        els.push(El::Txt(o.text.clone(), txt(13.0, 400), f, 230.0));
                        out.push(Pill { bg: Some(t.c("secondaryContainer")), ..group(MEDIA, els, (12.0, 12.0), 8.0) });
                    } else if let Some(m) = &cx.media {
                        let text = if m.artist.is_empty() { m.title.clone() } else { format!("{} • {}", m.title, m.artist) };
                        out.push(group(MEDIA, vec![El::Ring { size: 26.0, value: m.progress(), icon: if m.playing { "pause" } else { "play_arrow" }, isize: 16.0, fill: true, color: fg, art: None },
                            El::Txt(text, txt(13.0, 400), fg, 230.0)], (5.0, 10.0), 6.0));
                    }
                }
                "island" => { let ob = &on_bar; if let Some(p) = self.island(c, cx, ob) { out.push(p); } }
                "workspaces" => {
                    let mon = self.monitor(cx);
                    let shown = mon.and_then(|m| m.workspaces.iter().find(|w| w.displayed)).map(|w| w.name.clone());
                    let labels = cx.cfg.strs("workspaceLabels");
                    let always = cx.cfg.bool("alwaysShowNumbers");
                    self.ws_names.clear();
                    let mut els = vec![];
                    for i in self.my_workspaces(cx) {
                        let w = cx.wm.monitors.iter().flat_map(|m| &m.workspaces).find(|w| w.name == i.to_string());
                        let occ = w.is_some_and(|w| w.occupied);
                        let active = shown.as_deref() == Some(&i.to_string());
                        let idx = self.ws_names.len() as u32;
                        self.ws_names.push(i.to_string());
                        let label = labels.get(i as usize - 1).filter(|l| !l.is_empty()).cloned().unwrap_or_else(|| i.to_string());
                        let inner = if active || always {
                            let col = if active { t.c("onPrimary") } else if occ { fg } else { t.c("outline") };
                            El::Txt(label, txt(12.0, if active { 600 } else { 400 }), col, 26.0)
                        } else if occ { El::Dot(8.0, muted) } else { El::Dot(5.0, alpha(t.c("outline"), 0.55)) };
                        els.push(El::Btn { id: WS + idx, w: 26.0, h: 26.0, label: Box::new(inner), bg: active.then(|| t.c("primary")), round: 13.0 });
                    }
                    out.push(group(WS, els, (4.0, 4.0), 0.0));
                }
                "clock" => {
                    let now = util::now();
                    let text = if v { util::clock(now.wHour, now.wMinute, cx.cfg.get("use24h").as_bool() != Some(false)) } else { sys::luxon(cx.cfg.str("clockFormat"), &now) };
                    out.push(group(CLOCK, vec![El::Txt(text, txt(13.5, 400), fg, 400.0)], (10.0, 10.0), 6.0));
                    if !island_has("timer") {
                        if let Some((done, icon, text)) = Self::timer_chip(cx) {
                            let f = if done { t.c("onPrimary") } else { fg };
                            out.push(Pill { bg: done.then(|| t.c("primary")), ..group(TIMER, vec![El::Ico(icon, 18.0, done, f), El::Txt(text, txt(13.0, 400), f, 120.0)], (10.0, 10.0), 4.0) });
                        }
                    }
                    if !island_has("recording") && Self::rec_started() > 0.0 {
                        let f = t.c("onError");
                        out.push(Pill { bg: Some(t.c("error")), ..group(REC, vec![El::Ico("stop_circle", 18.0, true, f),
                            El::Txt(hub::clock((hub::now_ms() - Self::rec_started()) / 1000.0), txt(13.0, 400), f, 120.0)], (10.0, 10.0), 4.0) });
                    }
                }
                "utils" => {
                    self.utils = cx.cfg.strs("bar.utils");
                    let icons = [("snip", "screenshot_region"), ("pick", "colorize"), ("wall", "wallpaper"), ("mode", if cx.theme.dark { "dark_mode" } else { "light_mode" })];
                    let els: Vec<El> = icons.iter().enumerate().filter(|(_, (k, _))| self.utils.iter().any(|u| u == k))
                        .map(|(i, (_, icon))| btn(UTIL + i as u32, 28.0, icon, 17.0, false, fg)).collect();
                    if !els.is_empty() { out.push(group(UTIL, els, (3.0, 3.0), 0.0)); }
                }
                "weather" => if let Some(w) = &cx.weather {
                    out.push(group(WEATHER, vec![El::Ico(weather::icon(w.status), 18.0, false, fg), El::Txt(weather::temp(w, cx.cfg.str("tempUnit")), txt(13.0, 400), fg, 80.0)], (10.0, 10.0), 4.0));
                },
                "wm" => {
                    self.wm_cmds.clear();
                    let mut els = vec![];
                    if cx.wm.paused {
                        self.wm_cmds.push("wm-toggle-pause".into());
                        let f = t.c("onError");
                        let label = if v { El::Ico("pause_circle", 20.0, false, f) } else { El::Row(vec![El::Ico("pause_circle", 20.0, false, f), El::Txt("Tiling paused".into(), txt(13.0, 400), f, 200.0)], 6.0) };
                        els.push(El::Btn { id: WMC, w: el_w(c.g, &label) + if v { 12.0 } else { 28.0 }, h: 32.0, label: Box::new(label), bg: Some(t.c("error")), round: 16.0 });
                    }
                    for (name, display) in &cx.wm.modes {
                        let i = self.wm_cmds.len() as u32;
                        self.wm_cmds.push(format!("wm-disable-binding-mode --name {name}"));
                        let shown = if v { display.chars().take(1).collect() } else { display.clone() }; // the mode's initial on a side bar
                        els.push(El::Btn { id: WMC + i, w: c.g.measure(&shown, txt(13.0, 400)).0.ceil() + if v { 12.0 } else { 28.0 }, h: 32.0, label: Box::new(El::Txt(shown, txt(13.0, 400), fg, 200.0)),
                            bg: Some(t.c("surfaceContainer")), round: 16.0 });
                    }
                    if !els.is_empty() { out.push(group(WMC, els, (10.0, 10.0), 6.0)); }
                }
                "netspeed" => if let Some(n) = &cx.net {
                    out.push(group(NET, vec![El::Col(vec![(format!("↑ {}", sys::rate(n.tx)), txt(11.0, 400), fg), (format!("↓ {}", sys::rate(n.rx)), txt(11.0, 400), fg)], 120.0)], (10.0, 10.0), 0.0));
                },
                "tray" => {
                    let px = (18.0 * c.scale).round() as u32;
                    self.tray_ids.clear();
                    let mut els = vec![];
                    for i in cx.tray.clone() {
                        if let Some(img) = self.tray_icon(&i, px) {
                            els.push(El::Img(TRAY + self.tray_ids.len() as u32, img, 18.0));
                            self.tray_ids.push(i.id.clone());
                        }
                    }
                    if !els.is_empty() { out.push(group(TRAY, els, (6.0, 6.0), 2.0)); }
                }
                "system" => {
                    let mut els = vec![];
                    if cx.cfg.bool("showKeyboardLayout") && !cx.kb.is_empty() { els.push(El::Txt(cx.kb.clone(), txt(12.0, 400), muted, 60.0)); }
                    if cx.audio.and_then(|a| a.mic_muted) == Some(true) { els.push(El::Ico("mic_off", 18.0, false, fg)); }
                    els.push(El::Ico(volume_icon(cx.audio.as_ref()), 18.0, false, fg));
                    els.push(El::Ico(sys::net_icon(cx.net.as_ref()), 18.0, false, fg));
                    if let Some(b) = &cx.battery {
                        let col = if b.percent < 20.0 && !b.charging { t.c("error") } else { fg };
                        els.push(El::IcoS(sys::battery_icon(b), 18.0, false, col));
                        els.push(El::Txt(format!("{}", b.percent.round()), txt(12.5, 400), col, 40.0));
                    }
                    out.push(Pill { hover: true, ..group(SYS, els, (12.0, 12.0), 8.0) });
                }
                _ => {}
            }
        }
        if let Some(w) = fit { for p in &mut out { fit_column(p, w); } }
        out
    }
}

/// Makes a group's content fit a side bar: no text beside the media ring or island icons, nothing wider than `w`.
fn fit_column(p: &mut Pill, w: f32) {
    if matches!(p.id, MEDIA | ISLAND) && p.els.iter().any(|e| !matches!(e, El::Txt(..))) { p.els.retain(|e| !matches!(e, El::Txt(..))); }
    p.fixed = None;
    p.gap = p.gap.max(3.0);
    for e in &mut p.els { squeeze(e, w); }
}

fn squeeze(e: &mut El, w: f32) {
    match e {
        El::Txt(_, _, _, max) | El::Col(_, max) => *max = max.min(w),
        El::Meter(mw, _) => *mw = mw.min(w),
        El::Btn { w: bw, .. } => *bw = bw.min(w),
        El::Row(items, _) => for i in items { squeeze(i, w) },
        _ => {}
    }
}

fn el_h(e: &El) -> f32 {
    match e {
        El::Ico(_, s, ..) | El::IcoS(_, s, ..) | El::Ring { size: s, .. } => *s,
        El::Txt(_, f, ..) => (f.size * 1.3).ceil(),
        El::Btn { h, .. } => *h,
        El::Row(items, _) => items.iter().map(el_h).fold(0.0, f32::max),
        El::Dot(s, _) => s.max(10.0),
        El::Meter(..) => 8.0,
        El::Img(_, _, s) => *s + 6.0,
        El::Col(lines, _) => lines.iter().map(|(_, f, _)| f.size * 1.15).sum(),
    }
}

/// A group on a side bar: its content stacked top to bottom.
fn pill_h(p: &Pill) -> f32 {
    let inner: f32 = p.els.iter().map(el_h).sum::<f32>() + p.gap * p.els.len().saturating_sub(1) as f32;
    inner + 2.0 * vpad(p)
}
fn vpad(p: &Pill) -> f32 { if p.kind == Kind::Plain { 0.0 } else { p.pad.0.clamp(3.0, 8.0) } }

/// Where each group goes on a side bar: left-side groups from the top, centre ones in the middle, right-side ones at the bottom.
fn place_column(heights: &[Vec<f32>; 3], x: f32, w: f32, top: f32, len: f32, tight: bool) -> Vec<(usize, usize, R)> {
    let gaps = if tight { [2.0; 3] } else { [6.0, 5.0, 6.0] };
    let sum = |h: &[f32], gap: f32| h.iter().sum::<f32>() + gap * h.len().saturating_sub(1) as f32;
    // the middle never starts above the end of the top groups
    let mid = (top + (len - sum(&heights[1], gaps[1])) / 2.0).max(top + sum(&heights[0], gaps[0]) + gaps[0]);
    let starts = [top, mid, top + len - sum(&heights[2], gaps[2])];
    let mut out = vec![];
    for (si, hs) in heights.iter().enumerate() {
        let gap = gaps[si];
        let mut y = starts[si];
        for (pi, h) in hs.iter().enumerate() { out.push((si, pi, R::new(x, y, w, *h))); y += h + gap; }
    }
    out
}

/// Fill, border and shadow of one group for the bar style (bar.html's #bar.<style> / .g-<groups> rules).
fn group_look(cx: &Ctx, p: &Pill) -> (Option<Color>, Option<Color>, bool) {
    let t = &cx.theme;
    let (style, groups) = (cx.cfg.str("bar.style"), cx.cfg.str("bar.groups"));
    let layer0 = alpha(t.c("background"), 1.0 - cx.cfg.get("transparency").as_f64().unwrap_or(0.0) as f32);
    let border = alpha(t.c("outlineVariant"), 0.45);
    if style == "islands" { return (Some(layer0), Some(border), true); } // `#bar.islands .group` outranks the state colours
    if p.kind != Kind::Group { return (None, None, false); }
    match groups {
        "outlined" => (None, Some(alpha(t.c("outlineVariant"), 0.7)), false),
        "none" => (None, None, false),
        _ => (Some(p.bg.unwrap_or(t.c("surfaceContainerLow"))), None, false),
    }
}

impl View for Bar {
    fn size(&self, _g: &Gfx, _cx: &Ctx, scale: f32) -> (f32, f32) {
        if self.edge.vertical() { (BAR_W, self.mon.3 as f32 / scale) } else { (self.mon.2 as f32 / scale, BAR_H) }
    }

    fn key(&self, cx: &Ctx) -> String {
        let st = util::now();
        let m = cx.media.as_ref().map_or(0, |m| (m.progress() * 200.0) as i32);
        format!("{}{}{}{m}", st.wHour, st.wMinute, st.wSecond)
    }

    fn frame_ms(&self) -> u32 { if self.island.moving() { 16 } else if self.pulsing { 100 } else { 0 } }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let style = cx.cfg.str("bar.style");
        let layer0 = alpha(t.c("background"), 1.0 - cx.cfg.get("transparency").as_f64().unwrap_or(0.0) as f32);
        let flat = style == "flat";
        let (v, edge) = (self.edge.vertical(), self.edge);
        let (bx, by, bw, bh) = match (flat, v) {
            (true, false) => (0.0, 0.0, c.w, BAR_H),
            (true, true) => (0.0, 0.0, c.w, c.h),
            (false, false) => (6.0, 6.0, c.w - 12.0, 44.0),
            (false, true) => (6.0, 6.0, c.w - 12.0, c.h - 12.0),
        };
        let pad = if flat { 10.0 } else { 5.0 };
        let (cx_, cy) = (bx + bw / 2.0, by + bh / 2.0);
        if style != "islands" {
            let bar = R::new(bx, by, bw, bh);
            let round = if flat { 0.0 } else { 23.0 };
            if !flat { c.shadow(2.0, 8.0, 0.25, |c, col| if let Some(col) = col { c.rect(bar, round, col) }); }
            c.rect(bar, round, alpha(t.c("outlineVariant"), 0.45));
            // flat: the border is the 1px edge facing the screen
            let inner = if !flat { R::new(bx + 1.0, by + 1.0, bw - 2.0, bh - 2.0) } else { match edge {
                Edge::Top => R::new(0.0, 0.0, c.w, c.h - 1.0), Edge::Bottom => R::new(0.0, 1.0, c.w, c.h - 1.0),
                Edge::Left => R::new(0.0, 0.0, c.w - 1.0, c.h), Edge::Right => R::new(1.0, 0.0, c.w - 1.0, c.h),
            } };
            c.rect(inner, if flat { 0.0 } else { 22.0 }, layer0);
        }
        let hover = self.hover;
        let pw = bw - 2.0 * pad; // a group's width on a side bar
        let fit_w = v.then_some(pw - 8.0);
        let mut sides = [self.pills(c, cx, "left", fit_w), self.pills(c, cx, "center", fit_w), self.pills(c, cx, "right", fit_w)];
        let placed: Vec<(usize, usize, R)> = if v {
            let (avail, gaps) = (bh - 2.0 * pad, [6.0, 5.0, 6.0]);
            let heights = |sides: &[Vec<Pill>; 3]| [0, 1, 2].map(|i| sides[i].iter().map(pill_h).collect::<Vec<f32>>());
            let mut hs = heights(&sides);
            let need = |hs: &[Vec<f32>; 3], g: [f32; 3]| hs.iter().zip(g).map(|(h, g)| h.iter().sum::<f32>() + g * h.len().saturating_sub(1) as f32).sum::<f32>();
            // too many groups for the monitor's height: tighten the padding and gaps
            let tight = need(&hs, gaps) > avail;
            if tight {
                for p in sides.iter_mut().flatten() { p.gap = p.gap.min(2.0); if p.kind != Kind::Plain { p.pad = (3.0, 3.0); } }
                hs = heights(&sides);
            }
            place_column(&hs, bx + pad, pw, by + pad, avail, tight)
        } else {
            let mut widths: Vec<Vec<f32>> = sides.iter().map(|s| s.iter().map(|p| pill_w(c.g, p)).collect()).collect();
            let sum = |w: &[f32], gap: f32| w.iter().sum::<f32>() + gap * w.len().saturating_sub(1) as f32;
            let (inner_x, inner_w) = (bx + pad, bw - 2.0 * pad);
            let cw = sum(&widths[1], 5.0);
            // grid 1fr auto 1fr: a side gets half of what the centre leaves, and a long window title
            // shrinks (ellipsis) to fit it instead of running under the centre groups
            let room = (inner_w - cw) / 2.0 - 8.0;
            for si in [0, 2] {
                let Some(pi) = sides[si].iter().position(|p| p.id == WINDOW) else { continue };
                let p = &mut sides[si][pi];
                let Some(w) = fit(&widths[si], pi, 6.0, room, p.pad.0 + p.pad.1 + 40.0) else { continue };
                if let Some(El::Col(_, max) | El::Txt(_, _, _, max)) = p.els.first_mut() { *max = w - p.pad.0 - p.pad.1; }
                widths[si][pi] = pill_w(c.g, p);
            }
            let total = |i: usize, gap: f32| sum(&widths[i], gap);
            let starts = [inner_x, inner_x + (inner_w - cw) / 2.0, inner_x + inner_w - total(2, 6.0)];
            let mut placed = vec![];
            for (si, s) in sides.iter().enumerate() {
                let gap = if si == 1 { 5.0 } else { 6.0 };
                let mut x = starts[si];
                for (pi, p) in s.iter().enumerate() {
                    let w = widths[si][pi];
                    let h = if p.kind == Kind::Plain { 34.0 } else { GH };
                    placed.push((si, pi, R::new(x, cy - h / 2.0, w, h)));
                    x += w + gap;
                }
            }
            placed
        };
        // one shadow pass for every group (islands style)
        let shadowed: Vec<R> = placed.iter().filter(|(si, pi, _)| group_look(cx, &sides[*si][*pi]).2).map(|p| p.2).collect();
        if !shadowed.is_empty() { c.shadow(2.0, 8.0, 0.25, |c, col| if let Some(col) = col { for r in &shadowed { c.rect(*r, r.h.min(r.w) / 2.0, col); } }); }
        self.pills.clear();
        for (si, pi, r) in placed {
            let p = &sides[si][pi];
            let (fill, border, _) = group_look(cx, p);
            let rad = r.h.min(r.w) / 2.0;
            if let Some(b) = border { c.rect(r, rad, b); }
            if let Some(f) = fill { c.rect(if border.is_some() { R::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0) } else { r }, rad - 1.0, f); }
            let hovers_group = p.hover || matches!(cx.cfg.str("bar.groups"), "outlined" | "none") && p.kind == Kind::Group;
            let over = c.hit(r, p.id) && hover == p.id;
            if over && hovers_group { c.rect(r, rad, alpha(t.c("onSurface"), 0.08)); }
            self.pills.push((p.id, r));
            let draw = |c: &mut Canvas| {
                if v {
                    let mut y = r.y + vpad(p);
                    for e in &p.els {
                        let h = el_h(e);
                        draw_el(c, cx, e, cx_ - el_w(c.g, e) / 2.0, y + h / 2.0, hover);
                        y += h + p.gap;
                    }
                    return;
                }
                let mut x = r.x + p.pad.0;
                for e in &p.els {
                    draw_el(c, cx, e, x, cy, hover);
                    x += el_w(c.g, e) + p.gap;
                }
            };
            if p.fixed.is_some() { c.clip_round(r, rad, draw); } else { draw(c); }
        }
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match ev {
            Input::Hover(id) => {
                self.hover = id;
                let over = id == ISLAND || (BADGE..CTL + 3).contains(&id);
                if over != self.di_hover { self.di_hover = over; cx.ver += 1; }
            }
            Input::Click(id, b) => self.click(id, b, cx),
            Input::Wheel(id, d) => {
                if id == SYS || (id == ISLAND && cx.cfg.str("island.scroll") != "track" && cx.cfg.str("island.scroll") != "none") { media::step_volume(if d > 0 { 5 } else { -5 }); }
                if id == ISLAND && cx.cfg.str("island.scroll") == "track" && hub::now_ms() - self.di_scroll_at > 500.0 {
                    self.di_scroll_at = hub::now_ms();
                    media::control(if d > 0 { "previous" } else { "next" });
                }
                if (WS..WS + 0x100).contains(&id) || id == WS {
                    // cycle this monitor's open workspaces (GlazeWM's next/prev crosses monitors)
                    let mine: Vec<String> = self.my_workspaces(cx).iter().map(|i| i.to_string()).collect();
                    let Some(m) = self.monitor(cx) else { return };
                    let mut open: Vec<&crate::glaze::Ws> = m.workspaces.iter().filter(|w| mine.contains(&w.name)).collect();
                    open.sort_by_key(|w| w.name.parse::<u32>().unwrap_or(0));
                    let i = open.iter().position(|w| w.displayed).unwrap_or(0) as i32;
                    if open.len() > 1 {
                        let n = open.len() as i32;
                        let next = &open[((i + if d < 0 { 1 } else { -1 }) % n + n) as usize % n as usize].name;
                        crate::glaze::command(&format!("focus --workspace {next}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn hover_popup(&self) -> Option<(u32, R)> {
        POPUPS.contains(&self.hover).then(|| self.pills.iter().find(|p| p.0 == self.hover).map(|p| (p.0, p.1))).flatten()
    }
}

impl Bar {
    fn click(&mut self, id: u32, b: Btn, cx: &mut Ctx) {
        let act = |this: &Bar, a: &str, cx: &mut Ctx| match a {
            "panel" => this.open_island_panel(cx),
            "play" => media::control("togglePlayPause"),
            "sidebar-left" | "sidebar-right" | "overview" => this.open_here(cx, a),
            _ => {}
        };
        match (id, b) {
            (LAUNCHER, Btn::Left) => self.open_here(cx, "sidebar-left"),
            (SYS | CLOCK, Btn::Left) => self.open_here(cx, "sidebar-right"),
            (TIMER, Btn::Left) => {
                hub::set("zor-ui.timerDone", None);
                hub::set("zor-ui.rightTab", Some("timer".into()));
                self.open_here(cx, "sidebar-right");
            }
            (REC, Btn::Left) => {
                hub::set("zor-ui.rec", None);
                let cfg = crate::conf::Config(cx.cfg.0.clone());
                std::thread::spawn(move || { proc::bridge(&cfg, "record", &["stop"]); }); // opens Explorer on the new file
            }
            (MEDIA, Btn::Left) => media::control("togglePlayPause"),
            (MEDIA, Btn::Middle) => media::control("next"),
            (MEDIA, Btn::Right) => self.open_here(cx, "sidebar-left"),
            (i, Btn::Left) if (CTL..CTL + 3).contains(&i) => media::control(["previous", "togglePlayPause", "next"][(i - CTL) as usize]),
            (i, Btn::Left) if (BADGE..BADGE + 16).contains(&i) => {
                if let Some(p) = Self::island_list(cx).get((i - BADGE) as usize) { self.di_focus = p.0.into(); }
            }
            (ISLAND, Btn::Left) => { let a = cx.cfg.str("island.click").to_string(); act(self, if a.is_empty() { "panel" } else { &a }, cx) }
            (ISLAND, Btn::Right) => { let a = cx.cfg.str("island.rightClick").to_string(); act(self, if a.is_empty() { "sidebar-left" } else { &a }, cx) }
            (i, Btn::Left) if (WS..WS + 0x100).contains(&i) => if let Some(n) = self.ws_names.get((i - WS) as usize) { crate::glaze::command(&format!("focus --workspace {n}")); },
            (i, Btn::Right) if (WS..WS + 0x100).contains(&i) || i == WS => self.open_here(cx, "overview"),
            (i, Btn::Left) if (UTIL..UTIL + 4).contains(&i) => match i - UTIL {
                0 => proc::run_bg(&cx.cfg, "toggles", &["-Action", "snip"]),
                1 => proc::run_bg(&cx.cfg, "toggles", &["-Action", "colorpick"]),
                2 => proc::run_bg(&cx.cfg, "switchwall", &[]),
                _ => proc::run_bg(&cx.cfg, "switchwall", &["-ToggleMode"]),
            },
            (i, Btn::Left) if (WMC..WMC + 0x100).contains(&i) => if let Some(cmd) = self.wm_cmds.get((i - WMC) as usize) { crate::glaze::command(cmd); },
            (i, b) if (TRAY..TRAY + 0x100).contains(&i) => if let Some(tid) = self.tray_ids.get((i - TRAY) as usize) {
                crate::tray::click(tid, match b { Btn::Left => crate::tray::Click::Left, Btn::Right => crate::tray::Click::Right, Btn::Middle => crate::tray::Click::Middle, Btn::Double => crate::tray::Click::Double });
            },
            _ => {}
        }
    }

    /// openIslandPanel() in bar.js: the panel opens under the island.
    fn open_island_panel(&self, cx: &mut Ctx) {
        let closed: f64 = hub::get("end4.closedAt.island-panel").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        if hub::now_ms() - closed < 600.0 { return; } // the click that closed it
        cx.notif = None;
        let Some((_, r)) = self.pills.iter().find(|p| p.0 == ISLAND) else { return };
        let s = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
        let (ax, ay) = anchor(self.edge, self.origin, *r, s);
        cx.actions.push(Action::Open { name: "island-panel".into(), mon: Some((self.mon.0, self.mon.1)), at: Some((ax, ay)) });
    }
}

// ------------------------------------------------------------------ hover popup (bar-popup.html)

pub struct Popup { pub id: u32 }

fn kv_rows(cx: &Ctx, id: u32) -> Option<(&'static str, String, Option<String>, Vec<(String, String, String)>)> {
    let kv = |i: &str, k: &str, v: String| (i.to_string(), k.to_string(), v);
    let tu = cx.cfg.str("tempUnit");
    Some(match id {
        RES => {
            let (mhz, cores, threads) = sys::cpu_info();
            let ((used, total), (su, st)) = sys::memory();
            let mut rows = vec![kv("memory", "CPU", format!("{}%", (cx.cpu * 100.0).round())), kv("speed", "Clock", format!("{:.2} GHz", mhz as f32 / 1000.0)),
                kv("developer_board", "Cores", format!("{cores} cores, {threads} threads")), kv("memory_alt", "RAM", format!("{} of {}", sys::gb(used), sys::gb(total)))];
            if st > 0.0 { rows.push(kv("swap_horiz", "Page file", format!("{} of {}", sys::gb(su), sys::gb(st)))); }
            ("monitoring", "Resources".into(), None, rows)
        }
        WEATHER => {
            let w = cx.weather.as_ref()?;
            let mut rows = vec![kv("cloud", "Sky", weather::text(w.status))];
            if let Some(s) = w.wind { rows.push(kv("air", "Wind", format!("{s:.1} km/h"))); }
            (weather::icon(w.status), if w.city.is_empty() { "Weather".into() } else { w.city.clone() }, Some(weather::temp(w, tu)), rows)
        }
        NET => {
            let n = cx.net.as_ref()?;
            let mut rows = vec![kv("arrow_downward", "Download", sys::rate(n.rx)), kv("arrow_upward", "Upload", sys::rate(n.tx)),
                kv("data_usage", "Since start-up", format!("{} down, {} up", sys::gb(n.total_rx as f64), sys::gb(n.total_tx as f64)))];
            if !n.ip.is_empty() { rows.push(kv("lan", "IP", n.ip.clone())); }
            (sys::net_icon(Some(n)), if !n.ssid.is_empty() { n.ssid.clone() } else if !n.name.is_empty() { n.name.clone() } else { "Network".into() }, None, rows)
        }
        SYS => {
            let a = cx.audio.as_ref();
            let pretty = |k: &str| match k { "ethernet" => "Ethernet", "wifi" => "Wi-Fi", "proprietary_virtual" => "VPN", _ => "Connected" };
            let mut rows = vec![
                kv(volume_icon(a), "Volume", a.map_or("No device".into(), |a| format!("{}%{}", a.volume.round(), if a.muted { ", muted" } else { "" }))),
                kv(sys::net_icon(cx.net.as_ref()), "Network", cx.net.as_ref().map_or("Offline".into(), |n| if n.ssid.is_empty() { pretty(n.kind).into() } else { n.ssid.clone() })),
            ];
            if let Some(b) = &cx.battery {
                rows.push((sys::battery_icon(b), "Battery".into(), format!("{}%, {}", b.percent.round(), if b.charging { "charging" } else if b.full { "full" } else { "discharging" })));
                if let (false, Some(s)) = (b.charging, b.secs_left) { rows.push(kv("schedule", "Time left", if s >= 3600 { format!("{}h {}m", s / 3600, s / 60 % 60) } else { format!("{}m", s / 60) })); }
            }
            ("tune", "Quick settings (click to open)".into(), None, rows)
        }
        _ => return None,
    })
}

impl Popup {
    fn content_h(&self, cx: &Ctx) -> f32 {
        if self.id == CLOCK { return 13.0 * LH + 8.0 + 6.0 + 26.0 * (crate::desk::month_rows() as f32 + 1.0); }
        let Some((_, _, big, rows)) = kv_rows(cx, self.id) else { return 0.0 };
        13.0 * LH + 8.0 + if big.is_some() { 26.0 * LH } else { 0.0 } + rows.len() as f32 * (6.0 + 13.0 * LH)
    }
    fn content_w(&self, g: &Gfx, cx: &Ctx) -> f32 {
        if self.id == CLOCK { return 7.0 * 28.0; }
        let Some((_, title, _, rows)) = kv_rows(cx, self.id) else { return 220.0 };
        let row = rows.iter().map(|(_, k, v)| 22.0 + g.measure(k, txt(13.0, 400)).0 + 16.0 + g.measure(v, txt(13.0, 700)).0).fold(0.0, f32::max);
        row.max(24.0 + g.measure(&title, txt(13.0, 500)).0)
    }
}

impl View for Popup {
    fn size(&self, g: &Gfx, cx: &Ctx, _scale: f32) -> (f32, f32) {
        let w = (self.content_w(g, cx) + 28.0).clamp(220.0, 320.0);
        (w + 12.0, self.content_h(cx) + 24.0 + 12.0)
    }
    fn key(&self, _cx: &Ctx) -> String { util::now().wSecond.to_string() } // live numbers while open
    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let layer0 = alpha(t.c("background"), 1.0 - cx.cfg.get("transparency").as_f64().unwrap_or(0.0) as f32);
        let card = R::new(6.0, 6.0, c.w - 12.0, c.h - 12.0);
        c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(card, 23.0, col) });
        c.rect(card, 23.0, alpha(t.c("outlineVariant"), 0.45));
        c.rect(R::new(7.0, 7.0, card.w - 2.0, card.h - 2.0), 22.0, layer0);
        let (x, w) = (card.x + 14.0, card.w - 28.0);
        let mut y = card.y + 12.0;
        let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
        let h4 = |c: &mut Canvas, y: f32, icon: &str, title: &str| {
            c.icon(icon, R::new(x, y, 18.0, 13.0 * LH), ico(18.0, false), t.c("primary"));
            c.text(title, R::new(x + 24.0, y, w - 24.0, 13.0 * LH), txt(13.0, 500), fg, Align::Left);
        };
        if self.id == CLOCK {
            let st = util::now();
            h4(c, y, "calendar_month", &date_fmt(&st, "dddd, MMMM d, yyyy"));
            y += 13.0 * LH + 8.0 + 6.0;
            for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
                c.text(d, R::new(x + i as f32 * 28.0, y, 28.0, 26.0), txt(11.0, 400), muted, Align::Center);
            }
            for (i, (d, out, today)) in crate::desk::month_cells(&st).into_iter().enumerate() {
                let (cx_, cy_) = (x + (i % 7) as f32 * 28.0, y + 26.0 * (i / 7 + 1) as f32);
                if today { c.circle(cx_ + 14.0, cy_ + 13.0, 13.0, t.c("primary")); }
                let col = if today { t.c("onPrimary") } else if out { alpha(fg, 0.35) } else { fg };
                c.text(&d.to_string(), R::new(cx_, cy_, 28.0, 26.0), txt(12.0, if today { 600 } else { 400 }), col, Align::Center);
            }
            return;
        }
        let Some((icon, title, big, rows)) = kv_rows(cx, self.id) else { return };
        h4(c, y, icon, &title);
        y += 13.0 * LH + 8.0;
        if let Some(b) = big { c.text(&b, R::new(x, y, w, 26.0 * LH), txt(26.0, 500), fg, Align::Left); y += 26.0 * LH; }
        for (i, k, v) in rows {
            let rh = 13.0 * LH;
            c.icon(&i, R::new(x, y + 3.0, 16.0, rh), ico(16.0, false), muted);
            c.text(&k, R::new(x + 22.0, y + 3.0, w - 22.0, rh), txt(13.0, 400), muted, Align::Left);
            c.text(&v, R::new(x, y + 3.0, w, rh), txt(13.0, 700), fg, Align::Right);
            y += rh + 6.0;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspaces_split_between_monitors() {
        assert_eq!(split_workspaces(10, 5, true), [1, 2, 3, 4, 5]);
        assert_eq!(split_workspaces(10, 5, false), [6, 7, 8, 9, 10]);
        assert_eq!(split_workspaces(10, 3, false), [4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(split_workspaces(6, 5, false), [6], "the old one-workspace second bar");
        assert_eq!(split_workspaces(6, 2, false), [3, 4, 5, 6]);
    }
    use super::*;
    #[test]
    fn layout_fills_in_defaults() {
        let cfg = crate::conf::Config::parse(r#"{"bar": {"modules": [["clock", "left"], ["bogus", "left"], ["clock", "right"]]}}"#);
        let l = layout(&cfg);
        assert_eq!(l[0], ("clock".to_string(), "left".to_string()));
        assert_eq!(l.iter().filter(|m| m.0 == "clock").count(), 1);
        assert_eq!(l.len(), 13);
        assert_eq!(l.iter().find(|m| m.0 == "netspeed").unwrap().1, "hidden");
    }
    #[test]
    fn bar_edges() {
        use windows::Win32::Foundation::RECT;
        let m = RECT { left: 1920, top: 0, right: 3000, bottom: 1920 };
        let r = |e| { let r = strip(m, e, 56); (r.left, r.top, r.right, r.bottom) };
        assert_eq!(r(Edge::Top), (1920, 0, 3000, 56));
        assert_eq!(r(Edge::Bottom), (1920, 1864, 3000, 1920));
        assert_eq!(r(Edge::Left), (1920, 0, 1976, 1920));
        assert_eq!(r(Edge::Right), (2944, 0, 3000, 1920));
        assert_eq!([Edge::parse("left"), Edge::parse("bottom"), Edge::parse("right"), Edge::parse(""), Edge::parse("nonsense")],
            [Edge::Left, Edge::Bottom, Edge::Right, Edge::Top, Edge::Top]);
        assert!(Edge::Left.vertical() && !Edge::Bottom.vertical());
    }

    #[test]
    fn popups_open_into_the_screen() {
        let g = R::new(100.0, 10.0, 80.0, 34.0); // centre (140, 27)
        assert_eq!(popup_spot(Edge::Top, (0, 0), g, 1.0, (200, 100)), (40, 48));
        assert_eq!(popup_spot(Edge::Bottom, (0, 1000), g, 1.0, (200, 100)), (40, 1010 - 4 - 100));
        // a group at the bar's start can't push the popup off the monitor
        assert_eq!(popup_spot(Edge::Top, (0, 0), R::new(0.0, 10.0, 40.0, 34.0), 1.0, (200, 100)).0, 0);
        let v = R::new(6.0, 100.0, 46.0, 40.0); // centre y 120
        assert_eq!(popup_spot(Edge::Left, (0, 0), v, 1.0, (200, 100)), (56, 70));
        assert_eq!(popup_spot(Edge::Right, (1000, 0), v, 1.0, (200, 100)), (1006 - 4 - 200, 70));
        assert_eq!(anchor(Edge::Top, (0, 0), g, 1.0), (140, 46));
        assert_eq!(anchor(Edge::Bottom, (0, 1000), g, 1.0), (140, 1008));
        assert_eq!(anchor(Edge::Left, (0, 0), v, 1.0), (54, 120));
        assert_eq!(anchor(Edge::Right, (1000, 0), v, 1.0), (1004, 120));
    }

    #[test]
    fn side_bar_columns() {
        // top group from the top, centre group in the middle, bottom group against the bottom
        let p = place_column(&[vec![34.0, 40.0], vec![100.0], vec![60.0]], 5.0, 46.0, 5.0, 500.0, false);
        let at = |si, pi| p.iter().find(|x| x.0 == si && x.1 == pi).unwrap().2;
        assert_eq!((at(0, 0).y, at(0, 1).y), (5.0, 45.0));
        assert_eq!(at(1, 0).y, 5.0 + (500.0 - 100.0) / 2.0);
        assert_eq!(at(2, 0).y + at(2, 0).h, 505.0);
        // not enough room: gaps shrink and the middle group stays clear of the top ones
        let t = place_column(&[vec![200.0, 200.0], vec![150.0], vec![100.0]], 0.0, 46.0, 0.0, 600.0, true);
        let y = |si, pi| t.iter().find(|x| x.0 == si && x.1 == pi).unwrap().2.y;
        assert_eq!((y(0, 1), y(1, 0)), (202.0, 404.0));
        assert!(p.iter().all(|x| x.2.x == 5.0 && x.2.w == 46.0));
        // text next to the media ring is dropped, long text and meters are cut to the column
        let f = txt(13.0, 400);
        let mut m = Pill { id: MEDIA, els: vec![El::Ico("music_note", 20.0, true, Color::default()), El::Txt("Some long song".into(), f, Color::default(), 230.0)], pad: (5.0, 10.0), gap: 6.0, bg: None, hover: false, fixed: Some(90.0), kind: Kind::Group };
        fit_column(&mut m, 38.0);
        assert_eq!((m.els.len(), m.fixed), (1, None));
        let mut t = Pill { id: CLOCK, els: vec![El::Txt("12:34".into(), f, Color::default(), 400.0), El::Meter(110.0, 50.0)], ..m };
        fit_column(&mut t, 38.0);
        assert!(matches!(&t.els[0], El::Txt(_, _, _, 38.0)) && matches!(&t.els[1], El::Meter(38.0, _)));
        assert!(pill_h(&t) > 2.0 * 8.0);
    }

    #[test]
    fn long_titles_shrink_to_their_side() {
        // launcher 34, title 600, clock 150 with 6px gaps = 796 in a 500px side
        assert_eq!(fit(&[34.0, 600.0, 150.0], 1, 6.0, 500.0, 68.0), Some(304.0));
        assert_eq!(fit(&[34.0, 100.0], 1, 6.0, 500.0, 68.0), None);
        assert_eq!(fit(&[34.0, 600.0, 450.0], 1, 6.0, 500.0, 68.0), Some(68.0));
    }
    #[test]
    fn helpers() {
        assert_eq!(pretty_app("msedge.exe"), "Microsoft Edge");
        assert_eq!(pretty_app("spotify"), "Spotify");
        assert!((bezier(0.34, 1.35, 0.64, 1.0, 1.0) - 1.0).abs() < 1e-3);
        assert!(bezier(0.34, 1.35, 0.64, 1.0, 0.6) > 1.0); // overshoots like the CSS spring
        let a = media::Audio { volume: 50.0, ..Default::default() };
        assert_eq!(volume_icon(Some(&a)), "volume_down");
    }
}
