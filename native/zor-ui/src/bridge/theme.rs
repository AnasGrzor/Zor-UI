//! Rust port of scripts/switchwall.ps1 and scripts/end4_theme.py: pick a wallpaper, make a Material You
//! palette from it and push that to GlazeWM, Flow Launcher and Windows Terminal, then follow
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
            Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED},
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
mod intelligence;
use palette::*;
static THEME_WORK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Settings drafts use the exact apply path, with no I/O or system changes.
pub(crate) fn preview_profile(analysis: &Value, profile: &Value, dark: bool, scheme: &str) -> std::collections::HashMap<String, u32> {
    intelligence::generate_profile(analysis, profile, dark, scheme).0.into_iter()
        .filter_map(|(k, v)| Some((k, crate::conf::parse_hex(&v)?))).collect()
}

/// The colours (name -> 0xAARRGGBB) `seed` would give with the scheme the user has (what Settings' colour picker
/// previews). Same maths as run_theme, nothing is written.
pub(crate) fn preview(seed: u32, dark: bool, scheme: &str) -> std::collections::HashMap<String, u32> {
    let rgb = Rgb::new((seed >> 16) as u8, (seed >> 8) as u8, seed as u8);
    let smart = scheme == "auto";
    let (colors, _, _) = generate(rgb, dark, if smart { "tonal-spot" } else { scheme }, smart);
    colors.into_iter().filter_map(|(k, v)| Some((k, crate::conf::parse_hex(&v)?))).collect()
}

const VIDEO_EXT: &[&str] = &["mp4", "webm", "mkv", "mov", "avi", "wmv", "m4v"];
const BEGIN: &str = "# >>> zor-ui colors (managed by end4_theme.py)";
const END: &str = "# <<< zor-ui colors";

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

/// Decode only a small image for color analysis. Full-size pixels are unnecessary here.
fn load_analysis_rgb(path: &Path) -> Option<(usize, usize, Vec<u8>)> {
    let f = factory()?;
    let frame = open_frame(&f, path, true)?;
    unsafe {
        let (mut w, mut h) = (0, 0);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 { return None; }
        let s = (128.0 / w as f64).min(128.0 / h as f64).min(1.0);
        let (w, h) = (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1));
        let scaler = f.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, w, h, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut pixels = vec![0; w as usize * h as usize * 3];
        conv.CopyPixels(std::ptr::null(), w * 3, &mut pixels).ok()?;
        Some((w as usize, h as usize, pixels))
    }
}

/// The wallpaper's seed colour (source_from_image).
#[cfg(test)]
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
            encode_jpeg(&f, &conv, nw, nh, &pack_dir.join("wallpaper.jpg"))
        }
    })();
}

/// `src` (24bpp BGR, nw x nh) as a JPEG at `dest`, through a temp file so a reader never sees half of it.
fn encode_jpeg(f: &IWICImagingFactory, src: &IWICFormatConverter, nw: u32, nh: u32, dest: &Path) -> Option<()> {
    let tmp = dest.with_extension("jpg.tmp");
    unsafe {
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
        out.WriteSource(src, std::ptr::null()).ok()?;
        out.Commit().ok()?;
        enc.Commit().ok()?;
        drop((out, enc, stream));
    }
    std::fs::rename(&tmp, dest).ok()
}

/// RGB bytes (w x h) saved as a JPEG.
fn write_rgb_jpeg(w: usize, h: usize, rgb: &[u8], dest: &Path) -> Option<()> {
    let f = factory()?;
    unsafe {
        let bmp = f.CreateBitmapFromMemory(w as u32, h as u32, &GUID_WICPixelFormat24bppRGB, (w * 3) as u32, rgb).ok()?;
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&bmp, &GUID_WICPixelFormat24bppBGR, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        encode_jpeg(&f, &conv, w as u32, h as u32, dest)
    }
}

// ------------------------------------------------------------------ per-monitor fit

/// Start of the `len`-long window over `scores` with the biggest sum; a tie goes to the one nearest the middle.
fn best_window(scores: &[f64], len: usize) -> usize {
    let len = len.clamp(1, scores.len().max(1));
    let n = scores.len().saturating_sub(len);
    let mut sum: f64 = scores.iter().take(len).sum();
    let (mut best, mut at) = (sum, 0usize);
    let nearer = |a: usize, b: usize| a.abs_diff(n / 2) < b.abs_diff(n / 2);
    for s in 1..=n {
        sum += scores[s + len - 1] - scores[s - 1];
        if sum > best + 1e-9 || (sum > best - 1e-9 && nearer(s, at)) { (best, at) = (sum, s); }
    }
    at
}

/// The part (x, y, w, h) of a w x h RGB picture that fills a screen of `aspect` (width / height) and holds the
/// most to look at: bright, detailed columns (or rows) win over a plain middle. The whole picture when the
/// shapes already match. shortcut: "interesting" is brightness plus edge strength, not face detection; a bright
/// empty sky can win over a dim subject. Upgrade by letting the user pin a focus point.
fn focus_crop(w: usize, h: usize, rgb: &[u8], aspect: f64) -> (usize, usize, usize, usize) {
    if w == 0 || h == 0 || rgb.len() < w * h * 3 || !(aspect.is_finite() && aspect > 0.0) { return (0, 0, w, h); }
    let (cw, ch) = if w as f64 / h as f64 > aspect { (((h as f64 * aspect).round() as usize).clamp(1, w), h) } else { (w, ((w as f64 / aspect).round() as usize).clamp(1, h)) };
    if (cw, ch) == (w, h) { return (0, 0, w, h); }
    let luma = |x: usize, y: usize| { let i = (y * w + x) * 3; 0.299 * rgb[i] as f64 + 0.587 * rgb[i + 1] as f64 + 0.114 * rgb[i + 2] as f64 };
    let step = (w.max(h) / 400).max(1);
    if cw < w {
        let scores: Vec<f64> = (0..w).map(|x| (0..h).step_by(step).map(|y| luma(x, y) + 3.0 * (luma(x, y) - luma((x + 1).min(w - 1), y)).abs()).sum()).collect();
        (best_window(&scores, cw), 0, cw, ch)
    } else {
        let scores: Vec<f64> = (0..h).map(|y| (0..w).step_by(step).map(|x| luma(x, y) + 3.0 * (luma(x, y) - luma(x, (y + 1).min(h - 1))).abs()).sum()).collect();
        (0, best_window(&scores, ch), cw, ch)
    }
}

/// Which picture a monitor shows: its own pick (config `wallpaperMonitors`, by monitor number) when that is a
/// picture that exists, else the main wallpaper. shortcut: monitors are numbered in Windows' order, so
/// re-plugging them in another order moves the picks; upgrade by keying on the device path.
fn monitor_source(own: &Value, i: usize, main: &str) -> (String, bool) {
    let pick = own[i.to_string()].as_str().filter(|p| {
        let q = Path::new(p);
        q.is_file() && !q.extension().is_some_and(|e| VIDEO_EXT.contains(&e.to_string_lossy().to_lowercase().as_str()))
    });
    (pick.unwrap_or(main).to_string(), pick.is_some())
}

/// What one monitor shows: `file` is its picture already cut to the monitor's shape.
struct Plan { id: HSTRING, w: i32, h: i32, name: String, file: String, custom: bool }

/// The monitors IDesktopWallpaper knows (attached ones only), each with the picture to give it. Pictures that
/// don't match the monitor's shape are cut to it with `focus_crop` and saved in the pack folder.
fn plan(dw: &IDesktopWallpaper, main: &str, own: &Value) -> windows::core::Result<Vec<Plan>> {
    let (mut out, names) = (vec![], crate::util::monitor_names());
    for i in 0..unsafe { dw.GetMonitorDevicePathCount()? } {
        let id = unsafe {
            let p = dw.GetMonitorDevicePathAt(i)?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            HSTRING::from(s)
        };
        // a monitor Windows can't measure still gets the picture, just uncut
        let r = unsafe { dw.GetMonitorRECT(&id) }.unwrap_or_default();
        let (n, w, h) = (out.len(), r.right - r.left, r.bottom - r.top);
        let (src, custom) = monitor_source(own, n, main);
        // no main picture in state (a solid-color theme): show what Windows has on this monitor
        let src = if src.is_empty() { windows_picture(dw, &id) } else { src };
        let file =if w > 0 && h > 0 { fit_file(&src, n, w, h) } else { None }.unwrap_or(src);
        let name = crate::util::monitor_label(&names, (r.left, r.top), n);
        out.push(Plan { id, w, h, name, file, custom });
    }
    Ok(out)
}

/// The picture Windows shows on one monitor; empty for a solid color.
fn windows_picture(dw: &IDesktopWallpaper, id: &HSTRING) -> String {
    unsafe { dw.GetWallpaper(id) }.ok().and_then(|p| unsafe {
        let path = p.to_string().ok(); CoTaskMemFree(Some(p.0 as *const _)); path
    }).unwrap_or_default()
}

/// `src` cut to the shape of a `w` x `h` monitor, saved as pack/wp-<n>-<hash>.jpg; None when it already fits (or can't be read).
fn fit_file(src: &str, n: usize, w: i32, h: i32) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let (pw, ph, px) = load_rgb(Path::new(src))?;
    let (x, y, cw, ch) = focus_crop(pw, ph, &px, w as f64 / h.max(1) as f64);
    let dir = pack();
    let stale = |keep: &str| if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("wp-{n}-")) && name != keep { let _ = std::fs::remove_file(e.path()); }
        }
    };
    if (cw, ch) == (pw, ph) { stale(""); return None; }
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (src, std::fs::metadata(src).and_then(|m| m.modified()).ok(), x, y, cw, ch).hash(&mut hs);
    let name = format!("wp-{n}-{:x}.jpg", hs.finish());
    let dest = dir.join(&name);
    if !dest.exists() {
        let _ = std::fs::create_dir_all(&dir);
        let rows: Vec<u8> = (y..y + ch).flat_map(|r| px[(r * pw + x) * 3..(r * pw + x + cw) * 3].iter().copied()).collect();
        write_rgb_jpeg(cw, ch, &rows, &dest)?;
    }
    stale(&name);
    Some(dest.to_string_lossy().into_owned())
}

/// bridge `wall-monitors`: each monitor as { i, w, h, name, file, custom } with the picture it shows, for Settings.
pub fn call(_cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    if name == "theme-options" {
        let _guard = THEME_WORK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let p = Paths::live();
        let state = load_json(&p.state, Value::Null);
        let image = state["image"].as_str().unwrap_or("");
        return Some(match intelligence::analysis(Path::new(image), &p.pack, args.contains(&"regenerate")) {
            Some(a) => json!({"ok":true, "profile": intelligence::profile(&p.pack, &preference_id(&a, state["live"].as_str())), "analysis": a}),
            None => err("Pick a wallpaper picture to generate palettes"),
        });
    }
    if name != "wall-monitors" { return None; }
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let main = load_json(&Paths::live().state, json!({}))["image"].as_str().unwrap_or("").to_string();
    let own = load_json(&pack().join("config.json"), Value::Null)["wallpaperMonitors"].clone();
    let list = unsafe { CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER) }
        .and_then(|dw| plan(&dw, &main, &own))        .map(|v| v.iter().enumerate().map(|(i, m)| json!({ "i": i, "w": m.w, "h": m.h, "name": m.name, "file": m.file, "custom": m.custom })).collect::<Vec<_>>())
        .unwrap_or_default();
    Some(json!({ "monitors": list }))
}

// ------------------------------------------------------------------ writers

/// theme.json, through a temp file so the widgets never read half of it.
fn write_theme(pack_dir: &Path, payload: &Value) -> bool { save_json(&pack_dir.join("theme.json"), payload) }

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
    std::fs::write(themes_dir.join("zor-ui.xaml"), out.replace('\n', "\r\n")).is_ok()
}

/// Windows Terminal fragment with an "zor-ui" colour scheme.
fn write_terminal(frag_dir: &Path, c: &BTreeMap<String, String>, t: &BTreeMap<String, String>) {
    let get = |m: &BTreeMap<String, String>, k: &str| Value::from(m.get(k).cloned().unwrap_or_default());
    let mut s = serde_json::Map::new();
    for (k, v) in [("name", Value::from("zor-ui")), ("background", get(c, "surface")), ("foreground", get(c, "onSurface")), ("cursorColor", get(c, "primary")), ("selectionBackground", get(c, "secondaryContainer"))] {
        s.insert(k.into(), v);
    }
    for (i, n) in ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"].iter().enumerate() {
        s.insert(n.to_string(), get(t, &format!("term{i}")));
        s.insert(format!("bright{}{}", n[..1].to_uppercase(), &n[1..]), get(t, &format!("term{}", i + 8)));
    }
    let _ = std::fs::create_dir_all(frag_dir);
    let _ = std::fs::write(frag_dir.join("zor-ui.json"), serde_json::to_string_pretty(&json!({ "schemes": [s] })).unwrap_or_default());
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

/// Device paths and bounds keep palette routing independent of monitor enumeration elsewhere.
fn monitor_palettes(dir: &Path, main: Option<&str>, dark: bool, scheme: &str, per_monitor: Option<bool>, primary: Option<(&Value, &BTreeMap<String, String>)>) -> Vec<Value> {
    let cfg = load_json(&dir.join("config.json"), Value::Null);
    let separate = per_monitor.unwrap_or(cfg["wallpaperTheme"]["perMonitor"] == true);
    if !separate && cfg["wallpaperMonitors"].as_object().is_none_or(|m| m.values().all(|v| v.as_str().unwrap_or("").is_empty())) { return vec![]; }
    let Ok(dw) = (unsafe { CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER) }) else { return vec![] };
    let mut result = vec![];
    let mut generated = std::collections::HashMap::new();
    if let Some((a, colors)) = primary { generated.insert(a["identity"].as_str().unwrap_or("").to_string(), colors.clone()); }
    for i in 0..unsafe { dw.GetMonitorDevicePathCount() }.unwrap_or(0) {
        let Some((id, r)) = (unsafe {
            dw.GetMonitorDevicePathAt(i).ok().and_then(|p| {
                let id = p.to_string().ok(); CoTaskMemFree(Some(p.0 as *const _));
                let id = HSTRING::from(id?);
                Some((id.clone(), dw.GetMonitorRECT(&id).ok()?))
            })
        }) else { continue };
        let (src, _) = monitor_source(&cfg["wallpaperMonitors"], i as usize, main.unwrap_or(""));
        if let Some(a) = intelligence::analysis(Path::new(&src), dir, false) {
            let key = a["identity"].as_str().unwrap_or("").to_string();
            let colors = generated.entry(key.clone()).or_insert_with(|| {
                if !separate { return BTreeMap::new(); }
                let profile = intelligence::profile(dir, &key);
                let own_scheme = profile["scheme"].as_str().unwrap_or(scheme);
                intelligence::cached_generate(&a, &profile, dark, own_scheme, dir).0
            });
            let actual = Some(windows_picture(&dw, &id)).filter(|p| !p.is_empty()).unwrap_or(src);
            let mut a = a;
            if let Some(fitted) = intelligence::analysis(Path::new(&actual), dir, false) { a["regions"] = fitted["regions"].clone(); }
            result.push(json!({"device": id.to_string_lossy(), "bounds": [r.left, r.top, r.right, r.bottom], "wallpaper":actual, "colors": if separate { json!(colors) } else { json!({}) }, "analysis": a}));
        }
    }
    result
}

/// Video frames may change each time a live wallpaper starts. Preferences follow the live source.
fn preference_id(analysis: &Value, live: Option<&str>) -> String {
    live.filter(|s| !s.is_empty()).map(|s| format!("live:{s}")).unwrap_or_else(|| analysis["identity"].as_str().unwrap_or("").to_string())
}

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
            flow_template: root().join("flowlauncher").join("zor-ui.xaml.template"),
            flow_themes: PathBuf::from(appdata).join("FlowLauncher").join("Themes"),
            terminal: PathBuf::from(local).join("Microsoft").join("Windows Terminal").join("Fragments").join("zor-ui"),
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
    profile: Option<Value>,
    expected: Option<String>,
    per_monitor: Option<bool>,
}

/// end4_theme.py's main(): palette from the image/colour (or the last source), written everywhere,
/// state.json updated (only the keys the script owns). Err: what stopped it.
fn run_theme(o: &Opts, p: &Paths) -> Result<Value, String> {
    let _guard = THEME_WORK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = load_json(&p.state, json!({}));
    if !state.is_object() { state = json!({}); }
    let s = |k: &str| state[k].as_str().filter(|v| !v.is_empty()).map(String::from);
    let mut mode = o.mode.clone().or_else(|| s("mode")).unwrap_or_else(|| "dark".into());
    let mut scheme = o.scheme.clone().or_else(|| s("scheme")).unwrap_or_else(|| "auto".into());
    let (mut image, mut color) = (o.image.clone(), o.color.clone());
    if image.is_none() && color.is_none() { (image, color) = (s("image"), s("color")); }
    if image.as_deref().is_some_and(|i| !Path::new(i).exists()) { image = None; }
    if image.is_none() && color.is_none() { color = Some("#6750A4".into()); }

    let analysis = image.as_ref().and_then(|i| intelligence::analysis(Path::new(i), &p.pack, false));
    let live = match &o.live { Some(l) => Some(l.clone()), None if o.image.is_none() && o.color.is_none() => s("live"), None => None };
    let identity = analysis.as_ref().and_then(|a| a["identity"].as_str());
    let preference = analysis.as_ref().map(|a| preference_id(a, live.as_deref()));
    if o.expected.as_deref().is_some_and(|expected| Some(expected) != identity) { return Err("The wallpaper changed. Reopen its palette preview.".into()); }
    let mut profile = o.profile.clone().unwrap_or_else(|| preference.as_ref().map(|id| intelligence::profile(&p.pack, id)).unwrap_or(Value::Null));
    if o.scheme.is_none() { if let Some(saved) = profile["scheme"].as_str() { scheme = saved.into(); } }
    if o.mode.is_none() { if let Some(saved) = profile["mode"].as_str() { mode = saved.into(); } }
    if profile.is_null() { profile = json!({}); }
    if let Some(scheme) = &o.scheme { profile["scheme"] = json!(scheme); }
    if o.profile.is_some() || o.mode.is_some() { profile["mode"] = json!(mode); }
    intelligence::validate_profile(&profile)?;
    let seed = match (&image, &color) {
        (Some(i), _) => analysis.as_ref().and_then(|a| a["autoSeed"].as_str()).and_then(hex_rgb).ok_or_else(|| format!("Couldn't read the picture {i}"))?,
        (None, Some(c)) => hex_rgb(c).ok_or_else(|| format!("Not a #RRGGBB colour: {c}"))?,
        _ => return Err("no colour source".into()),
    };
    let dark = mode == "dark";
    let smart = scheme == "auto";
    let (colors, term, used, seed) = if let Some(a) = &analysis {
        intelligence::cached_generate(a, &profile, dark, &scheme, &p.pack)
    } else {
        let (c, t, used) = generate(seed, dark, if smart { "tonal-spot" } else { &scheme }, smart);
        (c, t, used, seed)
    };

    let monitors = monitor_palettes(&p.pack, image.as_deref(), dark, &scheme, o.per_monitor, analysis.as_ref().map(|a| (a, &colors)));
    let payload = json!({ "mode": mode, "scheme": scheme, "resolvedScheme": used, "seed": rgb_hex(seed), "wallpaper": image, "live": live, "colors": colors, "term": term, "analysis": analysis, "profile": profile, "monitors": monitors });
    let previous = load_json(&p.pack.join("theme.json"), Value::Null);
    if !write_theme(&p.pack, &payload) { return Err("Couldn't save theme.json".into()); }
    if let Some(per_monitor) = o.per_monitor {
        let path = p.pack.join("config.json");
        let mut cfg = load_json(&path, json!({}));
        if !cfg.is_object() { cfg = json!({}); }
        if !cfg["wallpaperTheme"].is_object() { cfg["wallpaperTheme"] = json!({}); }
        cfg["wallpaperTheme"]["perMonitor"] = json!(per_monitor);
        if !save_json(&path, &cfg) { return Err("Couldn't save monitor theme policy".into()); }
    }
    if (o.profile.is_some() || o.scheme.is_some() || o.mode.is_some()) && identity.is_some() {
        if !intelligence::save_profile(&p.pack, preference.as_deref().unwrap_or(""), &profile) { return Err("Couldn't save wallpaper preferences".into()); }
    }
    if previous["analysis"]["identity"] != payload["analysis"]["identity"] || !p.pack.join("wallpaper.jpg").exists() {
        if let Some(i) = &image { write_thumbnail(&p.pack, Path::new(i)); }
    }
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

/// Chooses a WAV recording for voice cloning (IFileOpenDialog). None: cancelled. Blocks until the dialog closes,
/// so call it off the UI thread.
pub(crate) fn pick_wav() -> Option<String> {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dlg.SetTitle(w!("Choose a voice recording")).ok()?;
        let spec = |n: PCWSTR, s: PCWSTR| COMDLG_FILTERSPEC { pszName: n, pszSpec: s };
        dlg.SetFileTypes(&[spec(w!("WAV recordings"), w!("*.wav")), spec(w!("All files"), w!("*.*"))]).ok()?;
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

/// Sets each monitor's picture (its own pick, else `image`) cut to the monitor's shape: IDesktopWallpaper
/// (it crossfades, like end4's), else the instant SPI call with the whole picture.
fn set_wallpaper(image: &str) {
    reg_str(w!(r"Control Panel\Desktop"), w!("WallpaperStyle"), "10"); // Fill
    reg_str(w!(r"Control Panel\Desktop"), w!("TileWallpaper"), "0");
    let fade = || -> windows::core::Result<()> {
        let dw: IDesktopWallpaper = unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER)? };
        let own = load_json(&pack().join("config.json"), Value::Null)["wallpaperMonitors"].clone();
        unsafe { dw.SetPosition(DWPOS_FILL)? };
        let plans = plan(&dw, image, &own)?;
        if plans.is_empty() { return unsafe { dw.SetWallpaper(PCWSTR::null(), &HSTRING::from(image)) }; }
        // every monitor is tried; one that refuses is logged and does not stop the rest
        let mut failed = 0;
        for (i, m) in plans.iter().enumerate().filter(|(_, m)| !m.file.is_empty()) {
            let r = unsafe { dw.SetWallpaper(&m.id, &HSTRING::from(m.file.as_str())) };
            crate::log::line(&format!("wallpaper: monitor {i} ({}x{}) <- {} {}", m.w, m.h, m.file, r.as_ref().map_or_else(|e| format!("failed: {e}"), |_| "ok".into())));
            failed += r.is_err() as usize;
        }
        if failed > 0 { Err(windows::core::Error::from(windows::Win32::Foundation::E_FAIL)) } else { Ok(()) }
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
    let (mut toggle, mut pick, mut theme_only, mut clear) = (false, false, false, false);
    let mut monitor = String::new();
    let (mut profile, mut expected) = (None, None);
    let mut per_monitor = None;
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
            "monitor" => monitor = val()?,
            "clear" => clear = true,
            "profile" => {
                let value: Value = serde_json::from_str(&val()?).map_err(|_| "Invalid theme preferences")?;
                intelligence::validate_profile(&value)?;
                profile = Some(value);
            }
            "expectedwallpaper" => expected = Some(val()?),
            "permonitor" => per_monitor = Some(val()?.parse::<bool>().map_err(|_| "PerMonitor must be true or false")?),
            _ => return Err(format!("A parameter cannot be found that matches parameter name '{name}'.")),
        }
    }
    if !["", "dark", "light"].contains(&mode.as_str()) { return Err(format!("Cannot validate argument on parameter 'Mode'. The argument \"{mode}\" does not belong to the set \"dark,light\".")); }
    if !scheme.is_empty() && scheme != "auto" && !SCHEMES.contains(&scheme.as_str()) { return Err(format!("Cannot validate argument on parameter 'Scheme'. The argument \"{scheme}\" is not a known scheme.")); }

    let state = load_json(&paths.state, Value::Null);
    if !monitor.is_empty() {
        // -Monitor N [-Image file | -Clear]: that monitor gets its own picture (or goes back to the main one); the theme stays
        let n: usize = monitor.parse().map_err(|_| format!("Cannot validate argument on parameter 'Monitor'. \"{monitor}\" is not a monitor number."))?;
        let file = if clear { String::new() } else {
            let f = if image.is_empty() { match pick_file() { Some(f) => f, None => return Ok(json!({ "ok": false, "cancelled": true })) } } else { image };
            resolve(&f)?
        };
        bridge_json(cfg, "config-set", &[&format!("wallpaperMonitors.{n}"), &json!(file).to_string()]).ok_or("Couldn't save the choice")?;
        if let Some(main) = state["image"].as_str().filter(|i| !i.is_empty() && Path::new(i).exists()) { set_wallpaper(main); }
        if load_json(&paths.pack.join("config.json"), Value::Null)["wallpaperTheme"]["perMonitor"] == true {
            run_theme(&Opts::default(), paths)?;
        }
        return Ok(json!({ "ok": true, "monitor": n, "file": file }));
    }
    let retheme = toggle || !mode.is_empty() || !scheme.is_empty() || profile.is_some() || theme_only;
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
        // closed even when Zor UI didn't start it (state.live empty): it draws over the Windows wallpaper.
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

    // 2. Palette -> GlazeWM / Flow Launcher / Terminal.
    let opt = |s: &String| (!s.is_empty()).then(|| s.clone());
    let result = run_theme(&Opts { image: opt(&image), color: opt(&color), mode: opt(&mode), scheme: opt(&scheme), live: opt(&live), profile, expected, per_monitor }, paths)
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

    #[test]
    fn preview_matches_what_apply_would_generate_and_follows_mode_and_scheme() {
        let seed = hex_rgb("#4F46E5").unwrap();
        let p = preview(0xFF4F_46E5, true, "tonal-spot");
        let (want, ..) = generate(seed, true, "tonal-spot", false);
        assert_eq!(p["primary"], crate::conf::parse_hex(&want["primary"]).unwrap());
        assert!(p.len() > 40 && p.contains_key("surfaceContainerHigh") && p.contains_key("onPrimary"));
        assert_ne!(p["background"], preview(0xFF4F_46E5, false, "tonal-spot")["background"]);
        assert_ne!(p["primary"], preview(0xFF4F_46E5, true, "fruit-salad")["primary"]);
        // "auto" is tonal-spot with the grey fallback, like run_theme
        assert_eq!(preview(0xFF4F_46E5, true, "auto")["primary"], p["primary"]);
        assert_eq!(preview(0xFF80_8080, true, "auto")["primary"], crate::conf::parse_hex(&generate(hex_rgb("#808080").unwrap(), true, "tonal-spot", true).0["primary"]).unwrap());
    }

    pub(super) fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zor-ui-theme-{}-{name}", std::process::id()));
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
        assert_eq!(std::fs::read_to_string(d.join("themes").join("zor-ui.xaml")).unwrap(), "#000002 #000003 Light #000000");
        assert!(!write_flow(&d.join("none"), &d.join("themes"), &colors(), true));
    }

    #[test]
    fn terminal_fragment_has_sixteen_colours() {
        let d = tmp("term");
        let (_, term, _) = generate(hex_rgb("#6750A4").unwrap(), true, "tonal-spot", false);
        write_terminal(&d, &colors(), &term);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("zor-ui.json")).unwrap()).unwrap();
        let s = &v["schemes"][0];
        assert_eq!(s["name"], "zor-ui");
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
    fn wallpaper_profiles_restore_and_stale_drafts_write_no_theme() {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let d = tmp("restore");
        let p = Paths { state: d.join("state.json"), pack: d.join("pack"), glazewm: d.join("glaze.yaml"), flow_template: d.join("template"), flow_themes: d.join("flow"), terminal: d.join("terminal"), reload: false };
        let a = d.join("a.jpg"); let b = d.join("b.jpg");
        write_rgb_jpeg(16, 16, &[180, 40, 60].repeat(256), &a).unwrap();
        write_rgb_jpeg(16, 16, &[40, 80, 180].repeat(256), &b).unwrap();
        run_theme(&Opts { image: Some(a.to_string_lossy().into()), ..Default::default() }, &p).unwrap();
        let generated = load_json(&p.pack.join("theme.json"), Value::Null);
        let profile = json!({"selection":"curated", "seed":"#2288AA", "intensity":0.7, "scheme":"expressive"});
        run_theme(&Opts { profile: Some(profile.clone()), expected: generated["analysis"]["identity"].as_str().map(String::from), ..Default::default() }, &p).unwrap();
        let saved = load_json(&p.pack.join("theme.json"), Value::Null);
        run_theme(&Opts { image: Some(b.to_string_lossy().into()), ..Default::default() }, &p).unwrap();
        let before = std::fs::read(p.pack.join("theme.json")).unwrap();
        assert!(run_theme(&Opts { profile: Some(profile), expected: generated["analysis"]["identity"].as_str().map(String::from), ..Default::default() }, &p).is_err());
        assert_eq!(std::fs::read(p.pack.join("theme.json")).unwrap(), before);
        run_theme(&Opts { image: Some(a.to_string_lossy().into()), mode: Some("light".into()), ..Default::default() }, &p).unwrap();
        let restored = load_json(&p.pack.join("theme.json"), Value::Null);
        assert_eq!(restored["profile"]["seed"], saved["profile"]["seed"]);
        assert_eq!(restored["profile"]["intensity"], saved["profile"]["intensity"]);
        assert_eq!(restored["scheme"], "expressive");
        assert_eq!(restored["mode"], "light");
        assert_eq!(preference_id(&json!({"identity":"frame-a"}), Some("we:123")), preference_id(&json!({"identity":"frame-b"}), Some("we:123")));
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

    /// A w x h picture, dark except a bright block over columns/rows `lit` (x range, y range).
    fn lit_picture(w: usize, h: usize, lit: (std::ops::Range<usize>, std::ops::Range<usize>)) -> Vec<u8> {
        (0..w * h).flat_map(|i| { let v = if lit.0.contains(&(i % w)) && lit.1.contains(&(i / w)) { 220 } else { 5 }; [v, v, v] }).collect()
    }

    #[test]
    fn focus_crop_follows_the_bright_part_and_stays_inside() {
        // 1600x900 landscape for a 9:16 screen: the window is 506 wide; the lit block sits at the right
        let px = lit_picture(1600, 900, (1100..1400, 0..900));
        let (x, y, w, h) = focus_crop(1600, 900, &px, 9.0 / 16.0);
        assert_eq!((y, w, h), (0, 506, 900));
        assert!(x >= 1100 - 506 && x + w <= 1600 && x + w > 1100, "x={x}");
        // lit in the middle: stays the middle
        let mid = lit_picture(1600, 900, (700..900, 0..900));
        let (x, ..) = focus_crop(1600, 900, &mid, 9.0 / 16.0);
        assert!(x + 506 > 700 && x < 900, "x={x}");
        // a tall picture for a wide screen slides along the rows
        let tall = lit_picture(900, 1600, (0..900, 1000..1200));
        let (x, y, w, h) = focus_crop(900, 1600, &tall, 16.0 / 9.0);
        assert_eq!((x, w, h), (0, 900, 506));
        assert!(y <= 1200 && y + h > 1000, "y={y}");
    }

    #[test]
    fn focus_crop_keeps_a_picture_that_already_fits_and_ignores_bad_input() {
        let px = lit_picture(160, 90, (0..10, 0..10));
        assert_eq!(focus_crop(160, 90, &px, 16.0 / 9.0), (0, 0, 160, 90));
        assert_eq!(focus_crop(160, 90, &px, 0.0), (0, 0, 160, 90));
        assert_eq!(focus_crop(160, 90, &px, f64::NAN), (0, 0, 160, 90));
        assert_eq!(focus_crop(160, 90, &px[..10], 0.5), (0, 0, 160, 90));
        assert_eq!(focus_crop(0, 0, &[], 0.5), (0, 0, 0, 0));
    }

    #[test]
    fn best_window_prefers_the_middle_on_a_tie() {
        assert_eq!(best_window(&[1.0; 10], 4), 3);
        assert_eq!(best_window(&[0.0, 0.0, 5.0, 5.0, 0.0, 0.0], 2), 2);
        assert_eq!(best_window(&[1.0, 2.0], 5), 0);
    }

    #[test]
    fn a_monitor_uses_its_own_picture_only_when_it_is_a_picture_that_exists() {
        let d = tmp("mon");
        let (png, clip) = (d.join("a.png"), d.join("b.mp4"));
        std::fs::write(&png, "x").unwrap();
        std::fs::write(&clip, "x").unwrap();
        let own = json!({ "1": png.to_string_lossy(), "2": clip.to_string_lossy(), "3": "C:/gone.png", "4": "" });
        assert_eq!(monitor_source(&own, 1, "main.jpg"), (png.to_string_lossy().into_owned(), true));
        for i in [0, 2, 3, 4] { assert_eq!(monitor_source(&own, i, "main.jpg"), ("main.jpg".into(), false), "{i}"); }
        assert_eq!(monitor_source(&Value::Null, 1, "main.jpg"), ("main.jpg".into(), false));
    }

    #[test]
    fn fit_file_cuts_a_wide_picture_for_a_portrait_monitor() {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let d = tmp("fit");
        let (w, h) = (1600, 900);
        let src = d.join("wide.jpg");
        write_rgb_jpeg(w, h, &lit_picture(w, h, (1100..1400, 0..900)), &src).unwrap();
        // the pack folder is the real one, so use a monitor number nothing else uses
        let out = fit_file(&src.to_string_lossy(), 97, 1080, 1920).expect("cut");
        let (cw, ch, _) = load_rgb(Path::new(&out)).unwrap();
        assert_eq!(ch, 900);
        assert!((cw as i64 - 506).abs() <= 1, "{cw}");
        assert_eq!(fit_file(&src.to_string_lossy(), 97, 1600, 900), None);
        assert!(!Path::new(&out).exists(), "an old cut is removed once the picture fits");
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
        assert!(switch(&cfg, &a(&["-Monitor", "left"]), &p).is_err());
    }
}
