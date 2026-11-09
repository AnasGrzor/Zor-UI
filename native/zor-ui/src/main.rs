//! zor-ui native shell. config.json "native" lists the parts it runs (all of them by default).
//! `zorui.exe --all` runs every part regardless of that list.
#![windows_subsystem = "windows"]

mod agent;
mod anim;
mod app;
mod appicon;
mod assist;
mod bridge;
mod bar;
mod conf;
mod creator;
mod desk;
mod drive;
mod driveframe;
mod dock;
mod frame;
mod gfx;
mod glass;
mod guides;
mod glaze;
mod buddy;
mod hub;
mod island;
mod keymap;
mod cast;
mod claude_usage;
mod log;
mod lyrics;
mod media;
mod menu;
mod overview;
mod pack;
mod perf;
mod pill;
mod playsong;
mod popup;
mod privacy;
mod proc;
mod render;
mod rescue;
mod screen;
mod settings;
mod sidebar_left;
mod sidebar_right;
mod sys;
mod tray;
mod tokens;
mod tray_flyout;
mod ui;
mod util;
mod viz;
mod webchat;
mod weather;
mod webread;
mod win;

use windows::{
    core::w,
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM},
        System::{Com::{CoInitializeEx, COINIT_APARTMENTTHREADED}, DataExchange::COPYDATASTRUCT, Threading::CreateMutexW},
        UI::{HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2}, WindowsAndMessaging::*},
    },
};

/// `zorui.exe --open <name>`: asks the running instance to open (or toggle) a popup; without one, nothing happens.
fn open_running(name: &str) {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') { return; }
    send_running(&format!("open {name}"));
}

/// Sends a command line to the running instance's message window; false when none is running.
fn send_running(text: &str) -> bool {
    unsafe {
        let Ok(h) = FindWindowExW(HWND_MESSAGE, None, win::CLASS, w!("zor-ui")) else { return false };
        let cds = COPYDATASTRUCT { dwData: app::COPYDATA_ID, cbData: text.len() as u32, lpData: text.as_ptr() as *mut _ };
        SendMessageW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize));
        true
    }
}

fn main() {
    // panic = "abort" ends the process with no message; this keeps the reason and line in crash.log
    std::panic::set_hook(Box::new(|info| {
        use std::io::Write;
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let name = std::thread::current().name().unwrap_or("?").to_string();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(conf::data_dir().join("crash.log")) {
            let _ = writeln!(f, "[unix {t}] thread {name}: {info}");
        }
    }));
    let args: Vec<String> = std::env::args().collect();
    let arg = |flag: &str| args.iter().position(|a| a == flag);
    // agent hooks run on every tool call: answer before any window or COM setup
    if let Some(i) = arg("--agent") { std::process::exit(bridge::agent_hook::cli(&args[i + 1..])); }
    if arg("--install-agents").is_some() { std::process::exit(bridge::agent_hook::cli(&["--install".to_string()])); }
    log::init();
    log::line(&format!("zorui {} started", env!("CARGO_PKG_VERSION")));
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // `--buddy listen` records your voice for the primary buddy; `--buddy "open steam"` hands it text
        if let Some(i) = arg("--buddy") {
            let what = args[i + 1..].join(" ");
            let line = if what == "listen" { "buddy listen".to_string() } else { format!("buddy ask {what}") };
            if !what.is_empty() { send_running(&line); }
            return;
        }
        if let Some(i) = arg("--open") { return open_running(args.get(i + 1).map_or("", String::as_str)); }
        for kind in ["--bridge", "--script"] {
            if let Some(i) = arg(kind) { std::process::exit(bridge::cli(kind, &args[i + 1..])); }
        }
        if arg("--update").is_some() { std::process::exit(bridge::update::cli()); }
        if let Some(i) = arg("--render") { return render::run(&args[i + 1..]); }
        // one copy only: a second start (login + manual) would stack duplicate widgets
        let _single = CreateMutexW(None, true, w!("zor-ui-native"));
        if GetLastError() == ERROR_ALREADY_EXISTS { return; }
        if app::App::start(arg("--all").is_some()).is_err() { return; }
        bridge::update::spawn();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
