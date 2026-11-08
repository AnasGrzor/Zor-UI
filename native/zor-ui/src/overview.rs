//! The overview (overview.js / .html): a search box over a grid of workspaces and their windows (click to
//! focus, middle-click to close, drag onto another workspace to move), and ";" for the clipboard history.

use crate::{
    app::{self, Action, Ctx},
    bar::pretty_app,
    conf::Config,
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    glaze::{self, GWin, Wm},
    hub, island::ago, menu::Item, proc, ui::{self, Edit, Scroll, TextBox},
    win::{Btn, Input, Phase, View},
};
use serde_json::Value;
use std::{collections::HashMap, sync::{Arc, Mutex}};

const PAD: f32 = 18.0;
const GAP: f32 = 10.0;
const TILE: f32 = 48.0;
const CLIP_BTN: u32 = 1;
const SEARCH: u32 = 2;
const TILES: u32 = 1000; // + index in the flat list
const HEADS: u32 = 2000; // + workspace index
const BODIES: u32 = 3000; // + workspace index
const CLIPS: u32 = 4000; // + index in the filtered clipboard list
const CLIP_LIST: u32 = 3999;

/// Workspace columns for a grid area: five on a wide screen, fewer on a tall one so cells keep a usable shape.
fn columns(w: f32, h: f32) -> usize { if w > h * 1.3 { 5 } else if w > h * 0.9 { 3 } else { 2 } }

pub struct Overview {
    area: (f32, f32),
    q: TextBox,
    focus: bool,
    sel: usize,
    /// Visible windows in display order (rebuilt every paint and input), for the keyboard.
    flat: Vec<GWin>,
    scrolls: HashMap<String, Scroll>,
    clip_scroll: Scroll,
    /// Clipboard history from toggles.ps1: None until loaded.
    clip: Arc<Mutex<Option<Value>>>,
    clip_asked: bool,
    /// Scroll the selection into view at the next paint (after a key).
    reveal: bool,
    /// Last paint: workspace cells (drop targets) and the visible clipboard items.
    cells: Vec<(R, String)>,
    clips: Vec<Value>,
    drag: Option<Dragging>,
    /// The click that ends a drag must not focus the window.
    dragged: bool,
}

struct Dragging { id: String, idx: usize, from: (f32, f32), at: (f32, f32), moving: bool }

impl Overview {
    /// `area`: the popup's CSS-px size.
    pub fn new(area: (f32, f32)) -> Overview {
        Overview { area, q: TextBox::new(false), focus: true, sel: 0, flat: vec![], scrolls: HashMap::new(), clip_scroll: Scroll::default(),
            clip: Default::default(), clip_asked: false, reveal: false, cells: vec![], clips: vec![], drag: None, dragged: false }
    }

    /// `query` in overview.js: trimmed and lower-cased.
    fn query(&self) -> String { self.q.text.trim().to_lowercase() }
    fn clip_mode(&self) -> bool { self.query().starts_with(';') }

    /// The clipboard entries that match what follows the ";".
    fn clip_items(&self) -> Vec<Value> {
        let q = self.query();
        let q = q[1.min(q.len())..].trim().to_string();
        let items = self.clip.lock().ok().and_then(|c| c.as_ref().and_then(|v| v["items"].as_array().cloned())).unwrap_or_default();
        items.into_iter().filter(|c| match c["text"].as_str().filter(|t| !t.is_empty()) { Some(t) => t.to_lowercase().contains(&q), None => q.is_empty() }).collect()
    }

    fn rows(&mut self, cx: &Ctx) -> usize {
        if self.clip_mode() { return self.clip_items().len(); }
        self.flat = build(&cx.wm, count(cx), &self.query()).into_iter().flat_map(|w| w.wins).collect();
        self.flat.len()
    }

    fn load_clip(&mut self, cx: &Ctx) {
        if self.clip_asked { return; }
        self.clip_asked = true;
        let (cfg, slot) = (Config(cx.cfg.0.clone()), self.clip.clone());
        std::thread::spawn(move || {
            let r = proc::run(&cfg, "toggles", &["-Action", "clipboard"]);
            if let Ok(mut s) = slot.lock() { *s = Some(if r.is_null() { serde_json::json!({ "error": "Could not read the clipboard history" }) } else { r }); }
            app::post(Box::new(|_| {}));
        });
    }

    fn pick(&mut self, cx: &mut Ctx, i: usize) {
        let Some(n) = self.clip_items().get(i).map(|c| c["i"].to_string()) else { return };
        let cfg = Config(cx.cfg.0.clone());
        std::thread::spawn(move || { proc::run(&cfg, "toggles", &["-Action", "clipboard-set", "-Value", &n]); });
        cx.actions.push(Action::Close("overview".into()));
    }

    fn focus_window(&self, cx: &mut Ctx, id: &str) {
        glaze::focus(id, self.flat.iter().any(|w| w.id == id && w.minimized));
        cx.actions.push(Action::Close("overview".into()));
    }

    fn set_query(&mut self, s: &str) { self.q.set(s); self.sel = 0; self.reveal = true; }

    fn nav(&mut self, vk: u32, shift: bool, cx: &Ctx) -> bool {
        let n = self.rows(cx);
        match step(self.sel, n, vk, shift) { Some(s) => { self.sel = s; self.reveal = true; true } None => false }
    }
}

fn count(cx: &Ctx) -> usize { cx.cfg.get("workspaces").as_u64().unwrap_or(10) as usize }

/// hashSlot() in core.js: a stable 0..n colour slot for an app name (JS charCodeAt: the first UTF-16 unit of each code point).
pub fn hash_slot(s: &str, n: u32) -> u32 {
    s.chars().fold(7u32, |a, c| a.wrapping_mul(31).wrapping_add(c.encode_utf16(&mut [0; 2])[0] as u32)) % n
}

/// Keyboard selection: Down / Tab forward, Up / Shift+Tab back, clamped to the list.
fn step(sel: usize, len: usize, vk: u32, shift: bool) -> Option<usize> {
    match (vk, shift) {
        (0x28, _) | (0x09, false) => Some((sel + 1).min(len.saturating_sub(1))),
        (0x26, _) | (0x09, true) => Some(sel.saturating_sub(1)),
        _ => None,
    }
}

pub struct WsView { pub name: String, pub display: String, pub exists: bool, pub total: usize, pub wins: Vec<GWin> }

/// The workspaces to show (1..count, then any others GlazeWM has) with the windows matching `query`
/// (lower-cased, no clipboard prefix). With a query, workspaces without a match are dropped.
pub fn build(wm: &Wm, count: usize, query: &str) -> Vec<WsView> {
    let known: Vec<(&str, &str)> = wm.monitors.iter().flat_map(|m| m.workspaces.iter().map(|w| (w.name.as_str(), w.display.as_str()))).collect();
    let mut names: Vec<String> = (1..=count).map(|i| i.to_string()).collect();
    for (n, _) in &known { if !names.iter().any(|x| x == n) { names.push(n.to_string()); } }
    names.into_iter().filter_map(|name| {
        let info = known.iter().find(|k| k.0 == name);
        let all: Vec<&GWin> = wm.windows.iter().filter(|w| w.ws == name).collect();
        let wins: Vec<GWin> = all.iter().filter(|w| query.is_empty() || format!("{} {}", w.title, w.process).to_lowercase().contains(query)).map(|w| (*w).clone()).collect();
        if !query.is_empty() && wins.is_empty() { return None; }
        let display = info.map(|k| if k.1.is_empty() { k.0 } else { k.1 }).unwrap_or(&name).to_string();
        Some(WsView { exists: info.is_some(), total: all.len(), display, name, wins })
    }).collect()
}

/// What --render shows without GlazeWM: a few windows (only with ZORUI_RENDER_SAMPLE set).
fn sample() -> Wm {
    use glaze::{Monitor, Ws};
    let w = |id: &str, t: &str, p: &str, ws: &str, focus: bool, floating: bool| GWin { id: id.into(), title: t.into(), process: p.into(), focus, floating, minimized: false, ws: ws.into(), maximized: false };
    let ws = |n: &str| Ws { name: n.into(), display: n.into(), displayed: n == "1", occupied: true };
    Wm {
        monitors: vec![Monitor { id: "m".into(), focus: true, workspaces: ["1", "2", "3"].map(ws).to_vec(), ..Default::default() }],
        windows: vec![w("a", "zor-ui - Visual Studio Code", "Code", "1", true, false), w("b", "Docs - Google Chrome", "chrome", "1", false, false),
            w("c", "Downloads", "explorer", "2", false, true), w("d", "PowerShell", "WindowsTerminal", "3", false, false)],
        workspace: "1".into(),
        ..Default::default()
    }
}

/// One window tile at `r` (also the drag ghost, which has no hit spot).
fn tile(c: &mut Canvas, cx: &Ctx, r: R, w: &GWin, id: Option<u32>, sel: bool, dim: bool) {
    let t = &cx.theme;
    let over = id.is_some_and(|i| c.hit(r, i));
    let hot = over || sel;
    let (bg, fg) = if hot { (t.c("secondaryContainer"), t.c("onSecondaryContainer")) } else { (t.c("surfaceContainer"), t.c("onSurface")) };
    let o = if dim { 0.4 } else { 1.0 };
    c.rect(r, 17.0, alpha(bg, o));
    if w.focus {
        c.rect(r, 17.0, alpha(t.c("primary"), o));
        c.rect(R::new(r.x + 2.0, r.y + 2.0, r.w - 4.0, r.h - 4.0), 15.0, alpha(bg, o));
    }
    let app = pretty_app(&w.process);
    let (cb, cf) = match hash_slot(&w.process, 3) {
        0 => ("primaryContainer", "onPrimaryContainer"),
        1 => ("secondaryContainer", "onSecondaryContainer"),
        _ => ("tertiaryContainer", "onTertiaryContainer"),
    };
    let b = R::new(r.x + 10.0, r.y + 8.0, 32.0, 32.0);
    c.rect(b, 12.0, alpha(t.c(cb), o));
    c.text(&app.chars().next().map(String::from).unwrap_or_default(), b, txt(14.0, 700), alpha(t.c(cf), o), Align::Center);
    let tw = r.w - 10.0 - 42.0 - 10.0 - if w.floating { 15.0 + 10.0 } else { 0.0 };
    let (h1, h2) = (12.5 * ui::LH, 11.5 * ui::LH);
    let ty = r.y + (r.h - h1 - h2) / 2.0;
    c.text(&app, R::new(r.x + 52.0, ty, tw, h1), txt(12.5, 500), alpha(fg, o), Align::Left);
    c.text(&w.title, R::new(r.x + 52.0, ty + h1, tw, h2), txt(11.5, 400), alpha(fg, 0.75 * o), Align::Left);
    if w.floating { c.icon("picture_in_picture", R::new(r.x + r.w - 10.0 - 15.0, r.y, 15.0, r.h), ico(15.0, false), alpha(fg, 0.7 * o)); }
}

impl View for Overview {
    fn size(&self, _: &Gfx, _: &Ctx, _: f32) -> (f32, f32) { self.area }
    fn key(&self, _: &Ctx) -> String { String::new() }
    fn frame_ms(&self) -> u32 { if self.focus { 500 } else { 0 } } // the caret blinks

    fn context(&mut self, id: u32, cx: &Ctx) -> Vec<Item> {
        if id >= CLIPS {
            let Some(n) = self.clip_items().get((id - CLIPS) as usize).map(|c| c["i"].to_string()) else { return vec![] };
            let cfg = Config(cx.cfg.0.clone());
            return vec![Item::new("content_paste", "Copy to clipboard", move |cx| {
                std::thread::spawn(move || { proc::run(&cfg, "toggles", &["-Action", "clipboard-set", "-Value", &n]); });
                cx.actions.push(Action::Close("overview".into()));
            })];
        }
        if !(TILES..HEADS).contains(&id) { return vec![]; }
        self.rows(cx);
        let Some(w) = self.flat.get((id - TILES) as usize) else { return vec![] };
        let (a, b, m) = (w.id.clone(), w.id.clone(), w.minimized);
        vec![
            Item::new("open_in_new", "Focus window", move |cx| { glaze::focus(&a, m); cx.actions.push(Action::Close("overview".into())); }),
            Item::new("close", "Close window", move |_| glaze::command_on(&b, "close")),
        ]
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let (w, h) = (c.w, c.h);
        let (edge, fill) = (alpha(t.c("outlineVariant"), 0.45), alpha(ui::layer0(cx), 0.94));
        c.backdrop(&[0x0F, c.w as u32, c.h as u32, (c.scale * 100.0) as u32, edge, fill], |c| {
            c.rect(R::new(0.0, 0.0, w, h), 30.0, edge);
            c.rect(R::new(1.0, 1.0, w - 2.0, h - 2.0), 29.0, fill);
        });
        // search
        let sw = 560f32.min(w - 2.0 * PAD);
        let sr = R::new((w - sw) / 2.0, PAD, sw, 50.0);
        c.rect(sr, 25.0, t.c("surfaceContainer"));
        c.hit(sr, SEARCH);
        c.icon("search", R::new(sr.x + 18.0, sr.y, 20.0, 50.0), ico(20.0, false), t.c("onSurfaceVariant"));
        let btn = R::new(sr.x + sr.w - 18.0 - 36.0, sr.y + 7.0, 36.0, 36.0);
        self.q.draw(c, cx, R::new(sr.x + 48.0, sr.y, btn.x - sr.x - 48.0 - 10.0, 50.0), txt(16.0, 400), "Search windows, or type ; for clipboard history", self.focus);
        ui::icon_btn(c, cx, btn, CLIP_BTN, "content_paste", ui::IconKind::Plain, 20.0, false);
        let clip = self.clip_mode();
        let hint = if clip { "Enter or click copies it back · arrows to move · Esc to close" } else { "Enter to focus · arrows to move · drag a window onto a workspace to move it · Esc to close" };
        c.text(hint, R::new(0.0, sr.y + 50.0 + 8.0, w, 12.0 * ui::LH), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Center);
        let g = R::new(PAD, sr.y + 50.0 + 8.0 + 12.0 * ui::LH + 14.0, w - 2.0 * PAD, h - PAD - (sr.y + 50.0 + 8.0 + 12.0 * ui::LH + 14.0));
        self.cells.clear();
        if clip { self.paint_clip(c, cx, g) } else { self.paint_grid(c, cx, g) }
        // the dragged tile follows the pointer
        if let Some(d) = self.drag.as_ref().filter(|d| d.moving) {
            if let Some(win) = self.flat.get(d.idx) { tile(c, cx, R::new(d.at.0 - 30.0, d.at.1 - 20.0, 210.0, TILE), win, None, false, false); }
        }
        self.reveal = false;
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match ev {
            Input::Focus(f) => self.focus = f,
            Input::Key { vk, ctrl, shift, alt } => {
                if alt && (0x30..=0x39).contains(&vk) {
                    let n = vk - 0x30;
                    glaze::command(&format!("focus --workspace {}", if n == 0 { 10 } else { n }));
                    return cx.actions.push(Action::Close("overview".into()));
                }
                if vk == 0x0D { // Enter
                    let n = self.rows(cx);
                    if self.sel < n {
                        if self.clip_mode() { let s = self.sel; self.pick(cx, s) } else { let id = self.flat[self.sel].id.clone(); self.focus_window(cx, &id) }
                    }
                } else if !self.nav(vk, shift, cx) && self.q.key(vk, ctrl, shift, alt) == Edit::Changed { self.sel = 0; self.reveal = true; }
            }
            Input::Char(ch) => if self.q.ch(ch) == Edit::Changed { self.sel = 0; self.reveal = true; },
            Input::Click(CLIP_BTN, Btn::Left) => { self.set_query(";"); self.focus = true; }
            Input::Click(i, Btn::Left) if i >= CLIPS => self.pick(cx, (i - CLIPS) as usize),
            Input::Click(i, Btn::Left) if (HEADS..BODIES).contains(&i) => {
                if let Some(n) = build(&cx.wm, count(cx), &self.query()).get((i - HEADS) as usize).map(|w| w.name.clone()) {
                    glaze::command(&format!("focus --workspace {n}"));
                    cx.actions.push(Action::Close("overview".into()));
                }
            }
            Input::Click(i, Btn::Left) if (TILES..HEADS).contains(&i) => {
                if std::mem::take(&mut self.dragged) { return; }
                self.rows(cx);
                if let Some(w) = self.flat.get((i - TILES) as usize) { let id = w.id.clone(); self.focus_window(cx, &id); }
            }
            Input::Click(i, Btn::Middle) if (TILES..HEADS).contains(&i) => {
                self.rows(cx);
                if let Some(w) = self.flat.get((i - TILES) as usize) { glaze::command_on(&w.id, "close"); }
            }
            Input::Drag(SEARCH, x, y, Phase::Start) => { self.q.press(x, y); self.focus = true; }
            Input::Drag(i, x, y, ph) if (TILES..HEADS).contains(&i) => {
                let idx = (i - TILES) as usize;
                match ph {
                    Phase::Start => {
                        self.dragged = false;
                        self.rows(cx);
                        self.drag = self.flat.get(idx).map(|w| Dragging { id: w.id.clone(), idx, from: (x, y), at: (x, y), moving: false });
                    }
                    Phase::Move => if let Some(d) = &mut self.drag {
                        d.at = (x, y);
                        if (x - d.from.0).hypot(y - d.from.1) >= 6.0 { d.moving = true; } // a plain click stays a click
                    },
                    Phase::End => if let Some(d) = self.drag.take().filter(|d| d.moving) {
                        self.dragged = true;
                        if let Some((_, ws)) = self.cells.iter().find(|(r, _)| r.contains(x, y)) { glaze::command_on(&d.id, &format!("move --workspace {ws}")); }
                    },
                }
            }
            Input::Wheel(i, d) => {
                if i == CLIP_LIST || i >= CLIPS { return self.clip_scroll.wheel(d, 1e9, 0.0); }
                let name = if i >= BODIES { build(&cx.wm, count(cx), &self.query()).get((i - BODIES) as usize).map(|w| w.name.clone()) }
                    else if i >= TILES { self.rows(cx); self.flat.get((i - TILES) as usize).map(|w| w.ws.clone()) } else { None };
                if let Some(n) = name { self.scrolls.entry(n).or_default().wheel(d, 1e9, 0.0); }
            }
            _ => {}
        }
    }
}

impl Overview {
    fn paint_grid(&mut self, c: &mut Canvas, cx: &Ctx, g: R) {
        let t = &cx.theme;
        let sampled = cx.wm.monitors.is_empty() && std::env::var_os("ZORUI_RENDER_SAMPLE").is_some();
        let wm = if sampled { sample() } else { cx.wm.clone() };
        if wm.monitors.is_empty() { return c.text("GlazeWM is not running", g, txt(12.0, 400), t.c("outline"), Align::Center); }
        let list = build(&wm, count(cx), &self.query());
        self.flat.clear();
        if list.is_empty() {
            let cell = R::new(g.x, g.y, (g.w - 4.0 * GAP) / 5.0, g.h);
            return c.text(&format!("No windows match “{}”", self.query()), cell, txt(12.0, 400), t.c("outline"), Align::Center);
        }
        let flat: Vec<GWin> = list.iter().flat_map(|w| w.wins.clone()).collect();
        self.sel = self.sel.min(flat.len().saturating_sub(1));
        let cols = columns(g.w, g.h);
        let rows = list.len().div_ceil(cols);
        let (cw, ch) = ((g.w - (cols - 1) as f32 * GAP) / cols as f32, (g.h - (rows - 1) as f32 * GAP) / rows as f32);
        let mut n = 0;
        for (k, ws) in list.iter().enumerate() {
            let cell = R::new(g.x + (k % cols) as f32 * (cw + GAP), g.y + (k / cols) as f32 * (ch + GAP), cw, ch);
            let cur = ws.name == wm.workspace;
            let drop = self.drag.as_ref().is_some_and(|d| d.moving && cell.contains(d.at.0, d.at.1));
            self.cells.push((cell, ws.name.clone()));
            if cur || drop {
                c.rect(cell, 23.0, t.c("primary")); // the outline (a drop target should be dashed; solid here)
                c.rect(R::new(cell.x + 2.0, cell.y + 2.0, cw - 4.0, ch - 4.0), 21.0, t.c(if drop { "surfaceContainerHigh" } else { "surfaceContainerLow" }));
            } else { c.rect(cell, 23.0, t.c("surfaceContainerLow")); }
            // header
            let hr = R::new(cell.x + 12.0, cell.y + 12.0, cw - 24.0, 34.0);
            c.hit(R::new(hr.x, hr.y, hr.w, 34.0), HEADS + k as u32);
            let b = R::new(hr.x + 4.0, hr.y + 2.0, 24.0, 24.0);
            c.circle(b.x + 12.0, b.y + 12.0, 12.0, t.c(if cur { "primary" } else { "surfaceContainerHigh" }));
            c.text(&ws.display, b, txt(12.0, 600), t.c(if cur { "onPrimary" } else { "onSurface" }), Align::Center);
            let label = if ws.exists { format!("{} window{}", ws.total, if ws.total == 1 { "" } else { "s" }) } else { "Empty".into() };
            c.text(&label, R::new(b.x + 32.0, b.y, hr.w - 40.0, 24.0), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left);
            // tiles
            let body = R::new(cell.x + 12.0, cell.y + 12.0 + 34.0, cw - 24.0, (ch - 24.0 - 34.0).max(0.0));
            c.hit(body, BODIES + k as u32);
            let content = ws.wins.len() as f32 * (TILE + 6.0) - if ws.wins.is_empty() { 0.0 } else { 6.0 };
            let mut sc = self.scrolls.remove(&ws.name).unwrap_or_default();
            let first = n;
            if self.reveal && self.sel >= first && self.sel < first + ws.wins.len() {
                let (top, bot) = ((self.sel - first) as f32 * (TILE + 6.0), (self.sel - first) as f32 * (TILE + 6.0) + TILE);
                if top < sc.y { sc.y = top } else if bot > sc.y + body.h { sc.y = bot - body.h }
            }
            let (sel, dragging) = (self.sel, self.drag.as_ref().filter(|d| d.moving).map(|d| d.idx));
            sc.region(c, cx, body, content, |c, top| {
                for (j, win) in ws.wins.iter().enumerate() {
                    let r = R::new(body.x, top + j as f32 * (TILE + 6.0), body.w, TILE);
                    if r.y + r.h < body.y || r.y > body.y + body.h { continue; }
                    let i = first + j;
                    tile(c, cx, r, win, Some(TILES + i as u32), i == sel, dragging == Some(i));
                }
            });
            self.scrolls.insert(ws.name.clone(), sc);
            n += ws.wins.len();
        }
        self.flat = flat;
    }

    fn paint_clip(&mut self, c: &mut Canvas, cx: &Ctx, g: R) {
        let t = &cx.theme;
        self.load_clip(cx);
        let loaded = self.clip.lock().ok().and_then(|s| s.clone());
        let msg = |c: &mut Canvas, s: &str| c.text(s, R::new(g.x, g.y, g.w, 40.0), txt(12.0, 400), t.c("outline"), Align::Center);
        let Some(v) = loaded else { return msg(c, "Loading…") };
        if let Some(e) = v["error"].as_str() { return msg(c, e); }
        let items = self.clip_items();
        if items.is_empty() { return msg(c, if v["items"].as_array().is_some_and(|a| !a.is_empty()) { "Nothing matches" } else { "Clipboard history is empty" }); }
        self.sel = self.sel.min(items.len() - 1);
        let (f, tw) = (txt(13.5, 400), g.w - 28.0 - 20.0 - 12.0 - 12.0 - 40.0);
        let hs: Vec<f32> = items.iter().map(|i| 20.0 + c.g.measure_wrapped(i["text"].as_str().filter(|s| !s.is_empty()).unwrap_or("Image"), f, tw).min(3.0 * 13.5 * ui::LH)).collect();
        let content = hs.iter().sum::<f32>() + 6.0 * (hs.len() - 1) as f32;
        let tops: Vec<f32> = hs.iter().scan(0.0, |y, h| { let t = *y; *y += h + 6.0; Some(t) }).collect();
        if self.reveal { let (a, b) = (tops[self.sel], tops[self.sel] + hs[self.sel]); if a < self.clip_scroll.y { self.clip_scroll.y = a } else if b > self.clip_scroll.y + g.h { self.clip_scroll.y = b - g.h } }
        c.hit(g, CLIP_LIST);
        let (sel, now) = (self.sel, hub::now_ms() / 1000.0);
        self.clip_scroll.region(c, cx, g, content, |c, top| {
            for (k, it) in items.iter().enumerate() {
                let r = R::new(g.x, top + tops[k], g.w, hs[k]);
                if r.y + r.h < g.y || r.y > g.y + g.h { continue; }
                let over = c.hit(r, CLIPS + k as u32);
                c.rect(r, 17.0, t.c(if over { "surfaceContainerHigh" } else { "surfaceContainer" }));
                if k == sel { c.rect(r, 17.0, t.c("primary")); c.rect(R::new(r.x + 2.0, r.y + 2.0, r.w - 4.0, r.h - 4.0), 15.0, t.c(if over { "surfaceContainerHigh" } else { "surfaceContainer" })); }
                let text = it["text"].as_str().filter(|s| !s.is_empty());
                c.icon(if text.is_some() { "notes" } else { "image" }, R::new(r.x + 14.0, r.y + 10.0, 20.0, 20.0), ico(20.0, false), t.c("onSurfaceVariant"));
                c.para(text.unwrap_or("Image"), R::new(r.x + 46.0, r.y + 10.0, tw, r.h - 10.0), f, t.c("onSurface"), Align::Left);
                let a = ago(now, it["time"].as_f64().unwrap_or(now));
                c.text(&a, R::new(r.x + r.w - 14.0 - 40.0, r.y + 10.0, 40.0, 11.5 * ui::LH), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Right);
            }
        });
        self.clips = items;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: &str, title: &str, process: &str, ws: &str) -> GWin { GWin { id: id.into(), title: title.into(), process: process.into(), focus: false, floating: false, minimized: false, ws: ws.into(), maximized: false } }

    #[test]
    fn tall_screens_get_fewer_columns() {
        assert_eq!([columns(1574.0, 799.0), columns(900.0, 800.0), columns(1015.0, 1536.0)], [5, 3, 2]);
    }

    #[test]
    fn slots_match_core_js() {
        // values from lib/core.js hashSlot
        assert_eq!([hash_slot("chrome", 3), hash_slot("Code", 3), hash_slot("explorer", 3), hash_slot("WindowsTerminal", 3), hash_slot("日本語x", 3)], [1, 2, 2, 2, 0]);
    }

    #[test]
    fn filter_and_extra_workspaces() {
        let sample = sample();
        let all = build(&sample, 2, "");
        assert_eq!(all.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]); // "3" is not in 1..=2 but exists
        assert_eq!(all[0].total, 2);
        let q = build(&sample, 2, "chrome");
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].name.as_str(), q[0].wins[0].id.as_str(), q[0].total), ("1", "b", 2));
        let none = Wm { windows: vec![win("x", "A", "code", "9")], ..Default::default() };
        let w = build(&none, 3, "");
        assert!(!w[0].exists && w[0].total == 0); // "Empty"
    }

    #[test]
    fn keyboard_selection() {
        assert_eq!(step(0, 3, 0x28, false), Some(1));
        assert_eq!(step(2, 3, 0x09, false), Some(2)); // clamped
        assert_eq!(step(0, 3, 0x09, true), Some(0));
        assert_eq!(step(2, 3, 0x26, false), Some(1));
        assert_eq!(step(0, 0, 0x28, false), Some(0));
        assert_eq!(step(0, 3, 0x41, false), None);
    }

    #[test]
    fn clipboard_query() {
        let mut o = Overview::new((100.0, 100.0));
        *o.clip.lock().unwrap() = Some(serde_json::json!({ "items": [{ "i": 0, "text": "Hello World", "time": 1 }, { "i": 1, "text": "", "time": 2 }, { "i": 2, "text": "other", "time": 3 }] }));
        o.q.set(";");
        assert_eq!(o.clip_items().len(), 3);
        o.q.set(";  WORLD ");
        assert_eq!(o.clip_items().iter().map(|c| c["i"].as_i64().unwrap()).collect::<Vec<_>>(), [0]);
        assert!(o.clip_mode());
    }
}
