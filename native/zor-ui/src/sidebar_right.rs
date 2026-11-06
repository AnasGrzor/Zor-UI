//! The right sidebar (sidebar-right.js + sidebar-right.html): header, quick toggles, sliders, per-app
//! volume, and the notifications / calendar / to-do / timer tabs. Shell calls run on threads and land in
//! `Shared` (data.rs); the view only draws it. Drawing of the tabs is in pages.rs.

mod data;
mod pages;

use crate::{
    app::{Action, Ctx},
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    glaze, hub, media, proc, ui,
    win::{Btn, Input, Phase, View},
};
use data::*;
use std::{
    collections::HashMap,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc},
    time::Duration,
};
use windows::Win32::System::SystemInformation::GetTickCount64;

const NAME: &str = "sidebar-right";
// hit ids
const ID_SETTINGS: u32 = 1;
const ID_RELOAD: u32 = 2;
const ID_POWER: u32 = 3;
const ID_POWER_ITEM: u32 = 10; // + 0..5
const ID_REC_MENU: u32 = 19; // the chevron on the Screen record tile
const ID_TOGGLE: u32 = 20; // + 0..16
const ID_SLIDER: u32 = 40; // volume, mic, brightness
const ID_MIXER_BTN: u32 = 50;
const ID_MIX_SLIDER: u32 = 100; // + row
const ID_MIX_MUTE: u32 = 900; // + row (200..300 belongs to the notification and timer pages)
const ID_TAB: u32 = 300; // + 0..4
const TABS: [&str; 4] = ["notif", "cal", "todo", "timer"];

/// A scrollable area: offset plus the sizes the last paint measured (the wheel needs them).
#[derive(Default)]
pub(crate) struct Sc { s: ui::Scroll, content: f32, view: f32 }

impl Sc {
    /// `Scroll::region`, then trims the hit spots the body added to the visible part (spots ignore clips).
    fn region(&mut self, c: &mut Canvas, cx: &Ctx, r: R, content_h: f32, body: impl FnOnce(&mut Canvas, f32)) {
        (self.content, self.view) = (content_h, r.h);
        let from = c.hits.len();
        self.s.region(c, cx, r, content_h, body);
        for h in &mut c.hits[from..] {
            let (x0, y0) = (h.0.x.max(r.x), h.0.y.max(r.y));
            let (x1, y1) = ((h.0.x + h.0.w).min(r.x + r.w), (h.0.y + h.0.h).min(r.y + r.h));
            h.0 = R::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
        }
    }
    fn wheel(&mut self, d: i32) { self.s.wheel(d, self.content, self.view); }
}

pub struct SidebarRight {
    area: (f32, f32),
    sh: Sh,
    alive: Arc<AtomicBool>,
    cfg: Option<Arc<crate::conf::Config>>,
    started: bool,
    power_open: bool,
    /// Restart and shut down need a second click.
    armed: [bool; 2],
    mixer_open: bool,
    tab: usize,
    cal: (i32, u32),
    todo: ui::TextBox,
    todo_focus: bool,
    sw_mode: bool,
    sc_mixer: Sc,
    sc_notif: Sc,
    sc_todo: Sc,
    sc_page: Sc,
    sc_laps: Sc,
    /// Slider rects of the last paint, for Drag.
    rects: Vec<(u32, R)>,
    /// (slider id, 0-100) while the button is held: shown instead of the live value.
    dragv: Option<(u32, f32)>,
    /// (toggle id, windows settings page) of the last paint.
    tg: Vec<(&'static str, &'static str)>,
    gens: HashMap<String, Arc<AtomicU64>>,
    render: bool,
}

impl SidebarRight {
    /// `area`: the CSS-px size the popup placement gives the view (popup::area).
    pub fn new(area: (f32, f32)) -> SidebarRight {
        let today = crate::util::now();
        SidebarRight {
            area, sh: Default::default(), alive: Arc::new(AtomicBool::new(true)), cfg: None, started: false, power_open: false, armed: [false; 2], mixer_open: false,
            tab: 0, cal: (today.wYear as i32, today.wMonth as u32), todo: ui::TextBox::new(false), todo_focus: false, sw_mode: false,
            sc_mixer: Default::default(), sc_notif: Default::default(), sc_todo: Default::default(), sc_page: Default::default(), sc_laps: Default::default(),
            rects: vec![], dragv: None, tg: vec![], gens: HashMap::new(), render: std::env::args().nth(1).as_deref() == Some("--render"),
        }
    }

    /// First paint: reads the saved tab / timer mode and starts the fetch threads (needs the config, which `new` doesn't get).
    fn start(&mut self, cx: &Ctx) {
        if self.started { return; }
        self.started = true;
        self.tab = hub::get("zor-ui.rightTab").and_then(|t| TABS.iter().position(|x| *x == t)).unwrap_or(0);
        self.sw_mode = cx.state.v["timerMode"].as_str() == Some("sw");
        if self.render {
            // --render can't click: ZORUI_SR="timer,sw,mixer,power" shows those states
            let o = std::env::var("ZORUI_SR").unwrap_or_default();
            let has = |k: &str| o.split(',').any(|x| x == k);
            if let Some(i) = TABS.iter().position(|t| has(t)) { self.tab = i; }
            (self.sw_mode, self.mixer_open, self.power_open) = (has("sw"), has("mixer"), has("power"));
            *lock(&self.sh) = sample();
            lock(&self.sh).mixer_open = self.mixer_open;
            return;
        }
        let cfg = Arc::new(crate::conf::Config(cx.cfg.0.clone()));
        self.cfg = Some(cfg.clone());
        start(cfg, self.sh.clone(), self.alive.clone());
    }

    /// Runs `f` on a thread with the toggle `id` shown busy; a second click meanwhile does nothing.
    fn exec(&self, id: &str, f: impl FnOnce(&crate::conf::Config, &Sh) + Send + 'static) {
        let Some(cfg) = self.cfg.clone() else { return };
        if !lock(&self.sh).busy.insert(id.to_string()) { return; }
        let (sh, id) = (self.sh.clone(), id.to_string());
        std::thread::spawn(move || {
            f(&cfg, &sh);
            lock(&sh).busy.remove(&id);
            repaint();
        });
    }

    /// Runs `f` on a thread after `ms` unless a newer call with the same key came first.
    fn debounce(&mut self, key: &str, ms: u64, f: impl FnOnce() + Send + 'static) {
        let gen = self.gens.entry(key.to_string()).or_default().clone();
        let n = gen.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            if gen.load(Ordering::SeqCst) == n { f(); }
        });
    }

    fn close(cx: &mut Ctx) { cx.actions.push(Action::Close(NAME.into())); }

    /// The `record.audio` menu at the pointer.
    fn record_menu(cx: &mut Ctx) {
        let mut p = windows::Win32::Foundation::POINT::default();
        unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p); }
        cx.actions.push(Action::Menu(record_items(&cx.cfg), (p.x, p.y)));
    }

    fn toggle(&mut self, id: &'static str, cx: &mut Ctx) {
        match id {
            "wifi" => self.exec(id, |cfg, sh| if let Some(b) = proc::run(cfg, "toggles", &["-Action", "wifi"])["wifi"].as_bool() { lock(sh).wifi = Some(b); }),
            "bluetooth" => self.exec(id, |cfg, sh| if let Some(b) = proc::run(cfg, "toggles", &["-Action", "bluetooth"])["bluetooth"].as_bool() { lock(sh).bluetooth = Some(b); }),
            "caffeine" => self.exec(id, |cfg, sh| if let Some(b) = proc::run(cfg, "toggles", &["-Action", "caffeine"])["caffeine"].as_bool() { lock(sh).caffeine = b; }),
            "dark" => self.exec(id, |cfg, _| { proc::run(cfg, "switchwall", &["-ToggleMode"]); }),
            "sound" => if let Some(a) = cx.audio.as_mut() { a.muted = !a.muted; media::set_mute(a.muted); },
            "output" => {
                // a menu of the output devices at the pointer; picking one makes it the default
                let mut p = windows::Win32::Foundation::POINT::default();
                unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p); }
                let items = output_items(&media::output_devices(), &self.sh);
                if !items.is_empty() { cx.actions.push(Action::Menu(items, (p.x, p.y))); }
            }
            "mic" => if let Some(a) = cx.audio.as_mut() { if let Some(m) = a.mic_muted { a.mic_muted = Some(!m); media::set_mic_mute(!m); } },
            "tiling" => if !cx.wm.monitors.is_empty() { glaze::command("wm-toggle-pause"); },
            "nightlight" => self.exec(id, |cfg, sh| {
                let r = proc::bridge(cfg, "nightlight", &["toggle"]);
                let ok = r["ok"].as_bool() != Some(false);
                { let mut s = lock(sh); s.night = r["on"].as_bool() == Some(true); s.night_err = if ok { String::new() } else { "Not supported here".into() }; }
                if !ok { proc::run(cfg, "toggles", &["-Action", "settings", "-Value", "nightlight"]); } // Windows' own
            }),
            "snip" => { Self::close(cx); proc::run_bg(&cx.cfg, "toggles", &["-Action", "snip"]); }
            "osk" => proc::run_bg(&cx.cfg, "toggles", &["-Action", "osk"]),
            "gamemode" => proc::run_bg(&cx.cfg, "toggles", &["-Action", "gamemode"]), // same as Mod+Shift+G
            "power" => self.exec(id, |cfg, sh| { let p = parse_plan(&proc::run(cfg, "toggles", &["-Action", "powerplan", "-Value", "next"])); if p.is_some() { lock(sh).plan = p; } }),
            "record" => {
                let recording = rec_started() > 0.0;
                self.exec(id, move |cfg, _| {
                    if recording { proc::bridge(cfg, "record", &["stop"]); hub::set("zor-ui.rec", None); return; }
                    let r = proc::bridge(cfg, "record", &["start"]);
                    if r["recording"].as_bool() == Some(true) {
                        hub::set("zor-ui.rec", Some(((r["started"].as_f64().unwrap_or(0.0) * 1000.0).round() as i64).to_string()));
                        crate::app::post(Box::new(|cx| Self::close(cx)));
                    }
                });
            }
            "colorpick" | "screentranslate" => { Self::close(cx); proc::run_bg(&cx.cfg, "toggles", &["-Action", id]); }
            _ => {}
        }
    }

    fn click(&mut self, id: u32, btn: Btn, cx: &mut Ctx) {
        if id != 230 { self.todo_focus = false; }
        let right = btn == Btn::Right;
        match id {
            ID_SETTINGS if right => proc::run_bg(&cx.cfg, "toggles", &["-Action", "settings", "-Value", ""]),
            ID_SETTINGS => { cx.actions.push(Action::open("settings")); Self::close(cx); }
            // right-click on Screen record: pick what it records (microphone, system sound, both, none)
            ID_REC_MENU => Self::record_menu(cx),
            _ if right && (ID_TOGGLE..ID_TOGGLE + 16).contains(&id) && self.tg.get((id - ID_TOGGLE) as usize).is_some_and(|t| t.0 == "record") => Self::record_menu(cx),
            _ if right && (ID_TOGGLE..ID_TOGGLE + 16).contains(&id) => {
                if let Some(&(_, page)) = self.tg.get((id - ID_TOGGLE) as usize).filter(|t| !t.1.is_empty()) { proc::run_bg(&cx.cfg, "toggles", &["-Action", "settings", "-Value", page]); }
            }
            _ if right => {}
            ID_RELOAD => glaze::command("wm-reload-config"),
            ID_POWER => { self.power_open = !self.power_open; self.armed = [false; 2]; }
            i if (ID_POWER_ITEM..ID_POWER_ITEM + 5).contains(&i) => {
                let k = (i - ID_POWER_ITEM) as usize;
                if k >= 3 && !self.armed[k - 3] { self.armed[k - 3] = true; return; }
                proc::run_bg(&cx.cfg, "toggles", &["-Action", ["lock", "sleep", "logout", "restart", "shutdown"][k]]);
            }
            i if (ID_TOGGLE..ID_TOGGLE + 16).contains(&i) => if let Some(&(t, _)) = self.tg.get((i - ID_TOGGLE) as usize) { self.toggle(t, cx); },
            ID_MIXER_BTN => {
                self.mixer_open = !self.mixer_open;
                lock(&self.sh).mixer_open = self.mixer_open;
                if let (true, Some(cfg)) = (self.mixer_open, self.cfg.clone()) {
                    let sh = self.sh.clone();
                    std::thread::spawn(move || { load_mixer(&cfg, &sh); repaint(); });
                }
            }
            i if (ID_MIX_MUTE..ID_MIX_MUTE + 100).contains(&i) => {
                let Some(cfg) = self.cfg.clone() else { return };
                let (app, muted) = {
                    let mut s = lock(&self.sh);
                    let Some(m) = s.mixer.get_mut((i - ID_MIX_MUTE) as usize) else { return };
                    m.muted = !m.muted;
                    (m.app.clone(), m.muted)
                };
                let sh = self.sh.clone();
                std::thread::spawn(move || {
                    let r = proc::bridge(&cfg, "mixer", &["mute", &app, if muted { "1" } else { "0" }]);
                    if r["apps"].is_array() { lock(&sh).mixer = parse_mixer(&r); }
                    repaint();
                });
            }
            i if (ID_TAB..ID_TAB + 4).contains(&i) => { self.tab = (i - ID_TAB) as usize; hub::set("zor-ui.rightTab", Some(TABS[self.tab].into())); }
            _ => self.page_click(id, cx),
        }
    }

    fn drag(&mut self, id: u32, x: f32, ph: Phase, cx: &mut Ctx) {
        if id == 230 { if ph == Phase::Start { self.todo_focus = true; } return; }
        let Some(&(_, r)) = self.rects.iter().find(|(i, _)| *i == id) else { return };
        if ph == Phase::End { self.dragv = None; return; }
        let v = (ui::slider_value(r, x) * 100.0).round();
        self.dragv = Some((id, v));
        match id {
            ID_SLIDER => { if let Some(a) = cx.audio.as_mut() { a.volume = v; } media::set_volume(v); }
            41 => { if let Some(a) = cx.audio.as_mut() { a.mic_volume = Some(v); } media::set_mic_volume(v); }
            42 => if let Some(cfg) = self.cfg.clone() {
                lock(&self.sh).brightness = Some(v);
                self.debounce("brightness", 120, move || { proc::run(&cfg, "toggles", &["-Action", "brightness", "-Value", &v.to_string(), "-Monitor", "active"]); });
            },
            i if (ID_MIX_SLIDER..ID_MIX_SLIDER + 100).contains(&i) => {
                let Some(cfg) = self.cfg.clone() else { return };
                let app = {
                    let mut s = lock(&self.sh);
                    let Some(m) = s.mixer.get_mut((i - ID_MIX_SLIDER) as usize) else { return };
                    m.volume = v;
                    m.app.clone()
                };
                self.debounce(&format!("mix:{app}"), 150, move || { proc::bridge(&cfg, "mixer", &["volume", &app, &v.to_string()]); });
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------- drawing

    fn paint_header(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, x0: f32, y: f32, cw: f32) {
        let t = &cx.theme;
        let name = [cx.cfg.str("userName"), cx.host.as_str()].into_iter().find(|s| !s.is_empty()).unwrap_or("Windows").to_string();
        let letter = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
        c.circle(x0 + 26.0, y + 22.0, 20.0, t.c("primaryContainer"));
        c.text(&letter, R::new(x0 + 6.0, y + 2.0, 40.0, 40.0), txt(18.0, 600), t.c("onPrimaryContainer"), Align::Center);
        let bx = x0 + cw - 2.0 - 36.0 * 3.0 - 20.0;
        let wx = x0 + 56.0;
        let ww = bx - 10.0 - wx;
        c.text(&name, R::new(wx, y + 2.0 + (40.0 - 33.75) / 2.0, ww, 18.75), txt(15.0, 500), t.c("onSurface"), Align::Left);
        let up = uptime_text(unsafe { GetTickCount64() }, &s.os);
        c.text(&up, R::new(wx, y + 2.0 + (40.0 - 33.75) / 2.0 + 18.75, ww, 15.0), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
        for (i, (id, icon)) in [(ID_SETTINGS, "settings"), (ID_RELOAD, "restart_alt"), (ID_POWER, "power_settings_new")].into_iter().enumerate() {
            ui::icon_btn(c, cx, R::new(bx + i as f32 * 46.0, y + 4.0, 36.0, 36.0), id, icon, ui::IconKind::Plain, 20.0, false);
        }
    }

    fn paint_power(&self, c: &mut Canvas, cx: &Ctx, x0: f32, y: f32, cw: f32) {
        let t = &cx.theme;
        let bw = (cw - 24.0) / 5.0;
        for (i, (icon, label)) in [("lock", "Lock"), ("bedtime", "Sleep"), ("logout", "Sign out"), ("restart_alt", "Restart"), ("power_settings_new", "Shut down")].into_iter().enumerate() {
            let r = R::new(x0 + i as f32 * (bw + 6.0), y, bw, 58.0);
            let danger = i == 4;
            let over = c.hit(r, ID_POWER_ITEM + i as u32);
            c.rect(r, 17.0, if danger { t.c("error") } else if over { t.c("surfaceContainerHigh") } else { t.c("surfaceContainer") });
            let fg = if danger { t.c("onError") } else { t.c("onSurface") };
            c.icon(icon, R::new(r.x, r.y + 10.0, r.w, 20.0), ico(20.0, false), fg);
            let label = if i >= 3 && self.armed[i - 3] { "Confirm" } else { label };
            c.text(label, R::new(r.x, r.y + 34.0, r.w, 14.4), txt(11.5, 400), fg, Align::Center);
        }
    }

    fn paint_toggles(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, x0: f32, y: f32, cw: f32) {
        let t = &cx.theme;
        let i = DefIn {
            s, audio: cx.audio, ssid: cx.net.as_ref().filter(|n| n.kind == "wifi").map_or("", |n| n.ssid.as_str()), dark: cx.theme.dark,
            paused: if cx.wm.monitors.is_empty() { None } else { Some(cx.wm.paused) }, kelvin: cx.cfg.get("nightLight.kelvin").as_i64().unwrap_or(4500),
            target: cx.cfg.str("translator.target"), sound: cx.cfg.str("record.audio"), rec: rec_started(), now: hub::now_ms(),
        };
        let defs = defs(&i);
        self.tg = defs.iter().map(|d| (d.id, d.settings)).collect();
        let tw = (cw - 6.0) / 2.0;
        for (k, d) in defs.iter().enumerate() {
            let r = R::new(x0 + (k % 2) as f32 * (tw + 6.0), y + (k / 2) as f32 * 64.0, tw, 58.0);
            let busy = s.busy.contains(d.id);
            let a = |col| if busy { alpha(col, 0.6) } else { col };
            let over = c.hit(r, ID_TOGGLE + k as u32);
            let (bg, fg, sub, bub) = if d.on {
                (t.c("primary"), t.c("onPrimary"), alpha(t.c("onPrimary"), 0.75), alpha(t.c("onPrimary"), 0.16))
            } else {
                (if over { t.c("surfaceContainerHigh") } else { t.c("surfaceContainer") }, t.c("onSurface"), t.c("onSurfaceVariant"), t.c("surfaceContainerHighest"))
            };
            c.rect(r, if d.on { 23.0 } else { 29.0 }, a(bg));
            c.circle(r.x + 29.0, r.y + 29.0, 19.0, a(bub));
            c.icon(d.icon, R::new(r.x + 10.0, r.y + 9.0, 38.0, 40.0), ico(20.0, d.on), a(fg));
            let tx = r.x + 58.0;
            let menu = d.id == "record";   // the tile with a chevron that opens its sound menu
            let tw = r.w - 58.0 - if menu { 34.0 } else { 12.0 };
            c.text(d.label, R::new(tx, r.y + 14.0, tw, 16.2), txt(13.5, 500), a(fg), Align::Left);
            c.text(&d.sub, R::new(tx, r.y + 30.2, tw, 13.8), txt(11.5, 400), a(sub), Align::Left);
            if menu {
                let cr = R::new(r.x + r.w - 32.0, r.y + 13.0, 28.0, 32.0);
                if c.hit(cr, ID_REC_MENU) { c.rect(cr, 14.0, a(bub)); }
                c.icon("expand_more", cr, ico(20.0, false), a(fg));
            }
        }
    }

    /// Slider of `value` 0-100 at `r`; remembers the rect for Drag, shows the drag value while held.
    fn slider(&mut self, c: &mut Canvas, cx: &Ctx, r: R, id: u32, icon: Option<&str>, value: f32, name: Option<&str>) {
        let v = match self.dragv { Some((i, d)) if i == id => d, _ => value };
        self.rects.push((id, r));
        ui::slider(c, cx, r, id, None, (v / 100.0).max(44.0 / r.w), &format!("{}%", v.round()));
        let on = cx.theme.c("onPrimary");
        if let Some(i) = icon { c.icon(i, R::new(r.x + 12.0, r.y, 20.0, r.h), ico(20.0, true), on); }
        if let Some(n) = name { c.text(n, R::new(r.x + 16.0, r.y, r.w - 16.0 - 60.0, r.h), txt(13.0, 400), on, Align::Left); }
    }

    fn paint_sliders(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, x0: f32, y: f32, cw: f32) -> f32 {
        let list = [(ID_SLIDER, "volume_up", cx.audio.map(|a| a.volume)), (41, "mic", cx.audio.and_then(|a| a.mic_volume)), (42, "light_mode", s.brightness)];
        let mut h = 0.0;
        for (id, icon, v) in list {
            let Some(v) = v else { continue };
            if h > 0.0 { h += 6.0; }
            self.slider(c, cx, R::new(x0, y + h, cw, 44.0), id, Some(icon), v, None);
            h += 44.0;
        }
        h
    }

    fn paint_mixer(&mut self, c: &mut Canvas, cx: &Ctx, s: &Shared, x0: f32, y: f32, cw: f32) -> f32 {
        let n = s.mixer.len();
        let content = if n == 0 { 17.0 } else { n as f32 * 50.0 - 6.0 };
        let r = R::new(x0, y, cw, content.min(240.0));
        let mut sc = std::mem::take(&mut self.sc_mixer);
        let mut rects = vec![];
        sc.region(c, cx, r, content, |c, top| {
            if n == 0 { c.text("No app is playing audio", R::new(r.x, top, r.w, 17.0), txt(13.0, 400), cx.theme.c("onSurfaceVariant"), Align::Left); }
            for (i, m) in s.mixer.iter().enumerate() {
                let ry = top + i as f32 * 50.0;
                if ry + 44.0 < r.y || ry > r.y + r.h { continue; }
                let sr = R::new(r.x, ry, cw - 42.0, 44.0);
                rects.push((ID_MIX_SLIDER + i as u32, sr));
                let v = match self.dragv { Some((id, d)) if id == ID_MIX_SLIDER + i as u32 => d, _ => m.volume };
                ui::slider(c, cx, sr, ID_MIX_SLIDER + i as u32, None, (v / 100.0).max(44.0 / sr.w), &format!("{}%", v.round()));
                c.text(&crate::bar::pretty_app(&m.app), R::new(sr.x + 16.0, ry, sr.w - 76.0, 44.0), txt(13.0, 400), cx.theme.c("onPrimary"), Align::Left);
                if m.muted { c.rect(sr, 17.0, alpha(ui::layer0(cx), 0.45)); }
                ui::icon_btn(c, cx, R::new(r.x + cw - 36.0, ry + 4.0, 36.0, 36.0), ID_MIX_MUTE + i as u32, if m.muted { "volume_off" } else { "volume_up" }, ui::IconKind::Plain, 20.0, false);
            }
        });
        self.sc_mixer = sc;
        self.rects.extend(rects);
        r.h
    }
}

fn rec_started() -> f64 { hub::get("zor-ui.rec").and_then(|t| t.parse().ok()).unwrap_or(0.0) }

/// One menu row per output device, the default one ticked; a row sets that device as the default.
fn output_items(devs: &[media::OutDev], sh: &Sh) -> Vec<crate::menu::Item> {
    devs.iter().map(|d| {
        let (id, name, sh) = (d.id.clone(), d.name.clone(), sh.clone());
        crate::menu::Item::new(if d.default { "check" } else { "speaker" }, &d.name, move |_| {
            std::thread::spawn(move || if media::set_output(&id) { lock(&sh).out_name = name; repaint(); });
        })
    }).collect()
}

/// One menu row per `record.audio` choice, the current one ticked; a row saves that choice for the next recording.
fn record_items(cfg: &crate::conf::Config) -> Vec<crate::menu::Item> {
    let cur = cfg.str("record.audio").to_string();
    crate::bridge::rec_audio::MODES.iter().map(|&(key, label, icon)| {
        let cfg = cfg.clone();
        crate::menu::Item::new(if key == cur { "check" } else { icon }, label, move |_| {
            std::thread::spawn(move || { proc::bridge(&cfg, "config-set", &["record.audio", &serde_json::json!(key).to_string()]); });
        })
    }).collect()
}

/// What `--render` shows: a plausible session without running the helpers.
fn sample() -> Shared {
    let n = |id: &str, app: &str, title: &str, body: &str, ago: f64| Notif { id: id.into(), app: app.into(), title: title.into(), body: body.into(), time: hub::now_ms() / 1000.0 - ago };
    Shared {
        loaded: true, wifi: Some(true), bluetooth: Some(false), caffeine: false, brightness: Some(70.0), night: false, plan_loaded: true, plan: Some("Balanced".into()),
        notifs: vec![
            n("1", "Discord", "Alex", "are we still on for tonight? I booked the table for eight", 120.0),
            n("2", "Discord", "Sam", "sent a photo", 900.0),
            n("3", "Mail", "Build finished", "", 7200.0),
        ],
        mixer: vec![Mix { app: "Spotify.exe".into(), volume: 62.0, muted: false }, Mix { app: "chrome.exe".into(), volume: 30.0, muted: true }],
        os: "Windows 11 Pro 25H2".into(), out_name: "Speakers (Realtek Audio)".into(),
        ..Default::default()
    }
}

impl Drop for SidebarRight { fn drop(&mut self) { self.alive.store(false, Ordering::Relaxed); } }

impl View for SidebarRight {
    fn size(&self, _: &Gfx, _: &Ctx, _: f32) -> (f32, f32) { self.area }

    fn key(&self, _: &Ctx) -> String {
        let now = hub::now_ms();
        format!("{}|{}|{}", unsafe { GetTickCount64() } / 60_000, if rec_started() > 0.0 { (now / 1000.0) as u64 } else { 0 }, if self.timer_ticking() { (now / 100.0) as u64 } else { 0 })
    }

    fn frame_ms(&self) -> u32 {
        if self.timer_ticking() { 100 } else if rec_started() > 0.0 { 1000 } else if self.todo_focus { 500 } else { 0 }
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.start(cx);
        self.rects.clear();
        let s: Shared = lock(&self.sh).clone();
        let (w, h) = self.area;
        let p = R::new(6.0, 6.0, w - 12.0, h - 12.0);
        ui::panel(c, cx, p, 30.0);
        let (x0, cw) = (p.x + 13.0, p.w - 26.0);
        let mut y = p.y + 13.0;
        self.paint_header(c, cx, &s, x0, y, cw);
        y += 42.0 + 10.0;
        if self.power_open {
            self.paint_power(c, cx, x0, y, cw);
            y += 58.0 + 10.0;
        }
        self.paint_toggles(c, cx, &s, x0, y, cw);
        y += 8.0 * 58.0 + 7.0 * 6.0 + 10.0; // 16 toggles, two per row
        y += self.paint_sliders(c, cx, &s, x0, y, cw) + 10.0;
        ui::chip(c, cx, x0, y, ID_MIXER_BTN, Some("tune"), "App volume", self.mixer_open);
        y += 32.0 + 10.0;
        if self.mixer_open { y += self.paint_mixer(c, cx, &s, x0, y, cw) + 10.0; }
        let bottom = R::new(x0, y, cw, (p.y + p.h - 13.0 - y).max(0.0));
        self.paint_bottom(c, cx, &s, bottom);
    }

    fn context(&mut self, id: u32, _cx: &Ctx) -> Vec<crate::menu::Item> { self.notif_menu(id) }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match ev {
            Input::Click(id, btn) => self.click(id, btn, cx),
            Input::Drag(id, x, y, ph) => {
                if id == 230 && ph == Phase::Start { self.todo.press(x, y); }
                self.drag(id, x, ph, cx);
            }
            Input::Wheel(id, d) => {
                let mixer = (ID_MIX_SLIDER..ID_MIX_SLIDER + 100).contains(&id) || (ID_MIX_MUTE..ID_MIX_MUTE + 100).contains(&id);
                if mixer && self.mixer_open { self.sc_mixer.wheel(d); } else { self.wheel_tab(d); }
            }
            Input::Key { vk, ctrl, shift, alt } if self.todo_focus && self.tab == 2 => if self.todo.key(vk, ctrl, shift, alt) == ui::Edit::Submit { self.add_todo(); },
            Input::Char(ch) if self.todo_focus && self.tab == 2 => { self.todo.ch(ch); }
            Input::Focus(false) => self.todo_focus = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;

    #[test]
    fn mixer_ids_leave_the_page_ids_alone() {
        // Clear (211), the calendar (220), add (231) and the timer (240..245) must not hit the mixer's arms
        for id in [210u32, 211, 220, 221, 231, 240, 242, 244] {
            assert!(!(ID_MIX_SLIDER..ID_MIX_SLIDER + 100).contains(&id) && !(ID_MIX_MUTE..ID_MIX_MUTE + 100).contains(&id), "{id}");
        }
    }

    #[test]
    fn output_menu_ticks_the_default() {
        let d = |n: &str, default| media::OutDev { id: n.into(), name: n.into(), default };
        let items = output_items(&[d("Speakers", false), d("Headset", true)], &Sh::default());
        assert_eq!(items.iter().map(|i| (i.icon, i.label.as_str())).collect::<Vec<_>>(), [("speaker", "Speakers"), ("check", "Headset")]);
        assert!(output_items(&[], &Sh::default()).is_empty());
    }
}
