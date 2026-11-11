//! System commands: config-set, open, nightlight, mixer, app-icons, launch.

use super::common::*;
use super::*;
use serde_json::json;
use std::{collections::BTreeMap, path::Path};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT, TRUE},
        Graphics::Gdi::{CreateDCW, DeleteDC, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW},
        Media::Audio::{eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator},
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{
            ColorSystem::SetDeviceGammaRamp,
            Shell::ShellExecuteW,
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

/// One-shot command `name`; None if this module doesn't port it (or got arguments the script would reject).
pub fn call(_cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    Some(match (name, args) {
        ("config-set", [k, v]) => config_set(&pack().join("config.json"), k, v),
        ("open", [url]) => open(url),
        ("nightlight", [] | [_] | [_, _]) => nightlight(&root().join("nightlight.json"), args.first().copied().unwrap_or("status"), args.get(1).copied().unwrap_or(""))?,
        ("mixer", [] | [_] | [_, _] | [_, _, _]) => mixer(args.first().copied().unwrap_or("list"), args.get(1).copied().unwrap_or(""), args.get(2).copied().unwrap_or("")),
        ("app-icons", _) => app_icons(args),
        ("launch", [n]) => launch(n),
        _ => return None,
    })
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Sets a dotted key in config.json (value is JSON).
fn config_set(config: &Path, key: &str, value_json: &str) -> Value {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') { return err("bad key"); }
    let Ok(value) = serde_json::from_str::<Value>(value_json) else { return err("bad value") };
    let mut cfg = load_json(config, json!({}));
    if !cfg.is_object() { cfg = json!({}); }
    let mut node = &mut cfg;
    let parts: Vec<&str> = key.split('.').collect();
    for p in &parts[..parts.len() - 1] {
        let Some(map) = node.as_object_mut() else { return err("bad key") };
        let child = map.entry(p.to_string()).or_insert(Value::Null);
        if !child.is_object() { *child = json!({}); }
        node = child;
    }
    if let (Some(map), Some(last)) = (node.as_object_mut(), parts.last()) { map.insert(last.to_string(), value); }
    if !save_json(config, &cfg) { return err("couldn't write config.json"); }
    json!({ "ok": true })
}

// ---------------------------------------------------------------- open / launch

/// ShellExecute "open" on a URL or path (default browser / default app).
/// True if Windows accepted it: ShellExecute returns a value above 32 on success.
pub(crate) fn shell_open(target: &str) -> bool {
    let t = wide(target);
    let r = unsafe { ShellExecuteW(HWND::default(), windows::core::w!("open"), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Where the pointer should jump: the center of the window that took the foreground, unless the
/// user already moved the pointer more than 40 px from where the click left it.
fn point_target(start: (i32, i32), now: (i32, i32), win: (i32, i32, i32, i32)) -> Option<(i32, i32)> {
    let moved = (now.0 - start.0).abs() > 40 || (now.1 - start.1).abs() > 40;
    (!moved).then_some(((win.0 + win.2) / 2, (win.1 + win.3) / 2))
}

/// `shell_open`, then moves the pointer to the center of the window that comes to the front (waits up to 3 s).
pub(crate) fn open_and_point(target: &str) {
    use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowRect, IsIconic, SetCursorPos}};
    let before = unsafe { GetForegroundWindow() }.0 as isize; // HWND isn't Send
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    shell_open(target);
    std::thread::spawn(move || {
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            unsafe {
                let fg = GetForegroundWindow();
                if fg.0 as isize == before || fg.0.is_null() || IsIconic(fg).as_bool() { continue; }
                let (mut now, mut r) = (POINT::default(), RECT::default());
                if GetCursorPos(&mut now).is_err() || GetWindowRect(fg, &mut r).is_err() { return; }
                if let Some((x, y)) = point_target((p.x, p.y), (now.x, now.y), (r.left, r.top, r.right, r.bottom)) { let _ = SetCursorPos(x, y); }
                return;
            }
        }
    });
}

fn open(url: &str) -> Value {
    if !(url.starts_with("http://") || url.starts_with("https://")) { return err("only http(s) links"); }
    shell_open(url);
    json!({ "ok": true })
}

/// Apps pinned in config.json's dock.pinned (entries with a name and a path).
fn dock_pinned() -> Vec<(String, String)> {
    let cfg = load_json(&pack().join("config.json"), json!({}));
    let list = cfg.get("dock").and_then(|d| d.get("pinned")).and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter().filter_map(|p| {
        let (n, path) = (p.get("name")?.as_str()?, p.get("path")?.as_str()?);
        (!n.is_empty() && !path.is_empty()).then(|| (n.to_string(), path.to_string()))
    }).collect()
}

/// Starts an app pinned to the dock; only paths saved in config.json can be started.
fn launch(name: &str) -> Value {
    if let Some((_, p)) = dock_pinned().into_iter().find(|(n, p)| n == name && Path::new(p).is_file()) {
        shell_open(&p);
        return json!({ "ok": true });
    }
    err(format!("{name} isn't pinned"))
}

// ---------------------------------------------------------------- app-icons

/// Full exe path of process `pid`, if we may query it.
pub(crate) fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(buf.get(..n as usize)?))
    }
}

/// `[\w .+-]{1,80}`
fn icon_name_ok(n: &str) -> bool {
    (1..=80).contains(&n.chars().count()) && n.chars().all(|c| c.is_alphanumeric() || " _.+-".contains(c))
}

/// Saves a PNG icon per app into the pack's icons folder and returns each app's exe path.
fn app_icons(names: &[&str]) -> Value {
    let want: BTreeMap<String, &str> = names.iter().filter(|n| icon_name_ok(n)).map(|n| (n.to_lowercase(), *n)).collect();
    let mut exes: BTreeMap<String, String> = dock_pinned().into_iter().filter(|(n, _)| want.contains_key(&n.to_lowercase())).collect();
    unsafe {
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut more = Process32FirstW(snap, &mut e).is_ok();
            while more {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let n = String::from_utf16_lossy(e.szExeFile.get(..len).unwrap_or_default());
                let n = n.strip_suffix(".exe").or_else(|| n.strip_suffix(".EXE")).unwrap_or(&n);
                if let Some(orig) = want.get(&n.to_lowercase()) {
                    if !exes.contains_key(*orig) {
                        if let Some(p) = exe_of(e.th32ProcessID) { exes.insert(orig.to_string(), p); }
                    }
                }
                more = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
    }
    let folder = pack().join("icons");
    let _ = std::fs::create_dir_all(&folder);
    let todo: Vec<(&String, &String)> = exes.iter().filter(|(n, e)| !folder.join(format!("{n}.png")).exists() && Path::new(e.as_str()).is_file()).collect();
    if !todo.is_empty() {
        let ps = icon_script(&todo, &folder);
        // shortcut: waits for PowerShell; use ExtractIconEx + a PNG encoder to drop it
        let _ = hidden("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps]).output();
    }
    json!({ "exes": exes })
}

/// The PowerShell that extracts each exe's icon to `<folder>\<name>.png`.
fn icon_script(todo: &[(&String, &String)], folder: &Path) -> String {
    // PowerShell also takes the curly single quotes as quote characters
    let q = |s: &str| format!("'{}'", s.replace(['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'], "''"));
    let saves: Vec<String> = todo.iter().map(|(n, e)| format!("try {{ [Drawing.Icon]::ExtractAssociatedIcon({}).ToBitmap().Save({}) }} catch {{ }}", q(e), q(&folder.join(format!("{n}.png")).to_string_lossy()))).collect();
    format!("Add-Type -AssemblyName System.Drawing; {}", saves.join("; "))
}

// ---------------------------------------------------------------- nightlight

/// Tanner Helland's colour-temperature approximation, as 0..1 multipliers.
fn kelvin_rgb(k: f64) -> (f64, f64, f64) {
    let t = k / 100.0;
    let r = if t <= 66.0 { 1.0 } else { (329.698727446 * (t - 60.0).powf(-0.1332047592) / 255.0).clamp(0.0, 1.0) };
    let g = if t <= 66.0 { (99.4708025861 * t.ln() - 161.1195681661) / 255.0 } else { 288.1221695283 * (t - 60.0).powf(-0.0755148492) / 255.0 };
    let b = if t >= 66.0 { 1.0 } else if t <= 19.0 { 0.0 } else { (138.5177312231 * (t - 10.0).ln() - 305.0447927307) / 255.0 };
    (r, g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

/// The 3x256 gamma ramp for a colour temperature (None: neutral).
fn gamma_ramp(kelvin: Option<i64>) -> [u16; 768] {
    let (r, g, b) = kelvin.map_or((1.0, 1.0, 1.0), |k| kelvin_rgb(k as f64));
    let mut ramp = [0u16; 768];
    for i in 0..256 {
        for (c, f) in [r, g, b].into_iter().enumerate() { ramp[c * 256 + i] = ((i as f64 * 257.0 * f) as u32).min(65535) as u16; }
    }
    ramp
}

unsafe extern "system" fn each_monitor(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    let devices = &mut *(lp.0 as *mut Vec<[u16; 32]>);
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(m, &mut mi as *mut _ as *mut MONITORINFO).as_bool() { devices.push(mi.szDevice); }
    TRUE
}

/// Applies the ramp to every monitor: (displays that accepted it, displays found).
fn set_gamma(kelvin: Option<i64>) -> (usize, usize) {
    let ramp = gamma_ramp(kelvin);
    let mut devices: Vec<[u16; 32]> = vec![];
    let mut ok = 0;
    unsafe {
        let _ = EnumDisplayMonitors(HDC::default(), None, Some(each_monitor), LPARAM(&mut devices as *mut _ as isize));
        for d in &devices {
            let hdc = CreateDCW(PCWSTR(d.as_ptr()), PCWSTR::null(), PCWSTR::null(), None);
            if !hdc.is_invalid() {
                if SetDeviceGammaRamp(hdc, ramp.as_ptr() as *const _).as_bool() { ok += 1; }
                let _ = DeleteDC(hdc);
            }
        }
    }
    (ok, devices.len())
}

/// nightlight [status|on|off|toggle] [kelvin]; state lives in `state_file`. None: unparsable kelvin (the script errors).
fn nightlight(state_file: &Path, action: &str, kelvin: &str) -> Option<Value> {
    let mut st = load_json(state_file, json!({ "on": false, "kelvin": 4500 }));
    let k = if !kelvin.is_empty() { kelvin.trim().parse::<i64>().ok()? } else {
        let c = load_json(&pack().join("config.json"), json!({}));
        let v = c.get("nightLight").and_then(|n| n.get("kelvin")).or_else(|| st.get("kelvin"));
        v.and_then(Value::as_f64).map_or(4500, |f| f as i64)
    };
    let on_now = st.get("on").is_some_and(truthy);
    let action = if action == "toggle" { if on_now { "off" } else { "on" } } else { action };
    if action == "on" || action == "off" {
        let (ok, total) = set_gamma((action == "on").then_some(k));
        if total > 0 && ok == 0 {
            return Some(json!({ "ok": false, "on": on_now, "error": "Windows refused the colour change on this display. Use Windows Night light instead." }));
        }
        st = json!({ "on": action == "on", "kelvin": k });
        save_json(state_file, &st);
    }
    let mut out = st.as_object().cloned().unwrap_or_default();
    out.insert("ok".into(), json!(true));
    Some(Value::Object(out))
}

// ---------------------------------------------------------------- mixer

/// Audio sessions on the default output, grouped by app name (Discord, browsers... run several).
fn mixer_groups() -> BTreeMap<String, Vec<ISimpleAudioVolume>> {
    let mut groups: BTreeMap<String, Vec<ISimpleAudioVolume>> = BTreeMap::new();
    let _ = (|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let m: IAudioSessionManager2 = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.Activate(CLSCTX_ALL, None).ok()?;
        let list = m.GetSessionEnumerator().ok()?;
        for i in 0..list.GetCount().ok()? {
            let Ok(c) = list.GetSession(i) else { continue };
            let (Ok(c2), Ok(vol)) = (c.cast::<IAudioSessionControl2>(), c.cast::<ISimpleAudioVolume>()) else { continue };
            let pid = c2.GetProcessId().unwrap_or(0);
            // shortcut: the script also hid its own Python's loopback session; that one is capture-side, so not listed here
            let name = match exe_of(pid).and_then(|e| Path::new(&e).file_stem().map(|s| s.to_string_lossy().into_owned())) {
                Some(n) => n,
                None if pid == 0 => "System sounds".into(),
                None => pid.to_string(),
            };
            groups.entry(name).or_default().push(vol);
        }
        Some(())
    })();
    groups
}

/// mixer [list] | mixer volume <app> <0-100> | mixer mute <app> <0|1>
fn mixer(action: &str, app: &str, value: &str) -> Value {
    let groups = mixer_groups();
    if action == "volume" || action == "mute" {
        let Some(sessions) = groups.get(app) else { return json!({ "error": format!("{app} isn't playing audio any more") }) };
        let vol = if action == "volume" {
            match value.trim().parse::<f32>() {
                Ok(v) => Some(v.clamp(0.0, 100.0) / 100.0),
                Err(_) => return json!({ "error": format!("could not convert string to float: '{value}'") }),
            }
        } else { None };
        for s in sessions {
            unsafe {
                match vol {
                    Some(v) => { let _ = s.SetMasterVolume(v, std::ptr::null()); }
                    None => { let _ = s.SetMute(value == "1", std::ptr::null()); }
                }
            }
        }
    }
    let mut apps: Vec<(&String, &Vec<ISimpleAudioVolume>)> = groups.iter().collect();
    apps.sort_by_key(|(n, _)| n.to_lowercase());
    let apps: Vec<Value> = apps.iter().filter_map(|(n, ss)| unsafe {
        let s = ss.first()?;
        Some(json!({ "app": n, "volume": (s.GetMasterVolume().unwrap_or(0.0) as f64 * 100.0).round() as i64, "muted": s.GetMute().is_ok_and(|m| m.as_bool()) }))
    }).collect();
    json!({ "apps": apps })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_jumps_to_the_window_center_unless_moved() {
        assert_eq!(point_target((100, 20), (100, 20), (0, 0, 1000, 600)), Some((500, 300)));
        assert_eq!(point_target((100, 20), (130, 50), (-1920, 0, 0, 1080)), Some((-960, 540))); // small jiggle, second monitor
        assert_eq!(point_target((100, 20), (400, 20), (0, 0, 1000, 600)), None); // user moved away
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("zor-ui-sys-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn kelvin_matches_python() {
        let (r, g, b) = kelvin_rgb(4500.0);
        assert_eq!(r, 1.0);
        assert!((g - 0.8530674700617857).abs() < 1e-9 && (b - 0.7350351155108239).abs() < 1e-9, "{g} {b}");
        assert_eq!(kelvin_rgb(6600.0), (1.0, 1.0, 1.0));
        let (r, g, b) = kelvin_rgb(1000.0);
        assert_eq!((r, b), (1.0, 0.0));
        assert!((g - 0.2663545845364998).abs() < 1e-9);
        let (r, g, b) = kelvin_rgb(10000.0);
        assert!(r < 1.0 && g < 1.0 && b == 1.0);
    }

    #[test]
    fn ramp_is_neutral_without_kelvin() {
        let r = gamma_ramp(None);
        assert_eq!((r[0], r[255], r[256 + 255], r[512 + 255]), (0, 65535, 65535, 65535));
        let w = gamma_ramp(Some(3000));
        assert_eq!(w[255], 65535);
        assert!(w[512 + 255] < w[255]);
    }

    #[test]
    fn config_set_nests() {
        let d = tmp("cfg");
        let c = d.join("config.json");
        assert_eq!(config_set(&c, "ai.model", "\"gem\""), json!({ "ok": true }));
        assert_eq!(load_json(&c, json!(null)), json!({ "ai": { "model": "gem" } }));
        assert_eq!(config_set(&c, "bad key", "1")["ok"], false);
        assert_eq!(config_set(&c, "a.b", "{oops")["ok"], false);
    }

    #[test]
    fn nightlight_status_reads_state() {
        let d = tmp("nl");
        let f = d.join("nightlight.json");
        std::fs::write(&f, r#"{"on":true,"kelvin":3200}"#).unwrap();
        assert_eq!(nightlight(&f, "status", "").unwrap(), json!({ "ok": true, "on": true, "kelvin": 3200 }));
        assert!(nightlight(&f, "status", "abc").is_none());
    }

    #[test]
    fn validators() {
        assert!(icon_name_ok("Visual Studio+1.x") && !icon_name_ok("a/b") && !icon_name_ok(""));
        assert_eq!(open("file:///c:/x")["ok"], false);
        assert!(truthy(&json!("a")) && !truthy(&json!(0)) && !truthy(&json!({})));
    }

    #[test]
    fn icon_script_curly_quotes() {
        let (n, e) = ("a\u{2019}b".to_string(), "C:/x.exe".to_string());
        assert!(icon_script(&[(&n, &e)], Path::new("C:/i")).contains("a''b.png'"));
    }

    #[test]
    fn icon_script_quotes() {
        let (n, e) = ("A'b".to_string(), "C:\\x\\it's.exe".to_string());
        let s = icon_script(&[(&n, &e)], Path::new("C:\\i"));
        assert!(s.contains("'C:\\x\\it''s.exe'") && s.contains("'C:\\i\\A''b.png'"), "{s}");
    }
}
