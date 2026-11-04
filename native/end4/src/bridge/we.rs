//! Wallpaper Engine commands of end4_bridge.py: we-status, we-list, we-apply, we-close, video-frame
//! and the we-sync stream. WE (Steam) is driven through its command line, `wallpaper64.exe -control ...`.

use super::common::*;
use super::*;
use serde_json::json;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Output,
    time::{Duration, Instant},
};
use windows::{
    core::{Interface, GUID, HSTRING, PCWSTR},
    Win32::{
        Foundation::GENERIC_READ,
        Graphics::Imaging::*,
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
        },
        System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
        UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS},
    },
};

/// Streaming commands this module runs.
pub const STREAMS: &[&str] = &["we-sync"];

const WE_APP: &str = "431960";
const VIDEO_EXT: &[&str] = &["mp4", "webm", "mkv", "mov", "avi", "wmv", "m4v"];
/// GENERIC_WRITE, which the windows crate keeps in another feature module.
const GENERIC_WRITE: u32 = 0x4000_0000;

/// One-shot command `name`; None if this module doesn't port it.
pub fn call(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    let arg = |i: usize| args.get(i).copied();
    Some(match name {
        "we-status" => we_status(),
        "we-list" => we_list(),
        "we-apply" => we_apply(cfg, arg(0)?, arg(1).unwrap_or("")),
        "we-close" => { we_close(); json!({ "ok": true }) }
        "video-frame" => video_frame(Path::new(arg(0)?), Path::new(arg(1)?)),
        _ => return None,
    })
}

/// Runs streaming command `name` (one of STREAMS) until it ends or `out.stopped()`.
pub fn stream(cfg: &Config, name: &str, _args: &[String], _stdin: &str, out: &Sink) {
    if name == "we-sync" { we_sync(cfg, out) }
}

// ---------------------------------------------------------------- Steam / WE discovery

/// Every `"path" "..."` in a libraryfolders.vdf (backslashes are escaped doubled in the file).
fn vdf_paths(text: &str) -> Vec<String> {
    let (mut found, mut rest) = (vec![], text);
    while let Some(i) = rest.find("\"path\"") {
        rest = &rest[i + 6..];
        let after = rest.trim_start();
        if after.len() == rest.len() { continue; } // the regex needs whitespace here
        if let Some(body) = after.strip_prefix('"') {
            if let Some(end) = body.find('"').filter(|&e| e > 0) { found.push(body[..end].replace("\\\\", "\\")); }
        }
    }
    found
}

/// A path with Windows separators (Steam's registry values use `/`).
fn norm(p: &str) -> PathBuf { PathBuf::from(p.replace('/', "\\")) }

fn reg_str(hive: HKEY, key: &str, name: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(hive, &HSTRING::from(key), &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)).ok().ok()?;
    }
    let n = (bytes as usize / 2).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string())
}

/// Steam library folders: END4_STEAM_LIBS (test override) or the install roots and the libraries they list.
fn steam_libraries() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var("END4_STEAM_LIBS") {
        return std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect();
    }
    let mut roots = vec![];
    for (hive, key) in [(HKEY_CURRENT_USER, r"Software\Valve\Steam"), (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam")] {
        for name in ["SteamPath", "InstallPath"] {
            if let Some(v) = reg_str(hive, key, name) { roots.push(norm(&v)); }
        }
    }
    roots.push(PathBuf::from(std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into())).join("Steam"));
    let mut libs = vec![];
    for root in roots {
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        if vdf.is_file() {
            let text = std::fs::read(&vdf).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            libs.extend(vdf_paths(&text).iter().map(|p| norm(p)));
        }
        libs.push(root);
    }
    let mut seen = std::collections::HashSet::new();
    libs.retain(|l| seen.insert(l.to_string_lossy().to_lowercase().trim_end_matches(['\\', '/']).to_string()) && l.exists());
    libs
}

/// Wallpaper Engine's folder: config's wallpaperEngine.path, else found in a Steam library.
fn we_dir() -> Option<PathBuf> {
    let cfg = Config::load();
    let custom = cfg.str("wallpaperEngine.path");
    if !custom.is_empty() && Path::new(custom).is_dir() { return Some(PathBuf::from(custom)); }
    steam_libraries().into_iter().map(|l| l.join("steamapps").join("common").join("wallpaper_engine")).find(|d| d.is_dir())
}

fn we_exe() -> Option<PathBuf> {
    let d = we_dir()?;
    ["wallpaper64.exe", "wallpaper32.exe"].iter().map(|n| d.join(n)).find(|p| p.is_file())
}

/// Is a Wallpaper Engine process alive?
pub(super) fn we_running() -> bool { !we_pids().is_empty() }

/// Process ids of every running Wallpaper Engine (wallpaper64.exe / wallpaper32.exe).
fn we_pids() -> Vec<u32> {
    let mut pids = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return pids };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase();
            if name == "wallpaper64.exe" || name == "wallpaper32.exe" { pids.push(e.th32ProcessID); }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    pids
}

/// Runs `cmd` for at most `secs`, capturing stdout; kills it on timeout (Python's subprocess timeout).
fn run_timeout(mut cmd: std::process::Command, secs: u64) -> std::io::Result<Output> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut pipe = child.stdout.take();
    let reader = std::thread::spawn(move || { let mut b = vec![]; if let Some(p) = pipe.as_mut() { let _ = p.read_to_end(&mut b); } b });
    let end = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(s) = child.try_wait()? { break s; }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output { status, stdout: reader.join().unwrap_or_default(), stderr: vec![] })
}

fn no_we() -> std::io::Error { std::io::Error::new(std::io::ErrorKind::NotFound, "Wallpaper Engine not found in your Steam libraries") }

/// `wallpaper64.exe -control <args>`.
pub(super) fn we_control(args: &[&str], secs: u64) -> std::io::Result<Output> {
    let mut c = hidden(we_exe().ok_or_else(no_we)?);
    c.arg("-control").args(args);
    run_timeout(c, secs)
}

/// (project folder, origin) for every wallpaper: workshop downloads, then myprojects and defaultprojects.
fn we_project_dirs() -> Vec<(PathBuf, &'static str)> {
    let mut dirs = vec![];
    let scan = |base: PathBuf, origin: &'static str, dirs: &mut Vec<(PathBuf, &'static str)>| {
        if let Ok(rd) = std::fs::read_dir(base) {
            dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.join("project.json").is_file()).map(|p| (p, origin)));
        }
    };
    for lib in steam_libraries() { scan(lib.join("steamapps").join("workshop").join("content").join(WE_APP), "workshop", &mut dirs); }
    if let Some(d) = we_dir() {
        for s in ["myprojects", "defaultprojects"] { scan(d.join("projects").join(s), s, &mut dirs); }
    }
    dirs
}

/// project.json (or config.json) as an object; {} when unreadable. load_json already skips a BOM.
fn project(path: &Path) -> Value {
    match load_json(path, json!({})) { v @ Value::Object(_) => v, _ => json!({}) }
}

/// Python's str() of a project.json field, "" when absent or falsy.
fn pystr(v: &Value) -> String {
    match v { Value::Null | Value::Bool(false) => String::new(), Value::String(s) => s.clone(), o => o.to_string() }
}

/// A folder name made safe to use as an id: anything but word characters, `.` and `-` becomes `_`.
fn safe_id(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' }).collect()
}

// ---------------------------------------------------------------- images (WIC instead of PIL)

/// Size that fits in a `max` square without enlarging (PIL's thumbnail).
fn fit_size(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w <= max && h <= max || w == 0 || h == 0 { return (w, h); }
    let k = max as f64 / w.max(h) as f64;
    (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1))
}

/// Decodes the first frame of `src`, shrinks it into a `fit` square if given, and writes it as RGB
/// to `dest` with the encoder `container` (JPEG or PNG). The partial file is removed on failure.
fn wic_save(src: &Path, dest: &Path, fit: Option<u32>, container: &GUID) -> bool {
    if let Some(d) = dest.parent() { let _ = std::fs::create_dir_all(d); }
    let ok = (|| -> Option<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let dec = f.CreateDecoderFromFilename(&HSTRING::from(src.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
            let frame = dec.GetFrame(0).ok()?;
            let (mut w, mut h) = (0, 0);
            frame.GetSize(&mut w, &mut h).ok()?;
            let mut bmp: IWICBitmapSource = frame.cast().ok()?;
            if let Some(m) = fit {
                let (nw, nh) = fit_size(w, h, m);
                if (nw, nh) != (w, h) {
                    let s = f.CreateBitmapScaler().ok()?;
                    s.Initialize(&bmp, nw, nh, WICBitmapInterpolationModeHighQualityCubic).ok()?;
                    bmp = s.cast().ok()?;
                    (w, h) = (nw, nh);
                }
            }
            let conv = f.CreateFormatConverter().ok()?;
            conv.Initialize(&bmp, &GUID_WICPixelFormat24bppBGR, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
            let stream = f.CreateStream().ok()?;
            let wide = HSTRING::from(dest.as_os_str());
            stream.InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE).ok()?;
            let enc = f.CreateEncoder(container, std::ptr::null()).ok()?;
            enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
            let mut out = None;
            enc.CreateNewFrame(&mut out, std::ptr::null_mut()).ok()?;
            let out = out?;
            out.Initialize(None).ok()?;
            out.SetSize(w, h).ok()?;
            let mut fmt = GUID_WICPixelFormat24bppBGR;
            out.SetPixelFormat(&mut fmt).ok()?;
            out.WriteSource(&conv, std::ptr::null()).ok()?;
            out.Commit().ok()?;
            enc.Commit().ok()
        }
    })().is_some();
    if !ok { let _ = std::fs::remove_file(dest); }
    ok
}

/// A 360px JPEG thumbnail of `src` at `dest`, reused while newer than the source.
fn thumb(src: &Path, dest: &Path) -> bool {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if let (Some(d), Some(s)) = (mtime(dest), mtime(src)) { if d >= s { return true; } }
    wic_save(src, dest, Some(360), &GUID_ContainerFormatJpeg)
}

// ---------------------------------------------------------------- commands

fn state() -> Value { load_json(&root().join("state.json"), json!({})) }

/// wallpaperEngine.follow as stored (true when unset).
fn follow() -> Value {
    match Config::load().get("wallpaperEngine.follow") { Value::Null => json!(true), v => v.clone() }
}

fn we_status() -> Value {
    let exe = we_exe();
    let live = pystr(state().get("live").unwrap_or(&Value::Null));
    json!({
        "installed": exe.is_some(),
        "exe": exe.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        "running": exe.is_some() && we_running(),
        "current": live.strip_prefix("we:").unwrap_or(""),
        "follow": follow(),
    })
}

fn we_list() -> Value {
    if we_exe().is_none() { return json!({ "installed": false, "items": [] }); }
    let thumbs = pack().join("we-thumbs");
    let mut items = vec![];
    for (d, origin) in we_project_dirs() {
        let p = project(&d.join("project.json"));
        let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pid = safe_id(&name);
        let preview = pystr(&p["preview"]);
        let mut th = String::new();
        if !preview.is_empty() && d.join(&preview).is_file() && thumb(&d.join(&preview), &thumbs.join(format!("{pid}.jpg"))) {
            th = format!("we-thumbs/{pid}.jpg");
        }
        let title = Some(pystr(&p["title"])).filter(|t| !t.is_empty()).unwrap_or(name);
        items.push(json!({
            "id": pid, "title": title, "type": pystr(&p["type"]).to_lowercase(), "origin": origin,
            "path": d.join("project.json").to_string_lossy(), "thumb": th,
            "rating": p.get("contentrating").cloned().unwrap_or(json!("")), "tags": p.get("tags").cloned().unwrap_or(json!([])),
        }));
    }
    items.sort_by_key(|i| i["title"].as_str().unwrap_or("").to_lowercase());
    json!({ "installed": true, "items": items })
}

/// ffmpeg: imageio_ffmpeg's bundled exe in the venv, else `ffmpeg` on PATH.
fn find_ffmpeg() -> Option<PathBuf> {
    let bundled = root().join(r"venv\Lib\site-packages\imageio_ffmpeg\binaries");
    let found = std::fs::read_dir(bundled).ok().and_then(|rd| {
        rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("ffmpeg")) && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")))
    });
    found.or_else(|| std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file()))
}

/// Grabs a frame (1s in, else the first) of `video` into `dest`.
fn video_frame(video: &Path, dest: &Path) -> Value {
    let Some(ff) = find_ffmpeg() else { return err("ffmpeg not available") };
    let grab = |seek: bool| {
        let mut c = hidden(&ff);
        c.args(["-y", "-loglevel", "error"]);
        if seek { c.args(["-ss", "1"]); }
        c.arg("-i").arg(video).args(["-frames:v", "1"]).arg(dest);
        run_timeout(c, 60)
    };
    if !grab(true).is_ok_and(|r| r.status.success()) || !dest.exists() { let _ = grab(false); } // very short clips
    json!({ "ok": dest.exists(), "path": dest.to_string_lossy() })
}

/// Re-themes end4 from a Wallpaper Engine wallpaper (project.json or media file).
fn we_theme(cfg: &Config, target: &Path) -> Value {
    let is_json = target.file_name().is_some_and(|n| n.eq_ignore_ascii_case("project.json"));
    let proj_file = if is_json { target.to_path_buf() } else { target.parent().unwrap_or(Path::new("")).join("project.json") };
    let p = if proj_file.is_file() { project(&proj_file) } else { json!({}) };
    let frame = root().join("live-frame.png");
    let _ = std::fs::create_dir_all(root());
    let dir = proj_file.parent().unwrap_or(Path::new(""));
    let media = if target.extension().is_some_and(|e| VIDEO_EXT.iter().any(|v| e.eq_ignore_ascii_case(v))) {
        Some(target.to_path_buf())
    } else if pystr(&p["type"]).to_lowercase() == "video" && !pystr(&p["file"]).is_empty() {
        Some(dir.join(pystr(&p["file"])))
    } else { None };
    let sw = |args: &[&str]| match crate::proc::run(cfg, "switchwall", args) { Value::Null => err("switchwall gave no result"), v => v };
    let (frame_s, live) = (frame.to_string_lossy().into_owned(), format!("we:{}", target.display()));
    if let Some(m) = media.filter(|m| m.is_file()) {
        video_frame(&m, &frame);
        // A full-resolution frame also becomes the Windows wallpaper (lock screen, apps that read it).
        if frame.is_file() { return sw(&["-Image", &frame_s, "-Live", &live]); }
    }
    let preview = pystr(&p["preview"]);
    if !preview.is_empty() && dir.join(&preview).is_file() {
        if !wic_save(&dir.join(&preview), &frame, None, &GUID_ContainerFormatPng) { return err("Could not read the wallpaper preview"); }
        // Previews are small, so only the colors change; the Windows wallpaper stays as it is.
        return sw(&["-Image", &frame_s, "-Live", &live, "-ThemeOnly"]);
    }
    err("No preview or video to take colors from")
}

/// Monitor indexes WE addresses, one per screen (a control command without -monitor touches only one).
fn monitors() -> Vec<String> { (0..unsafe { GetSystemMetrics(SM_CMONITORS) }.max(1)).map(|m| m.to_string()).collect() }

/// A project.json path or a project id (its folder name, made safe) -> its project.json.
fn find_project(ident: &str) -> Option<PathBuf> {
    if Path::new(ident).exists() { return Some(PathBuf::from(ident)); }
    we_project_dirs().into_iter().find(|(d, _)| d.file_name().is_some_and(|n| safe_id(&n.to_string_lossy()) == ident)).map(|(d, _)| d.join("project.json"))
}

fn we_apply(cfg: &Config, ident: &str, monitor: &str) -> Value {
    let Some(target) = find_project(ident) else { return err(format!("Wallpaper {ident} not found")) };
    match ensure_we_running(20) {
        Ok(true) => {}
        Ok(false) => return err("Wallpaper Engine didn't start"),
        Err(e) => return err(e),
    }
    // Without -monitor, Wallpaper Engine sets only one monitor; end4 has one wallpaper for all.
    let monitors: Vec<String> = if !monitor.is_empty() && monitor.bytes().all(|b| b.is_ascii_digit()) {
        vec![monitor.to_string()]
    } else {
        monitors()
    };
    for m in monitors {
        if let Err(e) = we_control(&["openWallpaper", "-file", &target.to_string_lossy(), "-monitor", &m], 20) { return err(e); }
    }
    json!({ "ok": true, "theme": we_theme(cfg, &target) })
}

/// Stops Wallpaper Engine. `-control closeWallpaper` exits 0 but leaves the scene drawn over the
/// desktop on current builds, so the process is ended; we_apply starts it again when needed.
fn close_all() -> bool {
    we_pids().into_iter().fold(true, |ok, pid| unsafe {
        let done = OpenProcess(PROCESS_TERMINATE, false, pid).is_ok_and(|h| {
            let r = TerminateProcess(h, 1).is_ok();
            let _ = windows::Win32::Foundation::CloseHandle(h);
            r
        });
        ok && done
    })
}

fn we_close() {
    if we_exe().is_some() && we_running() { close_all(); }
}

/// Same wallpaper -> same key, whether WE reports project.json, scene.pkg or the folder.
fn we_key(path: &str) -> String {
    let p: PathBuf = Path::new(path.trim().trim_matches('"')).components().collect();
    let parent = p.parent().filter(|d| d.join("project.json").is_file());
    let k = if p.is_dir() { p.as_path() } else { parent.unwrap_or(p.as_path()) };
    k.to_string_lossy().to_lowercase()
}

/// Starts Wallpaper Engine if needed and waits (up to `wait` s) for it to accept commands.
fn ensure_we_running(wait: u32) -> std::io::Result<bool> {
    if we_running() { return Ok(true); }
    hidden(we_exe().ok_or_else(no_we)?).spawn()?;
    for _ in 0..wait * 2 {
        std::thread::sleep(Duration::from_millis(500));
        if we_running() {
            std::thread::sleep(Duration::from_secs(3)); // give it a moment to accept commands
            return Ok(true);
        }
    }
    Ok(false)
}

/// The wallpaper WE shows now: first monitor's pick in its config.json, else `-control getWallpaper`.
fn we_current() -> String {
    // `-control getWallpaper` prints nothing on current Wallpaper Engine builds, hence config.json.
    let cfg = we_dir().map(|d| project(&d.join("config.json"))).unwrap_or_else(|| json!({}));
    let me = std::env::var("USERNAME").unwrap_or_default();
    let all = cfg.as_object().into_iter().flat_map(|o| o.values());
    for u in cfg.get(&me).into_iter().chain(all) {
        let sel = u.pointer("/general/wallpaperconfig/selectedwallpapers").and_then(Value::as_object);
        if let Some(first) = sel.and_then(|s| s.keys().min().and_then(|k| s.get(k))) {
            let f = first.get("file").and_then(Value::as_str).unwrap_or("");
            if !f.is_empty() && Path::new(f).exists() { return f.to_string(); }
        }
    }
    let Ok(r) = we_control(&["getWallpaper"], 10) else { return String::new() };
    let text = String::from_utf8_lossy(&r.stdout);
    let line = text.lines().map(|l| l.trim().trim_matches('"')).find(|l| !l.is_empty()).unwrap_or("");
    if !line.is_empty() && Path::new(line).exists() { line.to_string() } else { String::new() }
}

/// end4's last wallpaper was a still picture or a colour. A state with neither (missing or wiped)
/// says nothing, and must not close a Wallpaper Engine wallpaper the user is watching.
fn still_chosen(st: &Value) -> bool {
    let set = |k: &str| st.get(k).is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_str() != Some("") && v.as_i64() != Some(0));
    !set("live") && (set("image") || set("color"))
}

/// Close WE's wallpaper again? Only while following, within 90 s of start-up, at most 3 times,
/// 10 s apart, and only if end4's last pick was a still picture.
fn close_due(follow: bool, closes: u32, since_start: Duration, since_close: Duration, st: &Value) -> bool {
    follow && closes < 3 && since_start.as_secs_f64() < 90.0 && since_close.as_secs_f64() > 10.0 && still_chosen(st)
}

/// WE now shows `cur`; `last` is the previous key, `live` end4's state. None: nothing changed,
/// Some(true): re-theme, Some(false): remember it only. On start WE's saved wallpaper is often
/// stale (end4 may have closed it for a picture), so catch up only if end4 was following WE.
fn sync_decide(last: Option<&str>, cur: &str, live: &str) -> Option<bool> {
    if cur.is_empty() || last == Some(we_key(cur).as_str()) { return None; }
    let following = live.starts_with("we:");
    Some((last.is_some() || following) && !(following && we_key(&live[3..]) == we_key(cur)))
}

/// Waits `secs` in short steps; false if the stream was stopped meanwhile.
fn nap(secs: f64, out: &Sink) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs.max(0.0));
    while Instant::now() < end && !out.stopped() { std::thread::sleep(Duration::from_millis(100)); }
    !out.stopped()
}

/// Re-themes end4 whenever Wallpaper Engine's wallpaper changes (its UI, playlists...).
fn we_sync(cfg: &Config, out: &Sink) {
    if we_exe().is_none() { out.emit(&json!({ "skip": "Wallpaper Engine not installed" })); return; }
    if !out.emit(&json!({ "started": true })) { return; }
    let interval = std::env::var("END4_WE_INTERVAL").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(5.0);
    // Closing a wallpaper through WE's CLI doesn't clear its saved one, so WE reopens it when it
    // starts with Windows and hides the picture end4 set: close it again for a while after start-up.
    let (started, mut closes, mut last_close, mut last) = (Instant::now(), 0u32, None::<Instant>, None::<String>);
    while !out.stopped() {
        let follow = follow().as_bool().unwrap_or(true);
        let since_close = last_close.map_or(Duration::MAX, |t| t.elapsed());
        if close_due(follow, closes, started.elapsed(), since_close, &state()) && we_exe().is_some() && we_running() {
            if close_all() && !out.emit(&json!({ "closed": "restored end4's picture" })) { return; }
            (closes, last_close) = (closes + 1, Some(Instant::now()));
            if !nap(interval, out) { return; }
            continue;
        }
        if follow && we_exe().is_some() && we_running() {
            let cur = we_current();
            let live = pystr(state().get("live").unwrap_or(&Value::Null));
            if let Some(theme) = sync_decide(last.as_deref(), &cur, &live) {
                if theme {
                    let ok = we_theme(cfg, Path::new(&cur)).get("ok").and_then(Value::as_bool).unwrap_or(true);
                    if !out.emit(&json!({ "changed": cur, "ok": ok })) { return; }
                }
                last = Some(we_key(&cur));
            }
        }
        if !nap(interval, out) { return; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("end4-we-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn vdf_paths_are_unescaped() {
        let t = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\" \"D:\\\\SteamLibrary\"\n\t\t\"pathx\" \"no\"\n\t}\n}";
        assert_eq!(vdf_paths(t), vec![r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]);
        assert!(vdf_paths("\"path\"\"x\" \"path\" \"\"").is_empty());
    }

    #[test]
    fn ids_and_sizes() {
        assert_eq!(safe_id("12 34 (a)-b.c_é"), "12_34__a_-b.c_é");
        assert_eq!(fit_size(1920, 1080, 360), (360, 203));
        assert_eq!(fit_size(200, 100, 360), (200, 100));
        assert_eq!(fit_size(100, 1000, 360), (36, 360));
    }

    #[test]
    fn key_is_the_project_folder() {
        let d = tmp("key");
        std::fs::write(d.join("project.json"), "{}").unwrap();
        let want = d.to_string_lossy().to_lowercase();
        assert_eq!(we_key(&format!("\"{}\"", d.join("project.json").display())), want);
        assert_eq!(we_key(&d.join("scene.pkg").to_string_lossy()), want);
        assert_eq!(we_key(&format!(" {} ", d.display())), want);
        assert_eq!(we_key(r"C:\nowhere\file.mp4"), r"c:\nowhere\file.mp4");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn still_guard() {
        assert!(still_chosen(&json!({ "image": "a.png" })));
        assert!(still_chosen(&json!({ "color": "#fff", "live": "" })));
        assert!(!still_chosen(&json!({})));
        assert!(!still_chosen(&json!({ "image": "a.png", "live": "we:C:\\x" })));
    }

    #[test]
    fn close_window() {
        let (s, c) = (Duration::from_secs(5), Duration::MAX);
        let st = json!({ "image": "a.png" });
        assert!(close_due(true, 0, s, c, &st));
        assert!(!close_due(false, 0, s, c, &st));
        assert!(!close_due(true, 3, s, c, &st));
        assert!(!close_due(true, 0, Duration::from_secs(90), c, &st));
        assert!(!close_due(true, 1, s, Duration::from_secs(10), &st));
        assert!(!close_due(true, 0, s, c, &json!({})));
    }

    #[test]
    fn sync_decisions() {
        assert_eq!(sync_decide(None, "", ""), None);
        assert_eq!(sync_decide(None, r"C:\a\x.mp4", ""), Some(false)); // first sight, end4 not following
        assert_eq!(sync_decide(None, r"C:\a\x.mp4", r"we:C:\b\y.mp4"), Some(true)); // WE moved on
        assert_eq!(sync_decide(None, r"C:\a\x.mp4", r"we:C:\A\X.mp4"), Some(false)); // already themed
        assert_eq!(sync_decide(Some(r"c:\a\x.mp4"), r"C:\a\x.mp4", ""), None);
        assert_eq!(sync_decide(Some(r"c:\a\x.mp4"), r"C:\b\y.mp4", ""), Some(true));
    }

    #[test]
    fn thumbnail_round_trip() {
        // A 2x2 24-bit BMP, made by hand, becomes a JPEG and a PNG.
        let d = tmp("img");
        let mut bmp = b"BM".to_vec();
        bmp.extend(70u32.to_le_bytes()); bmp.extend([0; 4]); bmp.extend(54u32.to_le_bytes());
        bmp.extend(40u32.to_le_bytes()); bmp.extend(2u32.to_le_bytes()); bmp.extend(2u32.to_le_bytes());
        bmp.extend(1u16.to_le_bytes()); bmp.extend(24u16.to_le_bytes()); bmp.extend([0; 24]);
        bmp.extend([255, 0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 0, 255, 255, 255, 0]);
        std::fs::write(d.join("a.bmp"), &bmp).unwrap();
        assert!(thumb(&d.join("a.bmp"), &d.join("t").join("a.jpg")));
        assert_eq!(&std::fs::read(d.join("t").join("a.jpg")).unwrap()[..2], &[0xFF, 0xD8]);
        assert!(wic_save(&d.join("a.bmp"), &d.join("a.png"), None, &GUID_ContainerFormatPng));
        assert_eq!(&std::fs::read(d.join("a.png")).unwrap()[1..4], b"PNG");
        assert!(!thumb(&d.join("missing.png"), &d.join("m.jpg")));
        let _ = std::fs::remove_dir_all(d);
    }
}
