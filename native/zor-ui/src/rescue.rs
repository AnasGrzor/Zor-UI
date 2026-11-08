//! Brings back windows that GlazeWM hid with `hide_method: cloak` when GlazeWM is gone: a cloaked
//! window keeps running but has no taskbar button and sits on no workspace.

/// DWMWA_CLOAKED bits: set by the app (GlazeWM), by the shell (another virtual desktop), inherited from the owner.
const CLOAKED_APP: u32 = 1;
const CLOAKED_SHELL: u32 = 2;

/// GlazeWM may be down this long (IPC disconnected, process absent) before windows are brought back.
pub const GRACE_MS: u64 = 3_000;

/// A window is uncloaked when an app cloaked it. A shell cloak means another virtual desktop: leave it.
pub fn needs_uncloak(attr: u32) -> bool {
    attr & (CLOAKED_APP | CLOAKED_SHELL) == CLOAKED_APP
}

/// Recover once the IPC link has been down for GRACE_MS and no glazewm.exe is running.
/// A running glazewm with a dropped link is only reconnecting, and it still owns the cloaks.
pub fn should_recover(ipc_up: bool, glaze_running: bool, down_ms: u64) -> bool {
    !ipc_up && !glaze_running && down_ms >= GRACE_MS
}

// ---------------------------------------------------------------- OS side (not unit tested: needs real windows)

use windows::{
    Win32::{
        Foundation::{BOOL, CloseHandle, HWND, LPARAM},
        Graphics::Dwm::{DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED},
        System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        UI::WindowsAndMessaging::EnumWindows,
    },
};

/// Clears the app cloak on every top-level window and returns how many were uncloaked.
/// shortcut: assumes DWM lets another process clear GlazeWM's cloak; the count stays 0 if it refuses.
pub fn uncloak_all() -> usize {
    unsafe extern "system" fn each(h: HWND, n: LPARAM) -> BOOL {
        let mut attr = 0u32;
        let ok = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut attr as *mut u32 as *mut _, 4).is_ok();
        if ok && needs_uncloak(attr) && DwmSetWindowAttribute(h, DWMWA_CLOAK, &BOOL(0) as *const BOOL as *const _, 4).is_ok() {
            *(n.0 as *mut usize) += 1;
        }
        BOOL(1)
    }
    let mut n = 0usize;
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut n as *mut usize as isize)); }
    n
}

fn glaze_running() -> bool {
    let mut found = false;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return true }; // unknown: don't touch windows
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more && !found {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            found = String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("glazewm.exe");
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    found
}

/// Watches the GlazeWM link on a thread and uncloaks everything once per outage when GlazeWM is gone.
pub fn watch() {
    std::thread::spawn(|| {
        let mut down_since: Option<std::time::Instant> = None;
        let mut done = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if crate::glaze::connected() { down_since = None; done = false; continue; }
            let since = *down_since.get_or_insert_with(std::time::Instant::now);
            if !done && should_recover(false, glaze_running(), since.elapsed().as_millis() as u64) {
                done = true;
                crate::log::line(&format!("rescue: GlazeWM is gone, uncloaked {} windows", uncloak_all()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncloaks_app_cloaked_windows() {
        assert!(needs_uncloak(1));
    }

    #[test]
    fn leaves_visible_windows_alone() {
        assert!(!needs_uncloak(0));
    }

    #[test]
    fn leaves_other_virtual_desktops_alone() {
        assert!(!needs_uncloak(2));
        assert!(!needs_uncloak(3)); // app and shell: the shell cloak wins
    }

    #[test]
    fn inherited_cloak_alone_is_not_ours_but_with_app_bit_is() {
        assert!(!needs_uncloak(4));
        assert!(needs_uncloak(5));
    }

    #[test]
    fn recovers_when_glazewm_is_gone_past_the_grace_period() {
        assert!(should_recover(false, false, GRACE_MS));
        assert!(should_recover(false, false, GRACE_MS + 1));
    }

    #[test]
    fn waits_out_the_grace_period() {
        assert!(!should_recover(false, false, GRACE_MS - 1));
        assert!(!should_recover(false, false, 0));
    }

    #[test]
    fn never_recovers_while_glazewm_runs() {
        assert!(!should_recover(false, true, 60_000));
    }

    #[test]
    fn never_recovers_while_the_link_is_up() {
        assert!(!should_recover(true, false, 60_000));
        assert!(!should_recover(true, true, 60_000));
    }
}
