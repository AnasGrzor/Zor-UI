//! Rust port of end4_bridge.py's visualizer (WASAPI loopback + FFT), record (ffmpeg gdigrab) and
//! pick-image (file dialog). color-pick is in picker.rs; screen-translate in screen_translate.rs.

use super::common::*;
use super::*;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND},
        Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS},
        Media::Audio::*,
        System::{
            Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED},
            SystemInformation::GetLocalTime,
            Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE},
        },
        UI::{
            Shell::{Common::COMDLG_FILTERSPEC, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH},
            WindowsAndMessaging::GetForegroundWindow,
        },
    },
};

/// Streaming commands this module runs. Empty until `Sink` can write a raw line: the visualizer's
/// lines are plain "0.12 0.34 ..." text, not JSON (see `send`).
pub const STREAMS: &[&str] = &["visualizer"];

/// One-shot command `name`; None if this module doesn't port it.
pub fn call(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    match name {
        "record" => Some(record(cfg, args.first().copied().unwrap_or("status"))),
        "pick-image" => Some(match args {
            [slot] => pick_image(slot),
            _ => json!({ "error": "bad arguments: pick-image takes 1 argument" }),
        }),
        _ => None,
    }
}

/// Runs streaming command `name` (one of STREAMS) until it ends or `out.stopped()`.
pub fn stream(_cfg: &Config, name: &str, args: &[String], _stdin: &str, out: &Sink) {
    if name == "visualizer" {
        let bars = args.first().and_then(|b| b.parse::<i64>().ok()).unwrap_or(48).clamp(8, 256) as usize;
        visualizer(bars, out);
    }
}

// ===================================================================== visualizer

/// FFT -> log-spaced bars with cava-like auto-sensitivity and gravity falloff (Spectrum in end4_bridge.py).
struct Spectrum {
    rate: u32,
    size: usize,
    idx: Vec<Vec<usize>>,
    window: Vec<f64>,
    buf: Vec<f64>,
    level: Vec<f64>,
    peak: f64,
    weights: Vec<f64>,
}

impl Spectrum {
    fn new(bars: usize, rate: u32) -> Spectrum {
        let (size, lo, hi) = (2048usize, 50.0f64, 12000.0f64);
        let step = rate as f64 / size as f64;
        let edge = |i: usize| if i == bars { hi } else { lo * (hi / lo).powf(i as f64 / bars as f64) };
        let idx = (0..bars)
            .map(|i| {
                let (a, b) = (edge(i), edge(i + 1).max(edge(i) + step));
                (0..=size / 2).filter(|&k| { let f = k as f64 * step; f >= a && f < b }).collect()
            })
            .collect();
        let window = (0..size).map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / (size - 1) as f64).cos()).collect();
        let weights = (0..bars).map(|i| 0.8 + 0.8 * i as f64 / (bars - 1).max(1) as f64).collect();
        Spectrum { rate, size, idx, window, buf: vec![0.0; size], level: vec![0.0; bars], peak: 1e-3, weights }
    }

    /// Appends mono samples, keeping the newest `size`.
    fn push(&mut self, mono: &[f64]) {
        self.buf.extend_from_slice(mono);
        let extra = self.buf.len() - self.size;
        self.buf.drain(..extra);
    }

    /// The bar levels (0-1) `dt` seconds after the last frame.
    fn frame(&mut self, dt: f64) -> &[f64] {
        let (mut re, mut im): (Vec<f64>, Vec<f64>) = (self.buf.iter().zip(&self.window).map(|(a, w)| a * w).collect(), vec![0.0; self.size]);
        fft(&mut re, &mut im);
        let raw: Vec<f64> = self.idx.iter().zip(&self.weights)
            .map(|(ix, w)| {
                let mean = if ix.is_empty() { 0.0 } else { ix.iter().map(|&k| re[k].hypot(im[k])).sum::<f64>() / ix.len() as f64 };
                (mean * w).ln_1p()
            })
            .collect();
        self.peak = (self.peak * (1.0 - 0.5 * dt)).max(raw.iter().cloned().fold(f64::MIN, f64::max)).max(1e-3);   // auto-sensitivity
        let fall = 2.8 * dt;                                                                                        // gravity
        for (l, r) in self.level.iter_mut().zip(&raw) {
            let target = (r / self.peak).clamp(0.0, 1.0);
            *l = if target > *l { target * 0.6 + *l * 0.4 } else { target.max(*l - fall) };
        }
        &self.level
    }
}

/// In-place radix-2 FFT (length must be a power of two).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len <<= 1;
    }
}

/// One output line: the levels as "0.12 0.34 ...".
fn level_line(level: &[f64]) -> String { level.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" ") }

/// Writes a plain-text line (viz::parse_levels reads "0.12 0.34 ...", not JSON).
fn send(out: &Sink, line: &str) -> bool { out.emit_line(line) }

struct Shared {
    spec: Mutex<Spectrum>,
    last_audio: Mutex<Option<Instant>>,
}

/// The default output device's id, to notice when it changes.
unsafe fn default_render_id(en: &IMMDeviceEnumerator) -> Option<String> {
    let d = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let p = d.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

/// Captures what the default output plays (WASAPI loopback) into `sh` until the device changes (Ok)
/// or something fails (Err); the caller reconnects.
unsafe fn capture_once(en: &IMMDeviceEnumerator, bars: usize, sh: &Shared, stop: &dyn Fn() -> bool) -> windows::core::Result<()> {
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let id = default_render_id(en);
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let fmt = client.GetMixFormat()?;
    let (ch, rate, bits) = ((*fmt).nChannels as usize, (*fmt).nSamplesPerSec, (*fmt).wBitsPerSample);
    let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 1_000_000, 0, fmt, None);
    CoTaskMemFree(Some(fmt as _));
    init?;
    // shortcut: the shared-mode mix format is float32 (or int16); other layouts read as silence
    if ch == 0 || (bits != 32 && bits != 16) { return Err(windows::core::Error::from_hresult(windows::core::HRESULT(-1))); }
    {
        let mut s = sh.spec.lock().map_err(|_| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        if s.rate != rate { *s = Spectrum::new(bars, rate); }
    }
    let cap: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    let mut next_check = Instant::now() + Duration::from_secs(2);
    let result = (|| -> windows::core::Result<()> {
        while !stop() {
            std::thread::sleep(Duration::from_millis(10));
            while cap.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                let mono: Vec<f64> = (0..n).map(|f| {
                    if silent { return 0.0; }
                    let sum: f64 = (0..ch).map(|c| if bits == 32 { *(data as *const f32).add(f * ch + c) as f64 } else { *(data as *const i16).add(f * ch + c) as f64 / 32768.0 }).sum();
                    sum / ch as f64
                }).collect();
                cap.ReleaseBuffer(frames)?;
                if let (Ok(mut s), Ok(mut t)) = (sh.spec.lock(), sh.last_audio.lock()) { s.push(&mono); *t = Some(Instant::now()); }
            }
            if Instant::now() > next_check {   // follow default-device changes
                next_check = Instant::now() + Duration::from_secs(2);
                if default_render_id(en) != id { return Ok(()); }
            }
        }
        Ok(())
    })();
    let _ = client.Stop();
    result
}

/// The `visualizer` stream: ~30 lines of bar levels per second (one per second while silent).
fn visualizer(bars: usize, out: &Sink) {
    let sh = std::sync::Arc::new(Shared { spec: Mutex::new(Spectrum::new(bars, 48000)), last_audio: Mutex::new(None) });
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (sh2, stop2) = (sh.clone(), stop.clone());
    let cap = std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
        let stopped = || stop2.load(std::sync::atomic::Ordering::Relaxed);
        while !stopped() {
            if capture_once(&en, bars, &sh2, &stopped).is_err() {   // device changes, exclusive mode, etc.
                for _ in 0..10 { if stopped() { return; } std::thread::sleep(Duration::from_millis(100)); }
            }
        }
    });
    let fps = 30u32;
    let mut prev = Instant::now();
    let mut silent = 0u32;
    while !out.stopped() {
        std::thread::sleep(Duration::from_secs_f64(1.0 / fps as f64));
        let now = Instant::now();
        let dt = now.duration_since(prev).as_secs_f64();
        prev = now;
        let level: Vec<f64> = {
            let (Ok(mut s), Ok(t)) = (sh.spec.lock(), sh.last_audio.lock()) else { break };
            if t.map_or(true, |t| now.duration_since(t) > Duration::from_millis(250)) { s.buf.iter_mut().for_each(|v| *v *= 0.5); }   // nothing playing: let the bars fall
            s.frame(dt).to_vec()
        };
        if level.iter().cloned().fold(0.0, f64::max) < 0.01 {
            silent += 1;
            if silent > fps && silent % fps != 0 { continue; }   // idle: one line per second
        } else {
            silent = 0;
        }
        if !send(out, &level_line(&level)) { break; }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = cap.join();
}

// ===================================================================== record

fn ffmpeg_exe() -> PathBuf {
    let bin = root().join("venv/Lib/site-packages/imageio_ffmpeg/binaries");
    let bundled = std::fs::read_dir(bin).ok().and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("ffmpeg"))));
    bundled.unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

/// The pid's exe name, lowercased (None if the process is gone).
fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..n as usize]).to_lowercase())
    }
}

/// The running recording ({pid, file, started}) or None; a stale state file is removed.
fn recording(state: &Path) -> Option<Value> {
    let st = load_json(state, Value::Null);
    let alive = st.get("pid").and_then(Value::as_u64).and_then(|p| process_name(p as u32)).is_some_and(|n| n.contains("ffmpeg"));
    if st.is_null() { return None; }
    if !alive { let _ = std::fs::remove_file(state); return None; }
    Some(st)
}

/// `zor-ui-YYYYmmdd-HHMMSS` from the local clock.
fn stamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!("zor-ui-{:04}{:02}{:02}-{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Ends process `pid` and waits for it, but only when its exe path contains `part` (the pid may be reused).
fn kill(pid: u64, part: &str) {
    if !process_name(pid as u32).is_some_and(|n| n.contains(part)) { return; }
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid as u32) {
            let _ = TerminateProcess(h, 1);
            let _ = WaitForSingleObject(h, 3000);   // the video file must be closed before it is muxed
            let _ = CloseHandle(h);
        }
    }
}

/// Puts the audio track from `pcm` (the 48 kHz stereo f32 file rec_audio writes) into `video` as `dest`
/// and deletes both inputs. Without audio in the result (ffmpeg failed) the silent video becomes `dest`.
/// shortcut: the audio starts a few tenths of a second before the first video frame; no offset is applied.
fn mux(video: &Path, pcm: &Path, dest: &Path) {
    let rate = super::rec_audio::RATE.to_string();
    let ok = hidden(ffmpeg_exe()).args(["-nostdin", "-y", "-i"]).arg(video).args(["-f", "f32le", "-ar", &rate, "-ac", "2", "-i"]).arg(pcm)
        .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]).arg(dest)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().is_ok_and(|s| s.success());
    if ok { let _ = std::fs::remove_file(video); } else { let _ = std::fs::rename(video, dest); }
    let _ = std::fs::remove_file(pcm);
}

/// record start | stop | status. Records the primary monitor to Videos\Recordings, with the sound
/// `record.audio` asks for (none, mic, system or both).
fn record(cfg: &Config, action: &str) -> Value {
    let state = root().join("recording.json");
    let st = recording(&state);
    if action == "start" && st.is_none() {
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if !unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool() { return json!({ "error": "no display size" }); }
        let (w, h) = (dm.dmPelsWidth / 2 * 2, dm.dmPelsHeight / 2 * 2);   // primary monitor, physical pixels; x264 needs even sizes
        let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default());
        let folder = home.join("Videos").join("Recordings");
        let _ = std::fs::create_dir_all(&folder);
        let dest = folder.join(format!("{}.mp4", stamp()));
        let mode = cfg.str("record.audio");
        let audio = super::rec_audio::wanted(mode) != (false, false);
        // with sound, ffmpeg writes the video beside the final file and `mux` joins them on stop
        let video = if audio { dest.with_extension("video.mp4") } else { dest.clone() };
        // Fragmented MP4 (1 s fragments): stopping kills ffmpeg, and the file still plays.
        let mut c = hidden(ffmpeg_exe());
        c.args(["-nostdin", "-y", "-f", "gdigrab", "-framerate", "30", "-draw_mouse", "1", "-offset_x", "0", "-offset_y", "0", "-video_size", &format!("{w}x{h}"), "-i", "desktop",
            "-c:v", "libx264", "-preset", "ultrafast", "-crf", "23", "-pix_fmt", "yuv420p", "-g", "30", "-movflags", "+frag_keyframe+empty_moov+default_base_moof"])
            .arg(&video).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0000_0008 | 0x0000_0200 | NO_WINDOW);   // detached, own group: outlives the widget
        }
        let p = match c.spawn() { Ok(p) => p, Err(e) => return json!({ "error": format!("ffmpeg failed to start: {e}") }) };
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let mut st = json!({ "pid": p.id(), "file": dest.to_string_lossy(), "started": started });
        if audio {
            let pcm = dest.with_extension("pcm");
            let mut h = hidden(std::env::current_exe().unwrap_or_default());
            h.args(["--bridge", "record-audio", "--mode", mode, "--mic", cfg.str("record.mic"), "--out"]).arg(&pcm)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            {
                use std::os::windows::process::CommandExt;
                h.creation_flags(0x0000_0008 | 0x0000_0200 | NO_WINDOW);
            }
            st["video"] = json!(video.to_string_lossy());
            st["pcm"] = json!(pcm.to_string_lossy());
            if let Ok(hp) = h.spawn() { st["audioPid"] = json!(hp.id()); }   // no helper: stop keeps the silent video
        }
        save_json(&state, &st);
        return recording_json(Some(st));
    }
    if action == "stop" {
        if let Some(st) = st {
            if let Some(pid) = st["pid"].as_u64() { kill(pid, "ffmpeg"); }
            if let Some(pid) = st["audioPid"].as_u64() { kill(pid, "zorui"); }
            let _ = std::fs::remove_file(&state);
            let file = st["file"].as_str().unwrap_or_default().to_string();
            if let (Some(v), Some(p)) = (st["video"].as_str(), st["pcm"].as_str()) { mux(Path::new(v), Path::new(p), Path::new(&file)); }
            let _ = hidden("explorer").args(["/select,", &file]).spawn();
            return json!({ "recording": false, "file": file });
        }
    }
    recording_json(st)
}

/// {"recording": bool, **state}.
fn recording_json(st: Option<Value>) -> Value {
    let mut v = json!({ "recording": st.is_some() });
    if let (Some(Value::Object(s)), Some(o)) = (st, v.as_object_mut()) { o.extend(s); }
    v
}

// ===================================================================== pick-image

const IMAGE_EXT: [&str; 6] = [".bmp", ".gif", ".jpeg", ".jpg", ".png", ".webp"];

/// Copies `src` into `folder` as `<slot>-<unix time><ext>`, dropping the slot's older pictures
/// (a new name each time, so the card doesn't show a cached copy).
fn install_image(src: &Path, folder: &Path, slot: &str, now: u64) -> Value {
    let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    if !IMAGE_EXT.contains(&ext.as_str()) { return json!({ "error": "Pick a PNG, JPG, WebP, GIF or BMP picture" }); }
    if std::fs::metadata(src).map(|m| m.len()).unwrap_or(0) > 50_000_000 { return json!({ "error": "That picture is over 50 MB" }); }
    let _ = std::fs::create_dir_all(folder);
    let prefix = format!("{slot}-");
    for e in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix) { let _ = std::fs::remove_file(e.path()); }
    }
    let name = format!("{slot}-{now}{ext}");
    match std::fs::copy(src, folder.join(&name)) { Ok(_) => json!({ "file": name }), Err(e) => json!({ "error": e.to_string() }) }
}

/// The picture the user chooses (None: cancelled).
fn choose_picture(start: &Path) -> windows::core::Result<Option<PathBuf>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dlg.SetTitle(w!("Choose a picture"))?;
        let spec = HSTRING::from(IMAGE_EXT.iter().map(|e| format!("*{e}")).collect::<Vec<_>>().join(";"));
        dlg.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: w!("Pictures"), pszSpec: PCWSTR(spec.as_ptr()) }])?;
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None) { dlg.SetFolder(&item)?; }
        let owner: HWND = GetForegroundWindow();   // in front of Settings
        if let Err(e) = dlg.Show(owner) {
            if e.code().0 as u32 == 0x8007_04C7 { return Ok(None); }   // ERROR_CANCELLED
            return Err(e);
        }
        let p = dlg.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        Ok(s.map(PathBuf::from))
    }
}

/// "buddy-<slot>-<n>": a buddy's picture. The number makes each pick a new file, so a saved buddy keeps its own.
fn is_buddy_slot(s: &str) -> bool { s.strip_prefix("buddy-").is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) }

/// Asks for a picture and copies it into the pack's images folder (widgets can only show files inside the pack).
fn pick_image(slot: &str) -> Value {
    let ok = slot == "greet" || slot == "sticker" || is_buddy_slot(slot) || (slot.len() == 6 && slot.starts_with("image") && matches!(slot.as_bytes()[5], b'1'..=b'3'));
    if !ok { return json!({ "error": "bad slot" }); }
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default());
    let pics = home.join("Pictures");
    match choose_picture(if pics.is_dir() { &pics } else { &home }) {
        Ok(None) => json!({ "cancelled": true }),
        Ok(Some(src)) => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            install_image(&src, &pack().join("images"), slot, now)
        }
        Err(e) => json!({ "error": e.to_string() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values: the Python Spectrum(48) on 0.5 sin(2pi 1000 t) + 0.2 sin(2pi 5000 t), 2048 samples @ 48 kHz.
    fn sig() -> Vec<f64> { (0..2048).map(|i| { let t = i as f64 / 48000.0; 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin() + 0.2 * (2.0 * std::f64::consts::PI * 5000.0 * t).sin() }).collect() }

    #[test]
    fn spectrum_matches_python() {
        let mut s = Spectrum::new(48, 48000);
        s.push(&sig());
        let a = s.frame(1.0 / 30.0).to_vec();
        for (i, want) in [(25, 0.2896), (26, 0.6000), (27, 0.0447), (40, 0.3251), (0, 0.0002)] { assert!((a[i] - want).abs() < 6e-5, "bar {i}: {} vs {want}", a[i]); }
        s.buf.iter_mut().for_each(|v| *v *= 0.5);
        let b = s.frame(1.0 / 30.0).to_vec();
        for (i, want) in [(25, 0.3347), (26, 0.7638), (40, 0.3821)] { assert!((b[i] - want).abs() < 6e-5, "bar {i}: {} vs {want}", b[i]); }
        s.buf.iter_mut().for_each(|v| *v = 0.0);
        let c = s.frame(1.0 / 30.0).to_vec();
        for (i, want) in [(25, 0.2413), (26, 0.6705), (40, 0.2887), (27, 0.0)] { assert!((c[i] - want).abs() < 6e-5, "bar {i}: {} vs {want}", c[i]); }
        assert!((s.peak - 4.681_344_466_367_109_5).abs() < 1e-6, "{}", s.peak);   // Python's peak after the same three frames
    }

    #[test]
    fn fft_finds_a_bin_and_lines_format() {
        let (mut re, mut im): (Vec<f64>, Vec<f64>) = ((0..64).map(|i| (2.0 * std::f64::consts::PI * 4.0 * i as f64 / 64.0).cos()).collect(), vec![0.0; 64]);
        fft(&mut re, &mut im);
        assert!((re[4].hypot(im[4]) - 32.0).abs() < 1e-9 && re[5].hypot(im[5]) < 1e-9);
        assert_eq!(level_line(&[0.0, 0.126, 1.0]), "0.00 0.13 1.00");
    }

    #[test]
    fn push_keeps_newest() {
        let mut s = Spectrum::new(8, 48000);
        s.push(&vec![1.0; 3000]);
        s.push(&[2.0]);
        assert_eq!((s.buf.len(), s.buf[2046], s.buf[2047]), (2048, 1.0, 2.0));
    }

    #[test]
    fn install_image_copies_and_replaces() {
        let d = std::env::temp_dir().join(format!("zor-ui-media-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("a.PNG");
        std::fs::write(&src, b"x").unwrap();
        let folder = d.join("images");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("image1-1.png"), b"old").unwrap();
        std::fs::write(folder.join("image2-1.png"), b"keep").unwrap();
        assert_eq!(install_image(&src, &folder, "image1", 7), json!({ "file": "image1-7.png" }));
        assert!(!folder.join("image1-1.png").exists() && folder.join("image2-1.png").exists() && folder.join("image1-7.png").exists());
        std::fs::write(d.join("b.txt"), b"x").unwrap();
        assert!(install_image(&d.join("b.txt"), &folder, "image1", 8)["error"].is_string());
        assert_eq!(pick_image("nope"), json!({ "error": "bad slot" }));
        assert!(is_buddy_slot("buddy-a-123") && !is_buddy_slot("buddy-") && !is_buddy_slot("buddy-../x") && !is_buddy_slot("image1"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stale_recording_state_is_cleared() {
        let f = std::env::temp_dir().join(format!("zor-ui-rec-test-{}.json", std::process::id()));
        std::fs::write(&f, r#"{"pid": 4294967, "file": "x.mp4", "started": 1.0}"#).unwrap();
        assert!(recording(&f).is_none() && !f.exists());
        assert_eq!(recording_json(None), json!({ "recording": false }));
        assert_eq!(recording_json(Some(json!({ "pid": 1 }))), json!({ "recording": true, "pid": 1 }));
    }
}
