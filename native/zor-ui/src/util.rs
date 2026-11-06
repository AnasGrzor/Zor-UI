//! Small system helpers: local time and its names, HTTP through curl.exe, monitors, resource stats.

use std::os::windows::process::CommandExt;
use windows::{
    core::{HSTRING, PWSTR},
    Win32::{
        Foundation::*,
        Globalization::{GetDateFormatEx, ENUM_DATE_FORMATS_FLAGS},
        Graphics::Gdi::*,
        Storage::FileSystem::GetDiskFreeSpaceExW,
        System::{SystemInformation::*, Threading::GetSystemTimes, Time::*},
    },
};

pub fn now() -> SYSTEMTIME { unsafe { GetLocalTime() } }

/// Windows date picture ("dddd, MMMM d") in the user's locale, like toLocaleDateString.
pub fn date_fmt(st: &SYSTEMTIME, pic: &str) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetDateFormatEx(None, ENUM_DATE_FORMATS_FLAGS(0), Some(st), &HSTRING::from(pic), Some(&mut buf), None) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
}

/// `days` days after `st` (calendar arithmetic through FILETIME).
pub fn add_days(st: &SYSTEMTIME, days: i64) -> SYSTEMTIME {
    unsafe {
        let mut ft = FILETIME::default();
        let _ = SystemTimeToFileTime(st, &mut ft);
        let t = ((ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64) as i64 + days * 864_000_000_000;
        let ft = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
        let mut out = SYSTEMTIME::default();
        let _ = FileTimeToSystemTime(&ft, &mut out);
        out
    }
}

pub fn days_in_month(year: u16, month: u16) -> u16 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Monday = 0 (end4's calendars start on Monday).
pub fn monday_index(st: &SYSTEMTIME) -> u16 { (st.wDayOfWeek + 6) % 7 }

/// "HH:mm" or 12-hour "hh:mm" without AM/PM, like the cards' toLocaleTimeString call.
pub fn clock(h: u16, m: u16, h24: bool) -> String {
    let h = if h24 { h } else if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h:02}:{m:02}")
}

/// UTC offset in minutes for an IANA zone ("Europe/London") at the current moment, via Windows' ICU.
pub fn zone_offset(tz: &str) -> Option<i32> {
    use windows::Win32::Globalization::*;
    let id: Vec<u16> = tz.encode_utf16().collect();
    unsafe {
        let mut err = U_ZERO_ERROR;
        let cal = ucal_open(id.as_ptr(), id.len() as i32, windows::core::s!("en_US"), UCAL_GREGORIAN, &mut err);
        if err.0 > 0 || cal.is_null() { return None; }
        // ICU falls back to GMT for unknown ids; report that as unknown, like the JS card's catch
        let mut name = [0u16; 64];
        let n = ucal_getTimeZoneID(cal as _, name.as_mut_ptr(), 64, &mut err);
        let known = String::from_utf16_lossy(&name[..n.clamp(0, 64) as usize]) != "Etc/Unknown";
        let ms = ucal_get(cal as _, UCAL_ZONE_OFFSET, &mut err) + ucal_get(cal as _, UCAL_DST_OFFSET, &mut err);
        ucal_close(cal as _);
        (err.0 <= 0 && known).then_some(ms / 60_000)
    }
}

/// Local UTC offset in minutes.
pub fn local_offset() -> i32 {
    let mut tz = TIME_ZONE_INFORMATION::default();
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let dst = if r == 2 { tz.DaylightBias } else { 0 }; // TIME_ZONE_ID_DAYLIGHT
    -(tz.Bias + dst)
}

/// GET through Windows' curl.exe: no TLS stack in this process, nothing resident between calls.
/// Only http(s) goes to curl, and after `--`, so a URL from a remote API can't become an option.
pub fn web_url(url: &str) -> bool { url.starts_with("http://") || url.starts_with("https://") }

pub fn http_get(url: &str) -> Option<String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if !web_url(url) { return None; }
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-f", "-L", "--max-time", "20", "--", url])
        .creation_flags(CREATE_NO_WINDOW)
        .output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// (HTTP status, body) of a GET; status 0 when the request itself failed (offline, timeout).
pub fn http_status(url: &str) -> (u16, String) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if !web_url(url) { return (0, String::new()); }
    let Ok(out) = std::process::Command::new("curl.exe").args(["-s", "-L", "--max-time", "20", "-w", "\n%{http_code}", "--", url]).creation_flags(CREATE_NO_WINDOW).output() else { return (0, String::new()) };
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    match text.rsplit_once('\n') { Some((body, code)) => (code.trim().parse().unwrap_or(0), body.to_string()), None => (0, String::new()) }
}

/// encodeURIComponent: everything but letters, digits and -_.~ becomes %XX (UTF-8 bytes).
pub fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

const CF_UNICODETEXT: u32 = 13;

pub fn clipboard_get() -> Option<String> {
    use windows::Win32::System::{DataExchange::*, Memory::*};
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            let h = GetClipboardData(CF_UNICODETEXT).ok()?;
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() { return None; }
            let mut n = 0;
            while *p.add(n) != 0 { n += 1; }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(g);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

pub fn clipboard_set(s: &str) {
    use windows::Win32::System::{DataExchange::*, Memory::*};
    unsafe {
        if OpenClipboard(None).is_err() { return; }
        let _ = EmptyClipboard();
        let w: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
        if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let p = GlobalLock(g) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(g);
                let _ = SetClipboardData(CF_UNICODETEXT, HANDLE(g.0)); // the clipboard owns the block now
            }
        }
        let _ = CloseClipboard();
    }
}

pub fn hostname() -> String {
    let mut buf = [0u16; 256];
    let mut n = buf.len() as u32;
    unsafe { if GetComputerNameExW(ComputerNameDnsHostname, PWSTR(buf.as_mut_ptr()), &mut n).is_err() { return String::new(); } }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Monitor rectangles (physical px) and the primary monitor's work area.
pub fn monitors() -> (Vec<RECT>, RECT) {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Vec<(RECT, RECT, bool)>);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut mi).as_bool() { v.push((mi.rcMonitor, mi.rcWork, mi.dwFlags & 1 != 0)); }
        TRUE
    }
    let mut v: Vec<(RECT, RECT, bool)> = vec![];
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut _ as isize)); }
    let work = v.iter().find(|m| m.2).map(|m| m.1).unwrap_or(RECT { left: 0, top: 0, right: 1920, bottom: 1040 });
    (v.into_iter().map(|m| m.0).collect(), work)
}

/// A monitor's top-left corner on the desktop: unique per monitor, and the same however the monitor is rotated
/// (a rotated one can report its unrotated size).
type Spot = (i32, i32);

/// The name each active monitor reports (its EDID, e.g. "DELL U2723QE"), with the monitor's top-left corner
/// in desktop pixels so callers can match it to a monitor rectangle. A monitor with no name is left out.
pub fn monitor_names() -> Vec<(Spot, String)> {
    use windows::Win32::{Devices::Display::*, Foundation::WIN32_ERROR};
    let (mut np, mut nm) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != WIN32_ERROR(0) { return vec![]; }
        let (mut paths, mut modes) = (vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize], vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize]);
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != WIN32_ERROR(0) { return vec![]; }
        paths.truncate(np as usize);
        modes.truncate(nm as usize);
        paths.iter().filter_map(|p| {
            let src = modes.iter().find(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE && m.id == p.sourceInfo.id && m.adapterId.LowPart == p.sourceInfo.adapterId.LowPart && m.adapterId.HighPart == p.sourceInfo.adapterId.HighPart)?;
            let s = src.Anonymous.sourceMode;
            let mut t = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            t.header = DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32, adapterId: p.targetInfo.adapterId, id: p.targetInfo.id };
            if DisplayConfigGetDeviceInfo(&mut t.header) != 0 { return None; }
            let n = t.monitorFriendlyDeviceName;
            let name = String::from_utf16_lossy(&n[..n.iter().position(|c| *c == 0).unwrap_or(n.len())]).trim().to_string();
            (!name.is_empty()).then_some(((s.position.x, s.position.y), name))
        }).collect()
    }
}

/// The name of the monitor at `spot` from `monitor_names`, else "Display {n}" (n counts from 0 here, from 1 on screen).
/// Two monitors with the same name get " 1" / " 2" so they can be told apart.
pub fn monitor_label(names: &[(Spot, String)], spot: Spot, n: usize) -> String {
    let Some((_, name)) = names.iter().find(|(s, _)| *s == spot) else { return format!("Display {}", n + 1) };
    let same: Vec<&Spot> = names.iter().filter(|(_, o)| o == name).map(|(s, _)| s).collect();
    match same.iter().position(|s| **s == spot) { Some(i) if same.len() > 1 => format!("{name} {}", i + 1), _ => name.clone() }
}

/// CPU busy fraction since the previous call.
pub struct Cpu { last: (u64, u64) }
impl Cpu {
    pub fn new() -> Cpu { Cpu { last: Cpu::times() } }
    fn times() -> (u64, u64) {
        let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { let _ = GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)); }
        let f = |t: FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
        (f(i), f(k) + f(u)) // kernel time includes idle time
    }
    pub fn usage(&mut self) -> f32 {
        let now = Cpu::times();
        let (idle, total) = (now.0 - self.last.0, now.1 - self.last.1);
        self.last = now;
        if total == 0 { 0.0 } else { 1.0 - idle as f32 / total as f32 }
    }
}

pub fn memory_usage() -> f32 {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { let _ = GlobalMemoryStatusEx(&mut m); }
    m.dwMemoryLoad as f32 / 100.0
}

/// Used fraction of drive C:.
pub fn disk_usage() -> f32 {
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { let _ = GetDiskFreeSpaceExW(&HSTRING::from("C:\\"), None, Some(&mut total), Some(&mut free)); }
    if total == 0 { 0.0 } else { 1.0 - free as f32 / total as f32 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_web_urls_reach_curl() {
        assert!(web_url("https://a.b/c") && web_url("http://a.b"));
        assert!(!web_url("-o x") && !web_url("file:///c:/x") && !web_url(""));
        assert_eq!(http_get("--config x"), None);
    }
    use super::*;
    #[test]
    fn monitor_labels() {
        let names = vec![((0, 0), "DELL A".to_string()), ((-1080, 0), "DELL A".to_string()), ((1920, 0), "LG B".to_string())];
        assert_eq!(monitor_label(&names, (1920, 0), 2), "LG B");
        assert_eq!(monitor_label(&names, (0, 0), 0), "DELL A 1");
        assert_eq!(monitor_label(&names, (-1080, 0), 1), "DELL A 2");
        assert_eq!(monitor_label(&names, (5, 5), 3), "Display 4");
        assert_eq!(monitor_label(&[], (0, 0), 0), "Display 1");
    }
    #[test]
    fn calendar_maths() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        let st = SYSTEMTIME { wYear: 2026, wMonth: 12, wDay: 30, ..Default::default() };
        let t = add_days(&st, 3);
        assert_eq!((t.wYear, t.wMonth, t.wDay), (2027, 1, 2));
        assert_eq!(clock(0, 5, false), "12:05");
        assert_eq!(clock(16, 30, true), "16:30");
    }
    #[test]
    fn encoding() { assert_eq!(url_encode("a b&é"), "a%20b%26%C3%A9"); }
    #[test]
    fn zones() {
        assert_eq!(zone_offset("Asia/Kolkata"), Some(330));
        assert_eq!(zone_offset("Not/AZone"), None);
    }
}
