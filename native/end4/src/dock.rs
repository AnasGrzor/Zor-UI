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
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::{Shell::*, WindowsAndMessaging::*},
};

const H: f32 = 64.0; // zpack preset height
const ITEM: f32 = 44.0;
const ICON: f32 = 30.0;
/// The docks' (window handle, monitor rect) so a config or monitor change can remove their app bars and rebuild.
static CUR: Mutex<Vec<(isize, [i32; 4])>> = Mutex::new(Vec::new());

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
    mon: (i32, i32, i32),
    sh: Arc<Mutex<Shared>>,
    icons: HashMap<String, (u64, u32, Option<Pixmap>)>,
    /// Group names by hit id (filled by paint).
    slots: Vec<String>,
    wins: Vec<GWin>,
}

impl Dock {
    /// `mon`: the monitor's x, y and width in physical px, like `Bar::new`.
    pub fn new(mon: (i32, i32, i32)) -> Dock { Dock { mon, sh: Default::default(), icons: HashMap::new(), slots: vec![], wins: vec![] } }

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

impl View for Dock {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) { (self.mon.2 as f32 / scale, H) }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        #[cfg(debug_assertions)]
        if std::env::var_os("END4_SAMPLE").is_some() { // `--render` has no GlazeWM: made-up windows
            let w = |id: &str, p: &str, f: bool| GWin { id: id.into(), process: p.into(), focus: f, ..Default::default() };
            self.wins = vec![w("1", "msedge", false), w("2", "Code", true), w("3", "Code", false), w("4", "WindowsTerminal", false)];
        }
        let t = &cx.theme;
        let list = self.items(cx);
        self.want_icons(cx, &list.iter().map(|g| g.name.clone()).collect::<Vec<_>>());
        self.slots = list.iter().map(|g| g.name.clone()).collect();
        if list.is_empty() { return; } // #dock:empty { display: none }
        let sep = list.iter().position(|g| !g.pinned).filter(|&i| i > 0);
        let w = list.len() as f32 * ITEM + (list.len() as f32 - 1.0) * 4.0 + if sep.is_some() { 5.0 + 4.0 } else { 0.0 } + 12.0 + 2.0;
        let pill = R::new(((c.w - w) / 2.0).round(), H - 4.0 - (ITEM + 14.0), w, ITEM + 14.0);
        ui::panel(c, cx, pill, 30.0);
        let mut x = pill.x + 7.0;
        let y = pill.y + 7.0;
        for (i, g) in list.iter().enumerate() {
            if sep == Some(i) {
                c.rect(R::new(x + 2.0, y + 6.0, 1.0, ITEM - 12.0), 0.0, t.c("outlineVariant"));
                x += 5.0 + 4.0;
            }
            let b = R::new(x, y, ITEM, ITEM);
            let focused = g.wins.iter().any(|w| w.1);
            if focused { c.rect(b, 17.0, t.c("secondaryContainer")); }
            if c.hit(b, 1 + i as u32) && !focused { c.rect(b, 17.0, alpha(t.c("onSurface"), 0.08)); }
            let ir = R::new(b.x + (ITEM - ICON) / 2.0, b.y + (ITEM - ICON) / 2.0, ICON, ICON);
            match self.icon(c.g, &g.name, (ICON * c.scale).round() as u32) {
                Some(p) => c.image(&p, ir, 1.0),
                None => {
                    c.circle(ir.x + ICON / 2.0, ir.y + ICON / 2.0, ICON / 2.0, t.c("tertiaryContainer"));
                    let letter: String = bar::pretty_app(&g.name).chars().next().into_iter().collect();
                    c.text(&letter, ir, txt(14.0, 600), t.c("onTertiaryContainer"), Align::Center);
                }
            }
            if !g.wins.is_empty() { // .running / .focused indicator
                let iw = if focused { 14.0 } else { 6.0 };
                c.rect(R::new(b.x + (ITEM - iw) / 2.0, b.y + ITEM - 2.0 - 3.0, iw, 3.0), 1.5, if focused { t.c("primary") } else { t.c("outline") });
            }
            x += ITEM + 4.0;
        }
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        let Input::Click(id, btn) = ev else { return };
        let Some(name) = self.slots.get(id as usize - 1).cloned() else { return };
        let list = self.items(cx);
        let Some(g) = list.iter().find(|g| g.name == name) else { return };
        match btn {
            Btn::Left => match next_window(&g.wins) {
                Some(w) => glaze::command_on(w, "focus"),
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

/// SHAppBarMessage at the bottom edge (Win::dock_top only does the top): reserves `h` physical px of `mon`.
fn dock_bottom(hwnd: HWND, mon: RECT, h: i32) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & !(WS_EX_TOOLWINDOW.0 as isize); // Explorer skips tool windows in its layout
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        let mut d = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: crate::win::WM_APPBAR, uEdge: ABE_BOTTOM,
            rc: RECT { left: mon.left, top: mon.bottom - h, right: mon.right, bottom: mon.bottom }, ..Default::default() };
        SHAppBarMessage(ABM_NEW, &mut d);
        SHAppBarMessage(ABM_QUERYPOS, &mut d);
        d.rc.top = d.rc.bottom - h;
        SHAppBarMessage(ABM_SETPOS, &mut d);
    }
}

fn remove_appbar(h: isize) {
    unsafe {
        let mut d = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: HWND(h as *mut _), ..Default::default() };
        SHAppBarMessage(ABM_REMOVE, &mut d);
    }
}

/// Opens / closes the docks to match config.json (called where `desk::sync` is).
/// shortcut: docks aren't rebuilt on a monitor change (restart), and they need a native bar's GlazeWM feed to list windows.
pub fn sync(app: &mut App) {
    let want = app.cx.native("dock") && app.cx.cfg.bool("dock.enabled");
    let (mons, _) = crate::util::monitors();
    let spec: Vec<[i32; 4]> = if want { mons.iter().map(|m| [m.left, m.top, m.right, m.bottom]).collect() } else { vec![] };
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if !cur.is_empty() && cur.iter().map(|c| c.1).collect::<Vec<_>>() == spec { return; }
    for (h, _) in cur.drain(..) {
        remove_appbar(h);
        app.wins.retain(|w| w.hwnd.0 as isize != h);
    }
    for m in spec {
        let rect = RECT { left: m[0], top: m[1], right: m[2], bottom: m[3] };
        let Ok(mut w) = Win::new(Box::new(Dock::new((m[0], m[3], m[2] - m[0]))), "end4-pC / dock", m[0], m[3] - 1, Z::Top, false) else { continue };
        w.update(&app.g, &app.cx, true);
        w.move_to(m[0], m[3] - (H * w.scale()).round() as i32);
        w.show();
        let h = (H * w.scale()).round() as i32;
        dock_bottom(w.hwnd, rect, h); // after show: the shell ignores app bars that aren't visible yet
        w.move_to(m[0], m[3] - h);
        w.pin();
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
    fn pin_and_unpin() {
        let p = json!([{ "name": "Code", "path": "c.exe" }]);
        assert_eq!(repinned(&p, "Code", None), Some(json!([])));
        assert_eq!(repinned(&p, "Orca", Some("o.exe")), Some(json!([{ "name": "Code", "path": "c.exe" }, { "name": "Orca", "path": "o.exe" }])));
        assert_eq!(repinned(&p, "Orca", None), None);
    }
}
