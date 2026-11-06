//! Desktop cards (desktop.js / desktop.html): one bottom-most window per card, draggable.

mod np;
mod small;

use crate::{
    app::{Action, App, Ctx},
    conf::Config,
    hub,
    menu::Item,
    proc,
    gfx::{alpha, cookie_points, ico, polygon, txt, Align, Canvas, Gfx, Pixmap, Shape, R},
    util::{self, add_days, date_fmt, days_in_month, monday_index},
    weather,
    win::{Input, View, Win, Z},
};
use serde_json::json;
use std::{path::PathBuf, sync::{atomic::{AtomicIsize, Ordering}, Arc}, time::SystemTime};

/// Cards ported (all of them; desktop.js keeps the same list in NATIVE).
pub const PORTED: &[&str] = &["clock", "weather", "week", "greet", "calendar", "worldclock", "resources", "text", "image1", "image2", "image3", "sticker",
    "media", "lyrics", "cover", "todo", "notes", "timer"];

const W: f32 = 310.0; // #col width; cards sit 4px inside it
const CELL_W: f32 = 130.0; // a preset card grid cell, and the gap between cells
const CELL_H: f32 = 120.0;
const GAP: f32 = 12.0;
/// Inner size in logical px of a preset `sizeMode` ("<rows>x<cols>" cells), None for anything else.
pub fn grid_size(mode: &str) -> Option<(f32, f32)> {
    let (r, c) = mode.split_once('x')?;
    let cells = |n: &str| n.parse::<u32>().ok().filter(|n| (1..=6).contains(n)).map(|n| n as f32);
    let (r, c) = (cells(r)?, cells(c)?);
    Some((c * CELL_W + (c - 1.0) * GAP, r * CELL_H + (r - 1.0) * GAP))
}

/// Cards that follow `desktop.sizes.<id>`; every other card sizes itself from its content.
pub const SIZED: [&str; 7] = ["media", "calendar", "weather", "greet", "resources", "image1", "worldclock"];

/// (rows, columns) of preset cells a card window of this size holds: the inverse of `grid_size`.
fn cells(c: &Canvas) -> (u32, u32) {
    let n = |len: f32, cell: f32| (((len - 8.0 + GAP) / (cell + GAP)).round() as i32).max(1) as u32;
    (n(c.h, CELL_H), n(c.w, CELL_W))
}

/// The preset size of this card (outer window size, 4px margin each side) if the preset gave one.
fn sized(cx: &Ctx, id: &str) -> Option<(f32, f32)> {
    grid_size(cx.cfg.str(&format!("desktop.sizes.{id}"))).map(|(w, h)| (w + 8.0, h + 8.0))
}

const LH: f32 = 1.252; // Google Sans Flex line height (ascent + descent) per px of font size

/// Rough heights used only to stack cards that were never dragged (lib/desk.js).
fn slot_height(id: &str) -> f32 {
    match id {
        "clock" => 250.0, "weather" => 92.0, "week" | "greet" => 90.0, "media" => 92.0, "calendar" => 270.0, "todo" => 150.0,
        "notes" => 170.0, "timer" => 100.0, "worldclock" => 140.0, "resources" => 130.0, "text" => 60.0, "lyrics" => 390.0,
        "cover" => 290.0, "image1" | "image2" | "image3" => 310.0, "sticker" => 180.0, _ => 120.0,
    }
}

/// defaultSlot() in lib/desk.js: stacked in list order, wrapping into columns (logical px).
pub fn default_slot(widgets: &[String], id: &str, screen_h: f32) -> (f32, f32) {
    const GAP: f32 = 10.0; const WIDTH: f32 = 428.0; const TOP: f32 = 72.0; const LEFT: f32 = 16.0;
    let (mut x, mut y) = (LEFT, TOP);
    for w in widgets {
        let h = slot_height(w);
        if y > TOP && y + h > screen_h - 16.0 { x += WIDTH + GAP; y = TOP; }
        if w == id { return (x, y); }
        y += h + GAP;
    }
    (LEFT, TOP)
}

/// How far (physical px, signed) a span `mine` = (start, end) must move to line up with `others` (spans on the same
/// axis): start, centre and end meet start, centre and end, and a card sits `gap` from a neighbour's far edge.
/// 0 when nothing is within `thr`.
pub fn snap_axis(mine: (i32, i32), others: &[(i32, i32)], gap: i32, thr: i32) -> i32 {
    let mut best: Option<i32> = None;
    let mut consider = |d: i32| if d.abs() <= thr && best.map_or(true, |b| d.abs() < b.abs()) { best = Some(d); };
    let mid = |a: i32, b: i32| (a + b) / 2;
    for &(os, oe) in others {
        for t in [os, mid(os, oe), oe] {
            for m in [mine.0, mid(mine.0, mine.1), mine.1] { consider(t - m); }
        }
        consider(oe + gap - mine.0);
        consider(os - gap - mine.1);
    }
    best.unwrap_or(0)
}

/// Where a dragged card at `(x, y)` of size `(w, h)` ends up: snapped to the other cards' and the monitor's edges
/// and centres. `others` are (left, top, right, bottom) of what it can snap to; `m` is the transparent margin
/// around a card's visible shape.
pub fn snap_card((x, y): (i32, i32), (w, h): (i32, i32), m: i32, others: &[(i32, i32, i32, i32)], gap: i32, thr: i32) -> (i32, i32) {
    let xs: Vec<(i32, i32)> = others.iter().map(|o| (o.0, o.2)).collect();
    let ys: Vec<(i32, i32)> = others.iter().map(|o| (o.1, o.3)).collect();
    (x + snap_axis((x + m, x + w - m), &xs, gap, thr), y + snap_axis((y + m, y + h - m), &ys, gap, thr))
}

/// The visible rectangles of the other desktop cards and of every work area, for `snap_card`.
pub fn snap_targets(me: windows::Win32::Foundation::HWND, m: i32) -> Vec<(i32, i32, i32, i32)> {
    use windows::Win32::{Foundation::{BOOL, HWND, LPARAM, RECT}, UI::WindowsAndMessaging::{EnumWindows, GetWindowRect, GetWindowTextW, IsWindowVisible}};
    struct Scan { me: HWND, m: i32, out: Vec<(i32, i32, i32, i32)> }
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let s = &mut *(lp.0 as *mut Scan);
        let mut t = [0u16; 48];
        let n = GetWindowTextW(h, &mut t) as usize;
        if h != s.me && IsWindowVisible(h).as_bool() && String::from_utf16_lossy(&t[..n]).starts_with("Zor UI / desk-") {
            let mut r = RECT::default();
            if GetWindowRect(h, &mut r).is_ok() { s.out.push((r.left + s.m, r.top + s.m, r.right - s.m, r.bottom - s.m)); }
        }
        true.into()
    }
    let mut scan = Scan { me, m, out: vec![] };
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut scan as *mut _ as isize)); }
    let (mons, _) = util::monitors();
    scan.out.extend(mons.iter().map(|r| (r.left, r.top, r.right, r.bottom)));
    scan.out
}

/// Rows in this month's calendar grid.
pub fn month_rows() -> usize { month_cells(&util::now()).len() / 7 }

/// Opens enabled cards and closes disabled ones (on start and whenever config.json changes).
pub fn sync(app: &mut App) {
    let cfg = &app.cx.cfg;
    let want: Vec<String> = if app.cx.native("desktop") && cfg.get("desktopWidgets").as_bool() != Some(false) {
        cfg.strs("desktop.widgets").into_iter().filter(|w| PORTED.contains(&w.as_str())).collect()
    } else { vec![] };
    app.wins.retain(|w| w.drag_key.as_ref().map_or(true, |k| want.contains(k)));
    let (mons, work) = util::monitors();
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
    app.cx.state.reload(); // a preset apply writes the positions from another thread
    let saved_at = |cx: &Ctx, id: &str| cx.state.v["deskPos"][id].as_object().and_then(|p| Some((p.get("x")?.as_i64()? as i32, p.get("y")?.as_i64()? as i32)));
    let on_screen = |(x, y): (i32, i32)| mons.iter().any(|m| x >= m.left - 40 && x < m.right - 40 && y >= m.top && y < m.bottom - 40);
    for w in &app.wins {
        // cards that are already open follow a changed saved position (a preset moved them)
        let Some(id) = w.drag_key.as_deref() else { continue };
        if let Some(p) = saved_at(&app.cx, id).filter(|p| on_screen(*p) && *p != w.pos()) { w.move_to(p.0, p.1); }
    }
    for id in &want {
        if app.wins.iter().any(|w| w.drag_key.as_deref() == Some(id)) { continue; }
        let saved = saved_at(&app.cx, id);
        let (x, y) = saved.filter(|&p| on_screen(p)).unwrap_or_else(|| {
            let (x, y) = default_slot(&cfg.strs("desktop.widgets"), id, (work.bottom - work.top) as f32 / scale);
            ((x * scale) as i32 + work.left, (y * scale) as i32 + work.top)
        });
        let card = Card::new(id);
        let hwnd = card.hwnd.clone();
        let Ok(mut w) = Win::new(Box::new(card), &format!("Zor UI / desk-{id}"), x, y, Z::Bottom, false) else { continue };
        w.drag_key = Some(id.clone());
        hwnd.store(w.hwnd.0 as isize, Ordering::Relaxed);
        w.update(&app.g, &app.cx, true);
        w.show();
        app.wins.push(w);
    }
}

/// A card was dragged: remember where (physical px, like Zebar's localStorage).
pub fn save_pos(cx: &mut Ctx, id: &str, (x, y): (i32, i32)) {
    cx.state.update(|v| {
        if !v["deskPos"].is_object() { v["deskPos"] = json!({}); }
        v["deskPos"][id] = json!({ "x": x, "y": y });
    });
}

pub struct Card {
    id: String,
    /// The glass fill of this window and what it was made from.
    glass: Option<(String, Pixmap)>,
    img: Option<(PathBuf, SystemTime, u32, Pixmap)>,
    np: np::Np,
    notes: Option<small::Notes>,
    /// This card's window (notes takes keyboard focus through it).
    hwnd: Arc<AtomicIsize>,
}

fn clock_style(cx: &Ctx) -> &str { cx.cfg.str("desktop.clock") }

/// greeting() in core.js.
pub fn greeting(h: u16) -> &'static str {
    match h { 0..=4 => "Good Night", 5..=11 => "Good Morning", 12..=16 => "Good Afternoon", 17..=21 => "Good Evening", _ => "Good Night" }
}

/// monthGrid() in core.js: (day, outside this month, today), Monday first, whole weeks.
pub fn month_cells(now: &windows::Win32::Foundation::SYSTEMTIME) -> Vec<(u16, bool, bool)> {
    let first = add_days(now, 1 - now.wDay as i64);
    let offset = monday_index(&first);
    let days = days_in_month(now.wYear, now.wMonth);
    let prev = days_in_month(if now.wMonth == 1 { now.wYear - 1 } else { now.wYear }, if now.wMonth == 1 { 12 } else { now.wMonth - 1 });
    let mut cells: Vec<(u16, bool, bool)> = (0..offset).map(|i| (prev - offset + 1 + i, true, false)).collect();
    cells.extend((1..=days).map(|d| (d, false, d == now.wDay)));
    let mut n = 1;
    while cells.len() % 7 != 0 { cells.push((n, true, false)); n += 1; }
    cells
}

fn image_conf(cx: &Ctx, id: &str) -> (Shape, f32, String) {
    let c = cx.cfg.get(&format!("desktop.images.{id}"));
    let size = c["size"].as_f64().unwrap_or(if id == "sticker" { 140.0 } else { 280.0 }).clamp(120.0, 800.0) as f32;
    (Shape::parse(c["shape"].as_str().unwrap_or("cookie")), size, c["file"].as_str().unwrap_or("").to_string())
}

fn greet_conf(cx: &Ctx) -> (String, String, bool, String, String) {
    let g = cx.cfg.get("desktop.greet");
    (g["text"].as_str().unwrap_or("").into(), g["sub"].as_str().unwrap_or("").into(), g["date"].as_bool().unwrap_or(true),
        g["file"].as_str().unwrap_or("").into(), g["shape"].as_str().unwrap_or("").into())
}

impl Card {
    pub fn new(id: &str) -> Card {
        Card { id: id.into(), glass: None, img: None, np: np::Np::new(), notes: (id == "notes").then(small::Notes::new), hwnd: Default::default() }
    }

    fn key_time(&self, cx: &Ctx) -> String {
        let st = util::now();
        match self.id.as_str() {
            "clock" if matches!(clock_style(cx), "digital" | "pixel") => format!("{}{}{}", st.wHour, st.wMinute, st.wDay),
            "clock" => format!("{}{}{}", st.wHour, st.wMinute, st.wSecond),
            "worldclock" => format!("{}{}", st.wHour, st.wMinute),
            "greet" => format!("{}{}", st.wHour, st.wDay),
            "todo" => small::todo_key(),
            "notes" => self.notes.as_ref().map_or_else(String::new, |n| n.key()),
            "timer" => small::timer_key(),
            "week" | "calendar" => format!("{}{}", st.wDay, st.wMonth),
            _ => String::new(),
        }
    }

    /// This window's rectangle on screen (physical px).
    fn screen_rect(&self) -> Option<windows::Win32::Foundation::RECT> {
        use windows::Win32::{Foundation::{HWND, RECT}, UI::WindowsAndMessaging::GetWindowRect};
        let h = self.hwnd.load(Ordering::Relaxed);
        let mut r = RECT::default();
        (h != 0 && unsafe { GetWindowRect(HWND(h as _), &mut r) }.is_ok()).then_some(r)
    }

    /// The glass fill for `rects` (logical px, the visible shapes in this window), or None when glass is off or
    /// there is no wallpaper picture to look through. Re-made only when something it depends on changed.
    fn glass_back(&mut self, c: &Canvas, cx: &Ctx, rects: &[R]) -> Option<Pixmap> {
        let glass = crate::glass::Glass::from_cfg(&cx.cfg)?;
        let img = PathBuf::from(cx.state.v["image"].as_str().filter(|s| !s.is_empty())?);
        let r = self.screen_rect()?;
        let (w, h) = ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32);
        let (mons, _) = util::monitors();
        let (mx, my) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        let mon = mons.iter().find(|m| mx >= m.left && mx < m.right && my >= m.top && my < m.bottom).or(mons.first())?;
        let stamp = std::fs::metadata(&img).and_then(|m| m.modified()).ok()?;
        let key = format!("{glass:?}|{img:?}|{stamp:?}|{:?}|{:?}|{rects:?}|{}", (r.left, r.top), (w, h), c.scale);
        if let Some((k, p)) = &self.glass { if *k == key { return Some(p.clone()); } }
        let back = crate::glass::backdrop(c.g, &img, (mon.left, mon.top, mon.right, mon.bottom), (r.left, r.top, w, h))?;
        let shapes: Vec<(f32, f32, f32, f32, f32)> = rects.iter().map(|q| (q.x * c.scale, q.y * c.scale, q.w * c.scale, q.h * c.scale, 23.0 * c.scale)).collect();
        let out = crate::glass::render(&back, &shapes, &glass, c.scale)?;
        self.glass = Some((key, out.clone()));
        Some(out)
    }

    /// A picture from the pack's images folder (or the wallpaper), decoded at its drawn size.
    fn picture(&mut self, g: &Gfx, file: &str, px: u32) -> Option<Pixmap> {
        let dir = crate::conf::pack_dir();
        let path = if file == "wallpaper.jpg" { dir.join(file) } else { dir.join("images").join(file) };
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?; // a new wallpaper rewrites wallpaper.jpg
        if let Some((p, t, s, b)) = &self.img { if *p == path && *t == stamp && *s == px { return Some(b.clone()); } }
        let b = g.load_image(&path, px, px)?;
        self.img = Some((path, stamp, px, b.clone()));
        Some(b)
    }

    /// `picture` at `w` x `h` px (centre-cropped), for cards that fill a preset-sized frame.
    fn picture_wh(&mut self, g: &Gfx, file: &str, w: u32, h: u32) -> Option<Pixmap> {
        let path = crate::conf::pack_dir().join("images").join(file);
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        let key = (w << 16) | (h & 0xFFFF);
        if let Some((p, t, s, b)) = &self.img { if *p == path && *t == stamp && *s == key { return Some(b.clone()); } }
        let b = g.load_image(&path, w, h)?;
        self.img = Some((path, stamp, key, b.clone()));
        Some(b)
    }

    fn shaped_picture(&mut self, c: &mut Canvas, file: &str, shape: Shape, r: R, scale: f32) -> bool {
        let Some(bmp) = self.picture(c.g, file, (r.w * scale).round() as u32) else { return false };
        c.clip(shape.path(r), |c| c.image(&bmp, r, 1.0));
        true
    }
}

impl View for Card {
    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match self.id.as_str() {
            "media" | "lyrics" | "cover" => self.np.input(ev, cx),
            "todo" => small::todo_input(ev),
            "timer" => small::timer_input(ev, cx),
            "notes" => if let Some(n) = &mut self.notes { n.input(ev, &self.hwnd) },
            _ => {}
        }
    }

    fn context(&mut self, _id: u32, cx: &Ctx) -> Vec<Item> {
        let mut items = vec![];
        let copy = match self.id.as_str() {
            "clock" => Some(util::clock(util::now().wHour, util::now().wMinute, cx.cfg.get("use24h").as_bool() != Some(false))),
            "text" => Some(cx.cfg.str("desktop.text").to_string()),
            _ => None,
        };
        if let Some(text) = copy.filter(|t| !t.is_empty()) { items.push(Item::new("content_copy", "Copy", move |_| util::clipboard_set(&text))); }
        let section = match self.id.as_str() { id if id.starts_with("image") || id == "sticker" => "Image cards", "greet" => "Greeting card", "timer" => "Pomodoro", _ => "Desktop" };
        items.push(Item::new("settings", "Customize in Settings", move |cx| {
            hub::set(crate::settings::GOTO, Some(format!("bar|{section}")));
            cx.actions.push(Action::open("settings"));
        }));
        let (cfg, id) = (Config(cx.cfg.0.clone()), self.id.clone());
        let rest: Vec<String> = cx.cfg.strs("desktop.widgets").into_iter().filter(|w| *w != id).collect();
        items.push(Item::new("visibility_off", "Remove from desktop", move |_| {
            std::thread::spawn(move || { proc::bridge(&cfg, "config-set", &["desktop.widgets", &json!(rest).to_string()]); }); // reloads config.json
        }));
        items
    }

    fn frame_ms(&self) -> u32 {
        match self.id.as_str() {
            "media" | "lyrics" | "cover" => self.np.anim,
            "notes" if self.notes.as_ref().is_some_and(|n| n.focused) => 500,
            _ => 0,
        }
    }

    fn size(&self, g: &Gfx, cx: &Ctx, _scale: f32) -> (f32, f32) {
        let pad = 8.0; // #col padding, both sides
        let h3 = 12.0 + 13.0 * LH + 8.0; // .pad top + h3 + its margin
        if self.id == "media" {
            // a preset's size only applies while something is playing (np.size is the bare pad otherwise)
            let (w, nh) = self.np.size(cx, &self.id);
            if let (Some(sz), true) = (sized(cx, &self.id), nh > pad) { return sz; }
            return (w, nh);
        }
        if SIZED.contains(&self.id.as_str()) { // media returned above
            if let Some(sz) = sized(cx, &self.id) { return sz; }
        }
        match self.id.as_str() {
            "media" | "lyrics" | "cover" => self.np.size(cx, &self.id),
            "todo" => (W, small::todo_size(g) + pad - 8.0),
            "notes" => (W, self.notes.as_ref().map_or(pad, |n| n.size(g) + pad - 8.0)),
            "timer" => (W, small::timer_size() + pad - 8.0),
            "clock" => (W, if clock_vertical(cx) { 190.0 } else if matches!(clock_style(cx), "digital" | "pixel") { 150.0 } else { 250.0 } + pad),
            "weather" => (W, 92.0 + pad),
            "week" => (W, 12.0 + 26.0 + 2.0 * (8.0 + 26.0) + 10.0 + pad),
            "greet" => (W, 90.0 + pad),
            "calendar" => {
                let rows = month_cells(&util::now()).len() as f32 / 7.0 + 1.0;
                (W, h3 + rows * 26.0 + (rows - 1.0) * 2.0 + 12.0 + pad)
            }
            "worldclock" => (W, h3 + cx.cfg.strs("desktop.worldClocks").len() as f32 * (6.0 + 18.0 * LH) + 12.0 + pad),
            "resources" => (W, h3 + 56.0 + 4.0 + 12.0 * LH + 12.0 + pad),
            "text" => {
                let t = cx.cfg.str("desktop.text");
                (W, if t.is_empty() { pad } else { 24.0 + g.measure_wrapped(t, txt(20.0, 500), W - pad - 28.0) + pad })
            }
            id if id.starts_with("image") || id == "sticker" => {
                let (_, size, file) = image_conf(cx, id);
                if file.is_empty() { (size + 32.0, 28.0 + 53.0) } else { (size + 32.0, size + 28.0) }
            }
            _ => (W, pad),
        }
    }

    fn key(&self, cx: &Ctx) -> String {
        let at = if cx.cfg.bool("desktop.glass.enable") { self.screen_rect().map(|r| format!("{},{}", r.left, r.top)).unwrap_or_default() } else { String::new() };
        format!("{}{at}", self.key_time(cx))
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let st = util::now();
        let h24 = cx.cfg.get("use24h").as_bool() != Some(false);
        let (_, h) = (c.w, c.h);
        let card = R::new(4.0, 4.0, c.w - 8.0, h - 8.0);
        let big = sized(cx, &self.id).is_some();
        let tiles = big && self.id == "resources";
        let shapes: Vec<R> = if tiles { (0..3).map(|i| R::new(4.0 + i as f32 * (CELL_W + GAP), 4.0, CELL_W, h - 8.0)).collect() } else { vec![card] };
        let glassy = matches!(self.id.as_str(), "todo" | "notes" | "timer" | "weather" | "week" | "greet" | "calendar" | "worldclock" | "resources" | "media");
        let back = if glassy { self.glass_back(c, cx, &shapes) } else { None };
        let bg = |c: &mut Canvas| match &back {
            Some(b) => c.image(b, R::new(0.0, 0.0, 0.0, 0.0), 1.0),
            None => c.rect(card, 23.0, alpha(t.c("surfaceContainerLow"), 0.88)),
        };
        let muted = t.c("onSurfaceVariant");
        let fg = t.c("onSurface");
        let scale = c.scale;
        if matches!(self.id.as_str(), "media" | "lyrics" | "cover") {
            let id = self.id.clone();
            return self.np.paint(c, cx, &id, back.as_ref());
        }

        match self.id.as_str() {
            "todo" => { bg(c); small::todo_paint(c, cx); }
            "notes" => { bg(c); if let Some(n) = &mut self.notes { n.paint(c, cx); } }
            "timer" => { bg(c); small::timer_paint(c, cx); }
            "clock" => clock(c, cx, &st, h24),
            "weather" if big => {
                bg(c);
                if let Some(w) = &cx.weather {
                    match cells(c) { (r, k) if r >= 2 && k >= 3 => big_weather(c, cx, w), (r, _) if r >= 2 => square_weather(c, cx, w), _ => small_weather(c, cx, w) }
                }
            }
            "weather" => {
                bg(c);
                let Some(w) = &cx.weather else { return };
                c.clip_round(card, 23.0, |c| {
                    c.rect(R::new(4.0, 4.0, 92.0, 92.0), 0.0, t.c("primaryContainer"));
                    let top = 4.0 + (92.0 - (34.0 + 2.0 + 18.0 * LH)) / 2.0;
                    c.icon(weather::icon(w.status), R::new(4.0, top, 92.0, 34.0), ico(34.0, true), t.c("onPrimaryContainer"));
                    c.text(&weather::temp(w, cx.cfg.str("tempUnit")), R::new(4.0, top + 36.0, 92.0, 18.0 * LH), txt(18.0, 500), t.c("onPrimaryContainer"), Align::Center);
                });
                let (x, wd) = (4.0 + 92.0 + 14.0, W - 8.0 - 92.0 - 28.0);
                let block = 15.0 * LH + 4.0 + 12.0 * LH + 4.0 + 4.0 + 11.5 * LH;
                let mut y = 4.0 + (92.0 - block) / 2.0;
                let city = if w.city.is_empty() { "Weather" } else { &w.city };
                c.text(city, R::new(x, y, wd, 15.0 * LH), txt(15.0, 500), fg, Align::Left);
                y += 15.0 * LH + 4.0;
                c.text(&weather::text(w.status).to_lowercase(), R::new(x, y, wd, 12.0 * LH), txt(12.0, 400), muted, Align::Left);
                y += 12.0 * LH + 8.0;
                c.icon("air", R::new(x, y, 14.0, 11.5 * LH), ico(14.0, false), muted);
                if let Some(s) = w.wind { c.text(&format!("{s:.1} km/h"), R::new(x + 18.0, y, wd - 18.0, 11.5 * LH), txt(11.5, 400), muted, Align::Left); }
            }
            "week" => {
                bg(c);
                let chip = date_fmt(&st, "MMMM yyyy");
                let cw = c.g.measure(&chip, txt(12.5, 500)).0 + 24.0;
                c.rect(R::new(16.0, 16.0, cw, 26.0), 13.0, t.c("secondaryContainer"));
                c.text(&chip, R::new(16.0, 16.0, cw, 26.0), txt(12.5, 500), t.c("onSecondaryContainer"), Align::Center);
                let col = (W - 8.0 - 24.0) / 7.0;
                let monday = add_days(&st, -(monday_index(&st) as i64));
                for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
                    let x = 16.0 + i as f32 * col;
                    c.text(d, R::new(x, 50.0, col, 26.0), txt(11.0, 400), muted, Align::Center);
                    let day = add_days(&monday, i as i64);
                    let today = day.wDay == st.wDay && day.wMonth == st.wMonth;
                    if today { c.circle(x + col / 2.0, 84.0 + 13.0, 13.0, t.c("primary")); }
                    c.text(&day.wDay.to_string(), R::new(x, 84.0, col, 26.0), txt(12.0, if today { 600 } else { 400 }), if today { t.c("onPrimary") } else { fg }, Align::Center);
                }
            }
            "greet" if big => { bg(c); big_greet(self, c, cx, &st); }
            "resources" if big => big_resources(c, cx, back.as_ref()),
            "greet" => {
                bg(c);
                let name = [cx.cfg.str("userName").to_string(), cx.host.clone()].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| "there".into());
                let (text, sub, show_date, file, shape) = greet_conf(cx);
                let av = R::new(14.0, 14.0, 70.0, 70.0);
                let drawn = !file.is_empty() && self.shaped_picture(c, &file, if shape.is_empty() { Shape::Round(17.0) } else { Shape::parse(&shape) }, av, scale);
                if !drawn {
                    let letter = name.chars().next().map(|ch| ch.to_uppercase().collect::<String>()).unwrap_or_default();
                    let geom = if shape.is_empty() { Shape::Round(17.0) } else { Shape::parse(&shape) }.path(av);
                    if let Some(g) = geom { c.fill_path(&g, t.c("tertiaryContainer")); }
                    c.text(&letter, av, txt(30.0, 600), t.c("onTertiaryContainer"), Align::Center);
                }
                let fill = |s: &str| s.replace("{name}", &name);
                let x = 14.0 + 70.0 + 12.0;
                let wd = W - 8.0 - x - 6.0;
                let block = 15.0 * LH + 13.0 * LH + if show_date { 11.0 * LH } else { 0.0 };
                let mut y = 4.0 + (90.0 - block) / 2.0;
                c.text(&fill(if text.is_empty() { "Hi, {name}!" } else { &text }), R::new(x, y, wd, 15.0 * LH), txt(15.0, 600), fg, Align::Left);
                y += 15.0 * LH;
                c.text(&if sub.is_empty() { greeting(st.wHour).to_string() } else { fill(&sub) }, R::new(x, y, wd, 13.0 * LH), txt(13.0, 400), fg, Align::Left);
                y += 13.0 * LH;
                if show_date { c.text(&format!("Today • {}", date_fmt(&st, "dddd, MMM d")), R::new(x, y, wd, 11.0 * LH), txt(11.0, 400), muted, Align::Left); }
            }
            "calendar" if big => {
                bg(c);
                match cells(c) { (r, k) if r >= 2 && k >= 3 => big_calendar(c, cx, &st), (r, _) if r >= 2 => grid_calendar(c, cx, &st), _ => day_calendar(c, cx, &st) }
            }
            "calendar" => {
                bg(c);
                let y = h3(c, cx, "calendar_month", &date_fmt(&st, "MMMM yyyy"));
                let col = (W - 8.0 - 28.0) / 7.0;
                for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
                    c.text(d, R::new(18.0 + i as f32 * col, y, col, 26.0), txt(11.0, 400), muted, Align::Center);
                }
                for (i, (d, out, today)) in month_cells(&st).into_iter().enumerate() {
                    let (x, yy) = (18.0 + (i % 7) as f32 * col, y + 28.0 * (i / 7 + 1) as f32);
                    if today { c.circle(x + col / 2.0, yy + 13.0, 13.0, t.c("primary")); }
                    let colr = if today { t.c("onPrimary") } else if out { alpha(fg, 0.35) } else { fg };
                    c.text(&d.to_string(), R::new(x, yy, col, 26.0), txt(12.0, if today { 600 } else { 400 }), colr, Align::Center);
                }
            }
            "worldclock" if big && cells(c).0 == 1 && cells(c).1 >= 3 => { bg(c); row_worldclock(c, cx, &st, h24); }
            "worldclock" => {
                bg(c);
                let mut y = h3(c, cx, "public", "World clock");
                let here = util::local_offset();
                let utc_min = (st.wHour as i32 * 60 + st.wMinute as i32) - here;
                let row_h = 6.0 + 18.0 * LH;
                for tz in cx.cfg.strs("desktop.worldClocks") {
                    let r = R::new(18.0, y + 3.0, W - 8.0 - 28.0, row_h - 6.0);
                    match util::zone_offset(&tz) {
                        Some(off) => {
                            let m = (utc_min + off).rem_euclid(1440);
                            let city = tz.rsplit('/').next().unwrap_or(&tz).replace('_', " ");
                            let diff = ((off - here) as f32 / 30.0).round() / 2.0;
                            let tag = if diff == 0.0 { " local".to_string() } else { format!(" {}{}h", if diff > 0.0 { "+" } else { "" }, diff) };
                            let cw = c.g.measure(&city, txt(13.0, 400)).0;
                            c.text(&city, R::new(r.x, r.y, r.w, r.h), txt(13.0, 400), fg, Align::Left);
                            c.text(&tag, R::new(r.x + cw, r.y + 1.0, r.w - cw, r.h), txt(11.0, 400), muted, Align::Left);
                            c.text(&util::clock((m / 60) as u16, (m % 60) as u16, h24), r, txt(18.0, 500), fg, Align::Right);
                        }
                        None => c.text(&format!("{tz}: unknown time zone"), r, txt(13.0, 400), muted, Align::Left),
                    }
                    y += row_h;
                }
            }
            "resources" => {
                bg(c);
                let y = h3(c, cx, "monitoring", "Resources");
                let items = [(cx.cpu, "CPU"), (cx.mem, "RAM"), (cx.disk, "Disk C:")];
                let slot = (W - 8.0 - 28.0) / 3.0; // justify-content: space-around
                for (i, (v, label)) in items.iter().enumerate() {
                    let cxp = 18.0 + slot * (i as f32 + 0.5);
                    let (cy, r) = (y + 28.0, 25.5);
                    c.arc(cxp, cy, r, 1.0, 5.0, t.c("secondaryContainer"));
                    c.arc(cxp, cy, r, *v, 5.0, t.c("primary"));
                    c.text(&format!("{}%", (v * 100.0).round()), R::new(cxp - 28.0, y, 56.0, 56.0), txt(13.0, 500), fg, Align::Center);
                    c.text(label, R::new(cxp - 50.0, y + 60.0, 100.0, 12.0 * LH), txt(12.0, 400), fg, Align::Center);
                }
            }
            "text" => {
                let s = cx.cfg.str("desktop.text");
                if s.is_empty() { return; }
                bg(c);
                c.para(s, R::new(18.0, 16.0, W - 8.0 - 28.0, h - 32.0), txt(20.0, 500), fg, Align::Left);
            }
            "image1" if big && !image_conf(cx, "image1").2.is_empty() => {
                let file = image_conf(cx, "image1").2;
                let r = R::new(4.0, 4.0, c.w - 8.0, c.h - 8.0);
                if let Some(pic) = self.picture_wh(c.g, &file, (r.w * scale).round() as u32, (r.h * scale).round() as u32) {
                    c.shadow(6.0, 16.0, 0.4, |c, col| if let Some(col) = col { c.rect(r, 23.0, col) });
                    c.clip_round(r, 23.0, |c| c.image(&pic, r, 1.0));
                }
            }
            id if id.starts_with("image") || id == "sticker" => {
                let id = id.to_string();
                let (shape, size, file) = image_conf(cx, &id);
                let r = R::new(14.0, 14.0, size, size);
                if file.is_empty() || self.picture(c.g, &file, (size * scale).round() as u32).is_none() {
                    let box_ = R::new(14.0, 14.0, c.w - 28.0, 53.0);
                    c.rect(box_, 23.0, alpha(t.c("surfaceContainerLow"), 0.88));
                    c.icon("add_photo_alternate", R::new(30.0, 14.0, 20.0, 53.0), ico(20.0, false), muted);
                    c.text("Choose a picture in Settings", R::new(60.0, 14.0, c.w - 90.0, 53.0), txt(13.0, 400), muted, Align::Left);
                    return;
                }
                let geom = shape.path(r);
                // drop-shadow(0 6px 16px rgba(0,0,0,.4)) that follows the shape
                c.shadow(6.0, 16.0, 0.4, |c, col| if let (Some(col), Some(g)) = (col, &geom) { c.fill_path(g, col) });
                self.shaped_picture(c, &file, shape, r, scale);
            }
            _ => {}
        }
    }
}

/// The user card at a preset size: avatar and greeting side by side, or stacked in a one-cell card.
fn big_greet(card: &mut Card, c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME) {
    let t = &cx.theme;
    let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
    let name = [cx.cfg.str("userName").to_string(), cx.host.clone()].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| "there".into());
    let (text, sub, show_date, file, shape) = greet_conf(cx);
    let shape = if shape.is_empty() { Shape::Round(17.0) } else { Shape::parse(&shape) };
    let narrow = c.w < 200.0 || (c.h > 150.0 && c.w < 330.0); // text goes under the avatar
    let av_size = if c.w < 200.0 { 52.0 } else if narrow { 96.0 } else if c.h > 150.0 { 112.0 } else { 70.0 };
    let av = R::new(if narrow && c.w >= 200.0 { (c.w - av_size) / 2.0 } else { 14.0 }, if narrow { 16.0 } else { (c.h - av_size) / 2.0 }, av_size, av_size);
    let scale = c.scale;
    if file.is_empty() || !card.shaped_picture(c, &file, shape, av, scale) {
        if let Some(g) = shape.path(av) { c.fill_path(&g, t.c("tertiaryContainer")); }
        c.text(&name.chars().next().map(|ch| ch.to_uppercase().collect::<String>()).unwrap_or_default(), av, txt(26.0, 600), t.c("onTertiaryContainer"), Align::Center);
    }
    let fill = |s: &str| s.replace("{name}", &name);
    let hello = fill(if text.is_empty() { "Hi, {name}!" } else { &text });
    let line = if sub.is_empty() { greeting(st.wHour).to_string() } else { fill(&sub) };
    let (x, wd) = if narrow { (14.0, c.w - 8.0 - 12.0) } else { (14.0 + av_size + 12.0, c.w - 8.0 - 14.0 - av_size - 18.0) };
    let block = 15.0 * LH + 13.0 * LH + if show_date && !narrow { 11.0 * LH } else { 0.0 };
    let mut y = if narrow { av.y + av_size + 8.0 } else { (c.h - block) / 2.0 };
    let al = if narrow && c.w >= 200.0 { Align::Center } else { Align::Left };
    c.text(&hello, R::new(x, y, wd, 15.0 * LH), txt(15.0, 600), fg, al);
    y += 15.0 * LH;
    c.text(&line, R::new(x, y, wd, 13.0 * LH), txt(13.0, 400), fg, al);
    y += 13.0 * LH;
    if show_date && !narrow { c.text(&format!("Today \u{2022} {}", date_fmt(st, "dddd, MMM d")), R::new(x, y, wd, 11.0 * LH), txt(11.0, 400), muted, Align::Left); }
}

/// Resources as the preset shows it: CPU, RAM and disk as three separate one-cell tiles.
fn big_resources(c: &mut Canvas, cx: &Ctx, back: Option<&Pixmap>) {
    let t = &cx.theme;
    let fg = t.c("onSurface");
    for (i, (icon, v, label)) in [("speed", cx.cpu, "CPU"), ("memory", cx.mem, "RAM"), ("storage", cx.disk, "Disk")].iter().enumerate() {
        let r = R::new(4.0 + i as f32 * (CELL_W + GAP), 4.0, CELL_W, c.h - 8.0);
        // the glass picture covers the whole window: clip it to this tile or it paints over the earlier tiles
        match back { Some(b) => c.clip_round(r, 23.0, |c| c.image(b, R::new(0.0, 0.0, 0.0, 0.0), 1.0)), None => c.rect(r, 23.0, alpha(t.c("surfaceContainerLow"), 0.88)) }
        c.circle(r.x + r.w - 14.0 - 15.0, r.y + 14.0 + 15.0, 15.0, t.c("primaryContainer"));
        c.icon(icon, R::new(r.x + r.w - 14.0 - 30.0, r.y + 14.0, 30.0, 30.0), ico(16.0, false), t.c("onPrimaryContainer"));
        c.text(&format!("{}%", (v * 100.0).round()), R::new(r.x + 14.0, r.y + r.h - 14.0 - 14.0 * LH - 22.0 * LH, r.w - 20.0, 22.0 * LH), txt(22.0, 600), fg, Align::Left);
        c.text(label, R::new(r.x + 14.0, r.y + r.h - 14.0 - 14.0 * LH, r.w - 20.0, 14.0 * LH), txt(13.0, 400), fg, Align::Left);
    }
}

/// The tall calendar card: month and day on the left, the month grid on the right.
fn big_calendar(c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME) {
    let t = &cx.theme;
    let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
    c.circle(20.0 + 22.0, 24.0 + 22.0, 22.0, t.c("primaryContainer"));
    c.icon("calendar_month", R::new(20.0, 24.0, 44.0, 44.0), ico(22.0, false), t.c("onPrimaryContainer"));
    let by = c.h - 4.0 - 14.0; // bottom padding
    let day_y = by - 52.0 * LH + 6.0;
    let wd_y = day_y - 17.0 * LH;
    c.text(&date_fmt(st, "MMMM").to_uppercase(), R::new(20.0, wd_y - 15.0 * LH, 130.0, 15.0 * LH), txt(14.0, 500), fg, Align::Left);
    c.text(&date_fmt(st, "dddd"), R::new(20.0, wd_y, 130.0, 17.0 * LH), txt(17.0, 500), fg, Align::Left);
    c.text(&st.wDay.to_string(), R::new(20.0, day_y, 130.0, 52.0 * LH), txt(52.0, 500), fg, Align::Left);
    let x0 = c.w * 0.38;
    let col = (c.w - 8.0 - x0 - 18.0) / 7.0;
    let rows = month_cells(st).len() as f32 / 7.0 + 1.0;
    let step = ((c.h - 8.0 - 28.0) / rows).min(30.0);
    c.rect(R::new(x0 - 6.0, 18.0, col * 7.0 + 12.0, step * rows + 8.0), 14.0, alpha(t.c("surfaceContainerHigh"), 0.5));
    for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
        c.text(d, R::new(x0 + i as f32 * col, 22.0, col, step), txt(11.0, 600), fg, Align::Center);
    }
    for (i, (d, out, today)) in month_cells(st).into_iter().enumerate() {
        let (x, y) = (x0 + (i % 7) as f32 * col, 22.0 + step * (i / 7 + 1) as f32);
        if today { c.circle(x + col / 2.0, y + step / 2.0, (step / 2.0).min(col / 2.0), t.c("primary")); }
        let colr = if today { t.c("onPrimary") } else if out { alpha(muted, 0.5) } else { fg };
        c.text(&d.to_string(), R::new(x, y, col, step), txt(12.0, if today { 600 } else { 500 }), colr, Align::Center);
    }
}

/// The weather card at a one-cell size: temperature and place at the bottom, the condition icon on top.
fn small_weather(c: &mut Canvas, cx: &Ctx, w: &weather::Weather) {
    let t = &cx.theme;
    let fg = t.c("onSurface");
    c.circle(c.w - 4.0 - 14.0 - 15.0, 4.0 + 14.0 + 15.0, 15.0, t.c("primaryContainer"));
    c.icon(weather::icon(w.status), R::new(c.w - 4.0 - 14.0 - 30.0, 18.0, 30.0, 30.0), ico(16.0, true), t.c("onPrimaryContainer"));
    let (x, wd) = (18.0, c.w - 8.0 - 20.0);
    c.text(&weather::temp(w, cx.cfg.str("tempUnit")), R::new(x, c.h - 4.0 - 14.0 - 13.0 * LH - 24.0 * LH, wd, 24.0 * LH), txt(24.0, 600), fg, Align::Left);
    c.text(if w.city.is_empty() { "Weather" } else { &w.city }, R::new(x, c.h - 4.0 - 14.0 - 13.0 * LH, wd, 13.0 * LH), txt(13.0, 400), fg, Align::Left);
    if c.w > 250.0 {
        // wider than one cell: the condition next to the temperature
        let tx = 18.0 + c.g.measure(&weather::temp(w, cx.cfg.str("tempUnit")), txt(24.0, 600)).0 + 14.0;
        c.text(&weather::text(w.status), R::new(tx, c.h - 4.0 - 14.0 - 13.0 * LH - 18.0 * LH, c.w - tx - 60.0, 15.0 * LH), txt(15.0, 500), fg, Align::Left);
    }
}

/// The weather card two cells tall and narrower than three: temperature, place and four detail tiles.
fn square_weather(c: &mut Canvas, cx: &Ctx, w: &weather::Weather) {
    let t = &cx.theme;
    let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
    c.text(&weather::temp(w, cx.cfg.str("tempUnit")), R::new(20.0, 16.0, 150.0, 34.0 * LH), txt(34.0, 500), t.c("primary"), Align::Left);
    c.text(if w.city.is_empty() { "Weather" } else { &w.city }, R::new(20.0, 16.0 + 34.0 * LH + 2.0, c.w - 90.0, 13.0 * LH), txt(13.0, 500), fg, Align::Left);
    c.text(&weather::text(w.status), R::new(20.0, 16.0 + 34.0 * LH + 2.0 + 15.0 * LH, c.w - 90.0, 12.0 * LH), txt(12.0, 400), muted, Align::Left);
    let chip = R::new(c.w - 4.0 - 14.0 - 42.0, 18.0, 42.0, 42.0);
    c.circle(chip.x + 21.0, chip.y + 21.0, 21.0, t.c("primaryContainer"));
    c.icon(weather::icon(w.status), chip, ico(22.0, true), t.c("onPrimaryContainer"));
    let pct = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{}%", v.round()));
    let tiles = [("water_drop", "Humidity", pct(w.humidity)), ("air", "Wind", w.wind.map_or("-".into(), |s| format!("{:.1} m/s", s / 3.6))),
        ("rainy", "Rain", pct(w.rain)), ("visibility", "Visibility", w.visibility_km.map_or("-".into(), |v| format!("{v} km")))];
    let tw = (c.w - 8.0 - 28.0 - 6.0) / 2.0;
    let top = c.h - 4.0 - 14.0 - 2.0 * 54.0 - 6.0;
    for (i, (icon, label, v)) in tiles.iter().enumerate() {
        let r = R::new(14.0 + 4.0 + (i % 2) as f32 * (tw + 6.0), top + (i / 2) as f32 * 60.0, tw, 54.0);
        c.rect(r, 14.0, alpha(t.c("surfaceContainerHigh"), 0.55));
        c.icon(icon, R::new(r.x + 8.0, r.y, 18.0, r.h), ico(16.0, false), muted);
        c.text(label, R::new(r.x + 30.0, r.y + 9.0, r.w - 32.0, 11.0 * LH), txt(10.0, 400), muted, Align::Left);
        c.text(v, R::new(r.x + 30.0, r.y + 9.0 + 11.0 * LH, r.w - 32.0, 14.0 * LH), txt(13.0, 600), fg, Align::Left);
    }
}

/// The calendar at one row: the month and weekday over a large day number.
fn day_calendar(c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME) {
    let t = &cx.theme;
    let fg = t.c("onSurface");
    let by = c.h - 4.0 - 12.0;
    c.text(&st.wDay.to_string(), R::new(18.0, by - 46.0 * LH, 120.0, 46.0 * LH), txt(46.0, 500), fg, Align::Left);
    c.text(&date_fmt(st, "dddd"), R::new(18.0, by - 46.0 * LH - 15.0 * LH, c.w - 40.0, 15.0 * LH), txt(14.0, 500), fg, Align::Left);
    c.text(&date_fmt(st, "MMMM").to_uppercase(), R::new(18.0, by - 46.0 * LH - 30.0 * LH, c.w - 40.0, 14.0 * LH), txt(12.0, 500), t.c("onSurfaceVariant"), Align::Left);
    if c.w > 200.0 {
        let chip = R::new(c.w - 4.0 - 14.0 - 34.0, 18.0, 34.0, 34.0);
        c.circle(chip.x + 17.0, chip.y + 17.0, 17.0, t.c("primaryContainer"));
        c.icon("calendar_month", chip, ico(18.0, false), t.c("onPrimaryContainer"));
    }
}

/// The calendar two rows tall and narrower than three cells: the month grid under its title.
fn grid_calendar(c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME) {
    let t = &cx.theme;
    let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
    c.text(&date_fmt(st, "MMMM yyyy"), R::new(18.0, 14.0, c.w - 40.0, 15.0 * LH), txt(14.0, 600), fg, Align::Left);
    let rows = month_cells(st).len() as f32 / 7.0 + 1.0;
    let step = ((c.h - 8.0 - 14.0 - 15.0 * LH - 18.0) / rows).min(30.0);
    let col = (c.w - 8.0 - 28.0) / 7.0;
    let y0 = 14.0 + 15.0 * LH + 6.0;
    for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].iter().enumerate() {
        c.text(d, R::new(18.0 + i as f32 * col, y0, col, step), txt(10.5, 600), muted, Align::Center);
    }
    for (i, (d, out, today)) in month_cells(st).into_iter().enumerate() {
        let (x, y) = (18.0 + (i % 7) as f32 * col, y0 + step * (i / 7 + 1) as f32);
        if today { c.circle(x + col / 2.0, y + step / 2.0, (step / 2.0).min(col / 2.0), t.c("primary")); }
        let colr = if today { t.c("onPrimary") } else if out { alpha(muted, 0.5) } else { fg };
        c.text(&d.to_string(), R::new(x, y, col, step), txt(11.5, if today { 600 } else { 400 }), colr, Align::Center);
    }
}

/// The world clock in one row: each city as a column, name over time.
fn row_worldclock(c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME, h24: bool) {
    let t = &cx.theme;
    let zones = cx.cfg.strs("desktop.worldClocks");
    if zones.is_empty() { return; }
    let here = util::local_offset();
    let utc_min = (st.wHour as i32 * 60 + st.wMinute as i32) - here;
    let w = (c.w - 8.0 - 20.0) / zones.len() as f32;
    for (i, tz) in zones.iter().enumerate() {
        let r = R::new(14.0 + i as f32 * w, 4.0, w, c.h - 8.0);
        let city = tz.rsplit('/').next().unwrap_or(tz).replace('_', " ");
        c.text(&city, R::new(r.x, r.y + 20.0, r.w, 13.0 * LH), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Center);
        if let Some(off) = util::zone_offset(tz) {
            let m = (utc_min + off).rem_euclid(1440);
            c.text(&util::clock((m / 60) as u16, (m % 60) as u16, h24), R::new(r.x, r.y + 20.0 + 13.0 * LH + 4.0, r.w, 24.0 * LH), txt(22.0, 500), t.c("onSurface"), Align::Center);
        }
    }
}

/// The tall weather card: temperature and place on top, six detail tiles below.
fn big_weather(c: &mut Canvas, cx: &Ctx, w: &weather::Weather) {
    let t = &cx.theme;
    let (fg, muted) = (t.c("onSurface"), t.c("onSurfaceVariant"));
    let fahrenheit = cx.cfg.str("tempUnit") == "F";
    c.text(&weather::temp(w, cx.cfg.str("tempUnit")), R::new(20.0, 16.0, 150.0, 40.0 * LH), txt(40.0, 500), t.c("primary"), Align::Left);
    c.text(&weather::text(w.status), R::new(170.0, 22.0, 150.0, 15.0 * LH), txt(15.0, 500), fg, Align::Left);
    if let Some(f) = w.feels {
        let (v, u) = if fahrenheit { (f * 9.0 / 5.0 + 32.0, "F") } else { (f, "C") };
        c.text(&format!("Feels like {}\u{b0}{u}", v.round()), R::new(170.0, 22.0 + 15.0 * LH, 150.0, 12.0 * LH), txt(11.5, 400), muted, Align::Left);
    }
    let city_y = 16.0 + 40.0 * LH + 2.0;
    c.icon("location_on", R::new(20.0, city_y, 14.0, 13.0 * LH), ico(12.0, false), muted);
    c.text(if w.city.is_empty() { "Weather" } else { &w.city }, R::new(36.0, city_y, 200.0, 13.0 * LH), txt(13.0, 500), muted, Align::Left);
    let chip = R::new(c.w - 4.0 - 16.0 - 46.0, 18.0, 46.0, 46.0);
    c.circle(chip.x + 23.0, chip.y + 23.0, 23.0, t.c("primaryContainer"));
    c.icon(weather::icon(w.status), chip, ico(24.0, true), t.c("onPrimaryContainer"));
    let pct = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{}%", v.round()));
    let time = |s: &str| if s.is_empty() { "-".to_string() } else { s.to_string() };
    let tiles = [
        ("water_drop", "Humidity", pct(w.humidity)), ("air", "Wind", w.wind.map_or("-".into(), |s| format!("{:.2} m/s", s / 3.6))),
        ("rainy", "Rain Chance", pct(w.rain)), ("visibility", "Visibility", w.visibility_km.map_or("-".into(), |v| format!("{v} km"))),
        ("wb_twilight", "Sunrise", time(&w.sunrise)), ("bedtime", "Sunset", time(&w.sunset)),
    ];
    let gy = c.h - 4.0 - 12.0 - 2.0 * 54.0 - 6.0;
    let tw = (c.w - 8.0 - 32.0 - 2.0 * 6.0) / 3.0;
    for (i, (icon, label, v)) in tiles.iter().enumerate() {
        let r = R::new(16.0 + (i % 3) as f32 * (tw + 6.0), gy + (i / 3) as f32 * 60.0, tw, 54.0);
        c.rect(r, 14.0, alpha(t.c("surfaceContainerHigh"), 0.55));
        c.icon(icon, R::new(r.x + 8.0, r.y, 18.0, r.h), ico(16.0, false), muted);
        c.text(label, R::new(r.x + 30.0, r.y + 9.0, r.w - 32.0, 11.0 * LH), txt(10.0, 400), muted, Align::Left);
        c.text(v, R::new(r.x + 30.0, r.y + 9.0 + 11.0 * LH, r.w - 32.0, 14.0 * LH), txt(13.0, 600), fg, Align::Left);
    }
}

/// `.w h3`: icon + muted title. Returns the y where the card's content starts.
fn h3(c: &mut Canvas, cx: &Ctx, icon: &str, title: &str) -> f32 {
    let muted = cx.theme.c("onSurfaceVariant");
    let y = 16.0;
    c.icon(icon, R::new(18.0, y, 16.0, 13.0 * LH), ico(16.0, false), muted);
    c.text(title, R::new(40.0, y, W - 60.0, 13.0 * LH), txt(13.0, 500), muted, Align::Left);
    y + 13.0 * LH + 8.0
}

/// 3x5 dot-matrix digits for the pixel clock (end4's PixelClock): 15 bits per digit, row by row.
const PX: [&str; 10] = ["111101101101111", "010110010010111", "111001111100111", "111001111001111", "101101111001001",
    "111100111001111", "111100111101111", "111001001001001", "111101111101111", "111101111001111"];

/// The digital clock stacked: hours over minutes ("22" / "54").
fn stacked_time(h: u16, m: u16, h24: bool) -> (String, String) {
    let t = util::clock(h, m, h24);
    let (hh, mm) = t.split_once(':').unwrap_or((&t, ""));
    (hh.to_string(), mm.to_string())
}

fn clock_vertical(cx: &Ctx) -> bool { clock_style(cx) == "digital" && cx.cfg.bool("desktop.clockVertical") }

fn clock(c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME, h24: bool) {
    let t = &cx.theme;
    let (h, m, s) = ((st.wHour % 12) as f32, st.wMinute as f32, st.wSecond as f32);
    match clock_style(cx) {
        "digital" if clock_vertical(cx) => {
            let (hh, mm) = stacked_time(st.wHour, st.wMinute, h24);
            let date = date_fmt(st, "ddd, MM/dd");
            let fg = t.c("onSurface");
            c.shadow(2.0, 12.0, 0.35, |c, col| {
                let col = col.unwrap_or(fg);
                c.text(&hh, R::new(12.0, 4.0, W - 24.0, 80.0), txt(92.0, 400), col, Align::Left);
                c.text(&mm, R::new(12.0, 76.0, W - 24.0, 80.0), txt(92.0, 400), col, Align::Left);
                c.text(&date, R::new(12.0, 156.0, W - 24.0, 18.0 * LH), txt(18.0, 500), col, Align::Left);
            });
        }
        "digital" => {
            let time = util::clock(st.wHour, st.wMinute, h24);
            let date = date_fmt(st, "dddd, MMMM d");
            let top = 4.0 + (150.0 - 84.0 - 16.0 * LH) / 2.0;
            let fg = t.c("onSurface");
            c.shadow(2.0, 12.0, 0.35, |c, col| {
                c.text(&time, R::new(4.0, top, W - 8.0, 84.0), txt(84.0, 600), col.unwrap_or(fg), Align::Center);
                c.text(&date, R::new(4.0, top + 84.0, W - 8.0, 16.0 * LH), txt(16.0, 500), col.unwrap_or(fg), Align::Center);
            });
        }
        "pixel" => {
            let hh = if h24 { st.wHour } else if st.wHour % 12 == 0 { 12 } else { st.wHour % 12 };
            let glyphs: Vec<char> = format!("{hh:02}:{:02}", st.wMinute).chars().collect();
            let cell = 15.0;
            let cols: f32 = glyphs.iter().map(|&g| if g == ':' { 2.0 } else { 4.0 }).sum::<f32>() - 1.0;
            let (ox, oy) = (4.0 + (W - 8.0 - cols * cell) / 2.0, 4.0 + (150.0 - 5.0 * cell) / 2.0);
            let mut x = 0.0;
            for g in glyphs {
                let ncol = if g == ':' { 1 } else { 3 };
                for r in 0..5 {
                    for col in 0..ncol {
                        let lit = if g == ':' { r == 1 || r == 3 } else { PX[g as usize - '0' as usize].as_bytes()[r * 3 + col] == b'1' };
                        let color = if lit { t.c("primary") } else { alpha(t.c("onSurface"), 0.08) };
                        c.circle(ox + (x + col as f32) * cell + cell / 2.0, oy + r as f32 * cell + cell / 2.0, cell * 0.4, color);
                    }
                }
                x += ncol as f32 + 1.0;
            }
        }
        style => {
            let (ox, oy) = (4.0 + (W - 8.0 - 220.0) / 2.0, 4.0 + (250.0 - 220.0) / 2.0);
            let p = |x: f32, y: f32| (ox + x, oy + y);
            let hand = |c: &mut Canvas, angle: f32, len: f32, back: f32, width: f32, col| {
                let a = (angle - 90.0).to_radians();
                c.line(p(110.0 - back * a.cos(), 110.0 - back * a.sin()), p(110.0 + len * a.cos(), 110.0 + len * a.sin()), width, col);
            };
            let osc = t.c("onSecondaryContainer");
            let face = if style == "analog" {
                Shape::Circle.path(R::new(ox + 2.0, oy + 2.0, 216.0, 216.0))
            } else {
                polygon(&cookie_points(ox + 110.0, oy + 110.0, 108.0, 9, 0.06, 360))
            };
            // filter: drop-shadow(0 4px 12px 30%) on the whole dial
            c.shadow(4.0, 12.0, 0.3, |c, col| if let (Some(col), Some(g)) = (col, &face) { c.fill_path(g, col) });
            if style == "analog" {
                if let Some(g) = &face { c.fill_path(g, alpha(t.c("surfaceContainer"), 0.9)); }
                for i in 0..60 {
                    let a = ((i * 6) as f32 - 90.0).to_radians();
                    let r1 = if i % 5 != 0 { 94.0 } else { 86.0 };
                    c.line(p(110.0 + r1 * a.cos(), 110.0 + r1 * a.sin()), p(110.0 + 100.0 * a.cos(), 110.0 + 100.0 * a.sin()),
                        if i % 5 != 0 { 1.5 } else { 4.0 }, t.c("onSurfaceVariant"));
                }
            } else {
                if let Some(g) = &face { c.fill_path(g, alpha(t.c("secondaryContainer"), 0.92)); }
                for (n, x, y) in [("12", 110.0, 46.0), ("3", 176.0, 112.0), ("6", 110.0, 176.0), ("9", 44.0, 112.0)] {
                    let (cx_, cy_) = p(x, y);
                    c.text(n, R::new(cx_ - 50.0, cy_ - 40.0, 100.0, 80.0), txt(62.0, 700), alpha(osc, 0.22), Align::Center);
                }
            }
            hand(c, (h + m / 60.0) * 30.0, 52.0, 0.0, 13.0, osc);
            hand(c, (m + s / 60.0) * 6.0, 78.0, 0.0, 8.0, osc);
            hand(c, s * 6.0, 86.0, 14.0, 3.0, t.c("primary"));
            let (px_, py_) = p(110.0, 110.0);
            c.circle(px_, py_, 7.0, osc);
            if style != "analog" {
                for (txt_, x, y) in [(st.wDay.to_string(), 22.0, 22.0), (format!("{:02}", st.wMonth), 198.0, 198.0)] {
                    let (bx, by) = p(x, y);
                    c.circle(bx, by, 22.0, t.c("primaryContainer"));
                    c.text(&txt_, R::new(bx - 22.0, by - 22.0, 44.0, 44.0), txt(22.0, 600), t.c("onPrimaryContainer"), Align::Center);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stacked_clock_splits_hours_and_minutes() {
        assert_eq!(stacked_time(22, 54, true), ("22".into(), "54".into()));
        assert_eq!(stacked_time(0, 5, false), ("12".into(), "05".into()));
    }
    #[test]
    fn slots_wrap_into_columns() {
        let w: Vec<String> = ["clock", "weather", "calendar", "lyrics"].iter().map(|s| s.to_string()).collect();
        assert_eq!(default_slot(&w, "clock", 1040.0), (16.0, 72.0));
        assert_eq!(default_slot(&w, "weather", 1040.0), (16.0, 332.0));
        assert_eq!(default_slot(&w, "lyrics", 1040.0), (454.0, 72.0)); // 72+260+102+280 > 1024: next column
    }
    #[test]
    fn snapping_lines_cards_up() {
        // a card 5px off another's left edge snaps to it; 30px off stays where it is
        assert_eq!(snap_axis((105, 305), &[(100, 400)], 12, 8), -5);
        assert_eq!(snap_axis((130, 330), &[(100, 400)], 12, 8), 0);
        // centres meet: mine centre 200 vs other centre 205
        assert_eq!(snap_axis((150, 250), &[(100, 310)], 12, 8), 5);
        // a neighbour's far edge plus the gap: mine starts at 417, other ends at 400, gap 12
        assert_eq!(snap_axis((417, 500), &[(100, 400)], 12, 8), -5);
        // the closest of several candidates wins
        assert_eq!(snap_axis((106, 300), &[(100, 400), (108, 150)], 12, 8), 2);
        let others = [(228, 324, 642, 576)];
        // dragged window at (230, 600) with a 4px margin: its top is 2px under the other's bottom + 12 gap... snaps y
        assert_eq!(snap_card((226, 580), (300, 200), 4, &others, 12, 8), (224, 580 + (576 + 12 - 584)));
    }

    #[test]
    fn preset_grid_sizes() {
        assert_eq!(grid_size("2x3"), Some((414.0, 252.0)));
        assert_eq!(grid_size("1x1"), Some((130.0, 120.0)));
        assert_eq!(grid_size("1x2"), Some((272.0, 120.0)));
        assert_eq!(grid_size("0x2"), None);
        assert_eq!(grid_size("big"), None);
    }

    #[test]
    fn grid_is_whole_weeks() {
        let st = windows::Win32::Foundation::SYSTEMTIME { wYear: 2026, wMonth: 11, wDay: 4, wDayOfWeek: 3, ..Default::default() };
        let g = month_cells(&st);
        assert_eq!(g.len() % 7, 0);
        assert_eq!(g[6], (1, false, false)); // 1 Nov 2026 is a Sunday
        assert!(g.iter().any(|&(d, out, today)| d == 4 && !out && today));
    }
}
