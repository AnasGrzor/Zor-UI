//! System tray icons through systray-util: it stands in
//! for the taskbar's tray so apps send their icons here. Runs on its own thread.

use std::sync::{mpsc, Arc, Mutex, OnceLock};
use systray_util::{StableId, Systray, SystrayEvent, SystrayIconAction};

#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    pub id: String,
    pub tooltip: String,
    /// Straight RGBA pixels, width, height.
    pub rgba: Option<Arc<(Vec<u8>, u32, u32)>>,
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
            let mut order: Vec<String> = vec![];
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
                        on(order.iter().filter_map(|id| icons.iter().find(|i| i.stable_id.to_string() == *id)).map(|i| Icon {
                            id: i.stable_id.to_string(),
                            tooltip: i.tooltip.clone(),
                            rgba: i.icon_image.as_ref().map(|im| Arc::new((im.as_raw().clone(), im.width(), im.height()))),
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

// ---- the stand-in window ----

/// systray-util builds its stand-in `Shell_TrayWnd` as a normal captioned window (WS_OVERLAPPEDWINDOW) and never shows it. If anything
/// shows it (a program that sends the taskbar a command or a ShowWindow), it is an empty white window titled "Shell_TrayWnd".
/// Hides every visible window of this process with class `class`; returns how many it hid.
pub fn hide_ours(class: &str) -> usize {
    use windows::Win32::{
        Foundation::{BOOL, HWND, LPARAM},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, ShowWindow, SW_HIDE},
    };
    struct Job<'a> { class: &'a str, hid: usize }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let job = &mut *(l.0 as *mut Job);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut name = [0u16; 64];
        let n = GetClassNameW(h, &mut name).max(0) as usize;
        if pid == GetCurrentProcessId() && String::from_utf16_lossy(&name[..n]).eq_ignore_ascii_case(job.class) && IsWindowVisible(h).as_bool() {
            let _ = ShowWindow(h, SW_HIDE);
            job.hid += 1;
        }
        BOOL(1)
    }
    let mut job = Job { class, hid: 0 };
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut job as *mut Job as isize)); }
    job.hid
}

/// Makes every window of this process with class `class` fully transparent and click-through, shown or not, so one that gets shown
/// before the next guard tick (or while the UI thread is busy) is still invisible. Returns how many it changed; 0 once they are all done.
pub fn veil_ours(class: &str) -> usize {
    use windows::Win32::{
        Foundation::{BOOL, HWND, LPARAM},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetLayeredWindowAttributes, GetWindowLongPtrW, GetWindowThreadProcessId, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT},
    };
    struct Job<'a> { class: &'a str, done: usize }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let job = &mut *(l.0 as *mut Job);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut name = [0u16; 64];
        let n = GetClassNameW(h, &mut name).max(0) as usize;
        if pid == GetCurrentProcessId() && String::from_utf16_lossy(&name[..n]).eq_ignore_ascii_case(job.class) {
            let want = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0 as isize;
            let (style, mut alpha) = (GetWindowLongPtrW(h, GWL_EXSTYLE), 255u8);
            let _ = GetLayeredWindowAttributes(h, None, Some(&mut alpha), None);
            if style & want != want || alpha != 0 {
                SetWindowLongPtrW(h, GWL_EXSTYLE, style | want);
                let _ = SetLayeredWindowAttributes(h, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
                job.done += 1;
            }
        }
        BOOL(1)
    }
    let mut job = Job { class, done: 0 };
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut job as *mut Job as isize)); }
    job.done
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
    fn only_the_taskbar_class_is_guarded() {
        // no such window exists in the test process, so the guard has nothing to report
        assert_eq!(guard_standin(), None);
    }
}
