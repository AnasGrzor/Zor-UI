//! The app dock (dock.html, dock.js): a bottom app bar per monitor with the pinned apps, then every other app that has
//! a window. Icons are PNGs the bridge's `app-icons` writes into the pack's icons folder.

use crate::{
    app::{self, App, Ctx},
    bar,
    conf::{self, Config},
    gfx::{alpha, txt, Align, Canvas, Gfx, Pixmap, R},
    glaze::{self, GWin},
    proc, ui,
    win::{Btn, Input, View, Win, Z},
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use windows::Win32::{Foundation::POINT, UI::{HiDpi::GetDpiForSystem, WindowsAndMessaging::GetCursorPos}};

/// How the dock looks, from config dock.style / iconSize / radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look { pub icon: f32, pub item: f32, pub h: f32, pub gap: f32, pub radius: f32, pub plain: bool }

/// "hug": the pill 4 px from the edge. "float": the pill 12 px from it. "plain": a square strip along the whole edge.
pub fn look(cfg: &Config) -> Look {
    let icon = cfg.get("dock.iconSize").as_f64().unwrap_or(30.0).clamp(20.0, 56.0) as f32;
    let item = icon + 14.0;
    let (gap, plain) = match cfg.str("dock.style") { "float" => (12.0, false), "plain" => (0.0, true), _ => (4.0, false) };
    let radius = if plain { 0.0 } else { cfg.get("dock.radius").as_f64().unwrap_or(30.0).clamp(0.0, 40.0) as f32 };
    Look { icon, item, h: item + 14.0 + gap + 2.0, gap, radius, plain }
}
/// The docks' (window handle, monitor rect) so a config change can close them.
static CUR: Mutex<Vec<(isize, [i32; 6])>> = Mutex::new(Vec::new());

/// One dock button: an app and its windows.
#[derive(Debug, PartialEq)]
pub struct Group { pub name: String, pub pinned: bool, pub wins: Vec<(String, bool)> }

/// apps() in dock.js: pinned apps first (in pinned order), then the other apps with a window.
pub fn groups(pinned: &[String], wins: &[GWin]) -> Vec<Group> {
    let mut v: Vec<Group> = pinned.iter().map(|n| Group { name: n.clone(), pinned: true, wins: vec![] }).collect();
    for w in wins.iter().filter(|w| !w.process.is_empty()) {
        let i = v.iter().position(|g| g.name == w.process).unwrap_or_else(|| { v.push(Group { name: w.process.clone(), pinned: false, wins: vec![] }); v.len() - 1 });
        v[i].wins.push((w.id.clone(), w.focus));
    }
    v
}

/// The window a click focuses: the one after the focused one (clicking again cycles).
pub fn next_window(wins: &[(String, bool)]) -> Option<&str> {
    let at = wins.iter().position(|w| w.1).map_or(0, |i| i + 1);
    wins.get(at % wins.len().max(1)).map(|w| w.0.as_str())
}

/// config dock.pinned after pinning / unpinning `name` (None: can't pin, no exe path known).
pub fn repinned(pinned: &Value, name: &str, exe: Option<&str>) -> Option<Value> {
    let list: Vec<Value> = pinned.as_array().cloned().unwrap_or_default();
    let kept: Vec<Value> = list.iter().filter(|p| p["name"] != name).cloned().collect();
    if kept.len() != list.len() { return Some(Value::Array(kept)); }
    let mut kept = kept;
    kept.push(json!({ "name": name, "path": exe? }));
    Some(Value::Array(kept))
}

#[derive(Default)]
struct Shared {
    /// app -> exe path (from app-icons), for pinning.
    exes: HashMap<String, String>,
    asked: HashSet<String>,
    /// Bumped when new icon files may exist.
    stamp: u64,
}

pub struct Dock {
    /// The monitor's left, top, right, bottom in physical px.
    mon: [i32; 4],
    /// Docked to the right edge instead of the bottom (config dock.side).
    right: bool,
    sh: Arc<Mutex<Shared>>,
    icons: HashMap<String, (u64, u32, Option<Pixmap>)>,
    /// Group names by hit id (filled by paint).
    slots: Vec<String>,
    wins: Vec<GWin>,
    /// The hovered spot id (1 + slot), from `Input::Hover`.
    hover: u32,
    /// Each slot's rect and tooltip text (filled by paint).
    tips: Vec<(R, String)>,
    /// Auto-hide: the dock is drawn only while the pointer is at the bottom edge (or on the dock, plus a short grace).
    up: Cell<bool>,
    last_hit: Cell<Option<Instant>>,
}

impl Dock {
    pub fn new(mon: [i32; 4], right: bool) -> Dock { Dock { mon, right, sh: Default::default(), icons: HashMap::new(), slots: vec![], wins: vec![], hover: 0, tips: vec![], up: Cell::new(false), last_hit: Cell::new(None) } }

    /// Updates `up` from the pointer: reveal at the monitor's bottom (or right) 2 px, stay up while over the dock.
    fn track(&self, h: f32) -> bool {
        let mut p = POINT::default();
        let zone = if self.up.get() { (h * unsafe { GetDpiForSystem() } as f32 / 96.0) as i32 + 4 } else { 2 };
        let [l, t, r, b] = self.mon;
        let hit = unsafe { GetCursorPos(&mut p) }.is_ok() && if self.right { p.y >= t && p.y < b && p.x >= r - zone } else { p.x >= l && p.x < r && p.y >= b - zone };
        if hit { self.last_hit.set(Some(Instant::now())); }
        let up = hit || self.last_hit.get().is_some_and(|t| t.elapsed() < Duration::from_millis(400));
        self.up.set(up);
        up
    }

    fn pinned(cx: &Ctx) -> Vec<String> {
        cx.cfg.get("dock.pinned").as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(String::from)).collect()).unwrap_or_default()
    }

    /// Asks the bridge for icons of apps not asked about yet (one call for the batch).
    fn want_icons(&self, cx: &Ctx, names: &[String]) {
        let Ok(mut s) = self.sh.lock() else { return };
        let missing: Vec<String> = names.iter().filter(|n| s.asked.insert((*n).clone())).cloned().collect();
        if missing.is_empty() { return; }
        let (cfg, sh) = (Config(cx.cfg.0.clone()), self.sh.clone());
        std::thread::spawn(move || {
            let args: Vec<&str> = missing.iter().map(String::as_str).collect();
            let r = proc::bridge(&cfg, "app-icons", &args);
            if let (Ok(mut s), Some(e)) = (sh.lock(), r["exes"].as_object()) {
                for (k, v) in e { if let Some(p) = v.as_str() { s.exes.insert(k.clone(), p.to_string()); } }
                s.stamp += 1;
            }
            app::post(Box::new(|_| {}));
        });
    }

    fn icon(&mut self, g: &Gfx, name: &str, px: u32) -> Option<Pixmap> {
        let stamp = self.sh.lock().map_or(0, |s| s.stamp);
        if self.icons.get(name).map_or(true, |e| e.0 != stamp || e.1 != px) {
            let file = conf::pack_dir().join("icons").join(format!("{name}.png"));
            self.icons.insert(name.to_string(), (stamp, px, g.load_image(&file, px, px)));
        }
        self.icons.get(name).and_then(|e| e.2.clone())
    }

    fn items(&self, cx: &Ctx) -> Vec<Group> {
        let wins: &[GWin] = if self.wins.is_empty() { &cx.wm.windows } else { &self.wins };
        groups(&Self::pinned(cx), wins)
    }
}

/// Tooltip of a dock slot: the app name, plus the window count when it has several.
fn tip(g: &Group) -> String {
    let name = bar::pretty_app(&g.name);
    if g.wins.len() > 1 { format!("{name} ({} windows)", g.wins.len()) } else { name }
}

/// The dock pill inside a window of `cw` x `ch`: centred along the edge, 4 px in from it.
/// Returns the pill and the length of the icons inside it (a plain strip is longer than its icons).
fn pill_rect(right: bool, cw: f32, ch: f32, n: usize, sep: bool, k: &Look) -> (R, f32) {
    let len = n as f32 * k.item + (n as f32 - 1.0) * 4.0 + if sep { 5.0 + 4.0 } else { 0.0 } + 12.0 + 2.0;
    let thick = k.item + 14.0;
    let r = if right {
        let (y, l) = if k.plain { (0.0, ch) } else { (((ch - len) / 2.0).round(), len) };
        R::new(cw - k.gap - thick, y, thick, l)
    } else {
        let (x, l) = if k.plain { (0.0, cw) } else { (((cw - len) / 2.0).round(), len) };
        R::new(x, k.h - k.gap - thick, l, thick)
    };
    (r, len)
}

impl View for Dock {
    fn size(&self, _: &Gfx, cx: &Ctx, scale: f32) -> (f32, f32) {
        let [l, t, r, b] = self.mon;
        let h = look(&Config(cx.cfg.0.clone())).h;
        if self.right { (h, (b - t) as f32 / scale) } else { ((r - l) as f32 / scale, h) }
    }

    fn key(&self, cx: &Ctx) -> String { self.track(look(&Config(cx.cfg.0.clone())).h).to_string() }

    /// Polls the pointer for the reveal.
    fn frame_ms(&self) -> u32 { 50 }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        #[cfg(debug_assertions)]
        if std::env::var_os("ZORUI_SAMPLE").is_some() { self.up.set(true); // `--render` has no GlazeWM: made-up windows
            let w = |id: &str, p: &str, f: bool| GWin { id: id.into(), process: p.into(), focus: f, ..Default::default() };
            self.wins = vec![w("1", "msedge", false), w("2", "Code", true), w("3", "Code", false), w("4", "WindowsTerminal", false)];
        }
        let t = &cx.theme;
        let list = self.items(cx);
        self.want_icons(cx, &list.iter().map(|g| g.name.clone()).collect::<Vec<_>>());
        self.slots = list.iter().map(|g| g.name.clone()).collect();
        self.tips.clear();
        if list.is_empty() || !self.up.get() { return; } // #dock:empty { display: none }, and hidden until the pointer reaches the edge
        let sep = list.iter().position(|g| !g.pinned).filter(|&i| i > 0);
        let k = look(&Config(cx.cfg.0.clone()));
        let (item, icon) = (k.item, k.icon);
        let (pill, len) = pill_rect(self.right, c.w, c.h, list.len(), sep.is_some(), &k);
        ui::panel(c, cx, pill, k.radius);
        let slack = if self.right { (pill.h - len) / 2.0 } else { (pill.w - len) / 2.0 };
        let (mut x, mut y) = if self.right { (pill.x + 7.0, pill.y + 7.0 + slack) } else { (pill.x + 7.0 + slack, pill.y + 7.0) };
        for (i, g) in list.iter().enumerate() {
            if sep == Some(i) {
                if self.right { c.rect(R::new(x + 6.0, y + 2.0, item - 12.0, 1.0), 0.0, t.c("outlineVariant")); y += 5.0 + 4.0; }
                else { c.rect(R::new(x + 2.0, y + 6.0, 1.0, item - 12.0), 0.0, t.c("outlineVariant")); x += 5.0 + 4.0; }
            }
            let b = R::new(x, y, item, item);
            self.tips.push((b, tip(g)));
            let focused = g.wins.iter().any(|w| w.1);
            if focused { c.rect(b, 17.0, t.c("secondaryContainer")); }
            if c.hit(b, 1 + i as u32) && !focused { c.rect(b, 17.0, alpha(t.c("onSurface"), 0.08)); }
            let ir = R::new(b.x + (item - icon) / 2.0, b.y + (item - icon) / 2.0, icon, icon);
            match self.icon(c.g, &g.name, (icon * c.scale).round() as u32) {
                Some(p) => c.image(&p, ir, 1.0),
                None => {
                    c.circle(ir.x + icon / 2.0, ir.y + icon / 2.0, icon / 2.0, t.c("tertiaryContainer"));
                    let letter: String = bar::pretty_app(&g.name).chars().next().into_iter().collect();
                    c.text(&letter, ir, txt(14.0, 600), t.c("onTertiaryContainer"), Align::Center);
                }
            }
            if !g.wins.is_empty() { // .running / .focused indicator
                let iw = if focused { 14.0 } else { 6.0 };
                let col = if focused { t.c("primary") } else { t.c("outline") };
                if self.right { c.rect(R::new(b.x + item - 2.0 - 3.0, b.y + (item - iw) / 2.0, 3.0, iw), 1.5, col); }
                else { c.rect(R::new(b.x + (item - iw) / 2.0, b.y + item - 2.0 - 3.0, iw, 3.0), 1.5, col); }
            }
            if self.right { y += item + 4.0 } else { x += item + 4.0 }
        }
    }

    fn hover_popup(&self) -> Option<(u32, R, String)> {
        if !self.up.get() { return None; }
        let (r, tip) = self.tips.get((self.hover as usize).checked_sub(1)?)?;
        Some((self.hover, *r, tip.clone()))
    }

    fn popup_edge(&self) -> Option<bar::Edge> { Some(if self.right { bar::Edge::Right } else { bar::Edge::Bottom }) }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        if let Input::Hover(id) = ev { self.hover = id; return; }
        let Input::Click(id, btn) = ev else { return };
        let Some(name) = self.slots.get(id as usize - 1).cloned() else { return };
        let list = self.items(cx);
        let Some(g) = list.iter().find(|g| g.name == name) else { return };
        match btn {
            Btn::Left => match next_window(&g.wins) {
                Some(w) => glaze::focus(w, cx.wm.windows.iter().any(|x| x.id == w && x.minimized)),
                None => {
                    let cfg = Config(cx.cfg.0.clone());
                    std::thread::spawn(move || { proc::bridge(&cfg, "launch", &[&name]); });
                }
            },
            Btn::Right => {
                let exe = self.sh.lock().ok().and_then(|s| s.exes.get(&name).cloned()); // None: e.g. an elevated app, can't be relaunched
                let Some(list) = repinned(cx.cfg.get("dock.pinned"), &name, exe.as_deref()) else { return };
                let cfg = Config(cx.cfg.0.clone());
                std::thread::spawn(move || { proc::bridge(&cfg, "config-set", &["dock.pinned", &list.to_string()]); }); // reloads config.json
            }
            Btn::Middle | Btn::Double => {}
        }
    }
}

/// Opens / closes the docks to match config.json (called where `desk::sync` is).
/// shortcut: they need a native bar's GlazeWM feed to list windows.
pub fn sync(app: &mut App) {
    let want = app.cx.native("dock") && app.cx.cfg.bool("dock.enabled");
    let (mons, _) = crate::util::monitors();
    let side = (app.cx.cfg.str("dock.side") == "right") as i32;
    let k = look(&Config(app.cx.cfg.0.clone()));
    // the window's thickness depends on the look, so a change of icon size or style rebuilds it
    let spec: Vec<[i32; 6]> = if want { mons.iter().map(|m| [m.left, m.top, m.right, m.bottom, side, k.h as i32]).collect() } else { vec![] };
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if !cur.is_empty() && cur.iter().map(|c| c.1).collect::<Vec<_>>() == spec { return; }
    for (h, _) in cur.drain(..) {
        app.wins.retain(|w| w.hwnd.0 as isize != h);
    }
    for m in spec {
        let right = m[4] == 1;
        let (x0, y0) = if right { (m[2] - 1, m[1]) } else { (m[0], m[3] - 1) };
        let Ok(mut w) = Win::new(Box::new(Dock::new([m[0], m[1], m[2], m[3]], right)), "Zor UI / dock", x0, y0, Z::Top, false) else { continue };
        w.update(&app.g, &app.cx, true);
        let edge = (k.h * w.scale()).round() as i32;
        if right { w.move_to(m[2] - edge, m[1]) } else { w.move_to(m[0], m[3] - edge) }
        w.show();
        w.pin(); // floats over the windows: no app bar, so nothing is reserved
        cur.push((w.hwnd.0 as isize, m));
        app.wins.push(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn win(id: &str, p: &str, focus: bool) -> GWin { GWin { id: id.into(), process: p.into(), focus, ..Default::default() } }

    #[test]
    fn pinned_first_then_running_grouped() {
        let g = groups(&["Code".into(), "Spotify".into()], &[win("1", "msedge", false), win("2", "Code", true), win("3", "msedge", false), win("4", "", false)]);
        let names: Vec<_> = g.iter().map(|g| (g.name.as_str(), g.pinned, g.wins.len())).collect();
        assert_eq!(names, vec![("Code", true, 1), ("Spotify", true, 0), ("msedge", false, 2)]);
    }
    #[test]
    fn clicking_cycles_windows() {
        let w = vec![("1".to_string(), false), ("2".to_string(), true), ("3".to_string(), false)];
        assert_eq!(next_window(&w), Some("3"));
        assert_eq!(next_window(&[("1".to_string(), false), ("2".to_string(), false)]), Some("1")); // none focused: first
        assert_eq!(next_window(&[("1".to_string(), true)]), Some("1"));
        assert_eq!(next_window(&[]), None);
    }
    #[test]
    fn look_follows_config() {
        let cfg = |j: &str| Config::parse(j);
        let hug = look(&cfg("{}"));
        assert_eq!((hug.icon, hug.item, hug.gap, hug.radius, hug.plain), (30.0, 44.0, 4.0, 30.0, false));
        assert_eq!(hug.h, 64.0); // the old fixed height: 44 + 14 + 4 + 2
        let f = look(&cfg(r#"{"dock":{"style":"float","iconSize":40,"radius":12}}"#));
        assert_eq!((f.icon, f.gap, f.radius, f.h), (40.0, 12.0, 12.0, 82.0));
        let p = look(&cfg(r#"{"dock":{"style":"plain","radius":20}}"#));
        assert!(p.plain && p.radius == 0.0 && p.gap == 0.0);
        assert_eq!(look(&cfg(r#"{"dock":{"iconSize":500}}"#)).icon, 56.0);
    }
    #[test]
    fn pill_sits_on_the_chosen_edge() {
        let k = look(&Config::parse("{}"));
        let (b, len) = pill_rect(false, 1920.0, k.h, 3, false, &k);
        assert_eq!((b.h, b.x + b.w / 2.0, b.w), (k.item + 14.0, 960.0, len));
        assert!(b.y + b.h <= k.h - k.gap); // the gap to the edge is kept
        let (r, rlen) = pill_rect(true, k.h, 1080.0, 3, true, &k);
        assert_eq!((r.w, r.x + r.w + k.gap), (k.item + 14.0, k.h));
        assert!((r.y + r.h / 2.0 - 540.0).abs() <= 0.5 && rlen > len); // y is rounded; the separator adds length
        let p = look(&Config::parse(r#"{"dock":{"style":"plain"}}"#));
        let (strip, inner) = pill_rect(false, 1920.0, p.h, 3, false, &p);
        assert_eq!((strip.x, strip.w), (0.0, 1920.0));
        assert!(inner < strip.w);
    }
    #[test]
    fn tooltip_names_the_app_and_counts_windows() {
        let g = |n: &str, k: usize| Group { name: n.into(), pinned: true, wins: (0..k).map(|i| (i.to_string(), false)).collect() };
        assert_eq!(tip(&g("Code", 0)), bar::pretty_app("Code"));
        assert_eq!(tip(&g("Code", 1)), bar::pretty_app("Code"));
        assert_eq!(tip(&g("Code", 3)), format!("{} (3 windows)", bar::pretty_app("Code")));
    }
    #[test]
    fn hover_shows_a_tooltip_only_while_the_dock_is_up() {
        let mut d = Dock::new([0, 0, 1920, 1080], false);
        d.tips = vec![(R::new(10.0, 0.0, 44.0, 44.0), "Code".into()), (R::new(58.0, 0.0, 44.0, 44.0), "Edge".into())];
        d.hover = 2;
        assert!(d.hover_popup().is_none(), "hidden dock");
        d.up.set(true);
        let (id, r, t) = d.hover_popup().unwrap();
        assert_eq!((id, r.x, t.as_str()), (2, 58.0, "Edge"));
        d.hover = 0;
        assert!(d.hover_popup().is_none(), "pointer off the icons");
        d.hover = 9;
        assert!(d.hover_popup().is_none(), "unknown spot");
        assert_eq!(d.popup_edge(), Some(bar::Edge::Bottom));
        assert_eq!(Dock::new([0, 0, 1, 1], true).popup_edge(), Some(bar::Edge::Right));
    }
    #[test]
    fn pin_and_unpin() {
        let p = json!([{ "name": "Code", "path": "c.exe" }]);
        assert_eq!(repinned(&p, "Code", None), Some(json!([])));
        assert_eq!(repinned(&p, "Orca", Some("o.exe")), Some(json!([{ "name": "Code", "path": "c.exe" }, { "name": "Orca", "path": "o.exe" }])));
        assert_eq!(repinned(&p, "Orca", None), None);
    }
}
