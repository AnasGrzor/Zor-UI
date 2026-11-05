//! Rust port of scripts/toggles.ps1. colorpick and screentranslate (full-screen Python tools)
//! return None so the script still runs them.

use super::common::*;
use super::*;
use serde_json::json;
use std::process::Stdio;
use windows::{
    core::{w, BSTR, HSTRING, PCWSTR, VARIANT},
    ApplicationModel::DataTransfer::{Clipboard, SetHistoryItemAsContentStatus, StandardDataFormats},
    Data::Xml::Dom::XmlDocument,
    Win32::{
        Devices::Display::{DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR},
        Foundation::{BOOL, HANDLE, LPARAM, RECT},
        Graphics::Gdi::{EnumDisplayMonitors, MonitorFromWindow, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST},
        UI::WindowsAndMessaging::GetForegroundWindow,
    },
    Devices::Radios::{Radio, RadioKind, RadioState},
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    Win32::{
        Foundation::CloseHandle,
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            Power::SetSuspendState,
            Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            Threading::{GetCurrentProcessId, GetExitCodeProcess, OpenMutexW, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, SYNCHRONIZATION_SYNCHRONIZE},
            Wmi::{IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE},
        },
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    },
};

/// toggles.ps1 with its PowerShell-style arguments ("-Action", "wifi", "-Value", ...). None: not ported.
pub fn run(_cfg: &Config, args: &[&str]) -> Option<Value> {
    let (action, value, monitor) = parse_args(args);
    let action = action?;
    // "active", or a 0-based index into `monitors`
    let mon = if monitor.eq_ignore_ascii_case("active") { active_monitor() } else { monitor.trim().parse::<usize>().ok().and_then(|i| hmonitors().get(i).copied()) };
    let ok = || Some(json!({ "ok": true }));
    match action.to_lowercase().as_str() {
        "status" => Some(json!({
            "wifi": radio_state(RadioKind::WiFi), "bluetooth": radio_state(RadioKind::Bluetooth),
            "caffeine": caffeine_on(), "darkMode": dark_mode(), "brightness": brightness(mon),
        })),
        "monitors" => {
            // Every monitor with a brightness: DDC/CI ones, and the laptop panel (WMI) on the first one without DDC.
            let mut panel = wmi_brightness();
            let list: Vec<Value> = hmonitors().into_iter().enumerate().filter_map(|(i, m)| {
                let b = ddc_brightness(Some(m)).or_else(|| panel.take())?;
                Some(json!({ "name": format!("Display {}", i + 1), "i": i, "brightness": b }))
            }).collect();
            Some(json!({ "monitors": list }))
        }
        "wifi" => Some(json!({ "wifi": set_radio(RadioKind::WiFi, &value) })),
        "bluetooth" => Some(json!({ "bluetooth": set_radio(RadioKind::Bluetooth, &value) })),
        "caffeine" => Some(json!({ "caffeine": switch_caffeine() })),
        "powerplan" => {
            let mut plans = power_plans();
            if value.eq_ignore_ascii_case("next") && plans.len() > 1 {
                let i = plans.iter().position(|p| p.2).unwrap_or(0);
                let _ = hidden("powercfg").args(["/setactive", &plans[(i + 1) % plans.len()].0]).output();
                plans = power_plans();
            }
            Some(json!({ "plans": plans.iter().map(|p| json!({ "id": p.0, "name": p.1, "active": p.2 })).collect::<Vec<_>>() }))
        }
        "clipboard" => Some(clipboard_list().unwrap_or_else(|e| json!({ "error": e }))),
        "clipboard-set" => {
            let i: usize = value.trim().parse().ok()?; // negative / junk indexes behave oddly in PowerShell: leave to the script
            Some(clipboard_set_item(i).unwrap_or_else(|e| json!({ "error": e })))
        }
        "osk" => { shell("osk", ""); ok() }
        "snip" => { shell("ms-screenclip:", ""); ok() }
        "settings" => {
            let page = if value.chars().all(|c| c.is_ascii_alphabetic() || c == '-') { value.as_str() } else { "" };
            shell(&format!("ms-settings:{page}"), "");
            ok()
        }
        "taskmgr" => { shell("taskmgr", ""); ok() }
        "actioncenter" => { shell("ms-actioncenter:", ""); ok() }
        "editconfig" => {
            let p = std::env::var("USERPROFILE").unwrap_or_default();
            shell("notepad", &format!("\"{p}\\.glzr\\zebar\\zor-ui\\config.json\""));
            ok()
        }
        "lock" => { let _ = hidden("rundll32.exe").args(["user32.dll,", "LockWorkStation"]).spawn(); ok() }
        "sleep" => { unsafe { let _ = SetSuspendState(false, false, false); } ok() }
        "logout" => { let _ = hidden("shutdown.exe").arg("/l").spawn(); ok() }
        "restart" => { let _ = hidden("shutdown.exe").args(["/r", "/t", "0"]).spawn(); ok() }
        "shutdown" => { let _ = hidden("shutdown.exe").args(["/s", "/t", "0"]).spawn(); ok() }
        "brightness" => {
            let pct: i32 = match value.trim().parse() { Ok(p) => p, Err(_) => return Some(json!({ "error": format!("Cannot convert value \"{value}\" to type \"System.Int32\".") })) };
            Some(match set_brightness(mon, pct.clamp(1, 100)) { Ok(p) => json!({ "brightness": p }), Err(e) => json!({ "error": e }) })
        }
        "gamemode" => Some(json!({ "gamemode": game_mode() })),
        "timer-done" => Some(timer_done(&value)),
        // full-screen Python tools: still the script
        "colorpick" | "screentranslate" => None,
        _ => Some(json!({ "error": format!("unknown action '{action}'") })),
    }
}

/// `-Action X -Value Y [-Monitor active]` (names case-insensitive, `-Name:value` too) -> (action, value, monitor).
/// `-Monitor active` limits brightness to the monitor the foreground window is on.
fn parse_args(args: &[&str]) -> (Option<String>, String, String) {
    let (mut action, mut value, mut monitor) = (None, String::new(), String::new());
    let mut i = 0;
    while i < args.len() {
        let Some(rest) = args[i].strip_prefix('-') else { i += 1; continue };
        let (name, inline) = match rest.split_once(':') { Some((n, v)) => (n, Some(v.to_string())), None => (rest, None) };
        let val = match inline { Some(v) => Some(v), None => { i += 1; args.get(i).map(|s| s.to_string()) } };
        match name.to_lowercase().as_str() { "action" => action = val, "value" => value = val.unwrap_or_default(), "monitor" => monitor = val.unwrap_or_default(), _ => {} }
        i += 1;
    }
    (action, value, monitor)
}

fn find_radio(kind: RadioKind) -> Option<Radio> {
    let _ = Radio::RequestAccessAsync().ok()?.get();
    Radio::GetRadiosAsync().ok()?.get().ok()?.into_iter().find(|r| r.Kind().ok() == Some(kind))
}

fn radio_state(kind: RadioKind) -> Value {
    find_radio(kind).and_then(|r| r.State().ok()).map_or(Value::Null, |s| json!(s == RadioState::On))
}

/// Switches a radio ("on" / "off" / anything else = toggle); the new state, null if there is no such radio.
fn set_radio(kind: RadioKind, want: &str) -> Value {
    let Some(r) = find_radio(kind) else { return Value::Null };
    let on = r.State().ok() == Some(RadioState::On);
    let target = match want.to_lowercase().as_str() { "on" => true, "off" => false, _ => !on };
    if let Ok(op) = r.SetStateAsync(if target { RadioState::On } else { RadioState::Off }) { let _ = op.get(); }
    json!(target)
}

fn pid_file() -> std::path::PathBuf { root().join("caffeine.pid") }

fn caffeine_pid() -> Option<u32> { std::fs::read_to_string(pid_file()).ok()?.trim().parse().ok() }

fn caffeine_on() -> bool {
    let Some(pid) = caffeine_pid() else { return false };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code).is_ok() && code == 259; // STILL_ACTIVE
        let _ = CloseHandle(h);
        alive
    }
}

/// Toggles keep-awake: a hidden PowerShell holds ES_CONTINUOUS|SYSTEM|DISPLAY_REQUIRED (its pid is in caffeine.pid).
fn switch_caffeine() -> bool {
    if caffeine_on() {
        if let Some(pid) = caffeine_pid() {
            unsafe { if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) { let _ = TerminateProcess(h, 1); let _ = CloseHandle(h); } }
        }
        let _ = std::fs::remove_file(pid_file());
        return false;
    }
    let code = "Add-Type -Name P -Namespace W -MemberDefinition '[DllImport(\"kernel32.dll\")] public static extern uint SetThreadExecutionState(uint f);'\r\n\
                [W.P]::SetThreadExecutionState([uint32]\"0x80000003\") | Out-Null\r\nwhile ($true) { Start-Sleep -Seconds 3600 }";
    let utf16: Vec<u8> = code.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let child = hidden("powershell").args(["-NoProfile", "-EncodedCommand", &base64(&utf16)])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        Ok(c) => { let _ = std::fs::create_dir_all(root()); let _ = std::fs::write(pid_file(), c.id().to_string()); true }
        Err(_) => false,
    }
}

fn base64(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() { s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char) } else { s.push('=') }
        }
    }
    s
}

/// AppsUseLightTheme == 0.
fn dark_mode() -> bool {
    let mut v = 1u32;
    let mut n = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut n))
    };
    r.is_ok() && v == 0
}

/// Brightness of the monitor `mon` (DDC/CI), else the laptop panel (WMI); with no `mon`, the panel
/// first, then the first DDC/CI monitor. Null when nothing reports one.
fn brightness(mon: Option<HMONITOR>) -> Value {
    let b = if mon.is_some() { ddc_brightness(mon).or_else(wmi_brightness) } else { wmi_brightness().or_else(|| ddc_brightness(None)) };
    b.map_or(Value::Null, |b| json!(b))
}

/// First WMI panel's brightness (laptops).
fn wmi_brightness() -> Option<u8> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let go = || -> Option<u8> {
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let svc = loc.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).ok()?;
            let en = svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from("SELECT CurrentBrightness FROM WmiMonitorBrightness"),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None).ok()?;
            let mut objs = [None];
            let mut n = 0u32;
            en.Next(WBEM_INFINITE, &mut objs, &mut n).ok().ok()?;
            let o = objs[0].take()?;
            let mut v = VARIANT::default();
            o.Get(w!("CurrentBrightness"), 0, &mut v, None, None).ok()?;
            u32::try_from(&v).ok().map(|b| b as u8)
        };
        go()
    }
}

/// Every monitor, in the order EnumDisplayMonitors gives them.
fn hmonitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        (*(data.0 as *mut Vec<HMONITOR>)).push(m);
        BOOL(1)
    }
    let mut v: Vec<HMONITOR> = vec![];
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut _ as isize)); }
    v
}

/// The monitor the foreground window is on: the sidebar has focus while its slider is used.
fn active_monitor() -> Option<HMONITOR> {
    let m = unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST) };
    (!m.is_invalid()).then_some(m)
}

/// Runs `f` on the physical-monitor handle of `only` (every display when None) over DDC/CI: external
/// monitors, which WMI can't reach.
fn ddc_each(only: Option<HMONITOR>, mut f: impl FnMut(HANDLE)) {
    struct Each<'a> { only: Option<HMONITOR>, f: &'a mut dyn FnMut(HANDLE) }
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let e = &mut *(data.0 as *mut Each);
        if e.only.is_some_and(|o| o != m) { return BOOL(1); }
        let mut n = 0u32;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(m, &mut n).is_ok() && n > 0 {
            let mut a = vec![PHYSICAL_MONITOR::default(); n as usize];
            if GetPhysicalMonitorsFromHMONITOR(m, &mut a).is_ok() {
                for p in &a { let h = p.hPhysicalMonitor; // packed struct: copy the field, don't borrow it
                    (e.f)(h); }
                let _ = DestroyPhysicalMonitors(&a);
            }
        }
        BOOL(1)
    }
    let mut e = Each { only, f: &mut f };
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut e as *mut Each as isize)); }
}

/// A monitor's raw brightness range <-> percent.
fn raw_to_pct(raw: u32, min: u32, max: u32) -> u32 { if max <= min { 0 } else { (raw.saturating_sub(min) * 100 + (max - min) / 2) / (max - min) } }
fn pct_to_raw(pct: u32, min: u32, max: u32) -> u32 { min + ((max - min) * pct + 50) / 100 }

/// First DDC/CI monitor (of `only`) that reports a brightness, as a percentage.
fn ddc_brightness(only: Option<HMONITOR>) -> Option<u8> {
    let mut found = None;
    ddc_each(only, |h| unsafe {
        let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
        if found.is_none() && GetMonitorBrightness(h, &mut min, &mut cur, &mut max) != 0 { found = Some(raw_to_pct(cur, min, max) as u8); }
    });
    found
}

/// Sets `only` (every monitor when None) over DDC/CI; how many accepted it.
fn set_ddc_brightness(only: Option<HMONITOR>, pct: i32) -> usize {
    let mut done = 0;
    ddc_each(only, |h| unsafe {
        let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
        if GetMonitorBrightness(h, &mut min, &mut cur, &mut max) != 0 && SetMonitorBrightness(h, pct_to_raw(pct as u32, min, max)) != 0 { done += 1; }
    });
    done
}

/// With `mon`: that monitor over DDC/CI, else the laptop panel. Without: the panel, else every DDC/CI monitor.
/// The percentage set.
fn set_brightness(mon: Option<HMONITOR>, pct: i32) -> Result<i32, String> {
    if mon.is_some() && set_ddc_brightness(mon, pct) > 0 { return Ok(pct); }
    set_wmi_brightness(pct).or_else(|e| if mon.is_none() && set_ddc_brightness(None, pct) > 0 { Ok(pct) } else { Err(e) })
}

/// WmiSetBrightness on the first panel with brightness control (laptops); the percentage set.
fn set_wmi_brightness(pct: i32) -> Result<i32, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).map_err(msg)?;
        let svc = loc.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).map_err(msg)?;
        let en = svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from("SELECT __PATH FROM WmiMonitorBrightnessMethods"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None).map_err(msg)?;
        let mut objs = [None];
        let mut n = 0u32;
        en.Next(WBEM_INFINITE, &mut objs, &mut n).ok().map_err(msg)?;
        let o = objs[0].take().ok_or("This display has no brightness control.")?;
        let mut v = VARIANT::default();
        o.Get(w!("__PATH"), 0, &mut v, None, None).map_err(msg)?;
        let path = BSTR::try_from(&v).map_err(msg)?;
        let mut class = None;
        svc.GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), Default::default(), None, Some(&mut class), None).map_err(msg)?;
        let mut sig = None;
        class.ok_or("WmiMonitorBrightnessMethods is missing.")?.GetMethod(w!("WmiSetBrightness"), 0, &mut sig, std::ptr::null_mut()).map_err(msg)?;
        let inp = sig.ok_or("WmiSetBrightness is missing.")?.SpawnInstance(0).map_err(msg)?;
        inp.Put(w!("Timeout"), 0, &VARIANT::from(1i32), 0).map_err(msg)?;
        inp.Put(w!("Brightness"), 0, &VARIANT::from(pct), 0).map_err(msg)?;
        svc.ExecMethod(&path, &BSTR::from("WmiSetBrightness"), Default::default(), None, &inp, None, None).map_err(msg)?;
        Ok(pct)
    }
}

/// Process ids of every process whose exe name (lower case) is in `names`.
fn pids(names: &[&str]) -> Vec<u32> {
    let mut out = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if names.contains(&String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase().as_str()) { out.push(e.th32ProcessID); }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn kill(pid: u32) {
    unsafe { if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) { let _ = TerminateProcess(h, 1); let _ = CloseHandle(h); } }
}

/// Game mode: frees RAM by stopping the native shell, Zebar, Flow Launcher and the live wallpaper; run
/// again to bring them back. "On" = neither the shell (it holds the single-instance mutex) nor Zebar
/// runs, so no state file can go stale. GlazeWM keeps running so the key still works. Returns the new state.
fn game_mode() -> bool {
    let shell_up = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("zor-ui-native")).map(|h| { let _ = CloseHandle(h); }).is_ok() };
    let stopping = shell_up || !pids(&["zebar.exe"]).is_empty();
    let we = super::we::we_running();
    if stopping {
        pids(&["zebar.exe", "flow.launcher.exe"]).into_iter().for_each(kill);
        if we { let _ = super::we::we_control(&["stop"], 15); }
        let (me, others) = (unsafe { GetCurrentProcessId() }, pids(&["zorui.exe"]));
        others.iter().filter(|&&p| p != me).for_each(|&p| kill(p));
        // run from the shell itself (no other zorui.exe): it goes last
        if shell_up && others.iter().all(|&p| p == me) { std::process::exit(0); }
        true
    } else {
        if we { let _ = super::we::we_control(&["play"], 15); }
        let null = || (Stdio::null(), Stdio::null());
        let launch = |mut c: std::process::Command| { let (o, e) = null(); let _ = c.stdin(Stdio::null()).stdout(o).stderr(e).spawn(); };
        launch(hidden("zebar"));
        if let Ok(me) = std::env::current_exe() { launch(hidden(me)); }
        let flow = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("FlowLauncher").join("Flow.Launcher.exe");
        if flow.is_file() { launch(hidden(flow)); }
        false
    }
}

/// Windows toast when a pomodoro phase ends; `phase` is focus / short / long.
fn timer_done(phase: &str) -> Value {
    let text = match phase {
        "focus" => "Focus session done. Take a break.",
        "short" => "Break is over. Back to focus.",
        "long" => "Long break is over. Back to focus.",
        _ => return json!({ "error": format!("unknown phase '{phase}'") }),
    };
    let go = || -> windows::core::Result<()> {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(format!("<toast><visual><binding template='ToastGeneric'><text>Pomodoro</text><text>{text}</text></binding></visual><audio src='ms-winsoundevent:Notification.Reminder'/></toast>")))?;
        // PowerShell's registered AUMID: toasts need a registered app id
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe"))?
            .Show(&ToastNotification::CreateToastNotification(&xml)?)
    };
    match go() { Ok(()) => json!({ "ok": true }), Err(e) => json!({ "error": msg(e) }) }
}

/// (guid, name, active) from `powercfg /list`, matching the script's regex `([0-9a-f-]{36})\s+\((.+)\)(\s*\*)?`.
fn power_plans() -> Vec<(String, String, bool)> {
    let out = hidden("powercfg").arg("/list").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    out.lines().filter_map(parse_plan).collect()
}

fn parse_plan(line: &str) -> Option<(String, String, bool)> {
    let b = line.as_bytes();
    for s in 0..b.len().saturating_sub(35) {
        if !b[s..s + 36].iter().all(|&c| c.is_ascii_hexdigit() || c == b'-') { continue; }
        let rest = &line[s + 36..];
        let t = rest.trim_start();
        if t.len() == rest.len() || !t.starts_with('(') { continue; }
        let close = t.rfind(')')?;
        if close < 2 { continue; }
        return Some((line[s..s + 36].to_string(), t[1..close].to_string(), t[close + 1..].trim_start().starts_with('*')));
    }
    None
}

/// Win+V history, at most 30 items.
fn clipboard_list() -> Result<Value, String> {
    let r = history_items()?;
    let mut list = vec![];
    for (i, it) in r.into_iter().take(30).enumerate() {
        let c = it.Content().map_err(msg)?;
        let text = if c.Contains(&HSTRING::from("Text")).unwrap_or(false) {
            let t = c.GetTextAsync().and_then(|o| o.get()).map_err(msg)?.to_string();
            let u: Vec<u16> = t.encode_utf16().take(400).collect();
            json!(String::from_utf16_lossy(&u))
        } else { Value::Null };
        let image = StandardDataFormats::Bitmap().ok().and_then(|f| c.Contains(&f).ok()).unwrap_or(false);
        let time = it.Timestamp().map_err(msg)?.UniversalTime.div_euclid(10_000_000) - 11_644_473_600;
        list.push(json!({ "i": i, "text": text, "image": image, "time": time }));
    }
    Ok(json!({ "items": list }))
}

fn clipboard_set_item(i: usize) -> Result<Value, String> {
    let item = history_items()?.into_iter().nth(i).ok_or("That clipboard item is gone.")?;
    let st = Clipboard::SetHistoryItemAsContent(&item).map_err(msg)?;
    Ok(json!({ "ok": st == SetHistoryItemAsContentStatus::Success }))
}

fn history_items() -> Result<windows::Foundation::Collections::IVectorView<windows::ApplicationModel::DataTransfer::ClipboardHistoryItem>, String> {
    if !Clipboard::IsHistoryEnabled().map_err(msg)? {
        return Err("Clipboard history is off. Turn it on in Settings > System > Clipboard (or press Win+V).".into());
    }
    Clipboard::GetHistoryItemsAsync().and_then(|o| o.get()).and_then(|r| r.Items()).map_err(msg)
}

fn msg(e: windows::core::Error) -> String {
    let m = e.message().to_string();
    if m.trim().is_empty() { format!("Windows error 0x{:08X}", e.code().0 as u32) } else { m }
}

/// Start-Process equivalent: ShellExecute "open".
fn shell(file: &str, params: &str) {
    let (f, p) = (HSTRING::from(file), HSTRING::from(params));
    unsafe { ShellExecuteW(None, w!("open"), &f, if params.is_empty() { PCWSTR::null() } else { PCWSTR(p.as_ptr()) }, PCWSTR::null(), SW_SHOWNORMAL); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        assert_eq!(parse_args(&["-action", "wifi", "-VALUE", "on"]), (Some("wifi".into()), "on".into(), String::new()));
        assert_eq!(parse_args(&["-Action:status", "-Monitor", "active"]), (Some("status".into()), String::new(), "active".into()));
        assert_eq!(parse_args(&[]).0, None);
    }

    #[test]
    fn plans() {
        let l = "Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced) *";
        assert_eq!(parse_plan(l), Some(("381b4222-f694-41f0-9685-ff5bb260df2e".into(), "Balanced".into(), true)));
        let l = "Power Scheme GUID: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c  (High performance (x))";
        assert_eq!(parse_plan(l).map(|p| (p.1, p.2)), Some(("High performance (x)".into(), false)));
        assert_eq!(parse_plan("Existing Power Schemes (* Active)"), None);
    }

    #[test]
    fn ddc_range() {
        assert_eq!((raw_to_pct(75, 0, 100), pct_to_raw(75, 0, 100)), (75, 75));
        assert_eq!((raw_to_pct(50, 0, 255), pct_to_raw(100, 0, 255), pct_to_raw(1, 10, 110)), (20, 255, 11));
        assert_eq!(raw_to_pct(5, 7, 7), 0);
    }

    #[test]
    fn b64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
