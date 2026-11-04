//! Rust port of scripts/switchwall.ps1 and scripts/end4_theme.py: pick a wallpaper, make a Material You
//! palette from it and push that to Zebar, GlazeWM, Flow Launcher and Windows Terminal, then follow
//! Windows' light/dark setting and the lock screen.
//! shortcut: animated pictures use WIC's raw frame 1 (GIF frames aren't composed), thumbnails are
//! JPEG at WIC's default quality (Pillow used 85); upgrade if either shows.

use super::common::*;
use super::*;
use material_colors::color::Rgb;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, HWND, LPARAM, WPARAM},
        Graphics::Imaging::*,
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ},
        },
        UI::{
            Shell::{
                Common::COMDLG_FILTERSPEC, DesktopWallpaper, FileOpenDialog, FOLDERID_Pictures, IDesktopWallpaper, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
                SHGetKnownFolderPath, DWPOS_FILL, KF_FLAG_DEFAULT, SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{SendMessageTimeoutW, SystemParametersInfoW, HWND_BROADCAST, SMTO_ABORTIFHUNG, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER, WM_SETTINGCHANGE},
        },
    },
};

mod palette;
use palette::*;

const VIDEO_EXT: &[&str] = &["mp4", "webm", "mkv", "mov", "avi", "wmv", "m4v"];
const BEGIN: &str = "# >>> end4-pC colors (managed by end4_theme.py)";
const END: &str = "# <<< end4-pC colors";

// ------------------------------------------------------------------ images (WIC)

fn factory() -> Option<IWICImagingFactory> { unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() } }

fn open_frame(f: &IWICImagingFactory, path: &Path, second_if_animated: bool) -> Option<IWICBitmapFrameDecode> {
    unsafe {
        let dec = f.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let idx = if second_if_animated && dec.GetFrameCount().ok()? > 1 { 1 } else { 0 };
        dec.GetFrame(idx).ok()
    }
}

/// The picture as (width, height, RGB bytes), alpha dropped like PIL's convert("RGB").
fn load_rgb(path: &Path) -> Option<(usize, usize, Vec<u8>)> {
    let f = factory()?;
    let frame = open_frame(&f, path, true)?;
    unsafe {
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut w, mut h) = (0, 0);
        conv.GetSize(&mut w, &mut h).ok()?;
        let mut px = vec![0u8; w as usize * h as usize * 3];
        conv.CopyPixels(std::ptr::null(), w * 3, &mut px).ok()?;
        Some((w as usize, h as usize, px))
    }
}

/// The wallpaper's seed colour (source_from_image).
fn source_from_image(path: &Path) -> Option<Rgb> {
    let (w, h, px) = load_rgb(path)?;
    Some(source_from_rgb(&px, w, h, 128))
}

/// Small JPEG copy of the wallpaper (at most 960px) for the Settings widget, whose webview can't read arbitrary files.
fn write_thumbnail(pack_dir: &Path, image: &Path) {
    let _ = (|| -> Option<()> {
        let f = factory()?;
        let frame = open_frame(&f, image, false)?;
        unsafe {
            let (mut w, mut h) = (0, 0);
            frame.GetSize(&mut w, &mut h).ok()?;
            let s = (960.0 / w as f64).min(960.0 / h as f64).min(1.0);
            let (nw, nh) = (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1));
            let scale = f.CreateBitmapScaler().ok()?;
            scale.Initialize(&frame, nw, nh, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            let conv = f.CreateFormatConverter().ok()?;
            conv.Initialize(&scale, &GUID_WICPixelFormat24bppBGR, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
            let _ = std::fs::create_dir_all(pack_dir);
            let (tmp, dest) = (pack_dir.join("wallpaper.jpg.tmp"), pack_dir.join("wallpaper.jpg"));
            let stream = f.CreateStream().ok()?;
            stream.InitializeFromFilename(&HSTRING::from(tmp.as_path()), GENERIC_WRITE.0).ok()?;
            let enc = f.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null()).ok()?;
            enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
            let (mut out, mut opts) = (None, None);
            enc.CreateNewFrame(&mut out, &mut opts).ok()?;
            let out = out?;
            out.Initialize(opts.as_ref()).ok()?;
            out.SetSize(nw, nh).ok()?;
            let mut fmt = GUID_WICPixelFormat24bppBGR;
            out.SetPixelFormat(&mut fmt).ok()?;
            out.WriteSource(&conv, std::ptr::null()).ok()?;
            out.Commit().ok()?;
            enc.Commit().ok()?;
            drop((out, enc, stream));
            std::fs::rename(&tmp, &dest).ok()
        }
    })();
}

// ------------------------------------------------------------------ writers

/// Zebar's theme.json, through a temp file so the widgets never read half of it.
fn write_zebar(pack_dir: &Path, payload: &Value) -> bool { save_json(&pack_dir.join("theme.json"), payload) }

/// Replaces the managed colour block in GlazeWM's config. False if the file or its markers are missing.
fn write_glazewm(cfg: &Path, c: &BTreeMap<String, String>) -> bool {
    let Ok(text) = std::fs::read_to_string(cfg) else { return false };
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (p, o) = (&c["primary"], &c["outlineVariant"]);
    let block = format!(
        "{BEGIN}\nwindow_effects:\n  focused_window:\n    border: {{ enabled: true, color: '{p}' }}\n    hide_title_bar: {{ enabled: false }}\n    corner_style: {{ enabled: true, style: 'rounded' }}\n    transparency: {{ enabled: false, opacity: '95%' }}\n  other_windows:\n    border: {{ enabled: true, color: '{o}' }}\n    hide_title_bar: {{ enabled: false }}\n    corner_style: {{ enabled: true, style: 'rounded' }}\n    transparency: {{ enabled: false, opacity: '92%' }}\n{END}"
    );
    let (mut out, mut rest, mut found) = (String::new(), text.as_str(), false);
    while let Some(b) = rest.find(BEGIN) {
        let Some(e) = rest[b + BEGIN.len()..].find(END) else { break };
        out.push_str(&rest[..b]);
        out.push_str(&block);
        rest = &rest[b + BEGIN.len() + e + END.len()..];
        found = true;
    }
    if !found { return false; }
    out.push_str(rest);
    // Python's write_text turns \n into \r\n on Windows; keep the file in the same shape
    std::fs::write(cfg, out.replace('\n', "\r\n")).is_ok()
}

/// Fills Flow Launcher's theme template. False if there's no template.
fn write_flow(template: &Path, themes_dir: &Path, c: &BTreeMap<String, String>, dark: bool) -> bool {
    let Ok(text) = std::fs::read_to_string(template) else { return false };
    let mut out = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let get = |k: &str| c.get(k).cloned().unwrap_or_default();
    for (k, v) in [
        ("BG", get("surfaceContainerLow")), ("BORDER", get("outlineVariant")), ("FG", get("onSurface")), ("SUBFG", get("onSurfaceVariant")),
        ("SELBG", get("secondaryContainer")), ("SELFG", get("onSecondaryContainer")), ("PRIMARY", get("primary")), ("OUTLINE", get("outline")),
        ("COLORSCHEME", if dark { "Dark".into() } else { "Light".into() }),
    ] {
        out = out.replace(&format!("{{{{{k}}}}}"), &v);
    }
    let _ = std::fs::create_dir_all(themes_dir);
    std::fs::write(themes_dir.join("end4-pC.xaml"), out.replace('\n', "\r\n")).is_ok()
}

/// Windows Terminal fragment with an "end4-pC" colour scheme.
fn write_terminal(frag_dir: &Path, c: &BTreeMap<String, String>, t: &BTreeMap<String, String>) {
    let get = |m: &BTreeMap<String, String>, k: &str| Value::from(m.get(k).cloned().unwrap_or_default());
    let mut s = serde_json::Map::new();
    for (k, v) in [("name", Value::from("end4-pC")), ("background", get(c, "surface")), ("foreground", get(c, "onSurface")), ("cursorColor", get(c, "primary")), ("selectionBackground", get(c, "secondaryContainer"))] {
        s.insert(k.into(), v);
    }
    for (i, n) in ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"].iter().enumerate() {
        s.insert(n.to_string(), get(t, &format!("term{i}")));
        s.insert(format!("bright{}{}", n[..1].to_uppercase(), &n[1..]), get(t, &format!("term{}", i + 8)));
    }
    let _ = std::fs::create_dir_all(frag_dir);
    let _ = std::fs::write(frag_dir.join("end4-pC.json"), serde_json::to_string_pretty(&json!({ "schemes": [s] })).unwrap_or_default());
}

/// Asks GlazeWM to reload its config (best effort, 10 s at most).
fn reload_glazewm() {
    let pf = PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join("glzr.io").join("GlazeWM");
    let on_path = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("glazewm.exe")).find(|f| f.is_file()));
    let Some(exe) = on_path.or_else(|| [pf.join("cli").join("glazewm.exe"), pf.join("glazewm.exe")].into_iter().find(|p| p.exists())) else { return };
    let Ok(mut child) = hidden(exe).args(["command", "wm-reload-config"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() else { return };
    for _ in 0..100 {
        if matches!(child.try_wait(), Ok(Some(_))) { return; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
}

// ------------------------------------------------------------------ end4_theme.py main

/// Where end4_theme.py writes (all parameters so tests can aim them at a temp dir).
struct Paths {
    state: PathBuf,
    pack: PathBuf,
    glazewm: PathBuf,
    flow_template: PathBuf,
    flow_themes: PathBuf,
    terminal: PathBuf,
    reload: bool,
}

impl Paths {
    fn live() -> Paths {
        let (appdata, local) = (std::env::var("APPDATA").unwrap_or_default(), std::env::var("LOCALAPPDATA").unwrap_or_default());
        let home = crate::conf::home();
        Paths {
            state: root().join("state.json"),
            pack: pack(),
            glazewm: home.join(".glzr").join("glazewm").join("config.yaml"),
            flow_template: root().join("flowlauncher").join("end4-pC.xaml.template"),
            flow_themes: PathBuf::from(appdata).join("FlowLauncher").join("Themes"),
            terminal: PathBuf::from(local).join("Microsoft").join("Windows Terminal").join("Fragments").join("end4-pC"),
            reload: true,
        }
    }
}

#[derive(Default)]
struct Opts {
    image: Option<String>,
    color: Option<String>,
    mode: Option<String>,
    scheme: Option<String>,
    live: Option<String>,
}

/// end4_theme.py's main(): palette from the image/colour (or the last source), written everywhere,
/// state.json updated (only the keys the script owns). Err: what stopped it.
fn run_theme(o: &Opts, p: &Paths) -> Result<Value, String> {
    let mut state = load_json(&p.state, json!({}));
    if !state.is_object() { state = json!({}); }
    let s = |k: &str| state[k].as_str().filter(|v| !v.is_empty()).map(String::from);
    let mode = o.mode.clone().or_else(|| s("mode")).unwrap_or_else(|| "dark".into());
    let scheme = o.scheme.clone().or_else(|| s("scheme")).unwrap_or_else(|| "auto".into());
    let (mut image, mut color) = (o.image.clone(), o.color.clone());
    if image.is_none() && color.is_none() { (image, color) = (s("image"), s("color")); }
    if image.as_deref().is_some_and(|i| !Path::new(i).exists()) { image = None; }
    if image.is_none() && color.is_none() { color = Some("#6750A4".into()); }

    let seed = match (&image, &color) {
        (Some(i), _) => source_from_image(Path::new(i)).ok_or_else(|| format!("Couldn't read the picture {i}"))?,
        (None, Some(c)) => hex_rgb(c).ok_or_else(|| format!("Not a #RRGGBB colour: {c}"))?,
        _ => return Err("no colour source".into()),
    };
    let dark = mode == "dark";
    let smart = scheme == "auto";
    let (colors, term, used) = generate(seed, dark, if smart { "tonal-spot" } else { &scheme }, smart);

    let live = match &o.live { Some(l) => Some(l.clone()), None if o.image.is_none() && o.color.is_none() => s("live"), None => None };
    let payload = json!({ "mode": mode, "scheme": scheme, "resolvedScheme": used, "seed": rgb_hex(seed), "wallpaper": image, "live": live, "colors": colors, "term": term });
    write_zebar(&p.pack, &payload);
    if let Some(i) = &image { write_thumbnail(&p.pack, Path::new(i)); }
    let glaze_ok = write_glazewm(&p.glazewm, &colors);
    let flow_ok = write_flow(&p.flow_template, &p.flow_themes, &colors, dark);
    write_terminal(&p.terminal, &colors, &term);

    // read again: the app writes this file too, so only our keys are replaced
    let mut fresh = load_json(&p.state, json!({}));
    if !fresh.is_object() { fresh = json!({}); }
    for (k, v) in [("mode", json!(mode)), ("scheme", json!(scheme)), ("image", json!(image)), ("color", json!(if image.is_some() { None } else { color })), ("live", json!(live))] {
        fresh[k] = v;
    }
    save_json(&p.state, &fresh);

    if glaze_ok && p.reload { reload_glazewm(); }
    Ok(json!({ "ok": true, "mode": mode, "scheme": used, "seed": rgb_hex(seed), "primary": colors["primary"], "glazewm": glaze_ok, "flowLauncher": flow_ok }))
}

// ------------------------------------------------------------------ switchwall.ps1 helpers

fn reg_str(sub: PCWSTR, name: PCWSTR, v: &str) {
    let d: Vec<u16> = v.encode_utf16().chain(Some(0)).collect();
    unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, sub, name, REG_SZ.0, Some(d.as_ptr().cast()), (d.len() * 2) as u32); }
}

fn reg_dword(sub: PCWSTR, name: PCWSTR, v: u32) {
    unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, sub, name, REG_DWORD.0, Some((&v as *const u32).cast()), 4); }
}

/// Resolve-Path: absolute, no \\?\ prefix. Err with PowerShell's message if it doesn't exist.
fn resolve(p: &str) -> Result<String, String> {
    let c = std::fs::canonicalize(p).map_err(|_| format!("Cannot find path '{p}' because it does not exist."))?;
    Ok(c.to_string_lossy().trim_start_matches(r"\\?\").to_string())
}

fn find_lively() -> Option<PathBuf> {
    let local = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join(r"Programs\Lively Wallpaper\Livelycu.exe");
    let pf = PathBuf::from(std::env::var("ProgramFiles").unwrap_or_default()).join(r"Lively Wallpaper\Livelycu.exe");
    let on_path = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("Livelycu.exe")).find(|f| f.is_file()));
    [Some(local), Some(pf), on_path].into_iter().flatten().find(|p| p.is_file())
}

/// Is a process with one of these image names (no .exe) running?
fn process_running(names: &[&str]) -> bool {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        let mut found = false;
        while ok && !found {
            let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len())]).to_lowercase();
            found = names.iter().any(|x| n == format!("{x}.exe"));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        found
    }
}

/// The wallpaper dialog (IFileOpenDialog, on its own STA thread). None: cancelled.
fn pick_file() -> Option<String> {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dlg.SetTitle(w!("Choose a wallpaper")).ok()?;
        let spec = |n: PCWSTR, s: PCWSTR| COMDLG_FILTERSPEC { pszName: n, pszSpec: s };
        dlg.SetFileTypes(&[
            spec(w!("Images and videos"), w!("*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.gif;*.mp4;*.webm;*.mkv;*.mov")),
            spec(w!("Images"), w!("*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.gif")),
            spec(w!("Videos (live wallpaper)"), w!("*.mp4;*.webm;*.mkv;*.mov;*.avi;*.wmv")),
            spec(w!("All files"), w!("*.*")),
        ]).ok()?;
        if let Ok(pics) = SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None) {
            let dir = PathBuf::from(pics.to_string().unwrap_or_default());
            CoTaskMemFree(Some(pics.0 as *const _));
            let dir = if dir.join("Wallpapers").is_dir() { dir.join("Wallpapers") } else { dir };
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(dir.as_path()), None) { let _ = dlg.SetFolder(&item); }
        }
        dlg.Show(HWND::default()).ok()?;
        let name = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path
    })
    .join()
    .ok()
    .flatten()
}

/// Sets the picture on every monitor: IDesktopWallpaper (it crossfades, like end4's), else the instant SPI call.
fn set_wallpaper(image: &str) {
    reg_str(w!(r"Control Panel\Desktop"), w!("WallpaperStyle"), "10"); // Fill
    reg_str(w!(r"Control Panel\Desktop"), w!("TileWallpaper"), "0");
    let fade = || -> windows::core::Result<()> {
        let dw: IDesktopWallpaper = unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_INPROC_SERVER)? };
        unsafe {
            dw.SetPosition(DWPOS_FILL)?;
            dw.SetWallpaper(PCWSTR::null(), &HSTRING::from(image))
        }
    };
    if fade().is_err() {
        let mut p: Vec<u16> = image.encode_utf16().chain(Some(0)).collect();
        unsafe { let _ = SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(p.as_mut_ptr().cast()), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
    }
}

/// Windows' own app/system theme follows the palette, and running apps are told.
fn set_windows_theme(dark: bool) {
    let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let v = if dark { 0 } else { 1 };
    reg_dword(key, w!("AppsUseLightTheme"), v);
    reg_dword(key, w!("SystemUsesLightTheme"), v);
    unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("ImmersiveColorSet").as_ptr() as isize), SMTO_ABORTIFHUNG, 1000, None); }
}

/// Best effort: Windows may refuse this for desktop apps or when a policy controls the lock screen.
fn set_lock_screen(image: &str) -> bool {
    use windows::{Storage::StorageFile, System::UserProfile::LockScreen};
    (|| -> windows::core::Result<()> {
        let f = StorageFile::GetFileFromPathAsync(&HSTRING::from(image))?.get()?;
        LockScreen::SetImageFileAsync(&f)?.get()
    })()
    .is_ok()
}

/// A command's last JSON line through the bridge (None: it printed nothing parseable).
fn bridge_json(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    let v = crate::proc::bridge(cfg, name, args);
    (!v.is_null()).then_some(v)
}

// ------------------------------------------------------------------ switchwall.ps1

/// switchwall.ps1 with its PowerShell-style arguments ("-Image", path, "-ToggleMode", ...). None: not ported.
pub fn switchwall(cfg: &Config, args: &[&str]) -> Option<Value> {
    let cfg = cfg.clone();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    // WinRT's .get() and the shell APIs want COM initialised; a fresh thread keeps the caller's apartment out of it.
    std::thread::spawn(move || {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        Some(match switch(&cfg, &args, &Paths::live()) { Ok(v) => v, Err(e) => err(e) })
    })
    .join()
    .unwrap_or_else(|_| Some(err("switchwall crashed")))
}

fn switch(cfg: &Config, args: &[String], paths: &Paths) -> Result<Value, String> {
    let (mut image, mut color, mut mode, mut scheme, mut live) = (String::new(), String::new(), String::new(), String::new(), String::new());
    let (mut toggle, mut pick, mut theme_only) = (false, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a.trim_start_matches('-').to_lowercase();
        let mut val = || it.next().cloned().ok_or_else(|| format!("Missing an argument for parameter '{name}'."));
        match name.as_str() {
            "image" => image = val()?,
            "color" => color = val()?,
            "mode" => mode = val()?.to_lowercase(),
            "scheme" => scheme = val()?.to_lowercase(),
            "live" => live = val()?,
            "togglemode" => toggle = true,
            "pick" => pick = true,
            "themeonly" => theme_only = true,
            _ => return Err(format!("A parameter cannot be found that matches parameter name '{name}'.")),
        }
    }
    if !["", "dark", "light"].contains(&mode.as_str()) { return Err(format!("Cannot validate argument on parameter 'Mode'. The argument \"{mode}\" does not belong to the set \"dark,light\".")); }
    if !scheme.is_empty() && scheme != "auto" && !SCHEMES.contains(&scheme.as_str()) { return Err(format!("Cannot validate argument on parameter 'Scheme'. The argument \"{scheme}\" is not a known scheme.")); }

    let state = load_json(&paths.state, Value::Null);
    let retheme = toggle || !mode.is_empty() || !scheme.is_empty();
    if pick || (image.is_empty() && color.is_empty() && !retheme) {
        match pick_file() { Some(f) => image = f, None => return Ok(json!({ "ok": false, "cancelled": true })) }
    }
    if toggle {
        mode = if state["mode"].as_str().filter(|m| !m.is_empty()).unwrap_or("dark") == "dark" { "light" } else { "dark" }.into();
    }

    // 0. Live wallpaper: play the video with Wallpaper Engine (or Lively), theme from one of its frames.
    let lively = find_lively();
    let is_video = !image.is_empty() && Path::new(&image).extension().is_some_and(|e| VIDEO_EXT.contains(&e.to_string_lossy().to_lowercase().as_str()));
    if is_video && live.is_empty() {
        if bridge_json(cfg, "we-status", &[]).is_some_and(|v| v["installed"].as_bool().unwrap_or(false)) {
            // we-apply opens it in Wallpaper Engine and calls back into switchwall with a frame
            let res = bridge_json(cfg, "we-apply", &[&resolve(&image)?]).ok_or("Wallpaper Engine gave no answer")?;
            return Ok(if res["theme"].as_object().is_some_and(|t| !t.is_empty()) { res["theme"].clone() } else { res });
        }
        let lively = lively.as_ref().ok_or("Live wallpapers need Wallpaper Engine or Lively Wallpaper (winget install rocksdanister.LivelyWallpaper, or re-run install.ps1 -WithLively).")?;
        live = resolve(&image)?;
        let frame = root().join("live-frame.png");
        let _ = bridge_json(cfg, "video-frame", &[&live, &frame.to_string_lossy()]);
        let _ = hidden(lively).args(["setwp", "--file", &live]).output();
        if !frame.exists() { return Err(format!("Couldn't read a frame from {live}")); }
        image = frame.to_string_lossy().into_owned();
    } else if !image.is_empty() && live.is_empty() {
        // Back to a static wallpaper: stop whichever app played the live one. Wallpaper Engine is
        // closed even when end4 didn't start it (state.live empty): it draws over the Windows wallpaper.
        let state_live = state["live"].as_str().unwrap_or("");
        let we_running = state_live.is_empty() && process_running(&["wallpaper32", "wallpaper64"]);
        if !state_live.is_empty() || we_running {
            if we_running || state_live.starts_with("we:") { let _ = bridge_json(cfg, "we-close", &[]); }
            else if let Some(l) = &lively { let _ = hidden(l).args(["closewp", "--monitor", "-1"]).output(); }
        }
    }

    // 1. Wallpaper.
    if !image.is_empty() { image = resolve(&image)?; }
    if !image.is_empty() && !theme_only { set_wallpaper(&image); }

    // 2. Palette -> Zebar / GlazeWM / Flow Launcher / Terminal.
    let opt = |s: &String| (!s.is_empty()).then(|| s.clone());
    let result = run_theme(&Opts { image: opt(&image), color: opt(&color), mode: opt(&mode), scheme: opt(&scheme), live: opt(&live) }, paths)
        .map_err(|e| format!("end4_theme produced no output: {e}"))?;

    // 3. Windows' own light/dark setting follows the palette.
    set_windows_theme(result["mode"].as_str() == Some("dark"));

    // 4. Lock screen picture follows the wallpaper (config: syncLockScreen).
    if !image.is_empty() && !theme_only && load_json(&pack().join("config.json"), Value::Null)["syncLockScreen"] != json!(false) {
        let _ = set_lock_screen(&image);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("end4-theme-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn colors() -> BTreeMap<String, String> {
        ["primary", "outlineVariant", "surfaceContainerLow", "onSurface", "onSurfaceVariant", "secondaryContainer", "onSecondaryContainer", "outline", "surface"]
            .iter().enumerate().map(|(i, k)| (k.to_string(), format!("#00000{i}"))).collect()
    }

    #[test]
    fn glazewm_block_is_replaced_in_place() {
        let d = tmp("glaze");
        let f = d.join("config.yaml");
        std::fs::write(&f, format!("a: 1\r\n{BEGIN}\r\nold: x\r\n{END}\r\nb: 2\r\n")).unwrap();
        assert!(write_glazewm(&f, &colors()));
        let t = std::fs::read_to_string(&f).unwrap();
        assert!(t.starts_with("a: 1\r\n") && t.ends_with(&format!("{END}\r\nb: 2\r\n")) && t.contains("color: '#000000'") && !t.contains("old: x"));
        std::fs::write(&f, "no markers\n").unwrap();
        assert!(!write_glazewm(&f, &colors()));
        assert!(!write_glazewm(&d.join("missing.yaml"), &colors()));
    }

    #[test]
    fn flow_template_is_filled() {
        let d = tmp("flow");
        let t = d.join("t.template");
        std::fs::write(&t, "{{BG}} {{FG}} {{COLORSCHEME}} {{PRIMARY}}").unwrap();
        assert!(write_flow(&t, &d.join("themes"), &colors(), false));
        assert_eq!(std::fs::read_to_string(d.join("themes").join("end4-pC.xaml")).unwrap(), "#000002 #000003 Light #000000");
        assert!(!write_flow(&d.join("none"), &d.join("themes"), &colors(), true));
    }

    #[test]
    fn terminal_fragment_has_sixteen_colours() {
        let d = tmp("term");
        let (_, term, _) = generate(hex_rgb("#6750A4").unwrap(), true, "tonal-spot", false);
        write_terminal(&d, &colors(), &term);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("end4-pC.json")).unwrap()).unwrap();
        let s = &v["schemes"][0];
        assert_eq!(s["name"], "end4-pC");
        assert_eq!(s["brightPurple"], json!(term["term13"]));
        assert_eq!(s["black"], json!(term["term0"]));
    }

    #[test]
    fn theme_from_colour_writes_everything_and_keeps_foreign_state_keys() {
        let d = tmp("run");
        let p = Paths {
            state: d.join("state.json"), pack: d.join("pack"), glazewm: d.join("config.yaml"), flow_template: d.join("t.template"),
            flow_themes: d.join("themes"), terminal: d.join("frag"), reload: false,
        };
        std::fs::write(&p.state, "\u{feff}{\"mode\":\"light\",\"cards\":{\"x\":1},\"image\":\"C:/gone.png\"}").unwrap();
        std::fs::write(&p.glazewm, format!("{BEGIN}\n{END}\n")).unwrap();
        std::fs::write(&p.flow_template, "{{PRIMARY}}").unwrap();
        let r = run_theme(&Opts { color: Some("#E53935".into()), ..Default::default() }, &p).unwrap();
        assert_eq!((r["ok"].clone(), r["mode"].clone(), r["glazewm"].clone(), r["flowLauncher"].clone()), (json!(true), json!("light"), json!(true), json!(true)));
        let st = load_json(&p.state, Value::Null);
        assert_eq!((st["cards"]["x"].clone(), st["color"].clone(), st["image"].clone(), st["scheme"].clone()), (json!(1), json!("#E53935"), Value::Null, json!("auto")));
        let th = load_json(&p.pack.join("theme.json"), Value::Null);
        assert_eq!(th["seed"], "#E53935");
        assert_eq!(th["colors"]["primary"], r["primary"]);
        assert!(run_theme(&Opts { color: Some("nope".into()), ..Default::default() }, &p).is_err());
    }

    #[test]
    fn thumbnail_and_seed_from_a_real_picture() {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let d = tmp("img");
        // a 2000x1000 BMP, left half red and right half blue, made by hand
        let (w, h) = (2000usize, 1000usize);
        let mut bmp = vec![b'B', b'M'];
        bmp.extend(((54 + w * h * 3) as u32).to_le_bytes());
        bmp.extend([0u8; 4]);
        bmp.extend(54u32.to_le_bytes());
        bmp.extend(40u32.to_le_bytes());
        bmp.extend((w as i32).to_le_bytes());
        bmp.extend((h as i32).to_le_bytes());
        bmp.extend([1, 0, 24, 0]);
        bmp.extend([0u8; 24]);
        for _ in 0..h { for x in 0..w { bmp.extend(if x < w / 2 { [0, 0, 255] } else { [255, 0, 0] }); } }
        let img = d.join("a.bmp");
        std::fs::write(&img, bmp).unwrap();
        let seed = source_from_image(&img).unwrap();
        assert!(seed.red == 255 && seed.blue == 0 || seed.blue == 255 && seed.red == 0, "{seed:?}");
        write_thumbnail(&d, &img);
        let (tw, th, _) = load_rgb(&d.join("wallpaper.jpg")).unwrap();
        assert_eq!((tw, th), (960, 480));
    }

    #[test]
    fn switchwall_rejects_bad_arguments() {
        let cfg = Config::parse("{}");
        let p = Paths::live();
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(switch(&cfg, &a(&["-Mode", "blue"]), &p).is_err());
        assert!(switch(&cfg, &a(&["-Scheme", "nope"]), &p).is_err());
        assert!(switch(&cfg, &a(&["-Bogus"]), &p).is_err());
        assert!(switch(&cfg, &a(&["-Image"]), &p).is_err());
    }
}
