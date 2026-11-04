//! Desktop cards (desktop.js / desktop.html): one bottom-most window per card, draggable.

mod np;
mod small;

use crate::{
    app::{App, Ctx},
    gfx::{alpha, cookie_points, ico, polygon, txt, Align, Canvas, Gfx, Pixmap, Shape, R},
    util::{self, add_days, date_fmt, days_in_month, monday_index},
    weather,
    win::{Input, View, Win, Z},
};
use serde_json::json;
use std::{path::PathBuf, sync::{atomic::{AtomicIsize, Ordering}, Arc}, time::SystemTime};

/// Cards ported (all of them; desktop.js keeps the same list in NATIVE).
pub const PORTED: &[&str] = &["clock", "weather", "week", "greet", "calendar", "worldclock", "resources", "text", "image1", "image2", "image3",
    "media", "lyrics", "cover", "todo", "notes", "timer"];

const W: f32 = 310.0; // #col width; cards sit 4px inside it
const LH: f32 = 1.252; // Google Sans Flex line height (ascent + descent) per px of font size

/// Rough heights used only to stack cards that were never dragged (lib/desk.js).
fn slot_height(id: &str) -> f32 {
    match id {
        "clock" => 250.0, "weather" => 92.0, "week" | "greet" => 90.0, "media" => 92.0, "calendar" => 270.0, "todo" => 150.0,
        "notes" => 170.0, "timer" => 100.0, "worldclock" => 140.0, "resources" => 130.0, "text" => 60.0, "lyrics" => 390.0,
        "cover" => 290.0, "image1" | "image2" | "image3" => 310.0, _ => 120.0,
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
    for id in &want {
        if app.wins.iter().any(|w| w.drag_key.as_deref() == Some(id)) { continue; }
        let saved = app.cx.state.v["deskPos"][id].as_object().and_then(|p| Some((p.get("x")?.as_i64()? as i32, p.get("y")?.as_i64()? as i32)));
        let on_screen = |(x, y): (i32, i32)| mons.iter().any(|m| x >= m.left - 40 && x < m.right - 40 && y >= m.top && y < m.bottom - 40);
        let (x, y) = saved.filter(|&p| on_screen(p)).unwrap_or_else(|| {
            let (x, y) = default_slot(&cfg.strs("desktop.widgets"), id, (work.bottom - work.top) as f32 / scale);
            ((x * scale) as i32 + work.left, (y * scale) as i32 + work.top)
        });
        let card = Card::new(id);
        let hwnd = card.hwnd.clone();
        let Ok(mut w) = Win::new(Box::new(card), &format!("end4-pC / desk-{id}"), x, y, Z::Bottom, false) else { continue };
        w.drag_key = Some(id.clone());
        hwnd.store(w.hwnd.0 as isize, Ordering::Relaxed);
        w.update(&app.g, &app.cx, true);
        w.show();
        app.wins.push(w);
    }
}

/// A card was dragged: remember where (physical px, like Zebar's localStorage).
pub fn save_pos(cx: &mut Ctx, id: &str, (x, y): (i32, i32)) {
    if !cx.state.v["deskPos"].is_object() { cx.state.v["deskPos"] = json!({}); }
    cx.state.v["deskPos"][id] = json!({ "x": x, "y": y });
    cx.state.save();
}

pub struct Card {
    id: String,
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
    let size = c["size"].as_f64().unwrap_or(280.0).clamp(120.0, 800.0) as f32;
    (Shape::parse(c["shape"].as_str().unwrap_or("cookie")), size, c["file"].as_str().unwrap_or("").to_string())
}

fn greet_conf(cx: &Ctx) -> (String, String, bool, String, String) {
    let g = cx.cfg.get("desktop.greet");
    (g["text"].as_str().unwrap_or("").into(), g["sub"].as_str().unwrap_or("").into(), g["date"].as_bool().unwrap_or(true),
        g["file"].as_str().unwrap_or("").into(), g["shape"].as_str().unwrap_or("").into())
}

impl Card {
    pub fn new(id: &str) -> Card {
        Card { id: id.into(), img: None, np: np::Np::new(), notes: (id == "notes").then(small::Notes::new), hwnd: Default::default() }
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
        match self.id.as_str() {
            "media" | "lyrics" | "cover" => self.np.size(cx, &self.id),
            "todo" => (W, small::todo_size(g) + pad - 8.0),
            "notes" => (W, self.notes.as_ref().map_or(pad, |n| n.size(g) + pad - 8.0)),
            "timer" => (W, small::timer_size() + pad - 8.0),
            "clock" => (W, if matches!(clock_style(cx), "digital" | "pixel") { 150.0 } else { 250.0 } + pad),
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
            id if id.starts_with("image") => {
                let (_, size, file) = image_conf(cx, id);
                if file.is_empty() { (size + 32.0, 28.0 + 53.0) } else { (size + 32.0, size + 28.0) }
            }
            _ => (W, pad),
        }
    }

    fn key(&self, cx: &Ctx) -> String {
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

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let st = util::now();
        let h24 = cx.cfg.get("use24h").as_bool() != Some(false);
        let (_, h) = (c.w, c.h);
        let card = R::new(4.0, 4.0, W - 8.0, h - 8.0);
        let bg = |c: &mut Canvas| c.rect(card, 23.0, alpha(t.c("surfaceContainerLow"), 0.88));
        let muted = t.c("onSurfaceVariant");
        let fg = t.c("onSurface");
        let scale = c.scale;
        if matches!(self.id.as_str(), "media" | "lyrics" | "cover") {
            let id = self.id.clone();
            return self.np.paint(c, cx, &id);
        }

        match self.id.as_str() {
            "todo" => { bg(c); small::todo_paint(c, cx); }
            "notes" => { bg(c); if let Some(n) = &mut self.notes { n.paint(c, cx); } }
            "timer" => { bg(c); small::timer_paint(c, cx); }
            "clock" => clock(c, cx, &st, h24),
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
            id if id.starts_with("image") => {
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

fn clock(c: &mut Canvas, cx: &Ctx, st: &windows::Win32::Foundation::SYSTEMTIME, h24: bool) {
    let t = &cx.theme;
    let (h, m, s) = ((st.wHour % 12) as f32, st.wMinute as f32, st.wSecond as f32);
    match clock_style(cx) {
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
    fn slots_wrap_into_columns() {
        let w: Vec<String> = ["clock", "weather", "calendar", "lyrics"].iter().map(|s| s.to_string()).collect();
        assert_eq!(default_slot(&w, "clock", 1040.0), (16.0, 72.0));
        assert_eq!(default_slot(&w, "weather", 1040.0), (16.0, 332.0));
        assert_eq!(default_slot(&w, "lyrics", 1040.0), (454.0, 72.0)); // 72+260+102+280 > 1024: next column
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
