//! The small desk cards that edit shared state: "todo", "notes" and "timer" (desktop.js renderTodo / renderNotes / renderTimer).
//! They read and write the hub keys the Zebar copies and the right sidebar share.

use super::{h3, LH, W};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    hub,
    ui::{Edit, TextBox},
    win::{Input, Phase},
};
use serde_json::{json, Value};
use std::sync::{atomic::{AtomicIsize, Ordering}, Arc};
use windows::Win32::{
    Foundation::HWND,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

const TODO: &str = "zor-ui.todo";
const NOTES: &str = "zor-ui.notes";
const TIMER: &str = "zor-ui.timer";
const ITEM0: u32 = 100;
const AREA: u32 = 1;
const GO: u32 = 2;
const INNER: f32 = W - 8.0 - 28.0; // .pad content width
const ROW_GAP: f32 = 8.0; // .item padding 4px 0

fn head() -> f32 { 12.0 + 13.0 * LH + 8.0 }

// ------------------------------------------------------------------ to-do

/// The list with item `i` flipped between done and not done.
pub fn toggled(mut list: Value, i: usize) -> Value {
    if let Some(t) = list.get_mut(i) { t["done"] = json!(!t["done"].as_bool().unwrap_or(false)); }
    list
}

fn todo_list() -> Vec<(String, bool)> {
    hub::get_json(TODO).as_array().map(|a| a.iter().take(8).map(|t| (t["text"].as_str().unwrap_or("").to_string(), t["done"].as_bool().unwrap_or(false))).collect()).unwrap_or_default()
}

pub fn todo_key() -> String { hub::get(TODO).unwrap_or_default() }

pub fn todo_size(g: &crate::gfx::Gfx) -> f32 {
    let list = todo_list();
    let body = if list.is_empty() { g.measure_wrapped("Nothing to do. Add tasks in the right sidebar.", txt(13.0, 400), INNER) }
        else { list.iter().map(|(t, _)| g.measure_wrapped(t, txt(13.0, 400), INNER - 26.0).max(18.0) + ROW_GAP).sum() };
    head() + body + 12.0 + 8.0
}

pub fn todo_paint(c: &mut Canvas, cx: &Ctx) {
    let t = &cx.theme;
    let list = todo_list();
    let mut y = h3(c, cx, "done_outline", "To do");
    if list.is_empty() {
        let h = c.g.measure_wrapped("Nothing to do. Add tasks in the right sidebar.", txt(13.0, 400), INNER);
        c.para("Nothing to do. Add tasks in the right sidebar.", R::new(18.0, y, INNER, h), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left);
        return;
    }
    for (i, (text, done)) in list.iter().enumerate() {
        let h = c.g.measure_wrapped(text, txt(13.0, 400), INNER - 26.0).max(18.0);
        let row = R::new(18.0, y, INNER, h + ROW_GAP);
        if c.hit(row, ITEM0 + i as u32) { c.rect(row, 6.0, alpha(t.c("onSurface"), 0.05)); }
        c.icon(if *done { "check_circle" } else { "radio_button_unchecked" }, R::new(18.0, y + 4.0, 18.0, 18.0), ico(18.0, *done), t.c("primary"));
        let col = if *done { t.c("onSurfaceVariant") } else { t.c("onSurface") };
        c.para(text, R::new(44.0, y + 4.0, INNER - 26.0, h), txt(13.0, 400), col, Align::Left);
        if *done { // text-decoration: line-through (first line)
            let w = c.g.measure(text, txt(13.0, 400)).0.min(INNER - 26.0);
            c.line((44.0, y + 4.0 + 13.0 * LH / 2.0), (44.0 + w, y + 4.0 + 13.0 * LH / 2.0), 1.0, col);
        }
        y += h + ROW_GAP;
    }
}

pub fn todo_input(ev: Input) {
    if let Input::Click(id @ ITEM0..=107, _) = ev {
        hub::set(TODO, Some(toggled(hub::get_json(TODO), (id - ITEM0) as usize).to_string()));
    }
}

// ------------------------------------------------------------------ notes

pub struct Notes { pub tb: TextBox, pub focused: bool }

impl Notes {
    pub fn new() -> Notes { let mut tb = TextBox::new(true); tb.set(&hub::get(NOTES).unwrap_or_default()); Notes { tb, focused: false } }

    fn text(&self) -> String { if self.focused { self.tb.text.clone() } else { hub::get(NOTES).unwrap_or_default() } }

    pub fn key(&self) -> String { hub::get(NOTES).map_or(0, |s| s.len()).to_string() }

    pub fn size(&self, g: &crate::gfx::Gfx) -> f32 {
        // the textarea is 110 px and scrolls in the browser; here it grows with the text instead
        head() + g.measure_wrapped(&self.text(), txt(13.0, 400), INNER).max(110.0) + 12.0 + 8.0
    }

    pub fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        if !self.focused { let s = hub::get(NOTES).unwrap_or_default(); if s != self.tb.text { self.tb.set(&s); } }
        let y = h3(c, cx, "sticky_note_2", "Notes");
        let h = c.h - 8.0 - 12.0 - y;
        let r = R::new(18.0, y, INNER, h);
        c.hit(r, AREA);
        self.tb.draw(c, cx, r, txt(13.0, 400), "Write something", self.focused);
    }

    /// `hwnd`: the card's window, which becomes focusable while it is typed in.
    pub fn input(&mut self, ev: Input, hwnd: &Arc<AtomicIsize>) {
        match ev {
            Input::Drag(AREA, x, y, Phase::Start) => {
                self.tb.press(x, y);
                if !self.focused { self.focused = true; set_focus(hwnd.load(Ordering::Relaxed), true); }
            }
            Input::Focus(f) => {
                self.focused = f;
                if !f { set_focus(hwnd.load(Ordering::Relaxed), false); }
            }
            Input::Key { vk, ctrl, shift, alt } if self.focused => if self.tb.key(vk, ctrl, shift, alt) == Edit::Changed { self.save(); },
            Input::Char(ch) if self.focused => if self.tb.ch(ch) == Edit::Changed { self.save(); },
            _ => {}
        }
    }

    fn save(&self) { hub::set(NOTES, Some(self.tb.text.clone())); }
}

/// Win::set_focusable + Win::focus on a raw handle (the card only knows its own HWND).
fn set_focus(h: isize, on: bool) {
    if h == 0 { return; }
    let hwnd = HWND(h as *mut _);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let flag = WS_EX_NOACTIVATE.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, if on { ex & !flag } else { ex | flag });
        if on {
            keybd_event(0x87, 0, KEYBD_EVENT_FLAGS(0), 0); // F24: lets this process take the foreground
            keybd_event(0x87, 0, KEYEVENTF_KEYUP, 0);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

// ------------------------------------------------------------------ timer

/// pomo.toggle() / sw.toggle() of lib/timer.js on the stored state. `now`: unix ms.
pub fn toggle_pomo(t: &mut Value, left: f64, now: f64) {
    if t["running"].as_bool() == Some(true) { t["left"] = json!(left); t["running"] = json!(false); }
    else { t["endsAt"] = json!(now + left * 1000.0); t["left"] = Value::Null; t["running"] = json!(true); }
}

pub fn toggle_sw(t: &mut Value, now: f64) {
    let sw = &mut t["sw"];
    if sw["running"].as_bool() == Some(true) {
        sw["acc"] = json!(sw["acc"].as_f64().unwrap_or(0.0) + now - sw["start"].as_f64().unwrap_or(0.0));
        sw["running"] = json!(false);
    } else { sw["start"] = json!(now); sw["running"] = json!(true); }
}

fn phase_name(p: &str) -> &'static str { match p { "short" => "Short break", "long" => "Long break", _ => "Focus" } }

pub fn timer_key() -> String { util_secs() }
fn util_secs() -> String { (hub::now_ms() / 1000.0).floor().to_string() }

pub fn timer_size() -> f32 { head() + 44.0 + 12.0 + 8.0 }

pub fn timer_paint(c: &mut Canvas, cx: &Ctx) {
    let t = &cx.theme;
    let s = hub::timer();
    let running = s["running"].as_bool() == Some(true);
    let sw = !running && s["sw"]["running"].as_bool() == Some(true);
    let going = sw || running;
    let y = h3(c, cx, "timer", if sw { "Stopwatch" } else { phase_name(s["phase"].as_str().unwrap_or("focus")) });
    let time = hub::clock(if sw { hub::sw_elapsed(&s) } else { hub::left(&s, &cx.cfg) });
    c.text(&time, R::new(18.0, y, INNER - 56.0, 44.0), txt(34.0, 500), t.c("onSurface"), Align::Left);
    let b = R::new(18.0 + INNER - 44.0, y, 44.0, 44.0);
    c.rect(b, 17.0, t.c("primary"));
    if c.hit(b, GO) { c.rect(b, 17.0, alpha(t.c("onPrimary"), 0.08)); }
    c.icon(if going { "pause" } else { "play_arrow" }, b, ico(20.0, true), t.c("onPrimary"));
}

pub fn timer_input(ev: Input, cx: &Ctx) {
    if let Input::Click(GO, _) = ev {
        let mut s = hub::timer();
        let sw = s["running"].as_bool() != Some(true) && s["sw"]["running"].as_bool() == Some(true);
        let now = hub::now_ms();
        if sw { toggle_sw(&mut s, now); } else { let left = hub::left(&s, &cx.cfg); toggle_pomo(&mut s, left, now); }
        hub::set(TIMER, Some(s.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn todo_flip_keeps_other_fields() {
        let l = json!([{ "text": "a", "done": false, "id": 7 }, { "text": "b" }]);
        let l = toggled(l, 0);
        assert_eq!(l[0], json!({ "text": "a", "done": true, "id": 7 }));
        assert_eq!(toggled(toggled(l, 1), 1)[1]["done"], json!(false));
        assert_eq!(toggled(json!([]), 3), json!([])); // out of range: untouched
    }
    #[test]
    fn pomo_start_pause() {
        let mut t = json!({ "running": false, "left": 60.0, "endsAt": 0 });
        toggle_pomo(&mut t, 60.0, 1000.0);
        assert_eq!((t["running"].clone(), t["endsAt"].clone(), t["left"].clone()), (json!(true), json!(61000.0), Value::Null));
        toggle_pomo(&mut t, 42.0, 5000.0);
        assert_eq!((t["running"].clone(), t["left"].clone()), (json!(false), json!(42.0)));
    }
    #[test]
    fn stopwatch_accumulates() {
        let mut t = json!({ "sw": { "running": false, "start": 0, "acc": 500.0, "laps": [] } });
        toggle_sw(&mut t, 1000.0);
        assert_eq!(t["sw"]["running"], json!(true));
        toggle_sw(&mut t, 4000.0);
        assert_eq!((t["sw"]["running"].clone(), t["sw"]["acc"].clone()), (json!(false), json!(3500.0)));
    }
}
