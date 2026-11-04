//! end4-pC native shell. Replaces the Zebar widget pack part by part; config.json "native" lists the
//! parts it has taken over. `end4.exe --all` runs every ported part regardless (for testing next to Zebar).
#![windows_subsystem = "windows"]

mod app;
mod bar;
mod conf;
mod desk;
mod dock;
mod gfx;
mod glaze;
mod gojo;
mod hub;
mod island;
mod lyrics;
mod media;
mod overview;
mod popup;
mod proc;
mod render;
mod settings;
mod sidebar_left;
mod sidebar_right;
mod sys;
mod tray;
mod ui;
mod util;
mod viz;
mod weather;
mod win;

use windows::{
    core::w,
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM},
        System::{Com::{CoInitializeEx, COINIT_APARTMENTTHREADED}, DataExchange::COPYDATASTRUCT, Threading::CreateMutexW},
        UI::{HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2}, WindowsAndMessaging::*},
    },
};

/// `end4.exe --open <name>`: asks the running instance to open (or toggle) a popup; without one, Zebar's copy opens.
fn open_running(name: &str) {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') { return; }
    unsafe {
        if let Ok(h) = FindWindowExW(HWND_MESSAGE, None, win::CLASS, w!("end4")) {
            let text = format!("open {name}");
            let cds = COPYDATASTRUCT { dwData: app::COPYDATA_ID, cbData: text.len() as u32, lpData: text.as_ptr() as *mut _ };
            SendMessageW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize));
            return;
        }
    }
    proc::zebar_widget(name);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| args.iter().position(|a| a == flag);
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Some(i) = arg("--open") { return open_running(args.get(i + 1).map_or("", String::as_str)); }
        if let Some(i) = arg("--render") { return render::run(&args[i + 1..]); }
        // one copy only: a second start (login + manual) would stack duplicate widgets
        let _single = CreateMutexW(None, true, w!("end4-pC-native"));
        if GetLastError() == ERROR_ALREADY_EXISTS { return; }
        if app::App::start(arg("--all").is_some()).is_err() { return; }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
