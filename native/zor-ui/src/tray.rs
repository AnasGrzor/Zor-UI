//! System tray icons through systray-util: it stands in
//! for the taskbar's tray so apps send their icons here. Runs on its own thread.
//!
//! systray-util only sees icons added after it starts (on Windows 11 it cannot read the taskbar's existing ones),
//! so the icons of the last run are saved to `tray-cache.json` and sent back to the stand-in at start (`restore`).
//! Which icons go on the bar is decided by Windows' own "shown on the taskbar" setting (`is_promoted`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use serde_json::{json, Value};
use systray_util::{StableId, Systray, SystrayEvent, SystrayIcon, SystrayIconAction};
use windows::core::{GUID, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    pub id: String,
    pub tooltip: String,
    /// Straight RGBA pixels, width, height.
    pub rgba: Option<Arc<(Vec<u8>, u32, u32)>>,
    /// Windows shows it on the taskbar (its "Show icon and notifications" toggle is on).
    pub promoted: bool,
}

static ACT: OnceLock<Mutex<mpsc::Sender<(String, SystrayIconAction)>>> = OnceLock::new();

pub enum Click { Left, Right, Middle, Double }

pub fn click(id: &str, how: Click) {
    let a = match how { Click::Left => SystrayIconAction::LeftClick, Click::Right => SystrayIconAction::RightClick,
        Click::Middle => SystrayIconAction::MiddleClick, Click::Double => SystrayIconAction::LeftDoubleClick };
    if let Some(Ok(t)) = ACT.get().map(|m| m.lock()) { let _ = t.send((id.to_string(), a)); }
}

pub fn click_btn(id: &str, b: crate::win::Btn) {
    use crate::win::Btn;
    click(id, match b { Btn::Left => Click::Left, Btn::Right => Click::Right, Btn::Middle => Click::Middle, Btn::Double => Click::Double });
}

pub fn start(on: impl Fn(Vec<Icon>) + Send + 'static) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, SystrayIconAction)>();
    // std sender for the UI thread, forwarded into the tokio channel the tray loop waits on
    let (stx, srx) = mpsc::channel::<(String, SystrayIconAction)>();
    let _ = ACT.set(Mutex::new(stx));
    std::thread::spawn(move || for m in srx { if tx.send(m).is_err() { return; } });
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(async move {
            let Ok(mut tray) = Systray::new() else { return };
            // Until the saved icons are back, nothing is saved: a save now would drop the ones not yet restored.
            let restored = Arc::new(AtomicBool::new(false));
            let (saved, done) = (load_cache(), restored.clone());
            std::thread::spawn(move || { restore(&saved); done.store(true, Ordering::SeqCst); });
            let mut order: Vec<String> = vec![];
            let mut live = Live::default();
            loop {
                tokio::select! {
                    ev = tray.events() => {
                        let Some(ev) = ev else { return };
                        match &ev {
                            SystrayEvent::IconAdd(i) => { let id = i.stable_id.to_string(); if !order.contains(&id) { order.push(id); } }
                            SystrayEvent::IconRemove(id) => { let id = id.to_string(); order.retain(|x| *x != id); }
                            SystrayEvent::IconUpdate(_) => {}
                        }
                        let icons = tray.icons();
                        live.refresh();
                        let ordered: Vec<&SystrayIcon> = order.iter()
                            .filter_map(|id| icons.iter().find(|i| i.stable_id.to_string() == *id))
                            .filter(|i| i.is_visible).collect();
                        if restored.load(Ordering::SeqCst) {
                            let saved: Vec<Saved> = ordered.iter().filter_map(|i| Saved::of(i)).collect();
                            save_cache(&saved, &mut live.last_saved);
                        }
                        on(ordered.iter().map(|i| {
                            let exe = i.window_handle.and_then(|h| exe_for(&mut live.exes, h));
                            let guid = i.guid.map(|g| g.simple().to_string());
                            Icon {
                                id: i.stable_id.to_string(),
                                tooltip: i.tooltip.clone(),
                                rgba: i.icon_image.as_ref().map(|im| Arc::new((im.as_raw().clone(), im.width(), im.height()))),
                                promoted: exe.is_some_and(|e| is_promoted(&live.settings, &e, i.uid, guid.as_deref())),
                            }
                        }).collect());
                    }
                    act = rx.recv() => {
                        let Some((id, a)) = act else { return };
                        if let Ok(sid) = id.parse::<StableId>() { let _ = tray.send_action(&sid, &a); }
                    }
                }
            }
        });
    });
}

// ---- Windows' "shown on the taskbar" setting ----

/// One entry of `Control Panel\NotifyIconSettings`, the list Windows' "Other system tray icons" page writes.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    /// Exe path, or a known-folder path such as `{GUID}\App\App.exe`.
    pub exe: String,
    pub uid: Option<u32>,
    /// The icon's GUID without braces or dashes, lowercase.
    pub guid: Option<String>,
    pub promoted: bool,
}

/// Whether Windows shows the icon on the taskbar. An icon with a GUID is matched by it; any other icon by its exe and UID.
pub fn is_promoted(settings: &[Setting], exe: &str, uid: Option<u32>, guid: Option<&str>) -> bool {
    if let Some(g) = guid {
        if let Some(s) = settings.iter().find(|s| s.guid.as_deref() == Some(g)) { return s.promoted; }
    }
    settings.iter().find(|s| exe_matches(&s.exe, exe) && s.uid.is_none_or(|u| Some(u) == uid)).is_some_and(|s| s.promoted)
}

/// A known-folder path keeps only its part after the GUID, which must end the real exe path.
fn exe_matches(stored: &str, exe: &str) -> bool {
    let tail = match stored.strip_prefix('{').and_then(|s| s.split_once('}')) { Some((_, rest)) => rest, None => stored };
    !tail.is_empty() && exe.to_ascii_lowercase().ends_with(&tail.to_ascii_lowercase())
}

fn read_settings() -> Vec<Setting> {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;
    const KEY: &str = r"Control Panel\NotifyIconSettings";
    crate::privacy::subkeys(KEY).into_iter().filter_map(|sub| {
        let key = format!(r"{KEY}\{sub}");
        let exe = crate::bridge::we::reg_str(HKEY_CURRENT_USER, &key, "ExecutablePath")?;
        Some(Setting {
            exe,
            uid: reg_dword(&key, "UID"),
            guid: crate::bridge::we::reg_str(HKEY_CURRENT_USER, &key, "IconGuid")
                .map(|g| g.trim_matches(|c| c == '{' || c == '}').replace('-', "").to_ascii_lowercase()),
            promoted: reg_dword(&key, "IsPromoted") == Some(1),
        })
    }).collect()
}

fn reg_dword(key: &str, name: &str) -> Option<u32> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let (mut v, mut size) = (0u32, 4u32);
    unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut size)) }
        .ok().ok().map(|()| v)
}

/// Per-loop state: the registry list is read at most every 30 s, and each window's exe is looked up once.
#[derive(Default)]
struct Live {
    settings: Vec<Setting>,
    read_at: Option<Instant>,
    exes: HashMap<isize, Option<String>>,
    last_saved: String,
}

impl Live {
    fn refresh(&mut self) {
        if self.read_at.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
            self.settings = read_settings();
            self.read_at = Some(Instant::now());
        }
    }
}

/// shortcut: a window handle that is closed and reused by another program keeps the old exe, until zorui restarts.
fn exe_for(exes: &mut HashMap<isize, Option<String>>, hwnd: isize) -> Option<String> {
    exes.entry(hwnd).or_insert_with(|| {
        let mut pid = 0u32;
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(HWND(hwnd as _), Some(&mut pid)) };
        crate::bridge::sys::exe_of(pid)
    }).clone()
}

// ---- saved icons ----

/// What `restore` needs to add an icon again: where it lives and how to reach it.
#[derive(Clone, Debug, PartialEq)]
struct Saved {
    hwnd: isize,
    pid: u32,
    uid: Option<u32>,
    /// Simple GUID text (32 hex digits), as `Saved::of` writes it.
    guid: Option<String>,
    callback: Option<u32>,
    version: Option<u32>,
    icon: Option<isize>,
    tip: String,
}

impl Saved {
    fn of(i: &SystrayIcon) -> Option<Saved> {
        let hwnd = i.window_handle?;
        let mut pid = 0u32;
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(HWND(hwnd as _), Some(&mut pid)) };
        Some(Saved { hwnd, pid, uid: i.uid, guid: i.guid.map(|g| g.simple().to_string()), callback: i.callback_message,
            version: i.version, icon: i.icon_handle, tip: i.tooltip.clone() })
    }

    fn to_json(&self) -> Value {
        json!({ "hwnd": self.hwnd, "pid": self.pid, "uid": self.uid, "guid": self.guid, "callback": self.callback,
            "version": self.version, "icon": self.icon, "tip": self.tip })
    }

    fn from_json(v: &Value) -> Option<Saved> {
        Some(Saved {
            hwnd: v["hwnd"].as_i64()? as isize,
            pid: v["pid"].as_u64()? as u32,
            uid: v["uid"].as_u64().map(|x| x as u32),
            guid: v["guid"].as_str().map(String::from),
            callback: v["callback"].as_u64().map(|x| x as u32),
            version: v["version"].as_u64().map(|x| x as u32),
            icon: v["icon"].as_i64().map(|x| x as isize),
            tip: v["tip"].as_str()?.to_string(),
        })
    }
}

fn cache_path() -> PathBuf { crate::conf::data_dir().join("tray-cache.json") }

fn load_cache() -> Vec<Saved> {
    std::fs::read_to_string(cache_path()).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(Saved::from_json).collect()))
        .unwrap_or_default()
}

/// Writes only when the list changed, so a steady tray costs no disk writes.
fn save_cache(saved: &[Saved], last: &mut String) {
    let json = Value::Array(saved.iter().map(Saved::to_json).collect()).to_string();
    if json != *last {
        let _ = std::fs::write(cache_path(), &json);
        *last = json;
    }
}

/// systray-util's view of the shell's tray message (`WM_COPYDATA`, dwData 1): the same 956-byte layout, with 32-bit handles.
#[repr(C)]
#[derive(Clone, Copy)]
struct IconData {
    size: u32,
    hwnd: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    size_info: [u16; 256],
    anon: u32,
    title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TrayMsg {
    magic: i32,
    message: u32,
    data: IconData,
    version: u32,
}

const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 1;
const NIF_ICON: u32 = 2;
const NIF_TIP: u32 = 4;
const NIF_GUID: u32 = 0x20;

/// Adds the saved icons back to the stand-in once it exists (it is created a moment after systray-util starts).
/// Each one is sent as a NIM_SETVERSION message: systray-util does not know the icon, so it adds it, and
/// explorer ignores it when it still has the icon. Icons whose app is gone, or has a new process, are skipped.
fn restore(saved: &[Saved]) {
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SendMessageW, WM_COPYDATA};
    let until = Instant::now() + Duration::from_secs(3);
    let standin = loop {
        if let Some(&h) = ours("Shell_TrayWnd").first() { break h; }
        if Instant::now() > until { return; }
        std::thread::sleep(Duration::from_millis(50));
    };
    for s in saved {
        let mut pid = 0u32;
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(HWND(s.hwnd as _), Some(&mut pid)) };
        if pid != s.pid || !unsafe { IsWindow(HWND(s.hwnd as _)) }.as_bool() { continue; }
        let mut d: IconData = unsafe { std::mem::zeroed() };
        d.size = std::mem::size_of::<IconData>() as u32;
        d.hwnd = s.hwnd as u32;
        d.uid = s.uid.unwrap_or(0);
        d.anon = s.version.unwrap_or(0);
        let mut flags = NIF_TIP;
        if let Some(cb) = s.callback { d.callback = cb; flags |= NIF_MESSAGE; }
        if let Some(ic) = s.icon { d.icon = ic as u32; flags |= NIF_ICON; }
        for (dst, c) in d.tip.iter_mut().take(127).zip(s.tip.encode_utf16()) { *dst = c; }
        if let Some(g) = s.guid.as_deref().and_then(|g| u128::from_str_radix(g, 16).ok()) { d.guid = GUID::from_u128(g); flags |= NIF_GUID; }
        d.flags = flags;
        let msg = TrayMsg { magic: 0x3475_3423, message: NIM_SETVERSION, data: d, version: 0 };
        let cds = COPYDATASTRUCT { dwData: 1, cbData: std::mem::size_of::<TrayMsg>() as u32, lpData: &msg as *const TrayMsg as *mut _ };
        unsafe { SendMessageW(standin, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const COPYDATASTRUCT as isize)) };
    }
}

// ---- the stand-in window ----

/// Every window of this process with class `class`, shown or not.
pub fn ours(class: &str) -> Vec<HWND> { windows_of(class, true) }

/// Every window with class `class`: ours when `mine`, otherwise those of other processes (Explorer's taskbar).
pub fn windows_of(class: &str, mine: bool) -> Vec<HWND> {
    use windows::Win32::{
        Foundation::BOOL,
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId},
    };
    struct Job<'a> { class: &'a str, mine: bool, out: Vec<HWND> }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let job = &mut *(l.0 as *mut Job);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut name = [0u16; 64];
        let n = GetClassNameW(h, &mut name).max(0) as usize;
        if (pid == GetCurrentProcessId()) == job.mine && String::from_utf16_lossy(&name[..n]).eq_ignore_ascii_case(job.class) { job.out.push(h); }
        BOOL(1)
    }
    let mut job = Job { class, mine, out: vec![] };
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut job as *mut Job as isize)); }
    job.out
}

/// Puts Explorer's taskbar back above normal windows. A window in front can drop the taskbar's topmost flag,
/// and then an auto-hidden taskbar slides in behind it. Returns the title of the window in front when it raised one.
pub fn raise_taskbar() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowLongPtrW, GetWindowTextW, IsWindowVisible, IsZoomed, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_TOPMOST};
    // A real full-screen app (game, video) stays in front of the taskbar, as game mode does. A maximized window
    // also covers the whole monitor when the taskbar auto-hides, so it is not treated as full-screen.
    let fg = unsafe { GetForegroundWindow() };
    if crate::buddy::fullscreen_monitor().is_some() && !unsafe { IsZoomed(fg) }.as_bool() { return None; }
    let mut raised = 0;
    for class in ["Shell_TrayWnd", "Shell_SecondaryTrayWnd"] {
        for h in windows_of(class, false) {
            unsafe {
                if !IsWindowVisible(h).as_bool() || GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0 { continue; }
                let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            raised += 1;
        }
    }
    if raised == 0 { return None; }
    let mut t = [0u16; 128];
    let n = unsafe { GetWindowTextW(GetForegroundWindow(), &mut t) }.max(0) as usize;
    Some(String::from_utf16_lossy(&t[..n]))
}

/// systray-util builds its stand-in `Shell_TrayWnd` as a normal captioned window (WS_OVERLAPPEDWINDOW) and never shows it. If anything
/// shows it (a program that sends the taskbar a command or a ShowWindow), it is an empty white window titled "Shell_TrayWnd".
/// Hides every visible window of this process with class `class`; returns how many it hid.
pub fn hide_ours(class: &str) -> usize {
    use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_HIDE};
    let mut hid = 0;
    for h in ours(class) {
        if unsafe { IsWindowVisible(h).as_bool() } {
            let _ = unsafe { ShowWindow(h, SW_HIDE) };
            hid += 1;
        }
    }
    hid
}

/// Makes every window of this process with class `class` fully transparent and click-through, shown or not, so one that gets shown
/// before the next guard tick (or while the UI thread is busy) is still invisible. Returns how many it changed; 0 once they are all done.
pub fn veil_ours(class: &str) -> usize {
    use windows::Win32::UI::WindowsAndMessaging::{GetLayeredWindowAttributes, GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT};
    let want = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0 as isize;
    let mut done = 0;
    for h in ours(class) {
        unsafe {
            let (style, mut alpha) = (GetWindowLongPtrW(h, GWL_EXSTYLE), 255u8);
            let _ = GetLayeredWindowAttributes(h, None, Some(&mut alpha), None);
            if style & want != want || alpha != 0 {
                SetWindowLongPtrW(h, GWL_EXSTYLE, style | want);
                let _ = SetLayeredWindowAttributes(h, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
                done += 1;
            }
        }
    }
    done
}

/// Keeps the stand-in hidden, and invisible if it is shown anyway (`veil_ours`). When it had to hide it, returns the title of the window
/// in front, which is the best clue to who showed it.
pub fn guard_standin() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    veil_ours("Shell_TrayWnd");
    if hide_ours("Shell_TrayWnd") == 0 { return None; }
    let mut t = [0u16; 128];
    let n = unsafe { GetWindowTextW(GetForegroundWindow(), &mut t) }.max(0) as usize;
    Some(String::from_utf16_lossy(&t[..n]))
}

#[cfg(test)]
mod standin_tests {
    use super::*;
    use windows::{core::w, Win32::{Foundation::HWND, UI::WindowsAndMessaging::*}};

    #[test]
    fn a_visible_window_of_ours_with_that_class_is_hidden_and_others_are_left() {
        unsafe {
            let make = |class: windows::core::PCWSTR| CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("standin test"), WS_POPUP | WS_VISIBLE, 0, 0, 10, 10, None, None, None, None).unwrap();
            let (button, edit): (HWND, HWND) = (make(w!("BUTTON")), make(w!("EDIT")));
            assert!(IsWindowVisible(button).as_bool() && IsWindowVisible(edit).as_bool());
            assert!(hide_ours("BUTTON") >= 1);
            assert!(!IsWindowVisible(button).as_bool(), "the one with the class is hidden");
            assert!(IsWindowVisible(edit).as_bool(), "a window of another class is not touched");
            assert_eq!(hide_ours("BUTTON"), 0, "nothing left to hide");
            let _ = DestroyWindow(button);
            let _ = DestroyWindow(edit);
        }
    }

    #[test]
    fn a_veiled_window_is_transparent_and_click_through_and_others_are_left() {
        unsafe {
            let make = |class: windows::core::PCWSTR| CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("veil test"), WS_POPUP | WS_VISIBLE, 0, 0, 10, 10, None, None, None, None).unwrap();
            let (button, edit): (HWND, HWND) = (make(w!("BUTTON")), make(w!("EDIT")));
            assert!(veil_ours("BUTTON") >= 1);
            let (mut alpha, style) = (255u8, GetWindowLongPtrW(button, GWL_EXSTYLE));
            GetLayeredWindowAttributes(button, None, Some(&mut alpha), None).unwrap();
            assert_eq!(alpha, 0, "fully transparent");
            assert!(style & WS_EX_TRANSPARENT.0 as isize != 0 && style & WS_EX_LAYERED.0 as isize != 0, "clicks go through");
            assert_eq!(GetWindowLongPtrW(edit, GWL_EXSTYLE) & WS_EX_LAYERED.0 as isize, 0, "another class is not touched");
            assert_eq!(veil_ours("BUTTON"), 0, "already done");
            let _ = DestroyWindow(button);
            let _ = DestroyWindow(edit);
        }
    }

    #[test]
    fn windows_of_splits_ours_from_the_rest() {
        unsafe {
            // STATIC, not BUTTON: the veil test counts BUTTON windows and runs in parallel
            let b = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("split test"), WS_POPUP, 0, 0, 10, 10, None, None, None, None).unwrap();
            assert!(windows_of("STATIC", true).contains(&b));
            assert!(!windows_of("STATIC", false).contains(&b));
            let _ = DestroyWindow(b);
        }
    }

    #[test]
    fn only_the_taskbar_class_is_guarded() {
        // no such window exists in the test process, so the guard has nothing to report
        assert_eq!(guard_standin(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(exe: &str, uid: Option<u32>, guid: Option<&str>, promoted: bool) -> Setting {
        Setting { exe: exe.into(), uid, guid: guid.map(String::from), promoted }
    }

    #[test]
    fn a_guid_decides_before_the_exe_does() {
        let g = "ba82e2dcf40547ad8e5b2f1d7c3a9c11";
        let s = [setting(r"C:\app\a.exe", Some(0), Some(g), false), setting(r"C:\app\a.exe", Some(0), Some("0123"), true)];
        assert!(is_promoted(&s, r"C:\app\a.exe", Some(0), Some("0123")));
        assert!(!is_promoted(&s, r"C:\app\a.exe", Some(0), Some(g)));
    }

    #[test]
    fn a_known_folder_path_matches_the_real_exe_by_its_tail() {
        let s = [setting(r"{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\Everything\Everything.exe", Some(0), None, true)];
        assert!(is_promoted(&s, r"C:\Program Files\Everything\Everything.exe", Some(0), None));
        assert!(!is_promoted(&s, r"C:\Other\Everything.exe", Some(0), None));
    }

    #[test]
    fn a_different_uid_is_a_different_icon_and_no_entry_means_hidden() {
        let s = [setting(r"C:\app\a.exe", Some(1), None, true)];
        assert!(is_promoted(&s, r"C:\app\a.exe", Some(1), None));
        assert!(!is_promoted(&s, r"C:\app\a.exe", Some(2), None), "uid 2 has no entry");
        assert!(!is_promoted(&[], r"C:\app\a.exe", Some(1), None), "no entry: hidden in the flyout");
    }

    #[test]
    fn saved_icons_survive_a_round_trip_through_the_cache_file() {
        let s = Saved { hwnd: 131550, pid: 4242, uid: Some(1), guid: Some("ba82e2dcf40547ad8e5b2f1d7c3a9c11".into()), callback: Some(32770),
            version: Some(4), icon: Some(2163257), tip: "Zor UI".into() };
        assert_eq!(Saved::from_json(&s.to_json()), Some(s.clone()));
        let none = Saved { uid: None, guid: None, callback: None, version: None, icon: None, ..s.clone() };
        assert_eq!(Saved::from_json(&none.to_json()), Some(none));
    }

    #[test]
    fn the_shell_message_has_windows_layout() {
        // NOTIFYICONDATAW as systray-util reads it (32-bit handles): 956 bytes
        assert_eq!(std::mem::size_of::<IconData>(), 956);
    }
}
