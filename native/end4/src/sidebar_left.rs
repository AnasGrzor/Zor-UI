//! The left sidebar (sidebar-left.js): a tab row (home, AI, translate, anime, football) over the open tab.
//! Each tab is a submodule in sidebar_left/. Slow work (bridge commands, downloads) runs on threads that
//! drop their answer in a tab's `Inbox` and call `wake()`; the tab drains it on its next paint.
//!
//! Testing aid: END4_RENDER_TAB=<home|ai|translate|anime|football> makes `end4.exe --render sidebar-left` open
//! that tab and do its network loads inline (so the PNG shows data); END4_RENDER_DEMO=1 adds sample chat /
//! translator text. Neither is read by the running app's popup (they only exist in a shell you set them in).

mod ai;
mod anime;
mod football;
mod home;
mod img;
mod md;
mod translate;

use crate::{
    app::{self, Action, Ctx},
    conf::Config,
    gfx::{alpha, Canvas, Gfx, R},
    proc, ui,
    win::{Input, View},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const TABS: [(&str, &str, &str); 5] = [("home", "home", "Home"), ("ai", "neurology", "AI"), ("translate", "translate", "Translate"), ("anime", "photo_library", "Anime"), ("football", "sports_soccer", "Football")];
const TAB0: u32 = 1; // hit ids 1..=5
const PIN: u32 = 10;
const PAGE_UP: u32 = 0x21;
const PAGE_DOWN: u32 = 0x22;
const TAB_KEY: u32 = 0x09;

/// What a tab does for the shell. `r` is the body below the tab row; hit ids are in the tab's own range.
trait Tab {
    /// The tab became visible (first time: start its loads).
    fn show(&mut self, _cx: &Ctx) {}
    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R);
    fn input(&mut self, ev: &Input, cx: &mut Ctx, hover: u32);
    fn frame_ms(&self) -> u32 { 0 }
    fn captures_esc(&self) -> bool { false }
}

pub struct SidebarLeft {
    area: (f32, f32),
    tab: usize,
    started: bool,
    pinned: bool,
    hover: u32,
    home: home::Home,
    ai: ai::Ai,
    tr: translate::Translate,
    an: anime::Anime,
    fb: football::Football,
}

/// Ctrl+PageDown / Ctrl+Tab -> 1, Ctrl+PageUp / Ctrl+Shift+Tab -> -1.
fn tab_step(vk: u32, shift: bool) -> Option<i32> {
    match vk { PAGE_DOWN => Some(1), PAGE_UP => Some(-1), TAB_KEY => Some(if shift { -1 } else { 1 }), _ => None }
}

fn wrap_tab(cur: usize, dir: i32, n: usize) -> usize { (cur as i32 + dir).rem_euclid(n as i32) as usize }

impl SidebarLeft {
    /// `area`: the CSS-px size the popup placement gives the view (popup::area).
    pub fn new(area: (f32, f32)) -> SidebarLeft {
        SidebarLeft {
            area, tab: 0, started: false, pinned: false, hover: 0,
            home: Default::default(), ai: ai::Ai::new(), tr: translate::Translate::new(), an: anime::Anime::new(), fb: football::Football::new(),
        }
    }

    fn cur(&mut self) -> &mut dyn Tab {
        match self.tab { 0 => &mut self.home, 1 => &mut self.ai, 2 => &mut self.tr, 3 => &mut self.an, _ => &mut self.fb }
    }
    fn cur_ref(&self) -> &dyn Tab {
        match self.tab { 0 => &self.home, 1 => &self.ai, 2 => &self.tr, 3 => &self.an, _ => &self.fb }
    }

    fn go(&mut self, tab: usize, cx: &mut Ctx) {
        self.tab = tab.min(TABS.len() - 1);
        if !sync() {
            cx.state.v["leftTab"] = json!(TABS[self.tab].0);
            cx.state.save();
        }
        self.cur().show(cx);
    }
}

impl View for SidebarLeft {
    fn size(&self, _: &Gfx, _: &Ctx, _: f32) -> (f32, f32) { self.area }

    fn key(&self, _: &Ctx) -> String {
        match self.cur_ref().frame_ms() { 0 => String::new(), ms => ((crate::hub::now_ms() / ms as f64) as u64).to_string() }
    }

    fn frame_ms(&self) -> u32 { self.cur_ref().frame_ms() }
    fn captures_esc(&self) -> bool { self.cur_ref().captures_esc() }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        if !self.started {
            self.started = true;
            let name = std::env::var("END4_RENDER_TAB").ok().or_else(|| cx.state.v["leftTab"].as_str().map(String::from)).unwrap_or_default();
            self.tab = TABS.iter().position(|t| t.0 == name).unwrap_or(0);
            self.cur().show(cx);
        }
        let panel = R::new(6.0, 6.0, c.w - 12.0, c.h - 12.0);
        ui::panel(c, cx, panel, 30.0);
        let (x, y, w) = (panel.x + 12.0, panel.y + 12.0, panel.w - 24.0);
        // the tab row: only the open tab shows its label
        let items: Vec<(&str, &str)> = TABS.iter().enumerate().map(|(i, t)| (t.1, if i == self.tab { t.2 } else { "" })).collect();
        ui::tabs(c, cx, R::new(x, y, w - 42.0, 46.0), TAB0, &items, self.tab);
        let pin = R::new(x + w - 36.0, y + 5.0, 36.0, 36.0);
        let kind = if self.pinned { ui::IconKind::Tonal } else { ui::IconKind::Plain };
        ui::icon_btn(c, cx, pin, PIN, "keep", kind, 24.0, self.pinned);
        let body = R::new(x, y + 56.0, w, panel.y + panel.h - 12.0 - (y + 56.0));
        self.cur().paint(c, cx, body);
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match &ev {
            Input::Key { vk, ctrl: true, shift, .. } if tab_step(*vk, *shift).is_some() => {
                let next = wrap_tab(self.tab, tab_step(*vk, *shift).unwrap_or(1), TABS.len());
                return self.go(next, cx);
            }
            Input::Click(id, _) if (TAB0..TAB0 + TABS.len() as u32).contains(id) => return self.go((*id - TAB0) as usize, cx),
            Input::Click(PIN, _) => {
                self.pinned = !self.pinned;
                cx.actions.push(Action::TogglePin("sidebar-left".into()));
                return;
            }
            Input::Hover(id) => self.hover = *id,
            _ => {}
        }
        let hover = self.hover;
        self.cur().input(&ev, cx, hover);
    }
}

// ------------------------------------------------------------------ helpers the tabs share

/// `--render` mode: loads run inline so one paint shows their result.
fn sync() -> bool { std::env::var_os("END4_RENDER_TAB").is_some() }

/// Repaint everything (a thread delivered something).
fn wake() { app::post(Box::new(|_| {})); }

fn bg(f: impl FnOnce() + Send + 'static) { if sync() { f() } else { std::thread::spawn(f); } }

/// Where a thread leaves its answers for the tab to pick up at the next paint.
pub struct Inbox<T>(Arc<Mutex<Vec<T>>>);
impl<T> Clone for Inbox<T> { fn clone(&self) -> Self { Inbox(self.0.clone()) } }
impl<T> Inbox<T> {
    pub fn new() -> Inbox<T> { Inbox(Arc::new(Mutex::new(vec![]))) }
    pub fn push(&self, v: T) { if let Ok(mut q) = self.0.lock() { q.push(v); } }
    pub fn take(&self) -> Vec<T> { self.0.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default() }
}

/// One bridge command on a thread; `done` gets its JSON line (Null when it failed) and then the UI wakes.
fn call(cx: &Ctx, name: &str, args: Vec<String>, done: impl FnOnce(Value) + Send + 'static) {
    let (cfg, name) = (Config(cx.cfg.0.clone()), name.to_string());
    bg(move || {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        done(proc::bridge(&cfg, &name, &a));
        wake();
    });
}

/// `bridge('config-set', key, JSON.stringify(v))` in the background.
fn config_set(cx: &Ctx, key: &str, v: Value) { if !sync() { call(cx, "config-set", vec![key.into(), v.to_string()], |_| {}); } }

/// Three pulsing dots centred on (x, y): the "busy" mark (`.dots` / `.spinner`).
fn dots(c: &mut Canvas, cx: &Ctx, x: f32, y: f32) {
    let phase = crate::hub::now_ms() / 1000.0;
    for i in 0..3 {
        let a = 0.2 + 0.8 * (((phase - i as f64 * 0.15) * std::f64::consts::TAU).sin() * 0.5 + 0.5) as f32;
        c.circle(x + (i as f32 - 1.0) * 10.0, y, 3.0, alpha(cx.theme.c("primary"), a));
    }
}

/// `.field`: bordered input surface.
fn field(c: &mut Canvas, cx: &Ctx, r: R, focus: bool) {
    let edge = if focus { cx.theme.c("primary") } else { cx.theme.c("outlineVariant") };
    c.rect(r, 17.0, edge);
    c.rect(R::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0), 16.0, cx.theme.c("surfaceContainer"));
}

/// `.err`: a red-tinted message box. Returns its height.
fn err_box(c: &mut Canvas, cx: &Ctx, r: R, msg: &str) -> f32 {
    use crate::gfx::{txt, Align};
    let h = c.g.measure_wrapped(msg, txt(13.0, 400), r.w - 24.0) + 20.0;
    c.rect(R::new(r.x, r.y, r.w, h), 17.0, alpha(cx.theme.c("error"), 0.18));
    c.para(msg, R::new(r.x + 12.0, r.y + 10.0, r.w - 24.0, h - 20.0), txt(13.0, 400), cx.theme.c("onSurface"), Align::Left);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycling() {
        assert_eq!(tab_step(PAGE_DOWN, false), Some(1));
        assert_eq!(tab_step(PAGE_UP, false), Some(-1));
        assert_eq!(tab_step(TAB_KEY, true), Some(-1));
        assert_eq!(tab_step(0x41, false), None);
        assert_eq!(wrap_tab(0, -1, 5), 4);
        assert_eq!(wrap_tab(4, 1, 5), 0);
        assert_eq!(wrap_tab(2, 1, 5), 3);
    }
}
