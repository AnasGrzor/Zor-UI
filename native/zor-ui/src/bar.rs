//! The bar (bar.html / bar.js) and its hover popups (bar-popup.html), one bar per monitor.

use crate::{
    app::{Action, Ctx},
    gfx::{alpha, ico, txt, Align, Canvas, Color, Font, Gfx, Pixmap, R},
    hub, media, privacy, proc, sys,
    util::{self, date_fmt},
    weather,
    win::{Btn, Input, View},
};
use crate::tokens as tk;
use serde_json::Value;
use crate::anim::{self, Spring};
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
pub fn popup_spot(edge: Edge, bar: (i32, i32), bar_size: (i32, i32), r: R, s: f32, size: (i32, i32)) -> (i32, i32) {
    let at = |v: f32, o: i32| (o as f32 + v * s).round() as i32;
    let (cx, cy) = (at(r.x + r.w / 2.0, bar.0), at(r.y + r.h / 2.0, bar.1));
    let gap = (4.0 * s).round() as i32;
    // stay inside the bar's span (it covers the monitor) so the popup isn't cut off; the start edge wins if it can't fit
    let fit_x = |x: i32| x.min(bar.0 + bar_size.0 - size.0).max(bar.0);
    let fit_y = |y: i32| y.min(bar.1 + bar_size.1 - size.1).max(bar.1);
    match edge {
        Edge::Top => (fit_x(cx - size.0 / 2), at(r.y + r.h, bar.1) + gap),
        Edge::Bottom => (fit_x(cx - size.0 / 2), at(r.y, bar.1) - gap - size.1),
        Edge::Left => (at(r.x + r.w, bar.0) + gap, fit_y(cy - size.1 / 2)),
        Edge::Right => (at(r.x, bar.0) - gap - size.0, fit_y(cy - size.1 / 2)),
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
pub const CLAUDE: u32 = 19; pub const CLOCK: u32 = 7; const TIMER: u32 = 8; const REC: u32 = 9; pub const WEATHER: u32 = 11; pub const NET: u32 = 13; pub const SYS: u32 = 15;
const WS: u32 = 0x100; const UTIL: u32 = 0x200; const WMC: u32 = 0x300; const TRAY: u32 = 0x400; const BADGE: u32 = 0x500; const CTL: u32 = 0x600;
/// The chevron that opens the tray flyout; the last id of the tray range, past any icon index.
const TRAY_MORE: u32 = TRAY + 0xFF;

/// (shown, hidden) counts for `total` tray icons when the bar may show `visible` of them. 0 means all.
pub fn tray_split(total: usize, visible: usize) -> (usize, usize) {
    let shown = if visible == 0 { total } else { visible.min(total) };
    (shown, total - shown)
}

/// The tray icons that have a picture, split into the ones the bar shows and the ones the flyout holds.
pub fn tray_icons(cx: &Ctx) -> (Vec<crate::tray::Icon>, Vec<crate::tray::Icon>) {
    let all: Vec<_> = cx.tray.iter().filter(|i| i.rgba.is_some()).cloned().collect();
    let visible = cx.cfg.get("bar.trayVisible").as_u64().unwrap_or(3) as usize;
    split_icons(all, cx.cfg.get("bar.trayMatchWindows").as_bool().unwrap_or(true), visible)
}

/// Splits icons into (bar, flyout). "Match Windows" puts the icons Windows shows on the taskbar on the bar; otherwise the first `visible`.
pub fn split_icons(all: Vec<crate::tray::Icon>, match_windows: bool, visible: usize) -> (Vec<crate::tray::Icon>, Vec<crate::tray::Icon>) {
    if match_windows { return all.into_iter().partition(|i| i.promoted); }
    let mut all = all;
    let (shown, _) = tray_split(all.len(), visible);
    let hidden = all.split_off(shown);
    (all, hidden)
}

/// The chevron after the shown icons, pointing into the screen like Windows' `^`.
fn tray_more(edge: Edge, id: u32, col: Color) -> El {
    let icon = match edge { Edge::Top => "expand_more", Edge::Bottom => "expand_less", Edge::Left => "chevron_right", Edge::Right => "chevron_left" };
    btn(id, 24.0, icon, 18.0, false, col)
}

pub const TRAY_SAMPLE: usize = 8;

/// Made-up icon colors for the Settings preview.
pub fn tray_sample_colors() -> [Color; TRAY_SAMPLE] {
    [0xFF5865F2, 0xFF1DB954, 0xFFF59E0B, 0xFFEF4444, 0xFF06B6D4, 0xFFA855F7, 0xFFEC4899, 0xFF84CC16]
}

/// How many sample icons "Match Windows" puts on the bar: the first two, as if Windows shows them on the taskbar.
pub const TRAY_SAMPLE_PROMOTED: usize = 2;

/// (shown, hidden) for the sample icons, with the same rule as `tray_icons`.
pub fn tray_sample_split(match_windows: bool, visible: usize) -> (usize, usize) {
    if match_windows { (TRAY_SAMPLE_PROMOTED, TRAY_SAMPLE - TRAY_SAMPLE_PROMOTED) } else { tray_split(TRAY_SAMPLE, visible) }
}

/// The tray group with `split` = (shown, hidden) sample icons and the chevron when some are hidden, drawn with the bar's own element
/// code (`draw_el`). Returns its width. Ids are 0 so the Settings page gets no clicks from it.
pub fn draw_tray_sample(c: &mut Canvas, cx: &Ctx, x: f32, cy: f32, split: (usize, usize), edge: Edge) -> f32 {
    let (shown, hidden) = split;
    let mut els: Vec<El> = tray_sample_colors().iter().take(shown)
        .map(|&col| El::Btn { id: 0, w: 24.0, h: 24.0, label: Box::new(El::Dot(14.0, col)), bg: None, round: 6.0 }).collect();
    if hidden > 0 { els.push(tray_more(edge, 0, cx.theme.c("onSurface"))); }
    let w = 12.0 + els.iter().map(|e| el_w(c.g, e)).sum::<f32>() + 2.0 * els.len().saturating_sub(1) as f32;
    c.rect(R::new(x, cy - GH / 2.0, w, GH), GH / 2.0, cx.theme.c("surfaceContainer"));
    let mut x = x + 6.0;
    for e in &els { draw_el(c, cx, e, x, cy, u32::MAX); x += el_w(c.g, e) + 2.0; }
    w
}
/// Extra space between bar groups for `bar.spacing`: "tight", "cozy" (the default, no change) or "roomy".
pub fn spacing_extra(s: &str) -> f32 { match s { "tight" => -2.0, "roomy" => 8.0, _ => 0.0 } }

/// Width of one workspace button. The active one is a wider pill, but only on a horizontal bar: a side bar is too narrow.
pub fn ws_w(active: bool, vertical: bool) -> f32 { if active && !vertical { tk::WS_ACTIVE_W } else { tk::WS_W } }

/// Three sample groups (workspaces, clock, tray) spaced by `bar.spacing`, drawn with the bar's own
/// `group_look` and `draw_el`, so the Settings preview cannot drift from the bar. Returns the total width.
pub fn draw_spacing_sample(c: &mut Canvas, cx: &Ctx, x: f32, cy: f32, spacing: &str, style: &str, groups: &str) -> f32 {
    let t = &cx.theme;
    let ws = |active: bool, label: &str| {
        let col = if active { t.c("onPrimary") } else { t.c("onSurface") };
        El::Btn { id: 0, w: ws_w(active, false), h: tk::WS_W, label: Box::new(El::Txt(label.into(), txt(12.0, if active { 600 } else { 400 }), col, tk::WS_W)),
            bg: active.then(|| crate::ui::accent(cx)), round: tk::WS_W / 2.0 }
    };
    let mk = |els: Vec<El>, pad: f32, gap: f32| Pill { id: 0, els, pad: (pad, pad), gap, bg: None, hover: false, fixed: None, kind: Kind::Group, fade: 1.0 };
    let sample = [
        mk(vec![ws(true, "1"), ws(false, "2"), ws(false, "3")], 4.0, 0.0),
        mk(vec![El::Txt("12:30".into(), txt(13.5, 400), t.c("onSurface"), 60.0)], 10.0, 6.0),
        mk(tray_sample_colors().iter().take(3).map(|&col| El::Btn { id: 0, w: 24.0, h: 24.0, label: Box::new(El::Dot(14.0, col)), bg: None, round: 6.0 }).collect(), 6.0, 2.0),
    ];
    let gap = 6.0 + spacing_extra(spacing);
    let mut x0 = x;
    for p in &sample {
        let w = pill_w(c.g, p);
        let r = R::new(x0, cy - GH / 2.0, w, GH);
        let (fill, border, _) = group_look_for(cx, p, style, groups);
        let rad = r.h / 2.0;
        if let Some(b) = border { c.rect(r, rad, b); }
        if let Some(f) = fill { c.rect(if border.is_some() { R::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0) } else { r }, rad - 1.0, f); }
        let mut ex = r.x + p.pad.0;
        for e in &p.els { draw_el(c, cx, e, ex, cy, u32::MAX); ex += el_w(c.g, e) + p.gap; }
        x0 += w + gap;
    }
    x0 - gap - x
}

/// Groups with a hover popup (POPUPS in bar.js).
pub const POPUPS: [u32; 6] = [CLOCK, RES, WEATHER, NET, SYS, CLAUDE];

/// barLayout() in lib/barlayout.js: [(id, side)] in order, unknown ids dropped, missing ones added.
pub fn layout(cfg: &crate::conf::Config) -> Vec<(String, String)> {
    const MODULES: [(&str, &str); 14] = [("launcher", "left"), ("window", "left"), ("resources", "center"), ("media", "center"), ("island", "center"),
        ("workspaces", "center"), ("clock", "center"), ("utils", "center"), ("weather", "center"), ("claude", "hidden"), ("wm", "right"), ("netspeed", "hidden"), ("tray", "right"), ("system", "right")];
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
    Eq(Color, bool), // the island's equalizer bars, moving while `playing`
    Viz(Color, Vec<f32>), // the bar visualizer (#bar-viz): bars at 0-1 levels
}

/// #bar-viz: a 60 x 22 canvas of 20 bars.
const VIZ_W: f32 = 60.0;
const VIZ_H: f32 = 22.0;
const VIZ_BARS: u64 = 20;
const VIZ: u32 = 17;

/// The bar visualizer with its top-left corner at (x, y); also drawn by the Settings preview.
pub fn draw_viz(c: &mut Canvas, x: f32, y: f32, col: Color, levels: &[f32]) {
    for (bx, top, bw, bh) in viz_bars(levels, VIZ_W, VIZ_H) {
        let (x, y) = (x + bx, y + top);
        let r = (bw / 2.0).min(4.0); // roundRect(x, y, bw, bh, [r, r, 0, 0])
        if bh <= r * 2.0 { c.rect(R::new(x, y, bw, bh), r.min(bh / 2.0), col); }
        else { c.rect(R::new(x, y, bw, r * 2.0), r, col); c.rect(R::new(x, y + r * 2.0, bw, bh - r * 2.0), 0.0, col); }
    }
}

/// drawBars(.., 'bar'): each bar's (x, top, width, height) in the canvas. 1.5 px gaps, at least 2 px tall, full height at level 1.
fn viz_bars(levels: &[f32], w: f32, h: f32) -> Vec<(f32, f32, f32, f32)> {
    let n = levels.len().max(1) as f32;
    let gap = 1.5;
    let bw = (w / n - gap).max(1.0);
    levels.iter().enumerate().map(|(i, v)| {
        let bh = (v.clamp(0.0, 1.0) * h).max(2.0);
        (i as f32 * (bw + gap) + gap / 2.0, h - bh, bw, bh)
    }).collect()
}

/// The island's equalizer (the marketing page's .island .eq): 4 bars 3px wide, 3px apart, up to 18px tall.
const EQ_BARS: usize = 4;

/// The island's equalizer: one 3 px bar per level (0..1), 6 px apart, standing on `cy + 9`. Settings previews it with this too.
pub fn draw_eq(c: &mut Canvas, x: f32, cy: f32, col: Color, levels: &[f32]) {
    for (i, lv) in levels.iter().enumerate() {
        let h = 18.0 * lv;
        c.rect(R::new(x + i as f32 * 6.0, cy + 9.0 - h, 3.0, h), 1.5, col);
    }
}

/// Made-up levels for the Settings previews.
pub const EQ_SAMPLE: [f32; EQ_BARS] = [0.5, 1.0, 0.7, 0.85];
/// Height 0.4-1 of bar `i` at `ms`: each bar runs 40% -> 100% in 1 s and back (CSS `eq 1s ease-out infinite
/// alternate`, offset per bar); still at 40% while paused.
fn eq_level(ms: f64, i: usize, playing: bool) -> f32 {
    if !playing { return 0.4; }
    let off = [0.0, 0.3, 0.6, 0.15][i % EQ_BARS];
    let p = ((ms / 1000.0 + off) % 2.0) as f32;
    let x = if p < 1.0 { p } else { 2.0 - p }; // alternate: the way back replays the curve in reverse
    0.4 + 0.6 * bezier(0.16, 1.0, 0.3, 1.0, x)
}

/// Part of the repaint key, so a paused island (which isn't redrawing) picks up a new or previewed theme at once.
fn theme_key(t: &crate::conf::Theme) -> String { format!("{:x}{:x}{:x}", t.c("primary"), t.c("onSurface"), t.c("background")) }

/// The island's mute button: what it shows for the current state, and the island's control buttons (previous, play, next, mute).
fn mute_icon(muted: bool) -> &'static str { if muted { "volume_off" } else { "volume_up" } }
const CTL_N: u32 = 4;

struct Pill { id: u32, els: Vec<El>, pad: (f32, f32), gap: f32, bg: Option<Color>, hover: bool, fixed: Option<f32>, kind: Kind, fade: f32 }
#[derive(Clone, Copy, PartialEq)]
enum Kind { Group, Plain, Aw }

pub struct Bar {
    /// This bar's monitor (physical px): x, y, width, height.
    pub mon: (i32, i32, i32, i32),
    pub edge: Edge,
    /// Top-left of the bar window (physical px).
    pub origin: (i32, i32),
    island: Spring,
    /// The activity the island showed last frame, its colour, and when and from what colour it last changed.
    di_state: &'static str,
    di_bg: Option<Color>,
    di_fade: Option<(Instant, Option<Color>)>,
    di_hover: bool,
    island_cap: Option<f32>, // the most width the island may take on a narrow bar (from the last frame)
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
    tray_tips: Vec<String>,
    utils: Vec<String>,
    /// A pulsing dot is showing (recording, an agent waiting).
    pulsing: bool,
    /// The equalizer is moving (a song is playing in the island).
    eq: bool,
    /// The island's mic icon follows the voice level.
    mic_live: bool,
    /// The bar visualizer's levels and stream (started while `visualizer.bar` is on), and sound is playing in it.
    viz: std::sync::Arc<crate::viz::Shared>,
    viz_stream: Option<proc::Stream>,
    viz_loud: bool,
}

impl Bar {
    /// #bar-viz, shown after the island (or after the media group when the island isn't on the bar). Starts or stops the stream.
    fn viz_pill(&mut self, cx: &Ctx) -> Option<Pill> {
        // only while media plays: no flat bars on screen otherwise (dropping the stream stops the capture too)
        if !cx.cfg.bool("visualizer.bar") || !cx.media.as_ref().is_some_and(|m| m.playing) { self.viz_stream = None; self.viz_loud = false; return None; }
        if self.viz_stream.is_none() { self.viz_stream = crate::viz::listen(&cx.cfg, VIZ_BARS, self.viz.clone()); }
        let levels = self.viz.levels.lock().map(|g| g.clone()).unwrap_or_default();
        self.viz_loud = levels.iter().any(|&v| v > 0.0);
        if !self.viz_loud { self.viz.awake.store(false, std::sync::atomic::Ordering::Relaxed); }
        let col = cx.cfg.viz_color("visualizer.barColor").unwrap_or_else(|| cx.theme.c("primary"));
        Some(Pill { id: VIZ, els: vec![El::Viz(col, levels)], pad: (0.0, 0.0), gap: 0.0, bg: None, hover: false, fixed: None, kind: Kind::Plain, fade: 1.0 })
    }

    pub fn new(mon: (i32, i32, i32, i32), edge: Edge, origin: (i32, i32)) -> Bar {
        Bar { mon, edge, origin, island: Spring::default(), di_state: "", di_bg: None, di_fade: None, di_hover: false, island_cap: None, di_focus: String::new(), di_track: String::new(), di_track_until: 0.0, di_scroll_at: 0.0,
            art: None, tray_px: HashMap::new(), pills: vec![], hover: 0, ws_names: vec![], wm_cmds: vec![], tray_ids: vec![], tray_tips: vec![], utils: vec![], pulsing: false, eq: false, mic_live: false,
            viz: Default::default(), viz_stream: None, viz_loud: false }
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
pub(crate) fn rgba_to_pixmap(rgba: &[u8], w: u32, h: u32, px: u32) -> Option<Pixmap> {
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
        El::Eq(..) => (EQ_BARS * 6 - 3) as f32,
        El::Viz(..) => VIZ_W,
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
        El::Eq(col, playing) => {
            let now = hub::now_ms();
            let lv: Vec<f32> = (0..EQ_BARS).map(|i| eq_level(now, i, *playing)).collect();
            draw_eq(c, x, cy, *col, &lv);
        }
        El::Viz(col, levels) => draw_viz(c, x, cy - VIZ_H / 2.0, *col, levels),
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

/// Makes a horizontal bar's groups fit `room` with some air left, and returns the extra pixels
/// (0..=7) to add to every gap, and the width the island may take (None: no limit). In order: the
/// song text shrinks (down to 60), the least useful groups go (visualizer, weather, net speed, quick
/// buttons), the tray keeps 6 icons, the timer goes, and what is still short is taken from the island.
/// Its width is animated, so it cuts its own title to the cap: a hovered (wide) one gives way instead
/// of covering its neighbours, and a collapsed one may grow back to the same cap.
fn compact(sides: &mut [Vec<Pill>; 3], room: f32, w: impl Fn(&Pill) -> f32) -> (f32, Option<f32>) {
    use crate::tokens::BAR_GROUP_GAP;
    const MARGIN: f32 = 12.0; // least gap between the side groups and the centre, each side
    const AIR: f32 = 40.0; // spare room wanted on top of the least gaps
    if room > 1300.0 { return (0.0, None); } // a wide screen keeps the layout it always had
    let free = |s: &[Vec<Pill>; 3]| {
        let sum = |v: &Vec<Pill>, gap: f32| v.iter().map(&w).sum::<f32>() + gap * v.len().saturating_sub(1) as f32;
        room - (sum(&s[0], BAR_GROUP_GAP) + sum(&s[1], BAR_GROUP_GAP) + sum(&s[2], BAR_GROUP_GAP) + 2.0 * MARGIN)
    };
    while free(sides) < AIR {
        let widest = sides.iter_mut().flatten().filter(|p| p.id == MEDIA && p.fixed.is_none())
            .filter_map(|p| p.els.iter_mut().find_map(|e| if let El::Txt(_, _, _, m) = e { Some(m) } else { None }))
            .filter(|m| **m > 60.0).max_by(|a, b| a.total_cmp(b));
        let Some(m) = widest else { break };
        *m = (*m - 20.0).max(60.0);
    }
    for id in [VIZ, CLAUDE, WEATHER, NET, UTIL] {
        if free(sides) >= AIR { break; }
        for s in sides.iter_mut() { s.retain(|p| p.id != id); }
    }
    if free(sides) < AIR {
        // shortcut: icons cut here are not in the flyout either; fine since it only happens on a narrow screen with more than 6 shown
        for p in sides.iter_mut().flatten().filter(|p| p.id == TRAY) {
            let more = matches!(p.els.last(), Some(El::Btn { id: TRAY_MORE, .. })).then(|| p.els.pop()).flatten();
            p.els.truncate(6);
            p.els.extend(more);
        }
    }
    if free(sides) < AIR {
        for s in sides.iter_mut() { s.retain(|p| p.id != TIMER); }
    }
    let cap = sides.iter().flatten().find(|p| p.id == ISLAND).map(|p| (w(p) + free(sides) - AIR).max(60.0));
    let gaps: usize = sides.iter().map(|s| s.len().saturating_sub(1)).sum::<usize>() + 2;
    (((free(sides) - AIR).max(0.0) / gaps as f32).min(7.0), cap)
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
        let agents = hub::get_json("zor-ui.agents");
        let agents = agents.as_array().cloned().unwrap_or_default();
        let asks = di("agents") && agents.iter().any(|a| a["state"] == "waiting");
        let now = hub::now_ms();
        let mut l = vec![];
        if cx.notif.as_ref().is_some_and(|n| n.until > now) { l.push(("notification", true)); }
        if cx.osd.as_ref().is_some_and(|o| o.until > now) { l.push(("osd", true)); }
        if asks { l.push(("agent", true)); } // a waiting prompt wins outright, so a badge focus can't hide it
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
                let agents = hub::get_json("zor-ui.agents");
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
                    // the newest working session's tool; "×N" when several sessions of this agent are working
                    let ws: Vec<&Value> = a["sessions"].as_array().into_iter().flatten().filter(|x| x["state"] == "working").collect();
                    let tool = ws.first().map(|x| s(&x["tool"])).filter(|t| !t.is_empty()).map(|t| format!(" · {t}")).unwrap_or_default();
                    let count = if ws.len() > 1 { format!(" ×{}", ws.len()) } else { String::new() };
                    els.push(El::Txt(format!("{}{count}{tool}{pct}", s(&a["label"])), txt(13.0, 400), fg, text_w));
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
                if !wide && flag("eq") && m.playing { els.push(El::Eq(cx.cfg.viz_color("island.eqColor").unwrap_or_else(|| t.c("primary")), m.playing)); self.eq = m.playing; }
                if wide && flag("controls") {
                    els.push(btn(CTL, 26.0, "skip_previous", 18.0, true, fg));
                    els.push(btn(CTL + 1, 26.0, if m.playing { "pause" } else { "play_arrow" }, 18.0, true, fg));
                    els.push(btn(CTL + 2, 26.0, "skip_next", 18.0, true, if m.can_next { fg } else { alpha(fg, 0.4) })); // previous restarts the track, so it stays lit
                    els.push(btn(CTL + 3, 26.0, mute_icon(cx.audio.is_some_and(|a| a.muted)), 18.0, true, fg));
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
        // an app holds the mic (icon: red muted, green speaking, else orange) or camera (green); stays while it does
        if flag("privacy") {
            if cx.privacy.mic.is_some() {
                let (icon, col) = privacy::mic_look(cx.audio.is_some_and(|a| a.mic_muted == Some(true)), media::mic_speaking());
                els.push(El::Ico(icon, 16.0, true, col));
                self.mic_live = icon == "mic"; // the colour follows the voice, so keep repainting
            }
            if cx.privacy.cam.is_some() { els.push(El::Ico("videocam", 16.0, true, 0xFF30D158)); }
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
        // a new activity: the colour blends from the old one and the content fades in
        let now = Instant::now();
        if self.di_state != state {
            if !self.di_state.is_empty() { self.di_fade = Some((now, self.di_bg)); }
            self.di_state = state;
        }
        self.di_bg = bg;
        let mut fade = 1.0;
        if let Some((t0, from)) = self.di_fade {
            let f = anim::fade(now.duration_since(t0).as_secs_f32());
            if f >= 1.0 { self.di_fade = None; } else {
                fade = f;
                let rest = t.c("surfaceContainerLow"); // what a group with no state colour uses
                if from != bg { bg = Some(anim::mix(from.unwrap_or(rest), bg.unwrap_or(rest), f)); }
            }
        }
        let mut p = Pill { id: ISLAND, els, pad: (9.0, 9.0), gap: 7.0, bg, hover: false, fixed: None, kind: Kind::Group, fade };
        if let Some(cap) = self.island_cap {
            while pill_w(c.g, &p) > cap {
                let Some(El::Txt(_, _, _, m)) = p.els.iter_mut().find(|e| matches!(e, El::Txt(..))) else { break };
                if *m <= 60.0 { break; }
                *m = (*m - 10.0).max(60.0);
            }
        }
        let target = pill_w(c.g, &p);
        self.island.set(target);
        p.fixed = Some(self.island.tick(now).max(10.0));
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
        let group = |id, els, pad: (f32, f32), gap| Pill { id, els, pad, gap, bg: None, hover: false, fixed: None, kind: Kind::Group, fade: 1.0 };
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
                    let pad = if matches!(cx.cfg.str("bar.style"), "islands" | "clear") { (16.0, 16.0) } else { (crate::tokens::BAR_GROUP_PAD, crate::tokens::BAR_GROUP_PAD) };
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
                    // the visualizer follows system audio, so it shows with or without a media session
                    if !island_on { out.extend(self.viz_pill(cx)); }
                }
                "island" => { let ob = &on_bar; out.extend(self.island(c, cx, ob)); out.extend(self.viz_pill(cx)); }
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
                        els.push(El::Btn { id: WS + idx, w: ws_w(active, v), h: tk::WS_W, label: Box::new(inner), bg: active.then(|| crate::ui::accent(cx)), round: tk::WS_W / 2.0 });
                    }
                    out.push(group(WS, els, (4.0, 4.0), 0.0));
                }
                "clock" => {
                    let now = util::now();
                    let text = if v { util::clock(now.wHour, now.wMinute, cx.cfg.get("use24h").as_bool() != Some(false)) } else { sys::luxon(cx.cfg.str("clockFormat"), &now) };
                    out.push(group(CLOCK, vec![El::Txt(text, txt(13.5, 400), fg, 400.0)], (crate::tokens::CLOCK_PAD, crate::tokens::CLOCK_PAD), crate::tokens::BAR_GROUP_GAP));
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
                "claude" => if let Some(u) = &cx.claude {
                    let f = claude_chip_color(u.worst(), fg, t.c("error"));
                    out.push(group(CLAUDE, vec![El::Ico("smart_toy", 18.0, false, f), El::Txt(format!("{}%", u.worst()), txt(13.0, 400), f, 60.0)], (10.0, 10.0), 4.0));
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
                    self.tray_tips.clear();
                    let mut els = vec![];
                    let (shown, hidden) = tray_icons(cx);
                    for i in shown {
                        if let Some(img) = self.tray_icon(&i, px) {
                            els.push(El::Img(TRAY + self.tray_ids.len() as u32, img, 18.0));
                            self.tray_ids.push(i.id.clone());
                            self.tray_tips.push(i.tooltip.clone());
                        }
                    }
                    if !hidden.is_empty() { els.push(tray_more(self.edge, TRAY_MORE, fg)); }
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
        El::Eq(..) => 18.0,
        El::Viz(..) => VIZ_H,
    }
}

/// A group on a side bar: its content stacked top to bottom.
fn pill_h(p: &Pill) -> f32 {
    let inner: f32 = p.els.iter().map(el_h).sum::<f32>() + p.gap * p.els.len().saturating_sub(1) as f32;
    inner + 2.0 * vpad(p)
}
fn vpad(p: &Pill) -> f32 { if p.kind == Kind::Plain { 0.0 } else { p.pad.0.clamp(3.0, 8.0) } }

/// Where each group goes on a side bar: left-side groups from the top, centre ones in the middle, right-side ones at the bottom.
fn place_column(heights: &[Vec<f32>; 3], x: f32, w: f32, top: f32, len: f32, gaps: [f32; 3]) -> Vec<(usize, usize, R)> {
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

/// Islands style fills every group with the plain background, except the island pill: its text uses the
/// state colour's "on" pair, so it must keep the state fill or the text is unreadable.
fn islands_fill(p: &Pill, layer0: Color) -> Color {
    if p.id == ISLAND { p.bg.unwrap_or(layer0) } else { layer0 }
}

/// Fill, border and shadow of one group for the bar style (bar.html's #bar.<style> / .g-<groups> rules).
fn group_look(cx: &Ctx, p: &Pill) -> (Option<Color>, Option<Color>, bool) { group_look_for(cx, p, cx.cfg.str("bar.style"), cx.cfg.str("bar.groups")) }

/// `group_look` for a given style and group setting (the Settings preview passes the values being edited).
fn group_look_for(cx: &Ctx, p: &Pill, style: &str, groups: &str) -> (Option<Color>, Option<Color>, bool) {
    let t = &cx.theme;
    let layer0 = crate::ui::layer0(cx);
    let border = alpha(t.c("outlineVariant"), tk::GLASS_EDGE_A);
    if style == "clear" { return (None, None, false); } // a preset's "transparent" bar: only the contents show
    if style == "islands" { return (Some(islands_fill(p, layer0)), Some(border), true); }
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
        // sound starting or stopping in the bar visualizer repaints it
        let loud = self.viz_stream.is_some() && self.viz.levels.lock().is_ok_and(|g| g.iter().any(|&v| v > 0.0));
        format!("{}{}{}{m}{}{loud}", st.wHour, st.wMinute, st.wSecond, theme_key(&cx.theme))
    }

    fn frame_ms(&self) -> u32 { if self.island.moving() || self.di_fade.is_some() || self.viz_loud { 16 } else if self.eq || self.mic_live { 50 } else if self.pulsing { 100 } else { 0 } }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.mic_live = false; // island() turns it back on
        self.eq = false; // island() turns it back on; off when the island isn't on this bar
        self.viz_loud = false; // viz_pill() turns it back on
        let t = &cx.theme;
        let style = cx.cfg.str("bar.style");
        let layer0 = crate::ui::layer0(cx);
        let flat = style == "flat";
        let (v, edge) = (self.edge.vertical(), self.edge);
        let (bx, by, bw, bh) = match (flat, v) {
            (true, false) => (0.0, 0.0, c.w, BAR_H),
            (true, true) => (0.0, 0.0, c.w, c.h),
            (false, false) => (6.0, 6.0, c.w - 12.0, 44.0),
            (false, true) => (6.0, 6.0, c.w - 12.0, c.h - 12.0),
        };
        let pad = if flat { crate::tokens::BAR_EDGE_FLAT } else { crate::tokens::BAR_EDGE };
        let (cx_, cy) = (bx + bw / 2.0, by + bh / 2.0);
        if !matches!(style, "islands" | "clear") {
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
            let sp = spacing_extra(cx.cfg.str("bar.spacing"));
            let (avail, gaps) = (bh - 2.0 * pad, [crate::tokens::BAR_GROUP_GAP + sp; 3]);
            let heights = |sides: &[Vec<Pill>; 3]| [0, 1, 2].map(|i| sides[i].iter().map(pill_h).collect::<Vec<f32>>());
            let mut hs = heights(&sides);
            let need = |hs: &[Vec<f32>; 3], g: [f32; 3]| hs.iter().zip(g).map(|(h, g)| h.iter().sum::<f32>() + g * h.len().saturating_sub(1) as f32).sum::<f32>();
            // too many groups for the monitor's height: drop the least important ones, the order compact() uses,
            // rather than squeezing padding and gaps below the bar's minimum
            for id in [VIZ, CLAUDE, WEATHER, NET, UTIL, TIMER] {
                if need(&hs, gaps) <= avail { break; }
                for s in sides.iter_mut() { s.retain(|p| p.id != id); }
                hs = heights(&sides);
            }
            place_column(&hs, bx + pad, pw, by + pad, avail, gaps)
        } else {
            let (inner_x, inner_w) = (bx + pad, bw - 2.0 * pad);
            let (extra, cap) = compact(&mut sides, inner_w, |p| pill_w(c.g, p)); // a narrow (portrait) screen
            self.island_cap = cap;
            let sp = spacing_extra(cx.cfg.str("bar.spacing"));
            let (g5, g6) = (crate::tokens::BAR_GROUP_GAP + extra + sp, crate::tokens::BAR_GROUP_GAP + extra + sp);
            let mut widths: Vec<Vec<f32>> = sides.iter().map(|s| s.iter().map(|p| pill_w(c.g, p)).collect()).collect();
            let sum = |w: &[f32], gap: f32| w.iter().sum::<f32>() + gap * w.len().saturating_sub(1) as f32;
            let cw = sum(&widths[1], g5);
            // grid 1fr auto 1fr: a side gets half of what the centre leaves, and a long window title
            // shrinks (ellipsis) to fit it instead of running under the centre groups
            let room = (inner_w - cw) / 2.0 - 8.0;
            for si in [0, 2] {
                let Some(pi) = sides[si].iter().position(|p| p.id == WINDOW) else { continue };
                let p = &mut sides[si][pi];
                let Some(w) = fit(&widths[si], pi, g6, room, p.pad.0 + p.pad.1 + 40.0) else { continue };
                if let Some(El::Col(_, max) | El::Txt(_, _, _, max)) = p.els.first_mut() { *max = w - p.pad.0 - p.pad.1; }
                widths[si][pi] = pill_w(c.g, p);
            }
            let total = |i: usize, gap: f32| sum(&widths[i], gap);
            let starts = [inner_x, inner_x + (inner_w - cw) / 2.0, inner_x + inner_w - total(2, g6)];
            let mut placed = vec![];
            for (si, s) in sides.iter().enumerate() {
                let gap = if si == 1 { g5 } else { g6 };
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
            // fading in: the pill's own fill over the content, thinning out
            if p.fade < 1.0 { if let Some(f) = fill { c.rect(r, rad, alpha(f, 1.0 - p.fade)); } }
        }
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match ev {
            Input::Hover(id) => {
                self.hover = id;
                let over = id == ISLAND || (BADGE..CTL + CTL_N).contains(&id);
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

    fn hover_popup(&self) -> Option<(u32, R, String)> {
        if let Some(i) = self.hover.checked_sub(TRAY).filter(|&i| i < 0x100) {
            let tip = self.tray_tips.get(i as usize).filter(|t| !t.is_empty())?;
            let (_, r) = self.pills.iter().find(|p| p.0 == TRAY)?;
            return Some((self.hover, R::new(r.x + 6.0 + i as f32 * 26.0, r.y, 24.0, r.h), tip.clone())); // El::Img is 24 wide, 2 gap, 6 pad
        }
        POPUPS.contains(&self.hover).then(|| self.pills.iter().find(|p| p.0 == self.hover).map(|p| (p.0, p.1, String::new()))).flatten()
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
            (i, Btn::Left) if i == CTL + 3 => {
                if let Some(a) = cx.audio.as_mut() { a.muted = !a.muted; media::set_mute(a.muted); } // flip now, the poll confirms
            }
            (i, Btn::Left) if i == CTL || i == CTL + 2 => if let Some(m) = &cx.media { media::skip(m, i == CTL + 2) },
            (i, Btn::Left) if i == CTL + 1 => media::control("togglePlayPause"),
            (i, Btn::Left) if (BADGE..BADGE + 16).contains(&i) => {
                if let Some(p) = Self::island_list(cx).get((i - BADGE) as usize) { self.di_focus = p.0.into(); }
            }
            // a click on the notification banner goes to that notification
            (ISLAND, Btn::Left) if crate::app::Notif::target(&cx.notif, hub::now_ms()).is_some() => {
                match crate::app::Notif::target(&cx.notif, hub::now_ms()) {
                    Some(crate::bridge::claude_acct::CANCEL) => crate::bridge::claude_acct::cancel(), // the auto-switch banner
                    // a notification: show what it said in the island (its Open button launches the app)
                    Some(_) => if let Some(last) = hub::get("zor-ui.notifLast") {
                        hub::set("zor-ui.notifOpen", Some(last));
                        cx.actions.push(crate::app::Action::open("island-panel"));
                    },
                    None => {}
                }
                cx.notif = None;
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
            (TRAY_MORE, Btn::Left) => self.open_tray_flyout(cx),
            (i, b) if (TRAY..TRAY + 0x100).contains(&i) => if let Some(tid) = self.tray_ids.get((i - TRAY) as usize) { crate::tray::click_btn(tid, b); },
            _ => {}
        }
    }

    /// The tray flyout hangs from the tray group, like the island panel from the island.
    fn open_tray_flyout(&self, cx: &mut Ctx) {
        let closed: f64 = hub::get("zor-ui.closedAt.tray-flyout").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        if hub::now_ms() - closed < 600.0 { return; } // the click that closed it
        let Some((_, r)) = self.pills.iter().find(|p| p.0 == TRAY) else { return };
        let s = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
        let (ax, ay) = anchor(self.edge, self.origin, *r, s);
        cx.actions.push(Action::Open { name: "tray-flyout".into(), mon: Some((self.mon.0, self.mon.1)), at: Some((ax, ay)) });
    }

    /// openIslandPanel() in bar.js: the panel opens under the island.
    fn open_island_panel(&self, cx: &mut Ctx) {
        let closed: f64 = hub::get("zor-ui.closedAt.island-panel").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        if hub::now_ms() - closed < 600.0 { return; } // the click that closed it
        cx.notif = None;
        let Some((_, r)) = self.pills.iter().find(|p| p.0 == ISLAND) else { return };
        let s = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
        let (ax, ay) = anchor(self.edge, self.origin, *r, s);
        cx.actions.push(Action::Open { name: "island-panel".into(), mon: Some((self.mon.0, self.mon.1)), at: Some((ax, ay)) });
    }
}

// ------------------------------------------------------------------ hover popup (bar-popup.html)

pub struct Popup { pub id: u32, pub tip: String }
/// Extra width after a popup title: `Gfx::measure` runs a hair short of the painted width, which clipped the last word.
const TITLE_SLACK: f32 = 12.0;

/// The Claude chip turns the error color from this percent.
const CLAUDE_WARN: u8 = 90;

fn claude_chip_color<T>(worst: u8, normal: T, warn: T) -> T { if worst >= CLAUDE_WARN { warn } else { normal } }

/// The two usage rows of the Claude chip's hover popup.
fn claude_rows(session: u8, week: u8) -> Vec<(String, String, String)> {
    vec![("timer".into(), "Session (5 h)".into(), format!("{session}%")), ("date_range".into(), "Week (7 d)".into(), format!("{week}%"))]
}

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
        CLAUDE => {
            let u = cx.claude.as_ref()?;
            ("smart_toy", "Claude usage".into(), Some(format!("{}%", u.worst())), claude_rows(u.session, u.week))
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
        if !self.tip.is_empty() { return 13.0 * LH; }
        if self.id == CLOCK { return 13.0 * LH + 8.0 + 6.0 + 26.0 * (crate::desk::month_rows() as f32 + 1.0); }
        let Some((_, _, big, rows)) = kv_rows(cx, self.id) else { return 0.0 };
        13.0 * LH + 8.0 + if big.is_some() { 26.0 * LH } else { 0.0 } + rows.len() as f32 * (6.0 + 13.0 * LH)
    }
    fn content_w(&self, g: &Gfx, cx: &Ctx) -> f32 {
        if !self.tip.is_empty() { return g.measure(&self.tip, txt(13.0, 500)).0; }
        if self.id == CLOCK { return 7.0 * 28.0; }
        let Some((_, title, _, rows)) = kv_rows(cx, self.id) else { return 220.0 };
        let row = rows.iter().map(|(_, k, v)| 22.0 + g.measure(k, txt(13.0, 400)).0 + 16.0 + g.measure(v, txt(13.0, 700)).0).fold(0.0, f32::max);
        row.max(24.0 + g.measure(&title, txt(13.0, 500)).0 + TITLE_SLACK)
    }
}

impl View for Popup {
    fn size(&self, g: &Gfx, cx: &Ctx, _scale: f32) -> (f32, f32) {
        let w = (self.content_w(g, cx) + 28.0).clamp(if self.tip.is_empty() { 220.0 } else { 0.0 }, 320.0);
        (w + 12.0, self.content_h(cx) + 24.0 + 12.0)
    }
    fn key(&self, _cx: &Ctx) -> String { util::now().wSecond.to_string() } // live numbers while open
    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let layer0 = crate::ui::layer0(cx);
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
        if !self.tip.is_empty() {
            c.text(&self.tip, R::new(x, y, w, 13.0 * LH), txt(13.0, 500), fg, Align::Left);
            return;
        }
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
    /// drawBars 'bar': 20 bars in 60 px with 1.5 px gaps are 1.5 px wide; silence is 2 px stubs, level 1 fills the 22 px.
    #[test]
    fn bar_visualizer_geometry() {
        let bars = viz_bars(&[0.0, 1.0, 0.5, 2.0, -1.0], 60.0, 22.0);
        assert_eq!(bars[0], (0.75, 20.0, 10.5, 2.0));
        assert_eq!((bars[1].1, bars[1].3), (0.0, 22.0));
        assert_eq!(bars[2].3, 11.0);
        assert_eq!(bars[3].3, 22.0); // clamped
        assert_eq!(bars[4].3, 2.0);
        let twenty = viz_bars(&[0.0; 20], 60.0, 22.0);
        assert_eq!((twenty[0].2, twenty[1].0), (1.5, 3.75)); // gap / 2 + (bw + gap)
        assert!(viz_bars(&[], 60.0, 22.0).is_empty());
        assert_eq!(el_h(&El::Viz(0, vec![])), 22.0);
    }

    /// Off by default: no pill and no stream. (The stream itself needs real audio, so it isn't started here.)
    #[test]
    fn bar_visualizer_is_off_until_switched_on() {
        let mut bar = Bar::new((0, 0, 1920, 1080), Edge::Top, (0, 0));
        let cx = Ctx::new(false);
        assert!(bar.viz_pill(&cx).is_none() && bar.viz_stream.is_none() && !bar.viz_loud);
    }

    /// The island's equalizer bars show while media plays and are gone when it is paused.
    #[test]
    fn island_equalizer_only_while_playing() {
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 400 * 100 * 4];
        let mut c = Canvas::new(&g, &mut px, 400, 100, 1.0, 0).unwrap();
        let mut cx = Ctx::new(false);
        cx.cfg = crate::conf::Config::parse(r#"{"island":{"eq":true}}"#);
        let mut bar = Bar::new((0, 0, 1920, 1080), Edge::Top, (0, 0));
        for playing in [true, false] {
            cx.media = Some(crate::media::Media { playing, title: "t".into(), current: true, ..Default::default() });
            let p = bar.island(&mut c, &cx, &|_| true).expect("island with media");
            assert_eq!(p.els.iter().any(|e| matches!(e, El::Eq(..))), playing, "playing {playing}");
        }
    }

    /// The pill shows only while media plays, in both the island and the media slot; paused or no session gives none.
    #[test]
    fn bar_visualizer_shows_only_while_media_plays() {
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 400 * 100 * 4];
        let mut c = Canvas::new(&g, &mut px, 400, 100, 1.0, 0).unwrap();
        for modules in [r#"[["island","center"]]"#, r#"[["media","center"],["island","hidden"]]"#] {
            let mut cx = Ctx::new(false);
            cx.cfg = crate::conf::Config::parse(&format!(r#"{{"visualizer":{{"bar":true}},"bar":{{"modules":{modules}}}}}"#));
            let mut bar = Bar::new((0, 0, 1920, 1080), Edge::Top, (0, 0));
            for (media, shown) in [(None, false), (Some(crate::media::Media::default()), false), (Some(crate::media::Media { playing: true, ..Default::default() }), true)] {
                cx.media = media;
                let pills = bar.pills(&mut c, &cx, "center", None);
                assert_eq!(pills.iter().any(|p| p.id == VIZ), shown, "{modules}, media {:?}", cx.media);
            }
        }
    }

    /// Bars stay inside 40-100%, sit still while paused, move while playing, and start out of step.
    #[test]
    fn equalizer_levels() {
        for ms in (0..4000).step_by(37) {
            for i in 0..EQ_BARS {
                let l = eq_level(ms as f64, i, true);
                assert!((0.4..=1.0001).contains(&l), "bar {i} at {ms} ms: {l}");
                assert_eq!(eq_level(ms as f64, i, false), 0.4);
            }
        }
        assert!((eq_level(0.0, 0, true) - 0.4).abs() < 1e-3 && (eq_level(1000.0, 0, true) - 1.0).abs() < 1e-3);
        assert!((eq_level(2000.0, 0, true) - 0.4).abs() < 1e-3, "back down after the reverse pass");
        assert!(eq_level(250.0, 0, true) > eq_level(100.0, 0, true));
        let first: Vec<f32> = (0..EQ_BARS).map(|i| eq_level(0.0, i, true)).collect();
        assert!(first.windows(2).any(|w| (w[0] - w[1]).abs() > 0.1), "bars are out of step: {first:?}");
    }

    #[test]
    fn a_new_or_previewed_theme_changes_the_repaint_key() {
        let t = |p: &str, bg: &str| crate::conf::Theme::parse(&format!(r##"{{"colors":{{"primary":"{p}","background":"{bg}"}}}}"##));
        let base = theme_key(&t("#4F46E5", "#101010"));
        assert_eq!(base, theme_key(&t("#4F46E5", "#101010")));
        assert_ne!(base, theme_key(&t("#E54646", "#101010")));
        assert_ne!(base, theme_key(&t("#4F46E5", "#FFFFFF")));
        crate::conf::set_preview(Some(std::collections::HashMap::from([("primary".to_string(), 0xFF00_FF00)])));
        let previewed = theme_key(&t("#4F46E5", "#101010"));
        crate::conf::set_preview(None);
        assert_ne!(base, previewed);
        assert_eq!(base, theme_key(&t("#4F46E5", "#101010")));
    }

    #[test]
    fn islands_style_keeps_the_island_state_fill_only() {
        let pill = |id, bg| Pill { id, els: vec![], pad: (0.0, 0.0), gap: 0.0, bg, hover: false, fixed: None, kind: Kind::Group, fade: 1.0 };
        let (plain, state) = (0xFF0F_0E12, 0xFFFB_C8EE);
        assert_eq!(islands_fill(&pill(ISLAND, Some(state)), plain), state);
        assert_eq!(islands_fill(&pill(ISLAND, None), plain), plain);
        assert_eq!(islands_fill(&pill(MEDIA, Some(state)), plain), plain);
    }

    #[test]
    fn mute_button_shows_the_state_and_is_one_of_the_island_controls() {
        assert_eq!((mute_icon(true), mute_icon(false)), ("volume_off", "volume_up"));
        assert!(CTL + 3 < CTL + CTL_N && CTL + CTL_N <= CTL + 0x10);
        assert!(!(BADGE..BADGE + 16).contains(&(CTL + 3)), "the mute id must not collide with the badges");
    }

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
    fn spacing_steps_grow_and_the_default_changes_nothing() {
        assert_eq!(spacing_extra("cozy"), 0.0);
        assert_eq!(spacing_extra(""), 0.0, "an unset or unknown value is cozy");
        assert!(spacing_extra("tight") < spacing_extra("cozy") && spacing_extra("cozy") < spacing_extra("roomy"));
        assert!(5.0 + spacing_extra("tight") >= 3.0, "groups never touch");
    }

    #[test]
    fn only_the_active_workspace_is_wide_and_never_on_a_side_bar() {
        assert_eq!(ws_w(true, false), tk::WS_ACTIVE_W);
        assert_eq!((ws_w(false, false), ws_w(true, true)), (tk::WS_W, tk::WS_W));
        assert!(tk::WS_ACTIVE_W > tk::WS_W);
    }

    #[test]
    fn tray_split_follows_the_setting() {
        assert_eq!(tray_split(8, 3), (3, 5));
        assert_eq!(tray_split(8, 0), (8, 0)); // 0 shows every icon, no chevron
        assert_eq!(tray_split(2, 3), (2, 0)); // fewer icons than the limit: no chevron
        assert_eq!(tray_split(3, 3), (3, 0));
        assert_eq!(tray_split(0, 3), (0, 0));
        assert_eq!(tray_split(8, 12), (8, 0));
    }

    #[test]
    fn tray_sample_has_enough_icons_to_show_a_split() {
        assert_eq!(tray_sample_colors().len(), TRAY_SAMPLE);
        assert!(tray_split(TRAY_SAMPLE, 3).1 > 0);
        assert_eq!(tray_split(TRAY_SAMPLE, 5).0, 5);
    }

    #[test]
    fn match_windows_leaves_some_sample_icons_on_the_bar_and_some_in_the_flyout() {
        let (shown, hidden) = tray_sample_split(true, 0);
        assert_eq!(shown, TRAY_SAMPLE_PROMOTED);
        assert!(shown > 0 && hidden > 0);
        assert_eq!(tray_sample_split(false, 5), tray_split(TRAY_SAMPLE, 5), "switch off: the count");
    }

    #[test]
    fn split_follows_windows_or_the_count() {
        let icon = |id: &str, promoted: bool| crate::tray::Icon { id: id.into(), tooltip: String::new(), rgba: None, promoted };
        let all = || vec![icon("a", false), icon("b", true), icon("c", false), icon("d", true)];
        let (bar, flyout) = split_icons(all(), true, 1);
        assert_eq!(bar.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["b", "d"]);
        assert_eq!(flyout.len(), 2);
        let (bar, flyout) = split_icons(all(), true, 0);
        assert_eq!(bar.len(), 2, "the count is ignored when matching Windows");
        assert!(flyout.iter().all(|i| !i.promoted));
        let (bar, flyout) = split_icons(all(), false, 1);
        assert_eq!((bar.len(), flyout.len()), (1, 3), "count mode keeps the first N");
        assert!(split_icons(vec![], true, 0).0.is_empty());
    }

    #[test]
    fn narrow_bar_keeps_the_chevron() {
        let mut sides: [Vec<Pill>; 3] = [vec![], vec![], vec![]];
        let dot = || El::Btn { id: TRAY, w: 24.0, h: 24.0, label: Box::new(El::Dot(4.0, 0)), bg: None, round: 4.0 };
        let mut els: Vec<El> = (0..9).map(|_| dot()).collect();
        els.push(tray_more(Edge::Top, TRAY_MORE, 0));
        sides[2].push(Pill { id: TRAY, els, pad: (6.0, 6.0), gap: 2.0, bg: None, hover: false, fixed: None, kind: Kind::Plain, fade: 1.0 });
        compact(&mut sides, 100.0, |_| 300.0);
        let els = &sides[2][0].els;
        assert_eq!(els.len(), 7);
        assert!(matches!(els.last(), Some(El::Btn { id: TRAY_MORE, .. })));
    }

    #[test]
    fn tray_icon_hover_shows_its_tooltip() {
        let mut b = Bar::new((0, 0, 1920, 1080), Edge::Top, (0, 0));
        b.pills = vec![(TRAY, R::new(100.0, 4.0, 60.0, 40.0))];
        b.tray_tips = vec!["Discord".into(), String::new()];
        b.hover = TRAY + 1;
        assert!(b.hover_popup().is_none(), "no tooltip, no popup");
        b.hover = TRAY;
        let (id, r, tip) = b.hover_popup().unwrap();
        assert_eq!((id, tip.as_str(), r.x), (TRAY, "Discord", 106.0));
        b.hover = TRAY + 5;
        assert!(b.hover_popup().is_none());
    }
    #[test]
    fn layout_fills_in_defaults() {
        let cfg = crate::conf::Config::parse(r#"{"bar": {"modules": [["clock", "left"], ["bogus", "left"], ["clock", "right"]]}}"#);
        let l = layout(&cfg);
        assert_eq!(l[0], ("clock".to_string(), "left".to_string()));
        assert_eq!(l.iter().filter(|m| m.0 == "clock").count(), 1);
        assert_eq!(l.len(), 14);
        assert_eq!(l.iter().find(|m| m.0 == "netspeed").unwrap().1, "hidden");
    }
    #[test]
    fn claude_chip_warns_from_ninety_and_the_module_starts_hidden() {
        assert_eq!(claude_chip_color(89, "ok", "warn"), "ok");
        assert_eq!(claude_chip_color(90, "ok", "warn"), "warn");
        assert_eq!(claude_rows(38, 12), vec![("timer".to_string(), "Session (5 h)".to_string(), "38%".to_string()), ("date_range".to_string(), "Week (7 d)".to_string(), "12%".to_string())]);
        assert_eq!(layout(&crate::conf::Config::parse("{}")).iter().find(|m| m.0 == "claude").unwrap().1, "hidden");
        assert!(POPUPS.contains(&CLAUDE));
    }
    #[test]
    fn wanted_follows_the_chip_and_auto_switch() {
        let on = |s: &str| crate::bridge::claude_acct::wanted(&crate::conf::Config::parse(s));
        assert!(!on("{}"));
        assert!(on(r#"{"claude":{"autoSwitch":true}}"#));
        assert!(on(r#"{"bar":{"modules":[["claude","right"]]}}"#));
        assert!(!on(r#"{"bar":{"modules":[["claude","hidden"]]}}"#));
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
        let (hb, vb) = ((1920, 46), (46, 1080)); // bar sizes: horizontal and vertical
        assert_eq!(popup_spot(Edge::Top, (0, 0), hb, g, 1.0, (200, 100)), (40, 48));
        assert_eq!(popup_spot(Edge::Bottom, (0, 1000), hb, g, 1.0, (200, 100)), (40, 1010 - 4 - 100));
        // a group at the bar's start can't push the popup off the monitor
        assert_eq!(popup_spot(Edge::Top, (0, 0), hb, R::new(0.0, 10.0, 40.0, 34.0), 1.0, (200, 100)).0, 0);
        // a group at the bar's end can't either: the popup ends at the monitor edge
        let end = R::new(1880.0, 10.0, 40.0, 34.0);
        assert_eq!(popup_spot(Edge::Top, (0, 0), hb, end, 1.0, (290, 100)).0, 1920 - 290);
        assert_eq!(popup_spot(Edge::Top, (1920, 0), hb, end, 1.0, (290, 100)).0, 1920 + 1920 - 290); // second monitor
        let v = R::new(6.0, 100.0, 46.0, 40.0); // centre y 120
        assert_eq!(popup_spot(Edge::Left, (0, 0), vb, v, 1.0, (200, 100)), (56, 70));
        assert_eq!(popup_spot(Edge::Right, (1000, 0), vb, v, 1.0, (200, 100)), (1006 - 4 - 200, 70));
        let low = R::new(6.0, 1040.0, 46.0, 40.0);
        assert_eq!(popup_spot(Edge::Left, (0, 0), vb, low, 1.0, (200, 100)).1, 1080 - 100);
        assert_eq!(anchor(Edge::Top, (0, 0), g, 1.0), (140, 46));
        assert_eq!(anchor(Edge::Bottom, (0, 1000), g, 1.0), (140, 1008));
        assert_eq!(anchor(Edge::Left, (0, 0), v, 1.0), (54, 120));
        assert_eq!(anchor(Edge::Right, (1000, 0), v, 1.0), (1004, 120));
    }

    #[test]
    fn popup_title_has_slack_and_fits_max_width() {
        // "Quick settings (click to open)" is about 215px at 13px/500; with icon, slack and padding it must fit the 320 clamp
        assert!(TITLE_SLACK >= 8.0);
        assert!(24.0 + 250.0 + TITLE_SLACK + 28.0 <= 320.0);
    }

    #[test]
    fn side_bar_columns() {
        // top group from the top, centre group in the middle, bottom group against the bottom
        let p = place_column(&[vec![34.0, 40.0], vec![100.0], vec![60.0]], 5.0, 46.0, 5.0, 500.0, [6.0, 5.0, 6.0]);
        let at = |si, pi| p.iter().find(|x| x.0 == si && x.1 == pi).unwrap().2;
        assert_eq!((at(0, 0).y, at(0, 1).y), (5.0, 45.0));
        assert_eq!(at(1, 0).y, 5.0 + (500.0 - 100.0) / 2.0);
        assert_eq!(at(2, 0).y + at(2, 0).h, 505.0);
        // not enough room: gaps shrink and the middle group stays clear of the top ones
        let t = place_column(&[vec![200.0, 200.0], vec![150.0], vec![100.0]], 0.0, 46.0, 0.0, 600.0, [2.0; 3]);
        // roomy spacing widens the gap between the two top groups
        let r = place_column(&[vec![34.0, 40.0], vec![], vec![]], 0.0, 46.0, 0.0, 500.0, [14.0, 13.0, 14.0]);
        assert_eq!(r.iter().find(|x| x.1 == 1).unwrap().2.y, 34.0 + 14.0);
        let y = |si, pi| t.iter().find(|x| x.0 == si && x.1 == pi).unwrap().2.y;
        assert_eq!((y(0, 1), y(1, 0)), (202.0, 404.0));
        assert!(p.iter().all(|x| x.2.x == 5.0 && x.2.w == 46.0));
        // text next to the media ring is dropped, long text and meters are cut to the column
        let f = txt(13.0, 400);
        let mut m = Pill { id: MEDIA, els: vec![El::Ico("music_note", 20.0, true, Color::default()), El::Txt("Some long song".into(), f, Color::default(), 230.0)], pad: (5.0, 10.0), gap: 6.0, bg: None, hover: false, fixed: Some(90.0), kind: Kind::Group, fade: 1.0 };
        fit_column(&mut m, 38.0);
        assert_eq!((m.els.len(), m.fixed), (1, None));
        let mut t = Pill { id: CLOCK, els: vec![El::Txt("12:34".into(), f, Color::default(), 400.0), El::Meter(110.0, 50.0)], ..m };
        fit_column(&mut t, 38.0);
        assert!(matches!(&t.els[0], El::Txt(_, _, _, 38.0)) && matches!(&t.els[1], El::Meter(38.0, _)));
        assert!(pill_h(&t) > 2.0 * 8.0);
    }

    #[test]
    fn narrow_bar_drops_extras_in_order() {
        let pill = |id, w| Pill { id, els: vec![], pad: (0.0, 0.0), gap: 0.0, bg: None, hover: false, fixed: Some(w), kind: Kind::Group, fade: 1.0 };
        let mk = || [vec![pill(LAUNCHER, 300.0)], vec![pill(WEATHER, 100.0), pill(VIZ, 100.0)], vec![pill(NET, 100.0), pill(SYS, 300.0)]];
        let ids = |s: &[Vec<Pill>; 3]| s.iter().flatten().map(|p| p.id).collect::<Vec<_>>();
        let mut roomy = mk();
        assert_eq!(compact(&mut roomy, 2000.0, |p| p.fixed.unwrap()), (0.0, None)); // a wide screen is left alone
        assert_eq!(roomy.iter().flatten().count(), 5);
        // 900 wide + 8 of group gap + 24 of margin = 932 wanted with the visualizer; it goes, then 48 is left and 40 of it is air
        let mut s = mk();
        let extra = compact(&mut s, 880.0, |p| p.fixed.unwrap()).0;
        assert_eq!(ids(&s), vec![LAUNCHER, WEATHER, NET, SYS]);
        assert!((extra - 8.0 / 3.0).abs() < 1e-3); // 8 spare over 3 gaps
        // plenty of spare room widens the gaps, up to 7
        let mut s = mk();
        assert_eq!(compact(&mut s, 1200.0, |p| p.fixed.unwrap()).0, 7.0);
        assert_eq!(s.iter().flatten().count(), 5);
        // far too narrow: the always-kept groups stay
        let mut s = mk();
        compact(&mut s, 100.0, |p| p.fixed.unwrap());
        assert_eq!(ids(&s), vec![LAUNCHER, SYS]);
        // with nothing else to drop, the timer chip goes
        let mut s = [vec![pill(LAUNCHER, 300.0)], vec![pill(TIMER, 100.0)], vec![pill(SYS, 300.0)]];
        compact(&mut s, 700.0, |p| p.fixed.unwrap());
        assert_eq!(ids(&s), vec![LAUNCHER, SYS]);
    }

    #[test]
    fn wide_island_gives_way_on_a_narrow_bar() {
        // the island's width is animated (fixed), so compact() hands back a cap for it to cut its own title to
        let pill = |id, w| Pill { id, els: vec![], pad: (0.0, 0.0), gap: 0.0, bg: None, hover: false, fixed: Some(w), kind: Kind::Group, fade: 1.0 };
        let bar = |island| [vec![pill(LAUNCHER, 300.0)], vec![pill(ISLAND, island)], vec![pill(SYS, 300.0)]];
        let w = |p: &Pill| p.fixed.unwrap();
        // hovered: 300 + 410 + 300 + 24 margin = 1034 in 1000, so it must lose 34 plus 40 of air
        assert_eq!(compact(&mut bar(410.0), 1000.0, w).1, Some(336.0));
        // collapsed, 76 is free: it may grow back to the same cap (no flicker between the two)
        assert_eq!(compact(&mut bar(300.0), 1000.0, w).1, Some(336.0));
        // wide screens and bars without an island have no cap
        assert_eq!(compact(&mut bar(410.0), 2000.0, w).1, None);
        let mut none = [vec![pill(LAUNCHER, 300.0)], vec![], vec![pill(SYS, 300.0)]];
        assert_eq!(compact(&mut none, 1000.0, w).1, None);
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
