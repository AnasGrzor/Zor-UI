//! Rust port of scripts/toggles.ps1. Actions it can't do faithfully (brightness set, gamemode,
//! colorpick, screentranslate, timer-done) return None so the script still runs them.

use super::common::*;
use super::*;
use serde_json::json;
use std::process::Stdio;
use windows::{
    core::{w, BSTR, HSTRING, PCWSTR, VARIANT},
    ApplicationModel::DataTransfer::{Clipboard, SetHistoryItemAsContentStatus, StandardDataFormats},
    Devices::Radios::{Radio, RadioKind, RadioState},
    Win32::{
        Foundation::CloseHandle,
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            Power::SetSuspendState,
            Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
            Threading::{GetExitCodeProcess, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE},
            Wmi::{IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE},
        },
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    },
};

/// toggles.ps1 with its PowerShell-style arguments ("-Action", "wifi", "-Value", ...). None: not ported.
pub fn run(_cfg: &Config, args: &[&str]) -> Option<Value> {
    let (action, value) = parse_args(args);
    let action = action?;
    let ok = || Some(json!({ "ok": true }));
    match action.to_lowercase().as_str() {
        "status" => Some(json!({
            "wifi": radio_state(RadioKind::WiFi), "bluetooth": radio_state(RadioKind::Bluetooth),
            "caffeine": caffeine_on(), "darkMode": dark_mode(), "brightness": brightness(),
        })),
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
            shell("notepad", &format!("\"{p}\\.glzr\\zebar\\end4-pC\\config.json\""));
            ok()
        }
        "lock" => { let _ = hidden("rundll32.exe").args(["user32.dll,", "LockWorkStation"]).spawn(); ok() }
        "sleep" => { unsafe { let _ = SetSuspendState(false, false, false); } ok() }
        "logout" => { let _ = hidden("shutdown.exe").arg("/l").spawn(); ok() }
        "restart" => { let _ = hidden("shutdown.exe").args(["/r", "/t", "0"]).spawn(); ok() }
        "shutdown" => { let _ = hidden("shutdown.exe").args(["/s", "/t", "0"]).spawn(); ok() }
        "brightness" | "gamemode" | "colorpick" | "screentranslate" | "timer-done" => None,
        _ => Some(json!({ "error": format!("unknown action '{action}'") })),
    }
}

/// `-Action X -Value Y` (names case-insensitive, `-Name:value` too) -> (action, value).
fn parse_args(args: &[&str]) -> (Option<String>, String) {
    let (mut action, mut value) = (None, String::new());
    let mut i = 0;
    while i < args.len() {
        let Some(rest) = args[i].strip_prefix('-') else { i += 1; continue };
        let (name, inline) = match rest.split_once(':') { Some((n, v)) => (n, Some(v.to_string())), None => (rest, None) };
        let val = match inline { Some(v) => Some(v), None => { i += 1; args.get(i).map(|s| s.to_string()) } };
        match name.to_lowercase().as_str() { "action" => action = val, "value" => value = val.unwrap_or_default(), _ => {} }
        i += 1;
    }
    (action, value)
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

/// First monitor's WMI brightness (laptop panels); null when WMI has none.
fn brightness() -> Value {
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
        go().map_or(Value::Null, |b| json!(b))
    }
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

fn msg(e: windows::core::Error) -> String { e.message().to_string() }

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
        assert_eq!(parse_args(&["-action", "wifi", "-VALUE", "on"]), (Some("wifi".into()), "on".into()));
        assert_eq!(parse_args(&["-Action:status"]), (Some("status".into()), String::new()));
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
    fn b64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
