//! Global hotkeys. zorui owns them (a low-level keyboard hook) instead of GlazeWM's config, so a key
//! edited in Settings works at once. Window commands still go to GlazeWM over its IPC (`glaze::command`).

use crate::conf::Config;
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, mpsc, Mutex, OnceLock};

pub const WIN: u32 = 1;
pub const ALT: u32 = 2;
pub const CTRL: u32 = 4;
pub const SHIFT: u32 = 8;

/// What a key does.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    /// A command for the app, as `zorui.exe --open` sends it ("open settings", "buddy listen", "restart").
    Zor(String),
    /// `zorui.exe --script <args>`, no window.
    Script(String),
    /// A program from PATH or App Paths ("wt", "explorer ms-screenclip:").
    Exec(String),
    /// A GlazeWM command ("focus --direction left").
    Wm(String),
}

pub struct Bind {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    /// Default keys; "{m}" is the modifier (Alt or Win), "{o}" the overview key.
    pub keys: &'static [&'static str],
    pub act: fn() -> Do,
}

macro_rules! b {
    ($id:expr, $g:expr, $l:expr, [$($k:expr),+], $v:ident, $s:expr) => {
        Bind { id: $id, group: $g, label: $l, keys: &[$($k),+], act: || Do::$v($s.into()) }
    };
}

/// Every key you can change in Settings. Mod+1…0 (workspaces) are fixed and added by `resolve`.
pub const BINDS: &[Bind] = &[
    b!("sidebar-left", "Shell", "Left sidebar: home, AI, translator, anime, football", ["{m}+A"], Zor, "open sidebar-left"),
    b!("sidebar-right", "Shell", "Right sidebar: toggles, notifications, calendar", ["{m}+N"], Zor, "open sidebar-right"),
    b!("settings", "Shell", "Settings", ["{m}+I"], Zor, "open settings"),
    b!("keybinds", "Shell", "This keybinds list", ["{m}+/"], Zor, "open keybinds"),
    b!("overview", "Shell", "Overview", ["{o}"], Zor, "open overview"),
    b!("wallpaper", "Shell", "Pick a wallpaper and regenerate colors", ["{m}+Ctrl+T"], Script, "switchwall"),
    b!("darkmode", "Shell", "Dark / light", ["{m}+Ctrl+D"], Script, "switchwall -ToggleMode"),
    b!("gamemode", "Shell", "Game mode: free RAM (press again to restore)", ["{m}+Shift+G"], Script, "toggles -Action gamemode"),
    b!("restart", "Shell", "Restart the bar and reload GlazeWM", ["{m}+Ctrl+R"], Zor, "restart"),
    b!("buddy", "Shell", "Talk to the primary buddy", ["{m}+B"], Zor, "buddy listen"),
    b!("terminal", "Apps", "Terminal", ["{m}+Enter", "{m}+T"], Exec, "wt"),
    b!("files", "Apps", "File Explorer", ["{m}+E"], Exec, "explorer"),
    b!("browser", "Apps", "Edge", ["{m}+W"], Exec, "msedge"),
    b!("code", "Apps", "VS Code", ["{m}+C"], Exec, "code"),
    b!("snip", "Apps", "Screen snip", ["{m}+Shift+S"], Exec, "explorer ms-screenclip:"),
    b!("osk", "Apps", "On-screen keyboard", ["{m}+K"], Exec, "osk"),
    b!("focus-left", "Windows", "Focus the window to the left", ["{m}+Left"], Wm, "focus --direction left"),
    b!("focus-right", "Windows", "Focus the window to the right", ["{m}+Right"], Wm, "focus --direction right"),
    b!("focus-up", "Windows", "Focus the window above", ["{m}+Up"], Wm, "focus --direction up"),
    b!("focus-down", "Windows", "Focus the window below", ["{m}+Down"], Wm, "focus --direction down"),
    b!("move-left", "Windows", "Move the window left (also to another monitor)", ["{m}+Shift+Left"], Wm, "move --direction left"),
    b!("move-right", "Windows", "Move the window right", ["{m}+Shift+Right"], Wm, "move --direction right"),
    b!("move-up", "Windows", "Move the window up", ["{m}+Shift+Up"], Wm, "move --direction up"),
    b!("move-down", "Windows", "Move the window down", ["{m}+Shift+Down"], Wm, "move --direction down"),
    b!("split", "Windows", "Next window opens beside / below", ["{m}+V"], Wm, "toggle-tiling-direction"),
    b!("resize-mode", "Windows", "Resize mode: arrows to resize, Esc or Enter to finish", ["{m}+R"], Wm, "wm-enable-binding-mode --name resize"),
    b!("narrower", "Windows", "Narrower", ["{m}+-"], Wm, "resize --width -3%"),
    b!("wider", "Windows", "Wider", ["{m}+="], Wm, "resize --width +3%"),
    b!("close", "Windows", "Close", ["{m}+Q"], Wm, "close"),
    b!("fullscreen", "Windows", "Fullscreen", ["{m}+F"], Wm, "toggle-fullscreen"),
    b!("float", "Windows", "Float / tile", ["{m}+Shift+Space"], Wm, "toggle-floating --centered"),
    b!("tile", "Windows", "Tile a floating window", ["{m}+Shift+T"], Wm, "toggle-tiling"),
    b!("minimize", "Windows", "Minimize", ["{m}+M"], Wm, "toggle-minimized"),
    b!("ws-prev", "Workspaces", "Previous workspace", ["{m}+Ctrl+Left", "{m}+PgUp"], Wm, "focus --prev-active-workspace"),
    b!("ws-next", "Workspaces", "Next workspace", ["{m}+Ctrl+Right", "{m}+PgDn"], Wm, "focus --next-active-workspace"),
    b!("ws-mon-left", "Workspaces", "Move the workspace to the monitor on the left", ["{m}+Ctrl+Shift+Left"], Wm, "move-workspace --direction left"),
    b!("ws-mon-right", "Workspaces", "Move the workspace to the monitor on the right", ["{m}+Ctrl+Shift+Right"], Wm, "move-workspace --direction right"),
    b!("pause", "GlazeWM", "Pause tiling (press again to resume)", ["{m}+Shift+P"], Wm, "wm-toggle-pause"),
    b!("redraw", "GlazeWM", "Redraw all windows", ["{m}+Shift+W"], Wm, "wm-redraw"),
    b!("quit-wm", "GlazeWM", "Quit GlazeWM", ["{m}+Shift+E"], Wm, "wm-exit"),
];

/// Group names in the order the page shows them, with their icon.
pub const GROUPS: &[(&str, &str)] = &[("dashboard", "Shell"), ("apps", "Apps"), ("view_quilt", "Windows"), ("grid_view", "Workspaces"), ("build", "GlazeWM")];

/// The modifier name for a modKey setting ("lwin" or "alt").
pub fn mod_name(cfg: &Config) -> &'static str { if cfg.str("modKey") == "lwin" { "Win" } else { "Alt" } }

/// A default key with "{m}" and "{o}" filled in.
pub fn fill(key: &str, m: &str) -> String {
    let o = if m == "Win" { "Win+Tab" } else { "Alt+`" }; // Alt+Tab stays with Windows
    key.replace("{m}", m).replace("{o}", o)
}

/// The keys a bind answers to: your override when there is one, else the default.
pub fn keys_of(cfg: &Config, b: &Bind) -> Vec<String> {
    let own = cfg.strs(&format!("keybinds.{}", b.id));
    if own.iter().any(|k| parse(k).is_some()) { return own; }
    let m = mod_name(cfg);
    b.keys.iter().map(|k| fill(k, m)).collect()
}

/// Whether `b` has your own keys instead of the defaults.
pub fn changed(cfg: &Config, b: &Bind) -> bool { !cfg.strs(&format!("keybinds.{}", b.id)).is_empty() }

/// Every combo and what it does. A combo two binds share goes to the first.
pub fn resolve(cfg: &Config) -> Vec<(u32, u32, Do)> {
    let m = mod_name(cfg);
    let mut out: Vec<(u32, u32, Do)> = vec![];
    let mut add = |k: &str, d: Do| if let Some((mods, vk)) = parse(k) { if !out.iter().any(|o| o.0 == mods && o.1 == vk) { out.push((mods, vk, d)); } };
    for b in BINDS { for k in keys_of(cfg, b) { add(&k, (b.act)()); } }
    for n in 1..=10 {
        let key = n % 10;
        add(&format!("{m}+{key}"), Do::Wm(format!("focus --workspace {n}")));
        add(&format!("{m}+Shift+{key}"), Do::Wm(format!("move --workspace {n}")));
    }
    out
}

/// What already uses `k`, if anything: the label of another bind, or "a workspace key".
pub fn taken_by(cfg: &Config, id: &str, k: &str) -> Option<String> {
    let want = parse(k)?;
    let m = mod_name(cfg);
    for b in BINDS.iter().filter(|b| b.id != id) {
        if keys_of(cfg, b).iter().any(|o| parse(o) == Some(want)) { return Some(b.label.to_string()); }
    }
    (1..=10).any(|n| [parse(&format!("{m}+{}", n % 10)), parse(&format!("{m}+Shift+{}", n % 10))].contains(&Some(want))).then(|| "a workspace key".into())
}

/// The keys of `id` after replacing key number `i` with `new` (a list of one more when `i` is past the end).
pub fn with_key(cfg: &Config, id: &str, i: usize, new: &str) -> Vec<String> {
    let mut v = BINDS.iter().find(|b| b.id == id).map(|b| keys_of(cfg, b)).unwrap_or_default();
    if i < v.len() { v[i] = new.to_string(); } else { v.push(new.to_string()); }
    v
}

// ---------------------------------------------------------------- key names

const NAMED: &[(&str, u32)] = &[
    ("Enter", 0x0D), ("Space", 0x20), ("Tab", 0x09), ("Esc", 0x1B), ("Backspace", 0x08), ("Left", 0x25), ("Up", 0x26), ("Right", 0x27), ("Down", 0x28),
    ("PgUp", 0x21), ("PgDn", 0x22), ("End", 0x23), ("Home", 0x24), ("Insert", 0x2D), ("Delete", 0x2E),
    (";", 0xBA), ("=", 0xBB), (",", 0xBC), ("-", 0xBD), (".", 0xBE), ("/", 0xBF), ("`", 0xC0), ("[", 0xDB), ("\\", 0xDC), ("]", 0xDD), ("'", 0xDE),
];

fn key_vk(name: &str) -> Option<u32> {
    if let Some((_, vk)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) { return Some(*vk); }
    let mut cs = name.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase() as u32),
        (Some('f' | 'F'), Some(_)) => name[1..].parse::<u32>().ok().filter(|n| (1..=24).contains(n)).map(|n| 0x6F + n),
        _ => None,
    }
}

fn vk_key(vk: u32) -> Option<String> {
    if let Some((n, _)) = NAMED.iter().find(|(_, v)| *v == vk) { return Some(n.to_string()); }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        _ => None,
    }
}

/// "Alt+Shift+Left" as (modifier bits, virtual key). A combo needs at least one modifier.
pub fn parse(s: &str) -> Option<(u32, u32)> {
    let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let key = parts.pop().filter(|k| !k.is_empty())?;
    let mut mods = 0;
    for p in parts {
        mods |= match p.to_ascii_lowercase().as_str() { "win" | "lwin" | "super" => WIN, "alt" => ALT, "ctrl" | "control" => CTRL, "shift" => SHIFT, _ => return None };
    }
    if mods == 0 { return None; }
    Some((mods, key_vk(key)?))
}

/// The text for a combo, the inverse of `parse`.
pub fn format(mods: u32, vk: u32) -> Option<String> {
    let mut s = String::new();
    for (bit, n) in [(WIN, "Win"), (CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift")] { if mods & bit != 0 { s += n; s.push('+'); } }
    Some(s + &vk_key(vk)?)
}

// ---------------------------------------------------------------- the hook's logic

fn mod_bit(vk: u32) -> u32 {
    match vk { 0x5B | 0x5C => WIN, 0x12 | 0xA4 | 0xA5 => ALT, 0x11 | 0xA2 | 0xA3 => CTRL, 0x10 | 0xA0 | 0xA1 => SHIFT, _ => 0 }
}

#[derive(Debug, PartialEq)]
pub enum Out {
    Pass,
    Swallow,
    /// Index into the combo list.
    Fire(usize),
    /// Recording: the combo pressed, or None for Esc (cancel).
    Record(Option<(u32, u32)>),
}

/// Which modifiers are down and which keys the hook ate (so their key-up and repeats are eaten too).
#[derive(Default)]
pub struct Tracker { held: Vec<u32>, eaten: Vec<u32> }

impl Tracker {
    pub const fn new() -> Tracker { Tracker { held: Vec::new(), eaten: Vec::new() } }

    /// The keys the hook ate that are still down.
    pub fn eaten(&self) -> &[u32] { &self.eaten }

    pub fn mods(&self) -> u32 { self.held.iter().fold(0, |a, v| a | mod_bit(*v)) }

    /// Forgets the modifiers that are no longer down. Their key-up can be lost (secure desktop, a dropped event).
    pub fn drop_released(&mut self, is_down: impl Fn(u32) -> bool) { self.held.retain(|h| is_down(*h)); }

    pub fn event(&mut self, down: bool, vk: u32, combos: &[(u32, u32)], recording: bool) -> Out {
        if mod_bit(vk) != 0 {
            self.held.retain(|h| *h != vk);
            if down { self.held.push(vk); }
            return Out::Pass;
        }
        if !down { return match self.eaten.iter().position(|e| *e == vk) { Some(i) => { self.eaten.remove(i); Out::Swallow } None => Out::Pass }; }
        if self.eaten.contains(&vk) { return Out::Swallow; } // auto-repeat
        let mods = self.mods();
        if recording {
            self.eaten.push(vk);
            return Out::Record(if vk == 0x1B && mods == 0 { None } else { Some((mods, vk)) });
        }
        match combos.iter().position(|c| *c == (mods, vk)) {
            Some(i) => { self.eaten.push(vk); Out::Fire(i) }
            None => Out::Pass,
        }
    }
}

// ---------------------------------------------------------------- the hook

struct Shared { combos: Vec<(u32, u32, Do)>, tracker: Tracker }

static SHARED: Mutex<Shared> = Mutex::new(Shared { combos: Vec::new(), tracker: Tracker::new() });
static RECORDING: AtomicBool = AtomicBool::new(false);
/// When a recording gives up (ms since the epoch), so a forgotten one can't eat keys for long.
static UNTIL: AtomicU64 = AtomicU64::new(0);
const RECORD_MS: u64 = 15_000;

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) }
static RECORDED: Mutex<Option<Option<String>>> = Mutex::new(None);
static TX: OnceLock<Mutex<mpsc::Sender<Do>>> = OnceLock::new();
static HELD_TX: OnceLock<Mutex<mpsc::Sender<Vec<u32>>>> = OnceLock::new();

/// Where the hook lists the keys it ate while they are down. Windows never sees those keys, so a program that
/// waits for a release (the buddy's Hold dictation runs in its own process) reads them here.
fn held_file() -> std::path::PathBuf { std::env::temp_dir().join("zorui-keys-held") }

fn held_text(keys: &[u32]) -> String { keys.iter().map(u32::to_string).collect::<Vec<_>>().join(",") }

fn parse_held(text: &str) -> Vec<u16> { text.split(',').filter_map(|k| k.trim().parse().ok()).collect() }

/// Keys the hook ate that are still down.
pub fn eaten_held() -> Vec<u16> { parse_held(&std::fs::read_to_string(held_file()).unwrap_or_default()) }


/// Replaces the combos the hook answers to. Starts the hook and its worker the first time.
pub fn apply(cfg: &Config) {
    if let Ok(mut s) = SHARED.lock() { s.combos = resolve(cfg); }
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Do>();
        std::thread::spawn(move || for d in rx { run(d); });
        let (htx, hrx) = mpsc::channel::<Vec<u32>>();
        let _ = HELD_TX.set(Mutex::new(htx));
        let _ = std::fs::remove_file(held_file()); // a crash while a key was down leaves it behind
        std::thread::spawn(move || for k in hrx {
            if k.is_empty() { let _ = std::fs::remove_file(held_file()); } else { let _ = std::fs::write(held_file(), held_text(&k)); }
        });
        std::thread::spawn(hook_thread);
        std::thread::spawn(migrate_glaze);
        Mutex::new(tx)
    });
}

/// While true the hook eats the next combo and hands it to `take_recorded` instead of acting on it.
pub fn record(on: bool) {
    RECORDING.store(on, Ordering::SeqCst);
    UNTIL.store(now_ms() + RECORD_MS, Ordering::SeqCst);
    if on { if let Ok(mut r) = RECORDED.lock() { *r = None; } }
}

/// Whether a recording is waiting for a combo.
pub fn recording() -> bool { RECORDING.load(Ordering::SeqCst) && now_ms() < UNTIL.load(Ordering::SeqCst) }

/// The combo recorded since `record(true)`: Some(Some("Alt+K")) a combo, Some(None) cancelled, None nothing yet.
pub fn take_recorded() -> Option<Option<String>> { RECORDED.lock().ok().and_then(|mut r| r.take()) }

fn hook_thread() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, SetWindowsHookExW, MSG, WH_KEYBOARD_LL};
    unsafe {
        let Ok(_h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) else { crate::log::line("keymap: couldn't install the keyboard hook"); return };
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() { DispatchMessageW(&m); }
    }
}

unsafe extern "system" fn hook(code: i32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{Foundation::LRESULT, UI::Input::KeyboardAndMouse::GetAsyncKeyState, UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_SYSKEYDOWN}};
    if code >= 0 {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        if k.flags.0 & LLKHF_INJECTED.0 == 0 {
            let down = matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            // try_lock: never make the whole keyboard wait for another thread
            if let Ok(mut s) = SHARED.try_lock() {
                // the real state, since a key-up the hook missed would leave Win "held" (Win+Enter after unlock)
                s.tracker.drop_released(|vk| unsafe { GetAsyncKeyState(vk as i32) } < 0);
                let combos: Vec<(u32, u32)> = s.combos.iter().map(|c| (c.0, c.1)).collect();
                let mods = s.tracker.mods();
                let before = s.tracker.eaten().len();
                let out = s.tracker.event(down, k.vkCode, &combos, recording());
                if s.tracker.eaten().len() != before { if let Some(Ok(tx)) = HELD_TX.get().map(|t| t.lock()) { let _ = tx.send(s.tracker.eaten().to_vec()); } }
                match out {
                    Out::Pass => {}
                    Out::Swallow => return LRESULT(1),
                    Out::Fire(i) => {
                        if let Some(Ok(tx)) = TX.get().map(|t| t.lock()) { let _ = tx.send(s.combos[i].2.clone()); }
                        quiet(mods);
                        return LRESULT(1);
                    }
                    Out::Record(r) => {
                        // a key with no name (a media key) counts as a cancel
                        let text = r.and_then(|(m, v)| format(m, v));
                        if let Ok(mut o) = RECORDED.lock() { *o = Some(text); }
                        RECORDING.store(false, Ordering::SeqCst);
                        crate::app::post(Box::new(|_| {})); // wakes the app so Settings sees it
                        quiet(mods);
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// Taps an unused key so releasing Win or Alt after an eaten combo doesn't open the Start menu or a window's menu bar.
fn quiet(mods: u32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYBD_EVENT_FLAGS, VIRTUAL_KEY};
    if mods & (WIN | ALT) == 0 { return; }
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: flags, ..Default::default() } } };
    let ev = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(&ev, std::mem::size_of::<INPUT>() as i32); }
}

fn run(d: Do) {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    match d {
        Do::Zor(c) => crate::app::post(Box::new(move |cx| cx.actions.push(crate::app::Action::Command(c)))),
        Do::Wm(c) => crate::glaze::command(&c),
        Do::Script(a) => {
            let Ok(exe) = std::env::current_exe() else { return };
            let _ = std::process::Command::new(exe).arg("--script").args(a.split_whitespace()).creation_flags(NO_WINDOW).spawn();
        }
        Do::Exec(c) => { let _ = std::process::Command::new("cmd").args(["/c", "start", ""]).args(c.split_whitespace()).creation_flags(NO_WINDOW).spawn(); }
    }
}

/// GlazeWM's config without the keybindings the installer wrote, so a key isn't handled twice. Only a config
/// that has them (it opens the left sidebar) is touched, and the resize mode's own keys stay.
pub fn strip_glaze_keys(yaml: &str) -> Option<String> {
    if !yaml.contains("zorui.exe --open sidebar-left") { return None; }
    let mut out = String::new();
    let mut skipping = false;
    for l in yaml.split_inclusive('\n') {
        let top = !l.starts_with([' ', '\t', '#', '\r', '\n']);
        if l.starts_with("keybindings:") { skipping = true; out.push_str("keybindings: []\n"); continue; }
        if skipping && top { skipping = false; }
        if !skipping { out.push_str(l); }
    }
    Some(out)
}

/// Does that to the user's GlazeWM config, keeping a copy, and reloads GlazeWM once it is reachable.
fn migrate_glaze() {
    let path = crate::conf::home().join(r".glzr\glazewm\config.yaml");
    let Ok(old) = std::fs::read_to_string(&path) else { return };
    let Some(new) = strip_glaze_keys(&old) else { return };
    let _ = std::fs::write(path.with_extension("yaml.bak-keys"), &old);
    if std::fs::write(&path, new).is_err() { return; }
    for _ in 0..60 {
        if crate::glaze::connected() { crate::glaze::command("wm-reload-config"); return; }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: &str) -> Config { Config::parse(json) }
    const I: u32 = b'I' as u32;

    #[test]
    fn keys_parse_and_format_back() {
        for k in ["Alt+I", "Win+Shift+Left", "Ctrl+Alt+F12", "Alt+`", "Win+/", "Alt+Shift+Space", "Alt+PgUp", "Alt+-", "Ctrl+1"] {
            let (m, v) = parse(k).unwrap_or_else(|| panic!("{k}"));
            let back = format(m, v).unwrap();
            assert_eq!(parse(&back), Some((m, v)), "{k} -> {back}");
        }
        assert_eq!(format(ALT, I).as_deref(), Some("Alt+I"));
        assert_eq!(format(WIN | SHIFT, 0x25).as_deref(), Some("Win+Shift+Left"));
        assert_eq!(parse("alt+i"), parse("Alt+I"), "case does not matter");
        assert_eq!(parse("super+a"), parse("Win+A"));
    }

    #[test]
    fn a_bad_combo_does_not_parse() {
        for k in ["", "A", "Alt", "Alt+", "Hyper+A", "Alt+F25", "Alt+Foo", "Alt+Shift"] { assert_eq!(parse(k), None, "{k}"); }
    }

    #[test]
    fn defaults_follow_the_modifier() {
        assert_eq!(fill("{m}+A", "Win"), "Win+A");
        assert_eq!(fill("{o}", "Win"), "Win+Tab");
        assert_eq!(fill("{o}", "Alt"), "Alt+`");
        let id = |j: &str, i: &str| keys_of(&cfg(j), BINDS.iter().find(|b| b.id == i).unwrap());
        assert_eq!(id(r#"{"modKey":"lwin"}"#, "terminal"), ["Win+Enter", "Win+T"]);
        assert_eq!(id("{}", "settings"), ["Alt+I"]);
    }

    #[test]
    fn your_keys_replace_the_defaults_and_junk_is_ignored() {
        let b = BINDS.iter().find(|b| b.id == "settings").unwrap();
        let c = cfg(r#"{"keybinds":{"settings":["Ctrl+Alt+S"]}}"#);
        assert_eq!(keys_of(&c, b), ["Ctrl+Alt+S"]);
        assert!(changed(&c, b));
        let junk = cfg(r#"{"keybinds":{"settings":["nope"]}}"#);
        assert_eq!(keys_of(&junk, b), ["Alt+I"], "an unusable override falls back");
        assert!(!changed(&cfg("{}"), b));
    }

    #[test]
    fn replacing_one_key_keeps_the_others() {
        let c = cfg("{}");
        assert_eq!(with_key(&c, "terminal", 1, "Alt+Y"), ["Alt+Enter", "Alt+Y"]);
        assert_eq!(with_key(&c, "settings", 0, "Alt+Y"), ["Alt+Y"]);
        assert_eq!(with_key(&c, "settings", 1, "Alt+Y"), ["Alt+I", "Alt+Y"]);
        assert!(with_key(&c, "nope", 0, "Alt+Y").len() == 1);
    }

    #[test]
    fn the_defaults_have_no_clashes_and_every_one_parses() {
        for m in ["alt", "lwin"] {
            let c = cfg(&format!(r#"{{"modKey":"{m}"}}"#));
            let mut seen = vec![];
            for b in BINDS { for k in keys_of(&c, b) {
                let p = parse(&k).unwrap_or_else(|| panic!("{} {k}", b.id));
                assert!(!seen.contains(&p), "{m}: {k} is bound twice");
                seen.push(p);
            } }
            assert_eq!(resolve(&c).len(), seen.len() + 20, "plus the workspace keys");
        }
        let ids: Vec<_> = BINDS.iter().map(|b| b.id).collect();
        assert!(ids.iter().enumerate().all(|(i, a)| !ids[..i].contains(a)), "ids are unique");
        assert!(BINDS.iter().all(|b| GROUPS.iter().any(|g| g.1 == b.group)), "every group has a heading");
    }

    #[test]
    fn a_taken_key_names_its_owner() {
        let c = cfg("{}");
        assert_eq!(taken_by(&c, "files", "Alt+I").as_deref(), Some("Settings"));
        assert_eq!(taken_by(&c, "settings", "Alt+I"), None, "its own key is free for itself");
        assert_eq!(taken_by(&c, "files", "Alt+Ctrl+Q"), None);
        assert_eq!(taken_by(&c, "files", "Alt+3").as_deref(), Some("a workspace key"));
    }

    #[test]
    fn the_first_bind_wins_a_shared_combo() {
        let c = cfg(r#"{"keybinds":{"files":["Alt+I"]}}"#);
        let want = parse("Alt+I").unwrap();
        let all = resolve(&c);
        let hits: Vec<_> = all.iter().filter(|a| (a.0, a.1) == want).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].2, Do::Zor("open settings".into()), "settings comes before files");
    }

    #[test]
    fn the_tracker_fires_eats_the_release_and_ignores_repeats() {
        let combos = [parse("Alt+I").unwrap()];
        let mut t = Tracker::default();
        assert_eq!(t.event(true, 0xA4, &combos, false), Out::Pass, "Alt goes through");
        assert_eq!(t.event(true, I, &combos, false), Out::Fire(0));
        assert_eq!(t.event(true, I, &combos, false), Out::Swallow, "repeat");
        assert_eq!(t.event(false, I, &combos, false), Out::Swallow, "release of an eaten key");
        assert_eq!(t.event(false, I, &combos, false), Out::Pass);
        assert_eq!(t.event(true, b'J' as u32, &combos, false), Out::Pass, "not ours");
        assert_eq!(t.event(false, 0xA4, &combos, false), Out::Pass);
        assert_eq!(t.event(true, I, &combos, false), Out::Pass, "no Alt, no fire");
        assert_eq!(t.mods(), 0);
    }

    #[test]
    fn the_held_file_round_trips_and_ignores_junk() {
        assert_eq!(parse_held(&held_text(&[66, 73])), [66, 73]);
        assert_eq!(parse_held(""), Vec::<u16>::new());
        assert_eq!(parse_held("66,x, 73 ,"), [66, 73]);
    }

    #[test]
    fn the_tracker_lists_the_keys_it_ate_while_they_are_down() {
        let combos = [parse("Alt+B").unwrap()];
        let mut t = Tracker::default();
        t.event(true, 0xA4, &combos, false);
        assert!(t.eaten().is_empty());
        t.event(true, b'B' as u32, &combos, false);
        assert_eq!(t.eaten(), [b'B' as u32]);
        t.event(true, b'B' as u32, &combos, false);
        assert_eq!(t.eaten().len(), 1, "a repeat adds nothing");
        t.event(false, b'B' as u32, &combos, false);
        assert!(t.eaten().is_empty());
    }

    #[test]
    fn a_missed_win_release_does_not_make_the_next_enter_a_win_combo() {
        let combos = [parse("Win+Enter").unwrap()];
        let mut t = Tracker::default();
        t.event(true, 0x5B, &combos, false);
        t.drop_released(|_| false); // unlocked: the Win key-up never reached the hook
        assert_eq!(t.event(true, 0x0D, &combos, false), Out::Pass);
        assert_eq!(t.mods(), 0);
    }

    #[test]
    fn a_modifier_still_down_stays_held() {
        let mut t = Tracker::default();
        t.event(true, 0xA2, &[], false);
        t.drop_released(|vk| vk == 0xA2);
        assert_eq!(t.mods(), CTRL);
    }

    #[test]
    fn both_alt_keys_count_until_both_are_up() {
        let mut t = Tracker::default();
        t.event(true, 0xA4, &[], false);
        t.event(true, 0xA5, &[], false);
        t.event(false, 0xA4, &[], false);
        assert_eq!(t.mods(), ALT);
        t.event(false, 0xA5, &[], false);
        assert_eq!(t.mods(), 0);
    }

    #[test]
    fn recording_takes_the_combo_and_esc_cancels() {
        let combos = [parse("Alt+I").unwrap()];
        let mut t = Tracker::default();
        t.event(true, 0x5B, &combos, true);
        t.event(true, 0xA0, &combos, true);
        assert_eq!(t.event(true, b'K' as u32, &combos, true), Out::Record(Some((WIN | SHIFT, b'K' as u32))));
        t.event(false, b'K' as u32, &combos, true);
        t.event(false, 0x5B, &combos, true);
        t.event(false, 0xA0, &combos, true);
        assert_eq!(t.event(true, 0x1B, &combos, true), Out::Record(None), "bare Esc cancels");
        t.event(false, 0x1B, &combos, true);
        t.event(true, 0xA4, &combos, true);
        assert_eq!(t.event(true, I, &combos, true), Out::Record(Some((ALT, I))), "a taken combo records too; the page rejects it");
    }

    #[test]
    fn glaze_keybindings_are_cut_only_from_our_config() {
        let ours = "general:\n  x: 1\nbinding_modes:\n  - name: 'resize'\n    keybindings:\n      - commands: ['a']\n        bindings: ['h']\n\nkeybindings:\n  # shell\n  - commands: ['shell-exec zorui.exe --open sidebar-left']\n    bindings: ['alt+a']\n";
        let cut = strip_glaze_keys(ours).unwrap();
        assert!(cut.ends_with("keybindings: []\n") && !cut.contains("sidebar-left"));
        assert!(cut.contains("bindings: ['h']"), "the resize mode keeps its keys");
        let after = format!("{ours}window_rules:\n  - commands: ['ignore']\n");
        assert!(strip_glaze_keys(&after).unwrap().ends_with("keybindings: []\nwindow_rules:\n  - commands: ['ignore']\n"), "blocks after it stay");
        assert_eq!(strip_glaze_keys("keybindings:\n  - commands: ['close']\n    bindings: ['alt+q']\n"), None, "a user's own config is left alone");
    }
}
