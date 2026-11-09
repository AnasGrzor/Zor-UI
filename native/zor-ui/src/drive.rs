//! Take-over: after one yes from you, the AI moves the mouse and types to finish a task while you watch. Each round it
//! gets a picture of every monitor, answers with a short line and a few actions as plain JSON (any provider can do that),
//! and this module runs them with `SendInput`. It stops, and hands the mouse back, on Esc, when you move the mouse, when
//! you speak to the buddy, at a password field, in front of an app running as administrator, or at a round or time limit.
//!
//! Everything that decides something is a plain function or the loop `run`, which talks to the world through `Host`, so
//! the tests drive it with a fake model and fake hands. `Live` is the real `Host`.

use crate::{assist, conf::Config, screen};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::{
    core::w,
    Win32::{
        Foundation::{CloseHandle, BOOL, HMODULE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            LibraryLoader::GetModuleHandleW,
            Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{
            Accessibility::{CUIAutomation, IUIAutomation},
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::*,
        },
    },
};

/// Actions the model may send in one round; more are dropped.
pub const MAX_ACTIONS: usize = 5;
/// Longest text one `type` action may enter.
pub const MAX_TEXT: usize = 500;
/// How far (screen px) the pointer may be from where the drive left it before it counts as you moving it, and how long it must rest.
pub const MOVE_PX: i32 = 24;
pub const REST_MS: u64 = 150;
/// Pause after every action: the app moves focus when it handles the click, and keys sent before that go to the old control.
pub const SETTLE_MS: u64 = 150;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Btn { Left, Right }

#[derive(Debug, PartialEq, Clone)]
pub enum Act {
    Move,
    Click(Btn, u8),
    Type(String),
    /// A key combo as virtual-key codes, pressed in order and released in reverse.
    Keys(Vec<u16>),
    /// Notches right and down (negative: left and up).
    Scroll(i32, i32),
    Wait(u64),
    /// Pause and ask the user a yes/no question.
    Ask(String),
    /// A direct tool instead of clicking: the argument is a target, an app name or a window title ("" for `Windows`).
    Tool(Tool, String),
    /// Press at the action's position and let go at this one (picture pixels, same screen).
    Drag(f32, f32),
    /// Hold a key combo for this many ms (at most `MAX_HOLD_MS`), then let go.
    Hold(Vec<u16>, u64),
}

/// Things the buddy can do without driving the screen. `Open` and `Launch` ask you each time and show the exact target.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Tool { Open, Launch, Focus, Windows }

impl Tool {
    pub fn asks(self) -> bool { matches!(self, Tool::Open | Tool::Launch) }
}

/// One action as the model sent it. `pos` is in the picture's pixels on screen `screen` (default: screen 1).
#[derive(Debug, PartialEq, Clone)]
pub struct Action { pub act: Act, pub pos: Option<(f32, f32)>, pub screen: Option<usize>, pub label: String,
    /// Number of an element from the list in the prompt (1 is the first): the click goes to its centre instead of `pos`.
    pub el: Option<usize> }

impl Action {
    /// A few words for the model's history and the log.
    pub fn describe(&self) -> String {
        let at = match (self.el, self.pos) {
            (Some(i), _) => format!(" on element {i}"),
            (None, Some((x, y))) => format!(" at {},{}", x as i32, y as i32),
            _ => String::new(),
        };
        match &self.act {
            Act::Move => format!("moved the mouse{at}"),
            Act::Click(Btn::Left, 1) => format!("clicked{at}"),
            Act::Click(Btn::Left, n) => format!("clicked {n} times{at}"),
            Act::Click(Btn::Right, _) => format!("right-clicked{at}"),
            Act::Type(t) => format!("typed {} characters", t.chars().count()),
            Act::Keys(k) => format!("pressed {} keys", k.len()),
            Act::Scroll(dx, dy) => format!("scrolled {dx},{dy}{at}"),
            Act::Wait(ms) => format!("waited {ms} ms"),
            Act::Ask(q) => format!("asked \"{q}\""),
            Act::Tool(Tool::Open, t) => format!("opened {}", assist::short(t, 60)),
            Act::Tool(Tool::Launch, t) => format!("launched {}", assist::short(t, 60)),
            Act::Tool(Tool::Focus, t) => format!("brought \"{}\" to the front", assist::short(t, 60)),
            Act::Tool(Tool::Windows, _) => "listed the open windows".to_string(),
            Act::Drag(x, y) => format!("dragged{at} to {},{}", *x as i32, *y as i32),
            Act::Hold(k, ms) => format!("held {} keys for {ms} ms", k.len()),
        }
    }
}

/// Longest target, app name or title a tool takes.
pub const MAX_TARGET: usize = 300;

/// File types `open` refuses: they run code, so they are not "opening" anything. (The Start Menu is what `launch` uses.)
const NO_OPEN: [&str; 30] = ["exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "scr", "lnk", "reg", "hta", "jar", "cpl",
    "dll", "pif", "url", "docm", "xlsm", "pptm", "appx", "msix", "py", "sh"];

/// A web address or a path that `open` may hand to Windows. Only http(s) links and plain local paths pass: no other protocol
/// (`ms-settings:`, `file:`, `javascript:`), no network shares (they send your login), no `%VARS%`, no programs.
pub fn open_target(arg: &str) -> Result<String, String> {
    let t = arg.trim();
    if t.is_empty() || t.chars().count() > MAX_TARGET || t.chars().any(|c| c.is_control() || c == '"' || c == '%' || c == '|' || c == '<' || c == '>') {
        return Err("that is not something I can open".into());
    }
    if let Some(i) = t.find(':').filter(|&i| i > 1) {
        let lower = t.to_lowercase();
        let host = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
        return match (&t[..i].to_lowercase()[..], host) {
            ("http" | "https", Some(h)) if h.chars().next().is_some_and(|c| c.is_alphanumeric()) => Ok(t.to_string()),
            _ => Err("I only open web links that start with http or https, and files and folders".into()),
        };
    }
    if t.starts_with("\\\\") || t.starts_with("//") { return Err("I don't open network shares".into()); }
    let ext = t.rsplit(['\\', '/']).next().unwrap_or("").rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    if NO_OPEN.contains(&ext.as_str()) { return Err(format!("I don't open .{ext} files, they run code; use launch for apps")); }
    Ok(t.to_string())
}

/// The window title that best fits what the model asked for: an exact match, then one that starts with it, then one that contains it (any case).
pub fn pick_title(titles: &[String], query: &str) -> Option<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() { return None; }
    let rank = |t: &str| { let t = t.to_lowercase(); if t == q { Some(0) } else if t.starts_with(&q) { Some(1) } else if t.contains(&q) { Some(2) } else { None } };
    titles.iter().enumerate().filter_map(|(i, t)| rank(t).map(|r| (r, i))).min().map(|(_, i)| i)
}

/// Window titles for the model, short and capped.
pub fn titles_text(titles: &[String]) -> String {
    if titles.is_empty() { return "no windows are open".into(); }
    titles.iter().take(25).map(|t| assist::short(t, 60)).collect::<Vec<_>>().join("; ")
}

/// Longest a held key may stay down.
pub const MAX_HOLD_MS: u64 = 3000;
/// Elements listed for the model; more only make the prompt long.
pub const MAX_ELEMENTS: usize = 60;
/// Longest the walk over a window's controls may take; it keeps what it has found.
pub const WALK_MS: u64 = 400;
/// Rounds in a row that change nothing before the buddy gives up.
pub const STUCK_ROUNDS: u32 = 3;

/// One thing you can click or type into in the front window: what it is called, what it is, and where (screen pixels: left, top, right, bottom).
#[derive(Debug, PartialEq, Clone)]
pub struct El { pub name: String, pub role: &'static str, pub rect: (i32, i32, i32, i32) }

impl El {
    pub fn centre(&self) -> (i32, i32) { ((self.rect.0 + self.rect.2) / 2, (self.rect.1 + self.rect.3) / 2) }
}

/// The role words for the UI Automation control types worth listing; None for everything else (panes, text, images ...).
pub fn role_of(control_type: i32) -> Option<&'static str> {
    Some(match control_type {
        50000 => "button", 50002 => "checkbox", 50003 => "combobox", 50004 => "edit", 50005 => "link", 50007 => "list item",
        50011 => "menu item", 50013 => "radio", 50019 => "tab", 50024 => "tree item", 50031 => "split button",
        _ => return None,
    })
}

/// What the walk found, made ready for the prompt: no empty or off-screen boxes, nameless ones only when you can type in them,
/// no twins, top to bottom then left to right, at most `MAX_ELEMENTS`.
pub fn tidy_elements(mut v: Vec<El>) -> Vec<El> {
    v.retain(|e| e.rect.2 > e.rect.0 && e.rect.3 > e.rect.1 && (!e.name.trim().is_empty() || e.role == "edit"));
    v.sort_by_key(|e| (e.rect.1, e.rect.0));
    v.dedup_by(|b, a| a.rect == b.rect && a.name == b.name);
    v.truncate(MAX_ELEMENTS);
    v
}

/// The list as the model reads it: `[3] button "Save" at 640,360`. The number is what `el` takes.
pub fn elements_text(els: &[El]) -> String {
    if els.is_empty() { return "none found, so use the picture".into(); }
    els.iter().enumerate().map(|(i, e)| { let (x, y) = e.centre(); format!("[{}] {} \"{}\" at {x},{y}", i + 1, e.role, assist::short(e.name.trim(), 40)) }).collect::<Vec<_>>().join("\n")
}

/// Where an action lands on the real screen: the centre of the element it names, else the picture pixel mapped through its monitor.
pub fn target(caps: &[screen::Cap], els: &[El], a: &Action) -> Result<Option<(i32, i32)>, String> {
    match a.el {
        Some(i) => i.checked_sub(1).and_then(|i| els.get(i)).map(|e| Some(e.centre())).ok_or_else(|| format!("there is no element {i}; the list has {}", els.len())),
        None => resolve(caps, a),
    }
}

/// A cheap fingerprint of what the screens show. Equal fingerprints in a row mean nothing on screen changed.
/// shortcut: it hashes the picture bytes, so a pointer that moved makes the first dead click look like a change; the second one is caught. Upgrade to a block diff if that is too slow to notice.
pub fn screen_sig(caps: &[screen::Cap]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in caps { c.jpeg.hash(&mut h); }
    h.finish()
}

/// Presses at `from` and lets go at `to`, in small steps so the app sees a drag: (the press, the moves, the release).
pub fn drag_inputs(from: (i32, i32), to: (i32, i32), v: (i32, i32, i32, i32)) -> (Vec<INPUT>, Vec<INPUT>, Vec<INPUT>) {
    const STEPS: i32 = 8;
    let down = vec![move_input(from.0, from.1, v), mouse(0, 0, 0, MOUSEEVENTF_LEFTDOWN)];
    let moves = (1..=STEPS).map(|i| move_input(from.0 + (to.0 - from.0) * i / STEPS, from.1 + (to.1 - from.1) * i / STEPS, v)).collect();
    (down, moves, vec![mouse(0, 0, 0, MOUSEEVENTF_LEFTUP)])
}

/// The virtual-key code for one key name; the aliases people and models use (cmd, opt, esc, return, del ...) all work.
pub fn vk(name: &str) -> Option<u16> {
    let n = name.trim().to_lowercase();
    let code = match n.as_str() {
        "cmd" | "command" | "win" | "super" | "meta" | "windows" => VK_LWIN.0,
        "ctrl" | "control" => VK_CONTROL.0,
        "shift" => VK_SHIFT.0,
        "alt" | "opt" | "option" => VK_MENU.0,
        "esc" | "escape" => VK_ESCAPE.0,
        "return" | "enter" => VK_RETURN.0,
        "tab" => VK_TAB.0,
        "space" | "spacebar" => VK_SPACE.0,
        "backspace" => VK_BACK.0,
        "del" | "delete" | "forwarddelete" => VK_DELETE.0,
        "left" => VK_LEFT.0, "right" => VK_RIGHT.0, "up" => VK_UP.0, "down" => VK_DOWN.0,
        "home" => VK_HOME.0, "end" => VK_END.0, "pageup" => VK_PRIOR.0, "pagedown" => VK_NEXT.0, "insert" => VK_INSERT.0,
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric() => s.to_uppercase().as_bytes()[0] as u16, // 'A'..'Z', '0'..'9' are their own codes
        s => match s.strip_prefix('f').and_then(|d| d.parse::<u16>().ok()) {
            Some(i) if (1..=12).contains(&i) => VK_F1.0 + i - 1,
            _ => return None,
        },
    };
    Some(code)
}

/// "ctrl+shift+s" as key codes; None if any part is unknown or there is nothing.
pub fn parse_keys(s: &str) -> Option<Vec<u16>> {
    let keys: Option<Vec<u16>> = s.split('+').map(vk).collect();
    keys.filter(|k| !k.is_empty() && k.len() <= 4)
}

/// One action from the model's JSON; None for unknown types (screenshot, zoom ... are no-ops: every round starts with a fresh picture) and bad fields.
pub fn parse_action(v: &Value) -> Option<Action> {
    let pos = || Some((v["x"].as_f64()? as f32, v["y"].as_f64()? as f32));
    let text = |k: &str| v[k].as_str().map(str::trim).filter(|s| !s.is_empty());
    let (mut p, mut act) = (None, None);
    let el = v["el"].as_u64().filter(|n| *n >= 1).map(|n| n as usize);
    match v["type"].as_str()?.to_lowercase().as_str() {
        "mouse_move" | "move" => { p = pos().or_else(|| el.map(|_| (0.0, 0.0)))?.into(); act = Some(Act::Move); }
        "left_click" | "click" => { p = pos().or_else(|| el.map(|_| (0.0, 0.0)))?.into(); act = Some(Act::Click(Btn::Left, 1)); }
        "right_click" => { p = pos().or_else(|| el.map(|_| (0.0, 0.0)))?.into(); act = Some(Act::Click(Btn::Right, 1)); }
        "double_click" => { p = pos().or_else(|| el.map(|_| (0.0, 0.0)))?.into(); act = Some(Act::Click(Btn::Left, 2)); }
        "triple_click" => { p = pos().or_else(|| el.map(|_| (0.0, 0.0)))?.into(); act = Some(Act::Click(Btn::Left, 3)); }
        "type" => act = Some(Act::Type(v["text"].as_str().filter(|s| !s.is_empty())?.chars().take(MAX_TEXT).collect())),
        "keys" | "key" => act = Some(Act::Keys(parse_keys(text("keys").or_else(|| text("text"))?)?)),
        "scroll" => {
            p = pos();
            let n = |k: &str| v[k].as_i64().unwrap_or(0).clamp(-20, 20) as i32;
            act = Some(Act::Scroll(n("dx"), n("dy")));
        }
        "wait" => act = Some(Act::Wait(v["ms"].as_u64().unwrap_or(500).min(5000))),
        "drag" | "left_click_drag" => {
            p = pos();
            let to = (v["to_x"].as_f64()? as f32, v["to_y"].as_f64()? as f32);
            if p.is_none() && el.is_none() { return None; }
            act = Some(Act::Drag(to.0, to.1));
        }
        "hold_key" | "hold" => act = Some(Act::Hold(parse_keys(text("keys").or_else(|| text("text"))?)?, v["ms"].as_u64().unwrap_or(500).min(MAX_HOLD_MS))),
        "ask" => act = Some(Act::Ask(text("question").or_else(|| text("text"))?.chars().take(MAX_ASK).collect())),
        t @ ("open" | "launch" | "focus" | "windows") => {
            let arg: String = ["target", "text", "name", "title"].iter().find_map(|k| text(k)).unwrap_or("").chars().take(MAX_TARGET).collect();
            let tool = match t { "open" => Tool::Open, "launch" => Tool::Launch, "focus" => Tool::Focus, _ => Tool::Windows };
            if arg.is_empty() && tool != Tool::Windows { return None; }
            act = Some(Act::Tool(tool, arg));
        }
        _ => {}
    }
    Some(Action { act: act?, pos: p, el, screen: v["screen"].as_u64().map(|n| n as usize), label: v["label"].as_str().unwrap_or("").chars().take(40).collect() })
}

/// What the model answered: a short line to say, the actions (at most `MAX_ACTIONS`), and whether the task is finished.
#[derive(Debug, PartialEq)]
pub struct Reply { pub say: String, pub actions: Vec<Action>, pub done: bool }

/// The JSON object in a reply (maybe inside a code fence). None when there isn't one.
pub fn parse_reply(text: &str) -> Option<Reply> {
    let (a, b) = (text.find('{')?, text.rfind('}')?);
    let v: Value = serde_json::from_str(text.get(a..=b)?).ok()?;
    v.is_object().then(|| Reply {
        say: v["say"].as_str().unwrap_or("").trim().chars().take(160).collect(),
        actions: v["actions"].as_array().map(|a| a.iter().filter_map(parse_action).take(MAX_ACTIONS).collect()).unwrap_or_default(),
        done: v["done"].as_bool().unwrap_or(false),
    })
}

/// A click on something that sends, deletes or pays gets its own yes, even if the model forgot to ask.
pub fn risky(label: &str) -> bool {
    const WORDS: [&str; 15] = ["send", "delete", "remove", "erase", "pay", "buy", "purchase", "order", "submit", "transfer", "publish", "post", "uninstall", "format", "checkout"];
    label.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| WORDS.contains(&w))
}

/// Why a take-over ended.
#[derive(Debug, PartialEq, Clone)]
pub enum Stop { Done, Esc, Voice, UserMouse, Rounds, Time, Stuck, Refused(String), Error(String) }

impl Stop {
    /// The task did not finish and you didn't cause the stop: the buddy's line must not be missable.
    pub fn failed(&self) -> bool { matches!(self, Stop::Rounds | Stop::Time | Stop::Stuck | Stop::Refused(_) | Stop::Error(_)) }

    /// What the buddy says when he hands the mouse back.
    pub fn message(&self) -> String {
        match self {
            Stop::Done => "Done. The mouse is yours.".into(),
            Stop::Esc => "Stopped. The mouse is yours.".into(),
            Stop::Voice => "You called me, so I stopped. The mouse is yours.".into(),
            Stop::UserMouse => "You took the mouse, so I stopped.".into(),
            Stop::Rounds => "That took more steps than I'm allowed, so I stopped. The mouse is yours.".into(),
            Stop::Time => "I ran out of time and stopped. The mouse is yours.".into(),
            Stop::Stuck => "Nothing on the screen changed after my last few tries, so I stopped. The mouse is yours.".into(),
            Stop::Refused(why) | Stop::Error(why) => format!("{} The mouse is yours.", why),
        }
    }
}

/// Watches whether you moved the mouse: the drive tells it where the pointer should be, and a pointer that is far from there
/// is you. It waits for the pointer to rest before it says so, so it doesn't fight a hand that is still moving.
pub struct MouseWatch { expected: (i32, i32), moving: Option<((i32, i32), u64)> }

impl MouseWatch {
    pub fn new(at: (i32, i32)) -> MouseWatch { MouseWatch { expected: at, moving: None } }
    /// The drive just put (or found) the pointer here.
    pub fn expect(&mut self, at: (i32, i32)) { self.expected = at; self.moving = None; }
    /// Feed the pointer position and the time; true when you moved it and it has rested.
    pub fn took(&mut self, at: (i32, i32), now_ms: u64) -> bool {
        let far = |a: (i32, i32), b: (i32, i32)| (a.0 - b.0).abs() > MOVE_PX || (a.1 - b.1).abs() > MOVE_PX;
        match self.moving {
            None => { if far(at, self.expected) { self.moving = Some((at, now_ms)); } false }
            Some((last, since)) => {
                if (at.0 - last.0).abs() > 3 || (at.1 - last.1).abs() > 3 { self.moving = Some((at, now_ms)); false }
                else { now_ms.saturating_sub(since) >= REST_MS }
            }
        }
    }
}

// ---- SendInput builders: pure, so the tests can look at what would be sent ----

/// A screen coordinate as 0..=65535 over the virtual screen starting at `origin` with `size` pixels.
pub fn norm(p: i32, origin: i32, size: i32) -> i32 { ((p - origin) as i64 * 65535 / (size - 1).max(1) as i64).clamp(0, 65535) as i32 }

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

/// Move to screen pixel (x, y) on a virtual screen (left, top, width, height).
pub fn move_input(x: i32, y: i32, v: (i32, i32, i32, i32)) -> INPUT {
    mouse(norm(x, v.0, v.2), norm(y, v.1, v.3), 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)
}

/// `n` presses of a button, each a down and an up.
pub fn click_inputs(b: Btn, n: u8) -> Vec<INPUT> {
    let (d, u) = match b { Btn::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP), Btn::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP) };
    (0..n).flat_map(|_| [mouse(0, 0, 0, d), mouse(0, 0, 0, u)]).collect()
}

/// Wheel turns: `dy` notches down (positive) and `dx` right. One notch is 120.
pub fn scroll_inputs(dx: i32, dy: i32) -> Vec<INPUT> {
    let mut v = vec![];
    if dy != 0 { v.push(mouse(0, 0, -dy * 120, MOUSEEVENTF_WHEEL)); }
    if dx != 0 { v.push(mouse(0, 0, dx * 120, MOUSEEVENTF_HWHEEL)); }
    v
}

/// Text as Unicode key events (no clipboard, so yours stays untouched). Enter and Tab are real keys.
pub fn text_inputs(s: &str) -> Vec<INPUT> {
    let mut v = vec![];
    for c in s.replace("\r\n", "\n").chars() {
        match c {
            '\n' => { v.push(key(VK_RETURN.0, 0, KEYBD_EVENT_FLAGS(0))); v.push(key(VK_RETURN.0, 0, KEYEVENTF_KEYUP)); }
            '\t' => { v.push(key(VK_TAB.0, 0, KEYBD_EVENT_FLAGS(0))); v.push(key(VK_TAB.0, 0, KEYEVENTF_KEYUP)); }
            c => for u in c.encode_utf16(&mut [0u16; 2]) {
                v.push(key(0, *u, KEYEVENTF_UNICODE));
                v.push(key(0, *u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            },
        }
    }
    v
}

/// Key downs in order, then key ups in reverse.
pub fn combo_inputs(keys: &[u16]) -> (Vec<INPUT>, Vec<INPUT>) {
    (keys.iter().map(|k| key(*k, 0, KEYBD_EVENT_FLAGS(0))).collect(), keys.iter().rev().map(|k| key(*k, 0, KEYEVENTF_KEYUP)).collect())
}

/// Where an action lands on the real screen, or why it can't: the picture's pixel mapped through the monitor it names.
pub fn resolve(caps: &[screen::Cap], a: &Action) -> Result<Option<(i32, i32)>, String> {
    let Some((x, y)) = a.pos else { return Ok(None) };
    match screen::target(caps, a.screen) {
        Some(c) => Ok(Some(screen::to_screen(c, x, y))),
        None => Err(format!("there is no screen {}", a.screen.unwrap_or(1))),
    }
}

// ---- the loop ----

pub struct Limits { pub rounds: u32, pub ms: u64 }

/// Everything the loop needs from the outside, so tests can stand in for the PC.
pub trait Host {
    fn capture(&mut self) -> Result<Vec<screen::Cap>, String>;
    /// One model call: the goal, what has happened so far, the pictures. Returns the model's text.
    /// `els` is the elements list as text, for the prompt.
    fn model(&mut self, goal: &str, history: &[String], caps: &[screen::Cap], els: &str) -> Result<String, String>;
    /// What you can click or type into in the front window, if the PC can tell (none: the model uses the picture).
    fn elements(&mut self) -> Vec<El> { vec![] }
    /// Runs one action at its screen position (None when it has none); a drag also gets where it lets go.
    fn act(&mut self, a: &Action, at: Option<(i32, i32)>, to: Option<(i32, i32)>) -> Result<(), String>;
    /// Puts the pointer back where it was before the take-over.
    fn restore(&mut self, _at: (i32, i32)) {}
    fn cursor(&self) -> (i32, i32);
    fn now_ms(&self) -> u64;
    fn sleep(&mut self, ms: u64);
    /// Esc pressed or you spoke to the buddy.
    fn halted(&mut self) -> Option<Stop>;
    fn say(&mut self, line: &str);
    /// Shows where the next click goes.
    fn point(&mut self, at: (i32, i32), label: &str);
    fn confirm(&mut self, question: &str) -> bool;
    /// Like `confirm`, but you may answer with words instead of yes or no.
    fn ask(&mut self, question: &str) -> Answer { if self.confirm(question) { Answer::Yes } else { Answer::No } }
    fn password_focused(&self) -> bool;
    /// Runs one direct tool (the target is already checked and, for `Open` and `Launch`, approved). Returns what the model hears back.
    fn tool(&mut self, tool: Tool, arg: &str) -> Result<String, String>;
    /// The front window is running as administrator, so it ignores our input.
    fn front_blocks_input(&self) -> bool;
    fn release(&mut self);
}

const NAP_MS: u64 = 25;

struct Guard<'a, H: Host> { h: &'a mut H, watch: MouseWatch, start: u64, limit: u64 }

impl<H: Host> Guard<'_, H> {
    fn check(&mut self) -> Option<Stop> {
        if let Some(s) = self.h.halted() { return Some(s); }
        if self.h.now_ms().saturating_sub(self.start) > self.limit { return Some(Stop::Time); }
        let (at, now) = (self.h.cursor(), self.h.now_ms());
        self.watch.took(at, now).then_some(Stop::UserMouse)
    }
    /// Sleeps in short slices so Esc, your mouse and the clock are checked the whole time.
    fn pause(&mut self, ms: u64) -> Option<Stop> {
        let mut left = ms;
        loop {
            if let Some(s) = self.check() { return Some(s); }
            if left == 0 { return None; }
            let n = left.min(NAP_MS);
            self.h.sleep(n);
            left -= n;
        }
    }
}

/// Runs the take-over until it ends. `goal` is what you asked for. Always calls `release` at the end.
pub fn run<H: Host>(h: &mut H, goal: &str, lim: Limits) -> Stop {
    let (start, at) = (h.now_ms(), h.cursor());
    let mut g = Guard { watch: MouseWatch::new(at), start, limit: lim.ms, h };
    let stop = rounds(&mut g, goal, lim.rounds);
    g.h.release();
    // the pointer goes home unless you have it: after Esc, your voice or your own move it stays where you are
    if !matches!(stop, Stop::Esc | Stop::Voice | Stop::UserMouse) && g.h.cursor() != at { g.h.restore(at); }
    stop
}

fn rounds<H: Host>(g: &mut Guard<H>, goal: &str, max: u32) -> Stop {
    let mut history: Vec<String> = vec![];
    let (mut bad, mut dead, mut last_sig, mut acted) = (0, 0, None, false);
    g.h.say("Hands off the mouse while I drive. Esc stops me.");
    for round in 1..=max {
        if let Some(s) = g.check() { return s; }
        let caps = match g.h.capture() { Ok(c) => c, Err(e) => return Stop::Error(e) };
        // the same picture again after actions that should have changed it: tell the model, and give up after a few
        let sig = screen_sig(&caps);
        if acted && last_sig == Some(sig) {
            dead += 1;
            if dead >= STUCK_ROUNDS { return Stop::Stuck; }
            history.push(format!("round {round}: the screen looks exactly the same as before your last actions, so they did nothing; try something different"));
        } else { dead = 0; }
        last_sig = Some(sig);
        let els = g.h.elements();
        let text = match g.h.model(goal, &history, &caps, &elements_text(&els)) {
            Ok(t) => t,
            // a model call cut short by Esc, your voice or the clock reports that instead of an error
            Err(e) => return g.h.halted().or_else(|| (g.h.now_ms().saturating_sub(g.start) > g.limit).then_some(Stop::Time)).unwrap_or(Stop::Error(e)),
        };
        // your hand is still on the mouse after the yes, and the model takes seconds: only moves from here on count
        g.watch.expect(g.h.cursor());
        let Some(reply) = parse_reply(&text) else {
            bad += 1;
            if bad >= 2 { return Stop::Error("I couldn't understand my own plan.".into()); }
            history.push(format!("round {round}: your reply was not valid JSON; answer with only the JSON object"));
            continue;
        };
        if !reply.say.is_empty() { g.h.say(&reply.say); }
        let (mut approved, mut replied) = (false, false);
        acted = false;
        for a in &reply.actions {
            if let Some(s) = g.check() { return s; }
            match step(g, &caps, &els, a, &mut approved, &mut replied) {
                // only actions that change the screen count: waiting, asking and listing windows do not
                Ok(note) => { acted |= !matches!(a.act, Act::Wait(_) | Act::Ask(_) | Act::Tool(Tool::Windows, _)); history.push(format!("round {round}: {note}")); if replied { break; } }
                Err(Stop::Error(e)) => { history.push(format!("round {round}: could not do that, {e}")); break; }
                Err(s) => return s,
            }
        }
        if reply.done && !replied { return Stop::Done; }
        if reply.actions.is_empty() { history.push(format!("round {round}: you did nothing; if the task is finished say done")); }
        if history.len() > 12 { history.drain(..history.len() - 12); }
        if let Some(s) = g.pause(400) { return s; } // let the screen settle before the next picture
    }
    Stop::Rounds
}

/// Does one action. Err(Stop::Error) means just this action failed (the round ends, the model hears why); any other Err ends the take-over.
fn step<H: Host>(g: &mut Guard<H>, caps: &[screen::Cap], els: &[El], a: &Action, approved: &mut bool, replied: &mut bool) -> Result<String, Stop> {
    if let Act::Ask(q) = &a.act {
        let ans = g.h.ask(q);
        if ans == Answer::No { return Err(Stop::Refused("You said no, so I stopped.".into())); }
        g.watch.expect(g.h.cursor()); // your hand was on the mouse to answer
        if let Answer::Said(t) = ans {
            // words are instructions, not a yes: the model plans again from a fresh picture, and a risky click asks again
            *replied = true;
            return Ok(format!("{}; the user answered: \"{t}\". Follow it; the rest of your actions were dropped", a.describe()));
        }
        *approved = true;
        return Ok(format!("{}; the user said yes", a.describe()));
    }
    if let Act::Wait(ms) = a.act {
        return match g.pause(ms) { Some(s) => Err(s), None => Ok(a.describe()) };
    }
    if let Act::Tool(tool, arg) = &a.act {
        // tools send no input, so the password and administrator checks don't apply; opening and launching ask every time
        let arg = if *tool == Tool::Open { open_target(arg).map_err(Stop::Error)? } else { arg.clone() };
        if tool.asks() {
            let q = if *tool == Tool::Open { format!("The buddy wants to open {}. Allow it?", assist::short(&arg, 200)) } else { format!("The buddy wants to start the app \"{}\". Allow it?", assist::short(&arg, 80)) };
            if !g.h.confirm(&q) { return Err(Stop::Refused("You said no, so I stopped.".into())); }
            g.watch.expect(g.h.cursor()); // your hand was on the mouse to answer
        }
        let out = g.h.tool(*tool, &arg).map_err(Stop::Error)?;
        if let Some(s) = g.pause(if tool.asks() { 1200 } else { SETTLE_MS }) { return Err(s); } // an opened app needs a moment to appear
        return Ok(format!("{}{}", a.describe(), if out.is_empty() { String::new() } else { format!(": {out}") }));
    }
    let at = target(caps, els, a).map_err(Stop::Error)?;
    // a drag lets go on the same screen the picture names
    let to = match a.act { Act::Drag(x, y) => resolve(caps, &Action { act: Act::Move, pos: Some((x, y)), el: None, ..a.clone() }).map_err(Stop::Error)?, _ => None };
    if matches!(a.act, Act::Click(..) | Act::Drag(..)) && risky(&a.label) && !std::mem::take(approved) && !g.h.confirm(&format!("The buddy wants to click \"{}\". Allow it?", a.label)) {
        return Err(Stop::Refused("You said no, so I stopped.".into()));
    }
    if matches!(a.act, Act::Type(_) | Act::Hold(..)) && g.h.password_focused() {
        return Err(Stop::Refused("That's a password field, so type that one yourself.".into()));
    }
    if g.h.front_blocks_input() {
        return Err(Stop::Refused("The window in front runs as administrator and ignores me.".into()));
    }
    if let (Some(p), Act::Click(..) | Act::Drag(..)) = (at, &a.act) {
        g.h.point(p, &a.label);
        if let Some(s) = g.pause(900) { return Err(s); } // the buddy flies there and rings it first
    }
    g.h.act(a, at, to).map_err(|e| if e.contains("administrator") { Stop::Refused(e) } else { Stop::Error(e) })?;
    let aimed = to.or(at).unwrap_or_else(|| g.h.cursor()); // where we put it, not where it is: a grab during the action must still show
    g.watch.expect(aimed);
    if let Some(s) = g.pause(SETTLE_MS) { return Err(s); }
    Ok(a.describe())
}

// ---- the real PC ----

/// What the model is told.
pub const SYSTEM: &str = "You are a desktop buddy who is temporarily controlling the user's mouse and keyboard to finish one task. \
Each turn you get the goal, what you did so far, and a picture of every screen (each picture says its size and which screen it is). \
Answer with ONLY one JSON object: {\"say\":\"one short sentence about what you do next\",\"actions\":[...],\"done\":false}. \
Set done to true, with an empty actions list, when the goal is reached or cannot be reached. Actions (at most 5 per turn), with x and y \
in the picture's pixels (origin top-left) and screen set to the screen number when there is more than one: \
{\"type\":\"left_click\",\"x\":0,\"y\":0,\"screen\":1,\"label\":\"2 words naming the target\"}, right_click, double_click, triple_click, mouse_move, \
{\"type\":\"type\",\"text\":\"...\"}, {\"type\":\"keys\",\"keys\":\"ctrl+s\"}, {\"type\":\"scroll\",\"x\":0,\"y\":0,\"dy\":3} (dy positive scrolls down), \
{\"type\":\"wait\",\"ms\":500}, {\"type\":\"ask\",\"question\":\"...\"}. Direct tools are faster and surer than clicking, so use one when it fits and click only when none does: \
{\"type\":\"launch\",\"target\":\"app name\"} starts an installed app, {\"type\":\"open\",\"target\":\"https://... or a file or folder path\"} opens a link, file or folder \
(the user is asked each time; programs and scripts are refused), {\"type\":\"focus\",\"target\":\"part of a window title\"} brings a window to the front, \
{\"type\":\"windows\"} lists the open window titles (they come back in your history). Also {\"type\":\"drag\",\"x\":0,\"y\":0,\"to_x\":0,\"to_y\":0} and {\"type\":\"hold_key\",\"keys\":\"shift\",\"ms\":800} (at most 3 seconds). \
Each turn also lists the clickable things in the front window as [number] role \"name\" at x,y. When the thing you want is listed, click it by number, for example \
{\"type\":\"left_click\",\"el\":3,\"label\":\"Save\"}, instead of guessing pixels; if it is not listed, use the picture. Always put a click's label. Look at the new picture after every turn \
and only do what you can see. Never type into a password field. Before anything that sends, deletes, buys or pays, or that you cannot undo, \
use an ask action and wait for the answer. Also use ask when something is unclear, such as several matches for a name: list the options \
briefly. The user may answer yes, no, or with typed instructions; follow typed instructions, they are not a yes to a risky step. Do only the goal and nothing else. The first thing you say must ask the user to keep their hands off the mouse.";

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The buddy is really driving (you said yes): the screen shows a border and the Esc hint (driveframe.rs).
static DRIVING: AtomicBool = AtomicBool::new(false);
static ESC: AtomicBool = AtomicBool::new(false);
static VOICE: AtomicBool = AtomicBool::new(false);
/// How you answered the buddy's question.
#[derive(Debug, PartialEq, Clone)]
pub enum Answer { Yes, No, /// Typed instructions or a clarification.
    Said(String) }

/// The longest question the island shows; it wraps, and scrolls past a few lines.
pub const MAX_ASK: usize = 600;
/// The longest typed reply sent back to the model.
pub const MAX_SAID: usize = 400;

/// The question the island is showing and the answer once you gave it.
static ASK: std::sync::Mutex<Option<(String, Option<Answer>)>> = std::sync::Mutex::new(None);
static HOOK_TID: AtomicU32 = AtomicU32::new(0);

pub fn active() -> bool { ACTIVE.load(Ordering::Relaxed) }

pub fn driving() -> bool { DRIVING.load(Ordering::Relaxed) }

/// Sets the flag and wakes the UI thread, which adds or removes the border.
fn set_driving(on: bool) {
    DRIVING.store(on, Ordering::Relaxed);
    crate::app::post(Box::new(|_| {}));
}

/// Keeps one of our windows out of the model's screenshots.
pub fn hide_from_capture(h: HWND) { exclude_window(h, true); }
/// You spoke to the buddy (the hotkey): a take-over in progress stops.
pub fn request_stop() { if active() { VOICE.store(true, Ordering::Relaxed); } }

/// The question waiting for your answer, for the island to show.
pub fn pending() -> Option<String> { ASK.lock().ok().and_then(|a| a.as_ref().filter(|a| a.1.is_none()).map(|a| a.0.clone())) }
/// Your answer from the island's Allow or Deny.
pub fn answer(yes: bool) { give(if yes { Answer::Yes } else { Answer::No }) }
/// Your typed reply from the island. Blank text is no answer at all.
pub fn reply(text: &str) {
    let t: String = text.trim().chars().take(MAX_SAID).collect();
    if !t.is_empty() { give(Answer::Said(t)); }
}
/// A question is waiting for you (the voice hotkey then answers it instead of stopping the take-over).
pub fn waiting() -> bool { pending().is_some() }

/// What you said out loud: a plain yes or no answers like the buttons, anything else is words for the buddy.
pub fn spoken_answer(said: &str) -> Option<Answer> {
    let t = said.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    if t.is_empty() { return None; }
    Some(match t.as_str() {
        "yes" | "yeah" | "yep" | "yup" | "sure" | "ok" | "okay" | "allow" | "go ahead" | "do it" | "yes please" | "go on" => Answer::Yes,
        "no" | "nope" | "nah" | "deny" | "stop" | "cancel" | "don't" | "do not" | "no thanks" | "no thank you" => Answer::No,
        _ => Answer::Said(said.trim().chars().take(MAX_SAID).collect()),
    })
}

/// Answers the waiting question with what you said out loud. False when nothing is waiting or nothing was heard.
pub fn reply_spoken(said: &str) -> bool {
    match (waiting(), spoken_answer(said)) { (true, Some(a)) => { give(a); true } _ => false }
}

fn give(ans: Answer) { if let Ok(mut a) = ASK.lock() { if let Some(a) = a.as_mut() { a.1 = Some(ans); } } }

fn island_open() -> bool {
    unsafe { FindWindowW(None, w!("Zor UI / island-panel")).is_ok_and(|h| IsWindowVisible(h).as_bool()) }
}

/// Asks in the island panel and waits for Allow, Deny or a typed reply. No answer in 90 s, Esc, or your voice is a no.
pub fn ask_island(q: &str) -> Answer {
    if let Ok(mut a) = ASK.lock() { *a = Some((q.chars().take(MAX_ASK).collect(), None)); }
    // opening a panel that is already open would close it
    if !island_open() { crate::app::post(Box::new(|cx| cx.actions.push(crate::app::Action::open("island-panel")))); }
    let start = std::time::Instant::now();
    let yes = loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let a = ASK.lock().ok().and_then(|a| a.as_ref().and_then(|a| a.1.clone()));
        if let Some(y) = a { break y; }
        if ESC.load(Ordering::Relaxed) || VOICE.load(Ordering::Relaxed) || start.elapsed().as_secs() >= 90 { break Answer::No; }
    };
    if let Ok(mut a) = ASK.lock() { *a = None; }
    if yes != Answer::Yes { crate::app::post(Box::new(|cx| cx.actions.push(crate::app::Action::Close("island-panel".into())))); }
    yes
}

/// "Take over" is on in Settings. Off unless turned on: it moves your mouse and sends screenshots away.
pub fn enabled(cfg: &Config) -> bool { cfg.get("buddies.drive").as_bool().unwrap_or(false) }

pub fn limits(cfg: &Config) -> Limits {
    Limits { rounds: cfg.get("buddies.driveRounds").as_u64().unwrap_or(25).clamp(1, 200) as u32, ms: cfg.get("buddies.driveSecs").as_u64().unwrap_or(180).clamp(10, 3600) * 1000 }
}

unsafe extern "system" fn esc_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && w.0 as u32 == WM_KEYDOWN {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        // injected keys are ours (a model's "esc" key must not stop itself)
        if k.vkCode == VK_ESCAPE.0 as u32 && k.flags.0 & LLKHF_INJECTED.0 == 0 { ESC.store(true, Ordering::Relaxed); }
    }
    CallNextHookEx(None, code, w, l)
}

/// The low-level keyboard hook that sees Esc, on a thread of its own with a message loop. Dropping it removes the hook.
struct EscHook(Option<std::thread::JoinHandle<()>>);

impl EscHook {
    fn install() -> Option<EscHook> {
        let (tx, rx) = std::sync::mpsc::channel();
        let th = std::thread::spawn(move || unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(esc_hook), GetModuleHandleW(None).map(|m| m).unwrap_or(HMODULE::default()), 0);
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE); // creates this thread's queue before the id is shared
            let _ = tx.send(hook.is_ok().then(|| GetCurrentThreadId()));
            if let Ok(h) = hook {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
                let _ = UnhookWindowsHookEx(h);
            }
        });
        let tid = rx.recv().ok().flatten()?;
        HOOK_TID.store(tid, Ordering::Relaxed);
        Some(EscHook(Some(th)))
    }
}

impl Drop for EscHook {
    fn drop(&mut self) {
        unsafe { let _ = PostThreadMessageW(HOOK_TID.swap(0, Ordering::Relaxed), WM_QUIT, WPARAM(0), LPARAM(0)); }
        if let Some(t) = self.0.take() { let _ = t.join(); }
    }
}

/// An extended window style with the click-through flag set or cleared.
pub fn with_pass_through(ex: isize, on: bool) -> isize {
    let f = WS_EX_TRANSPARENT.0 as isize;
    if on { ex | f } else { ex & !f }
}

/// Makes the buddy windows let mouse clicks through (or not). Their ring and label sit right on the target, and a
/// click on them is a click on the buddy, not on the app.
fn pass_through(on: bool) {
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            let mut t = [0u16; 64];
            let n = GetWindowTextW(h, &mut t).max(0) as usize;
            if String::from_utf16_lossy(&t[..n]).starts_with("Zor UI / gojo") {
                SetWindowLongPtrW(h, GWL_EXSTYLE, with_pass_through(GetWindowLongPtrW(h, GWL_EXSTYLE), data.0 != 0));
            }
        }
        BOOL(1)
    }
    unsafe { let _ = EnumWindows(Some(each), LPARAM(on as isize)); }
}

/// Hides one of this app's windows from screenshots, or shows it again. True when the window changed.
fn exclude_window(h: HWND, on: bool) -> bool {
    unsafe {
        let mut now = 0u32;
        if GetWindowDisplayAffinity(h, &mut now).is_err() || (now == WDA_EXCLUDEFROMCAPTURE.0) == on { return false; }
        SetWindowDisplayAffinity(h, if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE }).is_ok()
    }
}

/// Keeps this app's windows (buddy, island, bar, bubbles) out of the pictures the model gets, so it never mistakes its own ring or
/// island for the task. Only while driving: your own screenshots and recordings must still show them. Returns how many windows changed.
fn exclude_own_windows(on: bool) -> usize {
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let (on, count) = &mut *(data.0 as *mut (bool, usize));
        if pid == GetCurrentProcessId() && exclude_window(h, *on) { *count += 1; }
        BOOL(1)
    }
    let mut state = (on, 0usize);
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut state as *mut _ as isize)); }
    state.1
}

fn send(inputs: &[INPUT]) -> bool { inputs.is_empty() || unsafe { SendInput(inputs, size_of::<INPUT>() as i32) as usize == inputs.len() } }

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) }
}

/// Is the process with this id running elevated? None when that can't be read.
fn elevated(pid: u32) -> Option<bool> {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut tok = Default::default();
        let ok = OpenProcessToken(p, TOKEN_QUERY, &mut tok).is_ok();
        let _ = CloseHandle(p);
        if !ok { return None; }
        let (mut e, mut n) = (TOKEN_ELEVATION::default(), 0u32);
        let r = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as *mut _), size_of::<TOKEN_ELEVATION>() as u32, &mut n);
        let _ = CloseHandle(tok);
        r.ok().map(|_| e.TokenIsElevated != 0)
    }
}

fn front_blocks() -> bool {
    unsafe {
        let w: HWND = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(w, Some(&mut pid));
        if pid == 0 { return false; }
        // we are elevated too: UIPI lets us through. Unknown target while we are not: assume it blocks.
        let me = elevated(windows::Win32::System::Threading::GetProcessId(GetCurrentProcess())).unwrap_or(false);
        !me && elevated(pid) != Some(false)
    }
}

/// Real windows you could switch to: visible, titled, not hidden by Windows, not a popup or tool window, not ours.
fn top_windows() -> Vec<(HWND, String)> {
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let out = &mut *(l.0 as *mut Vec<(HWND, String)>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, size_of::<u32>() as u32);
        let tool = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
        if IsWindowVisible(h).as_bool() && cloaked == 0 && !tool && pid != GetCurrentProcessId() && GetWindow(h, GW_OWNER).map_or(true, |o| o.0.is_null()) {
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, &mut buf).max(0) as usize;
            if n > 0 { out.push((h, String::from_utf16_lossy(&buf[..n]))); }
        }
        true.into()
    }
    let mut v: Vec<(HWND, String)> = vec![];
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut v as *mut _ as isize)); }
    v
}

/// The clickable and typeable things in the front window, through UI Automation. Slow or empty for some apps (games, some
/// Electron ones): then the list is short or empty and the model uses the picture. It stops after `WALK_MS`.
fn walk_front_window() -> Vec<El> {
    use windows::Win32::UI::Accessibility::TreeScope_Descendants;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return vec![] };
        let front = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(front, Some(&mut pid));
        if pid == GetCurrentProcessId() { return vec![]; } // our own windows are not the task
        let Ok(root) = uia.ElementFromHandle(front) else { return vec![] };
        let Ok(cond) = uia.CreateTrueCondition() else { return vec![] };
        let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else { return vec![] };
        let (n, t0) = (all.Length().unwrap_or(0), std::time::Instant::now());
        let mut out = vec![];
        for i in 0..n {
            if t0.elapsed().as_millis() as u64 > WALK_MS { break; }
            let Ok(e) = all.GetElement(i) else { continue };
            let Some(role) = e.CurrentControlType().ok().and_then(|c| role_of(c.0)) else { continue };
            if e.CurrentIsOffscreen().is_ok_and(|b| b.as_bool()) || !e.CurrentIsEnabled().is_ok_and(|b| b.as_bool()) { continue; }
            let Ok(r) = e.CurrentBoundingRectangle() else { continue };
            out.push(El { name: e.CurrentName().map(|s| s.to_string()).unwrap_or_default(), role, rect: (r.left, r.top, r.right, r.bottom) });
        }
        out
    }
}

fn focused_is_password() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return false };
        uia.GetFocusedElement().and_then(|e| e.CurrentIsPassword()).is_ok_and(|b| b.as_bool())
    }
}

struct Live<'a> { cfg: &'a Config, model: String, held: Vec<u16>, /// The left button is down because of a drag.
    button: bool, t0: std::time::Instant, limit_ms: u64 }

impl Host for Live<'_> {
    fn capture(&mut self) -> Result<Vec<screen::Cap>, String> {
        // windows made since the last round (an island question, a bubble) are hidden too; the screen needs a frame to drop them
        if exclude_own_windows(true) > 0 { std::thread::sleep(std::time::Duration::from_millis(80)); }
        screen::capture_all()
    }
    fn model(&mut self, goal: &str, history: &[String], caps: &[screen::Cap], els: &str) -> Result<String, String> {
        let done = if history.is_empty() { "nothing yet".to_string() } else { history.join("\n") };
        let convo = assist::convo_with(&self.model, SYSTEM, &format!("Goal: {goal}

What you did so far:
{done}

Clickable things in the front window:
{els}

The screens now are attached. Answer with the JSON."), caps);
        // the call runs on its own thread so Esc, your voice and the time limit still work while the model thinks
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = Config(self.cfg.0.clone());
        std::thread::spawn(move || { let _ = tx.send(assist::chat(&cfg, &convo, 90)); });
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(r) => return r,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err("I got no answer from the AI.".into()),
                Err(_) if ESC.load(Ordering::Relaxed) || VOICE.load(Ordering::Relaxed) || self.now_ms() > self.limit_ms => return Err("stopped while waiting for the AI".into()),
                Err(_) => {}
            }
        }
    }
    fn elements(&mut self) -> Vec<El> {
        let t = std::time::Instant::now();
        let v = tidy_elements(walk_front_window());
        crate::log::line(&format!("drive: {} elements in {} ms", v.len(), t.elapsed().as_millis()));
        v
    }
    fn restore(&mut self, at: (i32, i32)) { send(&[move_input(at.0, at.1, virtual_screen())]); }
    fn act(&mut self, a: &Action, at: Option<(i32, i32)>, to: Option<(i32, i32)>) -> Result<(), String> {
        crate::log::line(&format!("drive: {} {:?}", a.describe(), at));
        let fail = |what: &str| format!("Windows refused the {what}; the window in front may run as administrator.");
        if let Some((x, y)) = at { if !send(&[move_input(x, y, virtual_screen())]) { return Err(fail("mouse move")); } }
        let ok = match &a.act {
            Act::Move | Act::Wait(_) | Act::Ask(_) | Act::Tool(..) => true,
            Act::Click(b, n) => send(&click_inputs(*b, *n)),
            Act::Scroll(dx, dy) => send(&scroll_inputs(*dx, *dy)),
            Act::Type(t) => send(&text_inputs(t)),
            Act::Keys(k) => {
                self.held = k.clone();
                let ok = send(&combo_inputs(k).0);
                self.release(); // the key-ups, even when the downs failed half way
                ok
            }
            Act::Hold(k, ms) => {
                self.held = k.clone();
                let ok = send(&combo_inputs(k).0);
                // short slices so Esc or your voice let go at once
                let end = std::time::Instant::now() + std::time::Duration::from_millis((*ms).min(MAX_HOLD_MS));
                while ok && std::time::Instant::now() < end && !ESC.load(Ordering::Relaxed) && !VOICE.load(Ordering::Relaxed) { std::thread::sleep(std::time::Duration::from_millis(25)); }
                self.release();
                ok
            }
            Act::Drag(..) => {
                let (Some(from), Some(to)) = (at, to) else { return Err("a drag needs a start and an end".into()) };
                let (down, moves, _) = drag_inputs(from, to, virtual_screen());
                self.button = true; // `release` lets go even if we stop half way
                let mut ok = send(&down);
                std::thread::sleep(std::time::Duration::from_millis(80));
                for m in moves { if !ok { break; } ok = send(&[m]); std::thread::sleep(std::time::Duration::from_millis(20)); }
                self.release();
                ok
            }
        };
        if ok { Ok(()) } else { Err(fail("input")) }
    }
    fn cursor(&self) -> (i32, i32) { let mut p = Default::default(); unsafe { let _ = GetCursorPos(&mut p); } (p.x, p.y) }
    fn now_ms(&self) -> u64 { self.t0.elapsed().as_millis() as u64 }
    fn sleep(&mut self, ms: u64) { std::thread::sleep(std::time::Duration::from_millis(ms)); }
    fn halted(&mut self) -> Option<Stop> {
        if ESC.swap(false, Ordering::Relaxed) { Some(Stop::Esc) } else if VOICE.swap(false, Ordering::Relaxed) { Some(Stop::Voice) } else { None }
    }
    fn say(&mut self, line: &str) { assist::say_later(line, 6.0); }
    fn point(&mut self, at: (i32, i32), label: &str) { if crate::buddy::on() { assist::point_later(at.0, at.1, label); } }
    fn confirm(&mut self, q: &str) -> bool { ask_island(q) == Answer::Yes } // words are not a yes
    fn ask(&mut self, q: &str) -> Answer { ask_island(q) }
    fn password_focused(&self) -> bool { focused_is_password() }
    fn tool(&mut self, tool: Tool, arg: &str) -> Result<String, String> {
        crate::log::line(&format!("drive tool: {tool:?} {}", assist::short(arg, 80)));
        match tool {
            Tool::Open if crate::bridge::sys::shell_open(arg) => Ok(String::new()),
            Tool::Open => Err("Windows couldn't open that".into()),
            Tool::Launch if !assist::allowed(self.cfg, "open") => Err("Opening apps is turned off in Settings".into()),
            Tool::Launch => assist::launch_app(self.cfg, arg),
            Tool::Windows => Ok(titles_text(&top_windows().into_iter().map(|w| w.1).collect::<Vec<_>>())),
            Tool::Focus => {
                let all = top_windows();
                let i = pick_title(&all.iter().map(|w| w.1.clone()).collect::<Vec<_>>(), arg).ok_or_else(|| format!("no open window is called \"{}\"; use windows to see the titles", assist::short(arg, 40)))?;
                let hwnd = all[i].0;
                unsafe {
                    if IsIconic(hwnd).as_bool() { let _ = ShowWindow(hwnd, SW_RESTORE); }
                    let _ = SetForegroundWindow(hwnd);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if GetForegroundWindow() != hwnd { return Err("Windows kept another window in front; click it instead".into()); }
                }
                Ok(String::new())
            }
        }
    }
    fn front_blocks_input(&self) -> bool { front_blocks() }
    /// Sends a key-up for every key this drive pressed, then forgets them.
    fn release(&mut self) {
        let keys = std::mem::take(&mut self.held);
        send(&combo_inputs(&keys).1);
        if std::mem::take(&mut self.button) { send(&[mouse(0, 0, 0, MOUSEEVENTF_LEFTUP)]); }
    }
}

/// Starts a take-over for `goal` on a worker thread after asking you. Returns what the buddy says right now.
pub fn start(cfg: &Config, goal: &str) -> Result<String, String> { start_with(cfg, goal, None, None) }

/// `start`, with the words the island card shows instead of the goal, and a step that runs once you said yes (opening the app the
/// task needs, say) before the take-over begins. `Err` when it can't start.
pub fn start_with(cfg: &Config, goal: &str, shown: Option<String>, prep: Option<Box<dyn FnOnce() + Send>>) -> Result<String, String> {
    let model = assist::screen_model(cfg);
    if !crate::bridge::model_can_see(&model) { return Err("This model can't see images. Pick another one under Settings, Voice.".into()); }
    if ACTIVE.swap(true, Ordering::Relaxed) { return Err("I'm already driving. Press Esc to stop me.".into()); }
    let (cfg, goal) = (Config(cfg.0.clone()), goal.to_string());
    std::thread::spawn(move || {
        let lim = limits(&cfg);
        let mut live = Live { cfg: &cfg, model, held: vec![], button: false, t0: std::time::Instant::now(), limit_ms: lim.ms };
        ESC.store(false, Ordering::Relaxed);
        VOICE.store(false, Ordering::Relaxed);
        let q = format!("Take over your mouse and keyboard to: {}. Pictures of your screens go to the AI. Esc stops it.", assist::short(shown.as_deref().unwrap_or(&goal), 140));
        let mut failed = false;
        let line = if !live.confirm(&q) { "Okay, I won't.".to_string() } else if let Some(hook) = EscHook::install() {
            if let Some(p) = prep { p(); std::thread::sleep(std::time::Duration::from_secs(3)); } // the app needs a moment to appear
            live.t0 = std::time::Instant::now();
            pass_through(true);
            set_driving(true);
            let stop = run(&mut live, &goal, lim);
            set_driving(false);
            pass_through(false);
            exclude_own_windows(false);
            drop(hook);
            crate::log::line(&format!("drive ended: {stop:?}"));
            failed = stop.failed();
            stop.message()
        } else { failed = true; "I can't watch the Esc key, so I won't drive.".to_string() };
        ACTIVE.store(false, Ordering::Relaxed);
        if failed { assist::fail_later(&line) } else { assist::say_later(&line, 8.0) }
    });
    Ok("Checking with you first.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests that touch the shared `ASK` take turns.
    static ASK_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    use serde_json::json;
    use std::collections::VecDeque;

    fn act(v: Value) -> Option<Action> { parse_action(&v) }

    #[test]
    fn key_names_and_aliases_map_to_one_code() {
        for n in ["cmd", "Command", "win", "super", "meta", "windows"] { assert_eq!(vk(n), Some(VK_LWIN.0), "{n}"); }
        for n in ["alt", "opt", "option"] { assert_eq!(vk(n), Some(VK_MENU.0)); }
        for n in ["esc", "escape"] { assert_eq!(vk(n), Some(VK_ESCAPE.0)); }
        for n in ["return", "ENTER"] { assert_eq!(vk(n), Some(VK_RETURN.0)); }
        for n in ["del", "delete", "forwarddelete"] { assert_eq!(vk(n), Some(VK_DELETE.0)); }
        assert_eq!(vk("backspace"), Some(VK_BACK.0));
        assert_eq!((vk("pageup"), vk("pagedown")), (Some(VK_PRIOR.0), Some(VK_NEXT.0)));
        assert_eq!((vk("spacebar"), vk("space")), (Some(VK_SPACE.0), Some(VK_SPACE.0)));
        assert_eq!((vk("a"), vk("Z"), vk("7")), (Some(0x41), Some(0x5A), Some(0x37)));
        assert_eq!((vk("f1"), vk("f12"), vk("f13"), vk("f0")), (Some(VK_F1.0), Some(VK_F12.0), None, None));
        assert_eq!((vk(""), vk("banana"), vk("!")), (None, None, None));
    }

    #[test]
    fn combos_need_every_part_to_be_known() {
        assert_eq!(parse_keys("ctrl+shift+s"), Some(vec![VK_CONTROL.0, VK_SHIFT.0, 0x53]));
        assert_eq!(parse_keys(" Ctrl + S "), Some(vec![VK_CONTROL.0, 0x53]));
        assert_eq!(parse_keys("ctrl+banana"), None);
        assert_eq!(parse_keys(""), None);
        assert_eq!(parse_keys("ctrl+alt+shift+win+a"), None, "five keys at once is not a shortcut");
    }

    #[test]
    fn actions_parse_and_bad_ones_are_dropped() {
        let c = act(json!({ "type": "left_click", "x": 10.5, "y": 20, "screen": 2, "label": "Save" })).unwrap();
        assert_eq!(c, Action { act: Act::Click(Btn::Left, 1), pos: Some((10.5, 20.0)), el: None, screen: Some(2), label: "Save".into() });
        assert_eq!(act(json!({ "type": "double_click", "x": 1, "y": 2 })).unwrap().act, Act::Click(Btn::Left, 2));
        assert_eq!(act(json!({ "type": "triple_click", "x": 1, "y": 2 })).unwrap().act, Act::Click(Btn::Left, 3));
        assert_eq!(act(json!({ "type": "right_click", "x": 1, "y": 2 })).unwrap().act, Act::Click(Btn::Right, 1));
        assert!(act(json!({ "type": "left_click", "x": 1 })).is_none(), "a click needs both coordinates");
        assert!(act(json!({ "type": "screenshot" })).is_none() && act(json!({ "type": "zoom", "x": 1, "y": 1 })).is_none(), "observation actions are no-ops");
        assert!(act(json!({ "type": "format_disk" })).is_none() && act(json!({ "x": 1 })).is_none());
        assert_eq!(act(json!({ "type": "keys", "keys": "ctrl+s" })).unwrap().act, Act::Keys(vec![VK_CONTROL.0, 0x53]));
        assert!(act(json!({ "type": "keys", "keys": "ctrl+nope" })).is_none());
        assert!(act(json!({ "type": "type", "text": "" })).is_none());
        assert_eq!(act(json!({ "type": "wait", "ms": 999999 })).unwrap().act, Act::Wait(5000), "waits are capped");
        assert_eq!(act(json!({ "type": "scroll", "dy": 500, "dx": -3 })).unwrap().act, Act::Scroll(-3, 20), "scrolls are capped");
        assert_eq!(act(json!({ "type": "scroll", "dy": 2 })).unwrap().pos, None, "a scroll may stay where the pointer is");
        assert_eq!(act(json!({ "type": "ask", "question": "Delete it?" })).unwrap().act, Act::Ask("Delete it?".into()));
        let long = "x".repeat(MAX_TEXT + 100);
        assert!(matches!(act(json!({ "type": "type", "text": long })).unwrap().act, Act::Type(t) if t.chars().count() == MAX_TEXT));
    }

    #[test]
    fn a_reply_is_found_inside_fences_and_capped() {
        let r = parse_reply("```json\n{\"say\":\"I'll click Save.\",\"actions\":[{\"type\":\"left_click\",\"x\":640,\"y\":360}],\"done\":false}\n```").unwrap();
        assert_eq!((r.say.as_str(), r.actions.len(), r.done), ("I'll click Save.", 1, false));
        let many = json!({ "actions": (0..9).map(|_| json!({ "type": "wait" })).collect::<Vec<_>>() }).to_string();
        assert_eq!(parse_reply(&many).unwrap().actions.len(), MAX_ACTIONS);
        assert_eq!(parse_reply("{\"done\":true}").unwrap(), Reply { say: String::new(), actions: vec![], done: true });
        assert!(parse_reply("Sure, I'll do that!").is_none() && parse_reply("{broken").is_none() && parse_reply("[1,2]").is_none());
    }

    #[test]
    fn sending_things_are_spotted_by_their_label() {
        for l in ["Send", "Delete file", "Place order", "Pay now", "submit", "Remove from cart", "Post", "Check out / Checkout"] { assert!(risky(l), "{l}"); }
        for l in ["Save", "Open", "Compose", "Sender settings", "Postal code", "Cancel", ""] { assert!(!risky(l), "{l}"); }
    }

    #[test]
    fn coordinates_normalise_over_the_virtual_screen() {
        let v = (-1920, 0, 3840, 1080);
        assert_eq!(norm(-1920, v.0, v.2), 0);
        assert_eq!(norm(1919, v.0, v.2), 65535);
        assert_eq!(norm(0, v.0, v.2), 32776, "the primary monitor's left edge is just past the middle of a left+primary desktop");
        assert_eq!(norm(-5000, v.0, v.2), 0, "outside is held to the edge");
        assert_eq!(norm(99999, v.0, v.2), 65535);
        assert_eq!(norm(5, 0, 1), 65535, "a one pixel screen never divides by zero");
    }

    fn flags(i: &INPUT) -> u32 { unsafe { i.Anonymous.mi.dwFlags.0 } }

    #[test]
    fn mouse_inputs_are_absolute_on_the_virtual_desktop() {
        let m = move_input(-1920, 0, (-1920, 0, 3840, 1080));
        let want = (MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK).0;
        assert_eq!(flags(&m), want);
        assert_eq!(unsafe { (m.Anonymous.mi.dx, m.Anonymous.mi.dy) }, (0, 0));
        let c = click_inputs(Btn::Left, 2);
        assert_eq!(c.len(), 4);
        assert_eq!([flags(&c[0]), flags(&c[1])], [MOUSEEVENTF_LEFTDOWN.0, MOUSEEVENTF_LEFTUP.0]);
        assert_eq!(flags(&click_inputs(Btn::Right, 1)[1]), MOUSEEVENTF_RIGHTUP.0);
        let s = scroll_inputs(2, 3);
        assert_eq!((flags(&s[0]), unsafe { s[0].Anonymous.mi.mouseData } as i32), (MOUSEEVENTF_WHEEL.0, -360), "down is a negative wheel");
        assert_eq!((flags(&s[1]), unsafe { s[1].Anonymous.mi.mouseData } as i32), (MOUSEEVENTF_HWHEEL.0, 240));
        assert!(scroll_inputs(0, 0).is_empty());
    }

    #[test]
    fn text_goes_in_as_unicode_without_the_clipboard() {
        let t = text_inputs("a€\n😀");
        // a, €, Enter (down+up each) and the emoji as a surrogate pair
        assert_eq!(t.len(), 2 + 2 + 2 + 4);
        let k = |i: &INPUT| unsafe { (i.Anonymous.ki.wVk.0, i.Anonymous.ki.wScan, i.Anonymous.ki.dwFlags.0) };
        assert_eq!(k(&t[0]), (0, 'a' as u16, KEYEVENTF_UNICODE.0));
        assert_eq!(k(&t[1]), (0, 'a' as u16, (KEYEVENTF_UNICODE | KEYEVENTF_KEYUP).0));
        assert_eq!(k(&t[4]).0, VK_RETURN.0, "a newline is the Enter key");
        assert_eq!(k(&t[6]).1, 0xD83D);
        assert_eq!(k(&t[8]).1, 0xDE00);
        assert_eq!(text_inputs("a\r\nb").len(), 6, "CRLF is one Enter");
    }

    #[test]
    fn every_key_down_has_its_key_up_in_reverse() {
        let (d, u) = combo_inputs(&[VK_CONTROL.0, VK_SHIFT.0, 0x53]);
        let code = |i: &INPUT| unsafe { i.Anonymous.ki.wVk.0 };
        assert_eq!(d.iter().map(code).collect::<Vec<_>>(), [VK_CONTROL.0, VK_SHIFT.0, 0x53]);
        assert_eq!(u.iter().map(code).collect::<Vec<_>>(), [0x53, VK_SHIFT.0, VK_CONTROL.0]);
        assert!(u.iter().all(|i| unsafe { i.Anonymous.ki.dwFlags } == KEYEVENTF_KEYUP));
    }

    fn cap(x: i32, w: i32, cursor: bool) -> screen::Cap { screen::Cap { jpeg: vec![], x, y: 0, w, h: 1080, sw: 1280, sh: 720, cursor } }

    #[test]
    fn a_click_lands_on_the_monitor_it_names_even_at_negative_x() {
        let caps = [cap(0, 1920, true), cap(-1920, 1920, false)];
        let click = |screen, x| Action { act: Act::Click(Btn::Left, 1), pos: Some((x, 360.0)), el: None, screen, label: String::new() };
        assert_eq!(resolve(&caps, &click(None, 640.0)), Ok(Some((960, 540))));
        assert_eq!(resolve(&caps, &click(Some(2), 640.0)), Ok(Some((-960, 540))));
        assert!(resolve(&caps, &click(Some(3), 1.0)).unwrap_err().contains("no screen 3"));
        let type_ = Action { act: Act::Type("x".into()), pos: None, el: None, screen: None, label: String::new() };
        assert_eq!(resolve(&caps, &type_), Ok(None));
    }

    #[test]
    fn your_mouse_stops_it_only_after_it_rests() {
        let mut w = MouseWatch::new((100, 100));
        assert!(!w.took((110, 100), 0), "a small nudge is nothing");
        assert!(!w.took((400, 300), 10), "far away: you moved it, but it is still moving");
        assert!(!w.took((420, 300), 60));
        assert!(!w.took((420, 300), 150), "rested for under 150 ms since the last move");
        assert!(w.took((421, 301), 260), "rested: hand back");
        let mut w = MouseWatch::new((0, 0));
        assert!(!w.took((500, 500), 0));
        w.expect((500, 500));
        assert!(!w.took((500, 500), 1000), "the drive put it there itself");
    }

    #[test]
    fn the_island_shows_a_question_until_you_answer_it() {
        let _one = ASK_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(pending(), None);
        *ASK.lock().unwrap() = Some(("Send it?".into(), None));
        assert_eq!(pending(), Some("Send it?".into()));
        answer(true);
        assert_eq!(pending(), None, "answered: the island stops showing it");
        assert_eq!(ASK.lock().unwrap().as_ref().and_then(|a| a.1.clone()), Some(Answer::Yes));
        *ASK.lock().unwrap() = None;
        answer(false); // nothing is waiting: no panic, no stray answer
        assert!(ASK.lock().unwrap().is_none());
        *ASK.lock().unwrap() = Some(("Which Jimmy?".into(), None));
        reply("   "); // blank is not an answer
        assert_eq!(pending(), Some("Which Jimmy?".into()));
        reply("  the first one  ");
        assert_eq!(ASK.lock().unwrap().as_ref().and_then(|a| a.1.clone()), Some(Answer::Said("the first one".into())));
        *ASK.lock().unwrap() = None;
    }

    #[test]
    fn what_you_say_out_loud_answers_the_question() {
        let _one = ASK_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(spoken_answer("Yes."), Some(Answer::Yes));
        assert_eq!(spoken_answer("  Go ahead! "), Some(Answer::Yes));
        assert_eq!(spoken_answer("No, "), Some(Answer::No));
        assert_eq!(spoken_answer("Cancel"), Some(Answer::No));
        assert_eq!(spoken_answer("Yes, but pick the second Jimmy."), Some(Answer::Said("Yes, but pick the second Jimmy.".into())), "more than a yes is instructions");
        assert_eq!(spoken_answer("the one with the cartel poker game"), Some(Answer::Said("the one with the cartel poker game".into())));
        assert_eq!(spoken_answer(" ... "), None, "nothing heard");
        *ASK.lock().unwrap() = None;
        assert!(!reply_spoken("yes"), "nothing is waiting: the words go to the normal buddy instead");
        *ASK.lock().unwrap() = Some(("Which Jimmy?".into(), None));
        assert!(waiting() && !reply_spoken(""));
        assert!(reply_spoken("the second one"));
        assert!(!waiting(), "answered");
        assert_eq!(ASK.lock().unwrap().as_ref().and_then(|a| a.1.clone()), Some(Answer::Said("the second one".into())));
        *ASK.lock().unwrap() = None;
    }

    #[test]
    fn a_long_question_reaches_the_island_whole() {
        let long = "x".repeat(500);
        let a = parse_action(&json!({ "type": "ask", "question": long })).unwrap();
        assert_eq!(a.act, Act::Ask("x".repeat(500)), "not cut at 200");
        let a = parse_action(&json!({ "type": "ask", "question": "y".repeat(MAX_ASK + 50) })).unwrap();
        assert_eq!(a.act, Act::Ask("y".repeat(MAX_ASK)));
    }

    #[test]
    fn a_window_is_hidden_from_screenshots_only_while_asked() {
        unsafe {
            let h = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("capture test"), WS_POPUP, 0, 0, 10, 10, None, None, None, None).unwrap();
            let affinity = || { let mut a = 0u32; GetWindowDisplayAffinity(h, &mut a).unwrap(); a };
            assert_eq!(affinity(), 0, "a new window shows up in screenshots");
            assert!(exclude_window(h, true) && affinity() == WDA_EXCLUDEFROMCAPTURE.0);
            assert!(!exclude_window(h, true), "already hidden: nothing changed");
            assert!(exclude_own_windows(false) >= 1 && affinity() == 0, "the end of a drive shows every window again");
            assert!(exclude_own_windows(true) >= 1 && affinity() == WDA_EXCLUDEFROMCAPTURE.0, "the sweep finds this process's windows");
            exclude_own_windows(false);
            let _ = DestroyWindow(h);
        }
    }

    #[test]
    fn the_border_shows_only_between_the_yes_and_the_end() {
        assert!(!driving(), "idle at start");
        set_driving(true);
        assert!(driving());
        set_driving(false);
        assert!(!driving());
    }

    #[test]
    fn the_buddy_lets_clicks_through_only_while_driving() {
        let plain = WS_EX_TOPMOST.0 as isize | WS_EX_LAYERED.0 as isize;
        let on = with_pass_through(plain, true);
        assert_eq!(on, plain | WS_EX_TRANSPARENT.0 as isize);
        assert_eq!(with_pass_through(on, false), plain, "the other styles are untouched both ways");
        assert_eq!(with_pass_through(on, true), on);
    }

    #[test]
    fn only_stops_you_did_not_cause_count_as_failures() {
        for s in [Stop::Rounds, Stop::Time, Stop::Refused("No.".into()), Stop::Error("Broke.".into())] { assert!(s.failed(), "{s:?}"); }
        for s in [Stop::Done, Stop::Esc, Stop::Voice, Stop::UserMouse] { assert!(!s.failed(), "{s:?}"); }
    }

    #[test]
    fn every_stop_says_the_mouse_is_back() {
        for s in [Stop::Done, Stop::Esc, Stop::Voice, Stop::UserMouse, Stop::Rounds, Stop::Time, Stop::Stuck, Stop::Refused("No.".into()), Stop::Error("Broke.".into())] {
            assert!(s.message().contains("mouse"), "{s:?}");
        }
        assert_eq!(Stop::Error("Broke.".into()).message(), "Broke. The mouse is yours.");
    }

    #[test]
    fn limits_come_from_settings_and_stay_sane() {
        let c = |j: Value| Config::parse(&j.to_string());
        let l = limits(&c(json!({})));
        assert_eq!((l.rounds, l.ms), (25, 180_000));
        let l = limits(&c(json!({ "buddies": { "driveRounds": 0, "driveSecs": 1 } })));
        assert_eq!((l.rounds, l.ms), (1, 10_000));
        assert_eq!(limits(&c(json!({ "buddies": { "driveRounds": 99999 } }))).rounds, 200);
        assert!(!enabled(&c(json!({}))) && enabled(&c(json!({ "buddies": { "drive": true } }))), "off until turned on");
    }

    /// A pretend PC: a scripted model, recorded actions, a clock that only moves when asked to sleep.
    struct Fake {
        replies: VecDeque<Result<String, String>>,
        did: Vec<String>, did_at: Vec<u64>, said: Vec<String>, asked: Vec<String>, models: usize, released: bool,
        now: u64, mouse: (i32, i32), halt_after: Option<usize>, yes: bool, password: bool, blocked: bool, shaky: bool, grab_while_thinking: bool, think_ms: u64,
        /// What you type instead of yes or no.
        typed: Option<String>,
        /// Every tool the buddy ran, as "Tool arg".
        tools: Vec<String>,
        /// The elements the front window shows, a counter that makes each picture different (unless frozen), and where the pointer was sent home.
        els: Vec<El>, shots: u8, frozen: bool, home: Option<(i32, i32)>, dragged: Vec<((i32, i32), (i32, i32))>,
        /// Where each action with a position landed on the real screen.
        landed: Vec<(i32, i32)>,
    }

    fn fake(replies: &[&str]) -> Fake {
        Fake { replies: replies.iter().map(|r| Ok(r.to_string())).collect(), did: vec![], did_at: vec![], said: vec![], asked: vec![], models: 0, released: false,
               now: 0, mouse: (10, 10), halt_after: None, yes: true, password: false, blocked: false, shaky: false, grab_while_thinking: false, think_ms: 0, typed: None, tools: vec![], els: vec![], shots: 0, frozen: false, home: None, dragged: vec![], landed: vec![] }
    }

    impl Host for Fake {
        fn capture(&mut self) -> Result<Vec<screen::Cap>, String> { if !self.frozen { self.shots = self.shots.wrapping_add(1); } let mut c = cap(0, 1920, true); c.jpeg = vec![self.shots]; Ok(vec![c]) }
        fn elements(&mut self) -> Vec<El> { self.els.clone() }
        fn restore(&mut self, at: (i32, i32)) { self.home = Some(at); self.mouse = at; }
        fn model(&mut self, _: &str, _: &[String], _: &[screen::Cap], _: &str) -> Result<String, String> { self.models += 1; self.now += self.think_ms; if self.grab_while_thinking { self.mouse = (self.mouse.0 + 500, self.mouse.1 + 200); self.now += 5000; } self.replies.pop_front().unwrap_or(Ok("{\"done\":true}".into())) }
        fn act(&mut self, a: &Action, at: Option<(i32, i32)>, to: Option<(i32, i32)>) -> Result<(), String> {
            if let (Act::Drag(..), Some(f), Some(t)) = (&a.act, at, to) { self.dragged.push((f, t)); }
            self.did.push(a.describe());
            self.did_at.push(self.now);
            if let Some(p) = at { self.mouse = p; self.landed.push(p); }
            if let Some(t) = to { self.mouse = t; } // a drag ends where it lets go
            if self.shaky { self.mouse = (self.mouse.0 + 300, self.mouse.1); } // you grabbed the mouse right after
            Ok(())
        }
        fn cursor(&self) -> (i32, i32) { self.mouse }
        fn now_ms(&self) -> u64 { self.now }
        fn sleep(&mut self, ms: u64) { self.now += ms; }
        fn halted(&mut self) -> Option<Stop> { match self.halt_after { Some(0) => Some(Stop::Esc), Some(n) => { self.halt_after = Some(n - 1); None } None => None } }
        fn say(&mut self, l: &str) { self.said.push(l.into()); }
        fn point(&mut self, _: (i32, i32), _: &str) {}
        fn confirm(&mut self, q: &str) -> bool { self.asked.push(q.into()); self.yes }
        fn ask(&mut self, q: &str) -> Answer {
            match self.typed.take() { Some(t) => { self.asked.push(q.into()); Answer::Said(t) } None => if self.confirm(q) { Answer::Yes } else { Answer::No } }
        }
        fn password_focused(&self) -> bool { self.password }
        fn tool(&mut self, t: Tool, arg: &str) -> Result<String, String> { self.tools.push(format!("{t:?} {arg}")); Ok("done".into()) }
        fn front_blocks_input(&self) -> bool { self.blocked }
        fn release(&mut self) { self.released = true; }
    }

    const CLICK: &str = r#"{"say":"Clicking Save.","actions":[{"type":"left_click","x":640,"y":360,"label":"Save"}],"done":false}"#;
    const DONE: &str = r#"{"say":"All done.","actions":[],"done":true}"#;
    fn limits_(rounds: u32, ms: u64) -> Limits { Limits { rounds, ms } }

    #[test]
    fn tools_parse_and_need_their_target() {
        assert_eq!(act(json!({"type":"open","target":" https://a.com "})).map(|a| a.act), Some(Act::Tool(Tool::Open, "https://a.com".into())));
        assert_eq!(act(json!({"type":"launch","name":"Notepad"})).map(|a| a.act), Some(Act::Tool(Tool::Launch, "Notepad".into())));
        assert_eq!(act(json!({"type":"focus","title":"Chrome"})).map(|a| a.act), Some(Act::Tool(Tool::Focus, "Chrome".into())));
        assert_eq!(act(json!({"type":"windows"})).map(|a| a.act), Some(Act::Tool(Tool::Windows, String::new())));
        assert!(act(json!({"type":"open"})).is_none() && act(json!({"type":"launch","target":"  "})).is_none(), "no target, no action");
        let long = "a".repeat(MAX_TARGET + 50);
        assert!(matches!(act(json!({"type":"focus","target":long})).map(|a| a.act), Some(Act::Tool(_, t)) if t.chars().count() == MAX_TARGET));
    }

    #[test]
    fn open_takes_links_files_and_folders_only() {
        for ok in ["https://example.com/a?b=1", "HTTP://example.com", r"C:\Users\me\Pictures\cat.png", r"D:\docs", "notes.txt", "/home/me/a.pdf"] {
            assert_eq!(open_target(ok).as_deref(), Ok(ok), "{ok}");
        }
        for bad in ["", "   ", "ms-settings:network", "file:///C:/a.txt", "javascript:alert(1)", "https:/x.com", "https://", "ftp://x.com", r"\\server\share\a.txt", "//server/a",
                    r"C:\tools\setup.exe", r"C:\a\RUN.BAT", "x.ps1", "mail.docm", "a.LNK", "%USERPROFILE%", "a\"b", "a|b", "line\nbreak"] {
            assert!(open_target(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(open_target(&"a".repeat(MAX_TARGET + 1)).is_err());
    }

    #[test]
    fn a_window_is_found_by_exact_start_or_part_of_its_title() {
        let t: Vec<String> = ["Inbox - Mail", "Mail settings", "mail", "Notes"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_title(&t, "MAIL"), Some(2), "exact wins over start and part");
        assert_eq!(pick_title(&t, "mail s"), Some(1), "then one that starts with it");
        assert_eq!(pick_title(&t, "inbox"), Some(0));
        assert_eq!(pick_title(&t, "nothing"), None);
        assert_eq!(pick_title(&t, "  "), None);
        assert_eq!(titles_text(&[]), "no windows are open");
        let many: Vec<String> = (0..40).map(|i| format!("w{i}")).collect();
        assert_eq!(titles_text(&many).split("; ").count(), 25, "the list is capped");
    }

    #[test]
    fn opening_and_launching_ask_every_time_and_focus_does_not() {
        let r = r#"{"actions":[{"type":"open","target":"https://example.com"},{"type":"launch","target":"notepad"},{"type":"windows"},{"type":"focus","target":"notes"}],"done":false}"#;
        let mut f = fake(&[r, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
        assert_eq!(f.tools, ["Open https://example.com", "Launch notepad", "Windows ", "Focus notes"]);
        assert_eq!(f.asked.len(), 2, "one question each for open and launch, none for the harmless two");
        assert!(f.asked[0].contains("https://example.com") && f.asked[1].contains("notepad"), "the card names the exact target");
        let two = r#"{"actions":[{"type":"open","target":"https://a.com"},{"type":"open","target":"https://b.com"}],"done":false}"#;
        let mut f = fake(&[two, DONE]);
        run(&mut f, "x", limits_(25, 600_000));
        assert_eq!(f.asked.len(), 2, "a yes to the first link is not a yes to the second");
    }

    #[test]
    fn a_no_to_opening_runs_nothing_and_ends_the_drive() {
        let mut f = fake(&[r#"{"actions":[{"type":"open","target":"https://example.com"}],"done":false}"#]);
        f.yes = false;
        assert!(matches!(run(&mut f, "x", limits_(25, 600_000)), Stop::Refused(_)));
        assert!(f.tools.is_empty() && f.released);
    }

    #[test]
    fn a_program_is_refused_before_anyone_is_asked() {
        let mut f = fake(&[r#"{"actions":[{"type":"open","target":"C:\\tools\\setup.exe"}],"done":false}"#, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done, "the drive goes on, the model is told why");
        assert!(f.tools.is_empty() && f.asked.is_empty());
    }

    #[test]
    fn what_a_tool_returns_goes_into_the_models_history() {
        struct Spy(Fake, Vec<String>);
        impl Host for Spy {
            fn capture(&mut self) -> Result<Vec<screen::Cap>, String> { self.0.capture() }
            fn model(&mut self, g: &str, h: &[String], c: &[screen::Cap], e: &str) -> Result<String, String> { self.1 = h.to_vec(); self.0.model(g, h, c, e) }
            fn act(&mut self, a: &Action, at: Option<(i32, i32)>, to: Option<(i32, i32)>) -> Result<(), String> { self.0.act(a, at, to) }
            fn cursor(&self) -> (i32, i32) { self.0.cursor() }
            fn now_ms(&self) -> u64 { self.0.now_ms() }
            fn sleep(&mut self, ms: u64) { self.0.sleep(ms) }
            fn halted(&mut self) -> Option<Stop> { self.0.halted() }
            fn say(&mut self, l: &str) { self.0.say(l) }
            fn point(&mut self, p: (i32, i32), l: &str) { self.0.point(p, l) }
            fn confirm(&mut self, q: &str) -> bool { self.0.confirm(q) }
            fn password_focused(&self) -> bool { false }
            fn tool(&mut self, _: Tool, _: &str) -> Result<String, String> { Ok("Inbox; Notes".into()) }
            fn front_blocks_input(&self) -> bool { false }
            fn release(&mut self) { self.0.release() }
        }
        let mut s = Spy(fake(&[r#"{"actions":[{"type":"windows"}],"done":false}"#, DONE]), vec![]);
        run(&mut s, "x", limits_(25, 600_000));
        assert!(s.1.iter().any(|h| h.contains("listed the open windows: Inbox; Notes")), "{:?}", s.1);
    }

    fn el(name: &str, role: &'static str, rect: (i32, i32, i32, i32)) -> El { El { name: name.into(), role, rect } }

    #[test]
    fn a_click_may_name_an_element_and_a_drag_or_hold_parses() {
        let c = act(json!({ "type": "left_click", "el": 3, "label": "Save" })).unwrap();
        assert_eq!((c.el, c.act.clone()), (Some(3), Act::Click(Btn::Left, 1)));
        assert_eq!(c.describe(), "clicked on element 3");
        assert!(act(json!({ "type": "left_click", "el": 0 })).is_none(), "elements count from 1");
        assert_eq!(act(json!({ "type": "type", "text": "hi", "el": 2 })).unwrap().el, Some(2));
        let dr = act(json!({ "type": "drag", "x": 1, "y": 2, "to_x": 30, "to_y": 40 })).unwrap();
        assert_eq!((dr.act, dr.pos), (Act::Drag(30.0, 40.0), Some((1.0, 2.0))));
        assert!(act(json!({ "type": "drag", "el": 1, "to_x": 5, "to_y": 6 })).is_some(), "a drag may start on an element");
        assert!(act(json!({ "type": "drag", "x": 1, "y": 2 })).is_none() && act(json!({ "type": "drag", "to_x": 1, "to_y": 2 })).is_none(), "a drag needs both ends");
        assert_eq!(act(json!({ "type": "hold_key", "keys": "shift", "ms": 60000 })).unwrap().act, Act::Hold(vec![VK_SHIFT.0], MAX_HOLD_MS), "a held key is let go after at most 3 seconds");
        assert!(act(json!({ "type": "hold_key", "keys": "banana" })).is_none());
    }

    #[test]
    fn only_controls_you_can_use_are_listed_in_reading_order() {
        assert_eq!((role_of(50000), role_of(50004), role_of(50020), role_of(0)), (Some("button"), Some("edit"), None, None));
        let tidy = tidy_elements(vec![
            el("Zed", "button", (300, 100, 400, 140)), el("Save", "button", (20, 20, 120, 60)), el("Save", "button", (20, 20, 120, 60)),
            el("", "button", (500, 500, 600, 540)), el("", "edit", (20, 80, 320, 110)), el("Flat", "button", (10, 10, 10, 50)), el("Left", "button", (20, 100, 80, 140)),
        ]);
        let names: Vec<&str> = tidy.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Save", "", "Left", "Zed"], "twins, empty boxes and nameless buttons are gone; top to bottom, then left to right");
        let many: Vec<El> = (0..200).map(|i| el(&format!("b{i}"), "button", (0, i * 10, 50, i * 10 + 8))).collect();
        assert_eq!(tidy_elements(many).len(), MAX_ELEMENTS);
    }

    #[test]
    fn the_list_numbers_start_at_one_and_give_the_centre() {
        let els = tidy_elements(vec![el("Save", "button", (20, 20, 120, 60)), el("Name", "edit", (20, 80, 320, 110))]);
        assert_eq!(elements_text(&els), "[1] button \"Save\" at 70,40\n[2] edit \"Name\" at 170,95");
        assert!(elements_text(&[]).contains("use the picture"));
        let long = [el(&"x".repeat(200), "link", (0, 0, 10, 10))];
        assert!(elements_text(&long).chars().count() < 80, "a long name is cut");
    }

    #[test]
    fn an_element_beats_the_picture_and_a_wrong_number_is_reported() {
        let caps = [cap(0, 1920, true)];
        let els = [el("Save", "button", (20, 20, 120, 60)), el("Name", "edit", (20, 80, 320, 110))];
        let by = |n: usize| Action { act: Act::Click(Btn::Left, 1), pos: Some((640.0, 360.0)), el: Some(n), screen: None, label: String::new() };
        assert_eq!(target(&caps, &els, &by(2)), Ok(Some((170, 95))), "the element wins over the pixels");
        assert!(target(&caps, &els, &by(3)).unwrap_err().contains("no element 3"));
        assert!(target(&caps, &els, &by(0)).is_err());
        let pix = Action { el: None, ..by(1) };
        assert_eq!(target(&caps, &els, &pix), Ok(Some((960, 540))), "no element: the picture's pixel as before");
    }

    #[test]
    fn a_click_by_element_goes_to_its_centre_and_a_bad_number_is_told_not_fatal() {
        let r = r#"{"actions":[{"type":"left_click","el":2,"label":"Name"}],"done":false}"#;
        let mut f = fake(&[r, DONE]);
        f.els = vec![el("Save", "button", (20, 20, 120, 60)), el("Name", "edit", (20, 80, 320, 110))];
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
        assert_eq!(f.did, ["clicked on element 2"]);
        let bad = r#"{"actions":[{"type":"left_click","el":9,"label":"Name"}],"done":false}"#;
        let mut f = fake(&[bad, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done, "the drive goes on and the model is told");
        assert!(f.did.is_empty());
    }

    #[test]
    fn the_same_picture_after_actions_is_noticed_and_three_times_ends_it() {
        let a = [screen::Cap { jpeg: vec![1, 2, 3], ..cap(0, 1920, true) }];
        let b = [screen::Cap { jpeg: vec![1, 2, 4], ..cap(0, 1920, true) }];
        assert_eq!(screen_sig(&a), screen_sig(&a));
        assert_ne!(screen_sig(&a), screen_sig(&b));
        let mut f = fake(&[CLICK, CLICK, CLICK, CLICK, CLICK]);
        f.frozen = true;
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Stuck);
        assert_eq!((f.did.len(), f.models, f.released), (3, 3, true), "stops before a fourth try");
        assert!(Stop::Stuck.failed() && Stop::Stuck.message().contains("mouse"));
        // a changing screen is not stuck, and neither is waiting or listing windows on a still screen
        let mut f = fake(&[CLICK, CLICK, CLICK, CLICK, CLICK, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
        let idle = r#"{"actions":[{"type":"windows"},{"type":"wait","ms":10}],"done":false}"#;
        let mut f = fake(&[CLICK, idle, idle, idle, idle, idle, DONE]);
        f.frozen = true;
        f.shots = 1;
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
    }

    #[test]
    fn the_model_is_told_when_its_actions_did_nothing() {
        struct Spy(Fake, Vec<Vec<String>>);
        impl Host for Spy {
            fn capture(&mut self) -> Result<Vec<screen::Cap>, String> { self.0.capture() }
            fn model(&mut self, g: &str, h: &[String], c: &[screen::Cap], e: &str) -> Result<String, String> { self.1.push(h.to_vec()); self.0.model(g, h, c, e) }
            fn act(&mut self, a: &Action, at: Option<(i32, i32)>, to: Option<(i32, i32)>) -> Result<(), String> { self.0.act(a, at, to) }
            fn cursor(&self) -> (i32, i32) { self.0.cursor() }
            fn now_ms(&self) -> u64 { self.0.now_ms() }
            fn sleep(&mut self, ms: u64) { self.0.sleep(ms) }
            fn halted(&mut self) -> Option<Stop> { self.0.halted() }
            fn say(&mut self, l: &str) { self.0.say(l) }
            fn point(&mut self, p: (i32, i32), l: &str) { self.0.point(p, l) }
            fn confirm(&mut self, q: &str) -> bool { self.0.confirm(q) }
            fn password_focused(&self) -> bool { false }
            fn tool(&mut self, t: Tool, a: &str) -> Result<String, String> { self.0.tool(t, a) }
            fn front_blocks_input(&self) -> bool { false }
            fn release(&mut self) { self.0.release() }
        }
        let mut f = fake(&[CLICK, CLICK, DONE]);
        f.frozen = true;
        let mut s = Spy(f, vec![]);
        run(&mut s, "x", limits_(25, 600_000));
        assert!(!s.1[0].iter().any(|h| h.contains("exactly the same")), "before the first try there is nothing to compare");
        assert!(s.1[1].iter().any(|h| h.contains("exactly the same")), "{:?}", s.1[1]);
    }

    #[test]
    fn a_drag_presses_moves_in_steps_and_lets_go() {
        let v = (0, 0, 1920, 1080);
        let (down, moves, up) = drag_inputs((100, 100), (900, 500), v);
        assert_eq!((down.len(), moves.len(), up.len()), (2, 8, 1));
        assert_eq!(flags(&down[1]) & MOUSEEVENTF_LEFTDOWN.0, MOUSEEVENTF_LEFTDOWN.0);
        assert_eq!(flags(&up[0]) & MOUSEEVENTF_LEFTUP.0, MOUSEEVENTF_LEFTUP.0);
        let at = |i: &INPUT| unsafe { (i.Anonymous.mi.dx, i.Anonymous.mi.dy) };
        let end = move_input(900, 500, v);
        assert_eq!(at(moves.last().unwrap()), at(&end), "the last step is the end point");
        assert!(moves.windows(2).all(|w| at(&w[0]).0 <= at(&w[1]).0), "it moves one way");
        let drag = r#"{"actions":[{"type":"drag","x":320,"y":180,"to_x":640,"to_y":360,"label":"File"}],"done":false}"#;
        let mut f = fake(&[drag, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
        assert_eq!(f.dragged, [((480, 270), (960, 540))], "both ends are mapped through the monitor");
        let risky = r#"{"actions":[{"type":"drag","x":1,"y":1,"to_x":2,"to_y":2,"label":"Delete files"}],"done":false}"#;
        let mut f = fake(&[risky, DONE]);
        f.yes = false;
        assert!(matches!(run(&mut f, "x", limits_(25, 600_000)), Stop::Refused(_)));
        assert!(f.dragged.is_empty() && f.asked[0].contains("Delete files"), "a risky drag asks like a risky click");
    }

    #[test]
    fn a_held_key_runs_and_is_never_typed_into_a_password() {
        let hold = r#"{"actions":[{"type":"hold_key","keys":"shift","ms":900}],"done":false}"#;
        let mut f = fake(&[hold, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
        assert_eq!(f.did, ["held 1 keys for 900 ms"]);
        let mut f = fake(&[hold, DONE]);
        f.password = true;
        assert!(matches!(run(&mut f, "x", limits_(25, 600_000)), Stop::Refused(_)));
        assert!(f.did.is_empty());
    }

    #[test]
    fn the_pointer_goes_home_unless_you_have_it() {
        let mut f = fake(&[CLICK, DONE]);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Done);
        assert_eq!((f.home, f.mouse), (Some((10, 10)), (10, 10)), "it started at 10,10");
        let mut f = fake(&[DONE]);
        run(&mut f, "x", limits_(25, 600_000));
        assert_eq!(f.home, None, "nothing moved, nothing to put back");
        let mut f = fake(&[CLICK, CLICK, CLICK]);
        f.halt_after = Some(3);
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::Esc);
        assert_eq!(f.home, None, "after Esc the pointer stays where you are");
        let mut f = fake(&[CLICK, CLICK]);
        f.shaky = true;
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::UserMouse);
        assert_eq!(f.home, None, "and when you grabbed it");
        let mut f = fake(&[CLICK, CLICK]);
        assert_eq!(run(&mut f, "x", limits_(1, 600_000)), Stop::Rounds);
        assert_eq!(f.home, Some((10, 10)), "a failed run still puts it back");
    }

    #[test]
    fn the_prompt_teaches_elements_drag_and_hold() {
        for w in ["\"el\"", "drag", "hold_key", "front window"] { assert!(SYSTEM.contains(w), "{w}"); }
    }

    #[test]
    fn it_works_a_task_then_hands_back() {
        let mut f = fake(&[CLICK, DONE]);
        assert_eq!(run(&mut f, "click save", limits_(25, 60_000)), Stop::Done);
        assert_eq!(f.did, ["clicked at 640,360"]);
        assert_eq!(f.landed, [(960, 540)], "the click was mapped to the real screen");
        assert_eq!(f.said[0], "Hands off the mouse while I drive. Esc stops me.", "the first words ask for your hands off");
        assert!(f.said.contains(&"Clicking Save.".to_string()) && f.released && f.models == 2);
    }

    #[test]
    fn esc_stops_it_before_the_next_action() {
        let mut f = fake(&[CLICK, CLICK, CLICK]);
        f.halt_after = Some(3);
        assert_eq!(run(&mut f, "x", limits_(25, 60_000)), Stop::Esc);
        assert!(f.did.len() < 3 && f.released, "teardown always runs");
    }

    #[test]
    fn it_stops_at_the_round_and_time_limits() {
        let mut f = fake(&[CLICK, CLICK, CLICK, CLICK]);
        assert_eq!(run(&mut f, "x", limits_(2, 600_000)), Stop::Rounds);
        assert_eq!((f.models, f.released), (2, true));
        let mut f = fake(&[CLICK, CLICK, CLICK, CLICK]);
        assert_eq!(run(&mut f, "x", limits_(25, 1500)), Stop::Time);
        assert!(f.released);
    }

    #[test]
    fn grabbing_the_mouse_hands_it_back() {
        let mut f = fake(&[CLICK, CLICK, CLICK]);
        f.shaky = true;
        assert_eq!(run(&mut f, "x", limits_(25, 600_000)), Stop::UserMouse);
        assert_eq!(f.did.len(), 1, "no second click once you took over");
        assert!(f.released);
    }

    #[test]
    fn typing_waits_for_the_click_to_move_focus() {
        let mut f = fake(&[r#"{"actions":[{"type":"left_click","x":1,"y":1,"label":"Name box"},{"type":"type","text":"hi"}]}"#, DONE]);
        assert_eq!(run(&mut f, "x", limits_(5, 600_000)), Stop::Done);
        assert_eq!(f.did.len(), 2);
        assert!(f.did_at[1] - f.did_at[0] >= SETTLE_MS, "keys sent at once after a click go to the old control: {:?}", f.did_at);
    }

    #[test]
    fn moving_the_mouse_while_the_model_thinks_is_not_taking_over() {
        let mut f = fake(&[CLICK, DONE]);
        f.grab_while_thinking = true; // your hand was still on the mouse after clicking yes
        assert_eq!(run(&mut f, "x", limits_(5, 600_000)), Stop::Done);
        assert_eq!(f.did.len(), 1, "the click went ahead");
    }

    #[test]
    fn it_never_types_into_a_password_field() {
        let mut f = fake(&[r#"{"actions":[{"type":"type","text":"hunter2"}]}"#]);
        f.password = true;
        let s = run(&mut f, "log in", limits_(5, 60_000));
        assert!(matches!(&s, Stop::Refused(w) if w.contains("password")), "{s:?}");
        assert!(f.did.is_empty());
        let mut f = fake(&[r#"{"actions":[{"type":"type","text":"hello"}]}"#, DONE]);
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Done, "other fields are fine");
        assert_eq!(f.did, ["typed 5 characters"]);
    }

    #[test]
    fn it_stops_in_front_of_an_administrator_window() {
        let mut f = fake(&[CLICK]);
        f.blocked = true;
        assert!(matches!(run(&mut f, "x", limits_(5, 60_000)), Stop::Refused(w) if w.contains("administrator")));
        assert!(f.did.is_empty() && f.released);
    }

    #[test]
    fn a_risky_click_needs_its_own_yes() {
        let send = r#"{"actions":[{"type":"left_click","x":1,"y":1,"label":"Send"}]}"#;
        let mut f = fake(&[send, DONE]);
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Done);
        assert_eq!(f.asked.len(), 1, "the app asked even though the model didn't");
        let mut f = fake(&[send]);
        f.yes = false;
        assert!(matches!(run(&mut f, "x", limits_(5, 60_000)), Stop::Refused(_)));
        assert!(f.did.is_empty(), "a no means nothing is clicked");
        let asked = r#"{"actions":[{"type":"ask","question":"Send it?"},{"type":"left_click","x":1,"y":1,"label":"Send"}]}"#;
        let mut f = fake(&[asked, DONE]);
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Done);
        assert_eq!(f.asked, ["Send it?"], "one yes from the model's own ask covers that click");
    }

    #[test]
    fn typed_instructions_reach_the_model_and_are_not_a_yes() {
        let asked = r#"{"actions":[{"type":"ask","question":"Which Jimmy?"},{"type":"left_click","x":1,"y":1,"label":"Send"}],"done":true}"#;
        let send = r#"{"actions":[{"type":"left_click","x":1,"y":1,"label":"Send"}]}"#;
        let mut f = fake(&[asked, send, DONE]);
        f.typed = Some("jimmy_32193".into());
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Done);
        assert_eq!(f.models, 3, "the round ended at the reply, and done in the same reply did not end the task");
        assert_eq!(f.asked, ["Which Jimmy?", "The buddy wants to click \"Send\". Allow it?"], "the reply did not approve the risky click");
        assert_eq!(f.did, ["clicked at 1,1"], "the first click was dropped, the second went through after its own yes");
    }

    #[test]
    fn esc_during_a_slow_model_call_still_stops() {
        let mut f = fake(&[]);
        f.replies.push_back(Err("stopped while waiting for the AI".into()));
        f.halt_after = Some(1); // the check at the top of the round passes, the one after the failed call sees Esc
        assert_eq!(run(&mut f, "x", limits_(5, 600_000)), Stop::Esc);
        let mut f = fake(&[]);
        f.replies.push_back(Err("stopped while waiting for the AI".into()));
        f.think_ms = 700_000; // the clock ran out while waiting
        assert_eq!(run(&mut f, "x", limits_(5, 600_000)), Stop::Time);
    }

    #[test]
    fn bad_replies_get_one_retry_and_a_missing_screen_is_reported_back() {
        let mut f = fake(&["I think so!", DONE]);
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Done, "a second try works");
        let mut f = fake(&["nope", "still nope"]);
        assert!(matches!(run(&mut f, "x", limits_(5, 60_000)), Stop::Error(_)));
        let mut f = fake(&[r#"{"actions":[{"type":"left_click","x":1,"y":1,"screen":4}]}"#, DONE]);
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Done);
        assert!(f.did.is_empty(), "a click on a screen that isn't there is skipped");
        let mut f = fake(&[]);
        f.replies.push_back(Err("no brain".into()));
        assert_eq!(run(&mut f, "x", limits_(5, 60_000)), Stop::Error("no brain".into()));
        assert!(f.released);
    }
}
