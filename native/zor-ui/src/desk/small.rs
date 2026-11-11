//! The small desk cards that edit shared state: "todo", "notes" and "timer" (desktop.js renderTodo / renderNotes / renderTimer).
//! They read and write the hub keys the right sidebar shares.

use super::{h3, LH, W};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    hub,
    menu::Item,
    ui::{self, Edit, TextBox},
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

const EMPTY: &str = "Nothing to do yet. Type a task below.";
const ADD_GAP: f32 = 12.0; // space between the list (or empty note) and the add field
const ADD_H: f32 = 40.0; // the add field, same height as the sidebar's
const DEL: u32 = 1000; // delete button ids: DEL + index (toggle ids are ITEM0 + index)
const DEL_W: f32 = 28.0; // delete button width, reserved at the row's right edge
const TEXT_W: f32 = INNER - 26.0 - DEL_W; // a task's text width (after the tick and the delete button)

fn todo_list() -> Vec<(String, bool)> {
    hub::get_json(TODO).as_array().map(|a| a.iter().map(|t| (t["text"].as_str().unwrap_or("").to_string(), t["done"].as_bool().unwrap_or(false))).collect()).unwrap_or_default()
}

/// The list with `text` appended as an open task; None when `text` is blank.
pub fn appended(list: Value, text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() { return None; }
    // a missing key reads as Null (get_json): start a new list
    let mut list = if list.is_array() { list } else { json!([]) };
    list.as_array_mut()?.push(json!({ "text": text, "done": false }));
    Some(list)
}

/// The list without item `i` (out of range: untouched).
pub fn remove_at(mut list: Value, i: usize) -> Value {
    if let Some(a) = list.as_array_mut() { if i < a.len() { a.remove(i); } }
    list
}

pub fn todo_key() -> String { hub::get(TODO).unwrap_or_default() }

/// The to-do card: tick or delete a task, or type in the add field at its foot (focused like Notes).
pub struct Todo { pub tb: TextBox, pub focused: bool }

impl Todo {
    pub fn new() -> Todo { Todo { tb: TextBox::new(false), focused: false } }

    pub fn size(&self, g: &crate::gfx::Gfx) -> f32 {
        let list = todo_list();
        let body = if list.is_empty() { g.measure_wrapped(EMPTY, txt(13.0, 400), INNER) }
            else { list.iter().map(|(t, _)| g.measure_wrapped(t, txt(13.0, 400), TEXT_W).max(18.0) + ROW_GAP).sum() };
        head() + body + ADD_GAP + ADD_H + 12.0 + 8.0
    }

    pub fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let list = todo_list();
        let mut y = h3(c, cx, "done_outline", "To do");
        if list.is_empty() {
            let h = c.g.measure_wrapped(EMPTY, txt(13.0, 400), INNER);
            c.para(EMPTY, R::new(18.0, y, INNER, h), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left);
            y += h;
        }
        for (i, (text, done)) in list.iter().enumerate() {
            let h = c.g.measure_wrapped(text, txt(13.0, 400), TEXT_W).max(18.0);
            let row = R::new(18.0, y, INNER, h + ROW_GAP);
            if c.hit(row, ITEM0 + i as u32) { c.rect(row, 6.0, alpha(t.c("onSurface"), 0.05)); }
            c.icon(if *done { "check_circle" } else { "radio_button_unchecked" }, R::new(18.0, y + 4.0, 18.0, 18.0), ico(18.0, *done), t.c("primary"));
            let col = if *done { t.c("onSurfaceVariant") } else { t.c("onSurface") };
            c.para(text, R::new(44.0, y + 4.0, TEXT_W, h), txt(13.0, 400), col, Align::Left);
            if *done { // text-decoration: line-through (first line)
                let w = c.g.measure(text, txt(13.0, 400)).0.min(TEXT_W);
                c.line((44.0, y + 4.0 + 13.0 * LH / 2.0), (44.0 + w, y + 4.0 + 13.0 * LH / 2.0), 1.0, col);
            }
            // registered after the row, so it wins where the two overlap (hits: last match wins)
            let del = R::new(18.0 + INNER - DEL_W, y + (h + ROW_GAP - DEL_W) / 2.0, DEL_W, DEL_W);
            ui::icon_btn(c, cx, del, DEL + i as u32, "close", ui::IconKind::Plain, 16.0, false);
            y += h + ROW_GAP;
        }
        let field = R::new(18.0, y + ADD_GAP, INNER, ADD_H);
        c.hit(field, AREA);
        c.rect(field, 20.0, if self.focused { t.c("primary") } else { t.c("outlineVariant") });
        c.rect(R::new(field.x + 1.0, field.y + 1.0, field.w - 2.0, field.h - 2.0), 19.0, t.c("surfaceContainer"));
        self.tb.draw(c, cx, R::new(field.x + 12.0, field.y, field.w - 24.0, field.h), txt(13.0, 400), "Add a task", self.focused);
    }

    /// `hwnd`: the card's window, which becomes focusable while the add field is typed in.
    pub fn input(&mut self, ev: Input, hwnd: &Arc<AtomicIsize>) {
        match ev {
            Input::Click(id, _) if id >= DEL =>hub::set(TODO, Some(remove_at(hub::get_json(TODO), (id - DEL) as usize).to_string())),
            Input::Click(id, _) if (ITEM0..DEL).contains(&id) => hub::set(TODO, Some(toggled(hub::get_json(TODO), (id - ITEM0) as usize).to_string())),
            Input::Drag(AREA, x, y, Phase::Start) => {
                self.tb.press(x, y);
                if !self.focused { self.focused = true; set_focus(hwnd.load(Ordering::Relaxed), true); }
            }
            Input::Focus(f) => {
                self.focused = f;
                if !f { set_focus(hwnd.load(Ordering::Relaxed), false); }
            }
            Input::Key { vk, ctrl, shift, alt } if self.focused => if self.tb.key(vk, ctrl, shift, alt) == Edit::Submit {
                if let Some(list) = appended(hub::get_json(TODO), &self.tb.text) { hub::set(TODO, Some(list.to_string())); self.tb.set(""); }
            },
            Input::Char(ch) if self.focused => { self.tb.ch(ch); }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ notes

/// Notes card: shown height of the text box, px. Taller text scrolls inside it.
const NOTE_MIN: f32 = 110.0;
const NOTE_MAX: f32 = 220.0;

/// Height of the notes text box for `content` px of wrapped text.
pub fn note_view_h(content: f32) -> f32 { content.clamp(NOTE_MIN, NOTE_MAX) }

/// Scroll target that brings a caret (top `top`, height `h`, content px) into a `view`-px window at scroll `y`.
/// None when it is already visible.
pub fn caret_follow(y: f32, top: f32, h: f32, view: f32) -> Option<f32> {
    if top < y { Some(top) } else if top + h > y + view { Some(top + h - view) } else { None }
}

/// A right-click menu choice for the notes card. The menu runs outside the card, so it leaves the command here
/// and the card applies it at its next paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteCmd { Copy, Cut, Paste, All }

static NOTE_CMD: std::sync::Mutex<Option<NoteCmd>> = std::sync::Mutex::new(None);

fn take_cmd() -> Option<NoteCmd> { NOTE_CMD.lock().unwrap_or_else(|e| e.into_inner()).take() }

/// Items for the notes card's right-click menu.
pub fn notes_menu() -> Vec<Item> {
    fn pick(c: NoteCmd) -> impl FnOnce(&mut Ctx) + 'static {
        move |cx: &mut Ctx| {
            *NOTE_CMD.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
            cx.ver += 1; // repaint so the card applies it
        }
    }
    vec![
        Item::new("content_copy", "Copy", pick(NoteCmd::Copy)),
        Item::new("content_cut", "Cut", pick(NoteCmd::Cut)),
        Item::new("content_paste", "Paste", pick(NoteCmd::Paste)),
        Item::new("select_all", "Select all", pick(NoteCmd::All)),
    ]
}

pub struct Notes {
    pub tb: TextBox,
    pub focused: bool,
    scroll: ui::Scroll,
    /// Last paint's wrapped text height and shown height, for wheel steps.
    content: f32,
    view: f32,
    /// Set by typing or moving the pointer: the next paint scrolls the caret into view.
    follow: bool,
}

impl Notes {
    pub fn new() -> Notes {
        let mut tb = TextBox::new(true);
        tb.set(&hub::get(NOTES).unwrap_or_default());
        Notes { tb, focused: false, scroll: Default::default(), content: 0.0, view: NOTE_MIN, follow: false }
    }

    fn text(&self) -> String { if self.focused { self.tb.text.clone() } else { hub::get(NOTES).unwrap_or_default() } }

    pub fn key(&self) -> String { hub::get(NOTES).map_or(0, |s| s.len()).to_string() }

    pub fn size(&self, g: &crate::gfx::Gfx) -> f32 {
        head() + note_view_h(g.measure_wrapped(&self.text(), txt(13.0, 400), INNER)) + 12.0 + 8.0
    }

    pub fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        if !self.focused { let s = hub::get(NOTES).unwrap_or_default(); if s != self.tb.text { self.tb.set(&s); } }
        if let Some(cmd) = take_cmd() { self.apply(cmd); }
        let y = h3(c, cx, "sticky_note_2", "Notes");
        let h = c.h - 8.0 - 12.0 - y;
        let r = R::new(18.0, y, INNER, h);
        c.hit(r, AREA);
        let f = txt(13.0, 400);
        let content = c.g.measure_wrapped(&self.tb.text, f, INNER);
        self.content = content.max(h);
        self.view = h;
        if std::mem::take(&mut self.follow) {
            let (_, top, ch) = c.g.caret(&self.tb.text, f, INNER, true, self.tb.caret);
            if let Some(t) = caret_follow(self.scroll.y, top, ch, h) { self.scroll.to(t, self.content, h); }
        }
        let (tb, focus) = (&mut self.tb, self.focused);
        self.scroll.region(c, cx, r, self.content, |c, top| {
            tb.draw(c, cx, R::new(r.x, top, INNER, content.max(h)), f, "Write something", focus);
        });
    }

    fn apply(&mut self, cmd: NoteCmd) {
        let edit = match cmd {
            NoteCmd::Copy => { self.tb.copy(); Edit::None }
            NoteCmd::Cut => self.tb.cut(),
            NoteCmd::Paste => self.tb.paste(),
            NoteCmd::All => { self.tb.select_all(); Edit::None }
        };
        if edit == Edit::Changed { self.save(); }
        self.follow = true;
    }

    /// `hwnd`: the card's window, which becomes focusable while it is typed in.
    pub fn input(&mut self, ev: Input, hwnd: &Arc<AtomicIsize>) {
        match ev {
            Input::Drag(AREA, x, y, Phase::Start) => {
                self.tb.press(x, y);
                self.follow = true;
                if !self.focused { self.focused = true; set_focus(hwnd.load(Ordering::Relaxed), true); }
            }
            Input::Drag(AREA, x, y, Phase::Move) => { self.tb.drag(x, y); self.follow = true; }
            Input::Wheel(AREA, d) => self.scroll.wheel(d, self.content, self.view),
            Input::Focus(f) => {
                self.focused = f;
                if !f { set_focus(hwnd.load(Ordering::Relaxed), false); }
            }
            Input::Key { vk, ctrl, shift, alt } if self.focused => {
                self.follow = true;
                if self.tb.key(vk, ctrl, shift, alt) == Edit::Changed { self.save(); }
            }
            Input::Char(ch) if self.focused => {
                self.follow = true;
                if self.tb.ch(ch) == Edit::Changed { self.save(); }
            }
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
    fn notes_box_grows_then_scrolls() {
        assert_eq!(note_view_h(20.0), NOTE_MIN); // short text keeps the 110 px box
        assert_eq!(note_view_h(150.0), 150.0);
        assert_eq!(note_view_h(900.0), NOTE_MAX); // past the cap the text scrolls inside
    }

    #[test]
    fn caret_scrolls_only_when_hidden() {
        assert_eq!(caret_follow(0.0, 50.0, 20.0, 100.0), None); // visible
        assert_eq!(caret_follow(0.0, 120.0, 20.0, 100.0), Some(40.0)); // below: bottom at the view's bottom
        assert_eq!(caret_follow(80.0, 10.0, 20.0, 100.0), Some(10.0)); // above: top at the view's top
    }

    #[test]
    fn notes_menu_command_is_taken_once() {
        *NOTE_CMD.lock().unwrap() = Some(NoteCmd::Paste);
        assert_eq!(take_cmd(), Some(NoteCmd::Paste));
        assert_eq!(take_cmd(), None);
    }

    #[test]
    fn todo_flip_keeps_other_fields() {
        let l = json!([{ "text": "a", "done": false, "id": 7 }, { "text": "b" }]);
        let l = toggled(l, 0);
        assert_eq!(l[0], json!({ "text": "a", "done": true, "id": 7 }));
        assert_eq!(toggled(toggled(l, 1), 1)[1]["done"], json!(false));
        assert_eq!(toggled(json!([]), 3), json!([])); // out of range: untouched
    }
    #[test]
    fn todo_add_trims_and_skips_blank() {
        assert_eq!(appended(json!([]), "  buy milk ").unwrap(), json!([{ "text": "buy milk", "done": false }]));
        assert_eq!(appended(json!([{ "text": "a", "done": true }]), "b").unwrap()[1], json!({ "text": "b", "done": false }));
        assert!(appended(json!([]), "   ").is_none());
        assert_eq!(appended(Value::Null, "first").unwrap(), json!([{ "text": "first", "done": false }])); // no key yet
    }
    #[test]
    fn todo_delete_by_index() {
        assert_eq!(remove_at(json!([{ "text": "a" }, { "text": "b" }]), 0), json!([{ "text": "b" }]));
        assert_eq!(remove_at(json!([{ "text": "a" }]), 5), json!([{ "text": "a" }])); // out of range: untouched
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
