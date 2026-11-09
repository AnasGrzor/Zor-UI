//! Rust port of scripts/buddy_listen.py (the `buddy-listen` stream): records the microphone through
//! WASAPI until you stop talking, then transcribes it on this PC: with Windows' own recognizer by default, or
//! with a local Parakeet model when one is in %LOCALAPPDATA%\zor-ui\speech-models (or buddies.model names one).
//! The Parakeet recognizer is sherpa-onnx's C API, loaded at run time from %LOCALAPPDATA%\zor-ui\sherpa
//! (downloaded once from the sherpa-onnx release, only when Parakeet is used). Prints the same JSON lines as the script:
//! {"state": "starting" | "listening" | "thinking"}, {"level": rms, "peak": loudest sample, "floor": speech threshold}, {"partial": text so far} (Parakeet only), then {"text"} or {"error"}.

use super::{common::*, Sink};
use serde_json::json;
use std::{
    ffi::{c_char, c_void, CStr, CString},
    path::{Path, PathBuf},
    sync::{atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering}, Mutex, OnceLock},
    time::Duration,
};
use windows::{
    core::{PCWSTR, HSTRING},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::*,
        UI::Input::KeyboardAndMouse::GetAsyncKeyState,
        System::{
            Com::{StructuredStorage::{PropVariantClear, PropVariantToStringAlloc}, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ},
            LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_WITH_ALTERED_SEARCH_PATH},
        },
    },
};

pub const STREAMS: &[&str] = &["buddy-listen"];

const RATE: u32 = 16000;
const MODEL: &str = "parakeet-tdt-0.6b-v3-int8";
/// The sherpa-onnx build the struct offsets below were read from (include/sherpa-onnx/c-api/c-api.h).
const SHERPA_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.1/sherpa-onnx-v2.0.1-win-x64-shared-MD-Release-no-tts.tar.bz2";
const DOWNLOADS: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/";
const MODEL_FILES: [&str; 4] = ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"];
const DLLS: [&str; 3] = ["sherpa-onnx-c-api.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll"];

pub fn stream(_cfg: &crate::conf::Config, _name: &str, args: &[String], _stdin: &str, out: &Sink) {
    listen(args, out);
}

fn arg<'a>(args: &'a [String], flag: &str) -> &'a str {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("")
}

/// The model folder: `cfg_path` (buddies.model) or the default Parakeet folder in the app's speech-models
/// folder; "" when neither has tokens.txt.
fn model_dir(cfg_path: &str) -> String {
    let default = MODELS.iter().map(Model::path).find(|p| complete_model(p)).unwrap_or_else(|| crate::conf::speech_dir().join(MODEL)).to_string_lossy().into_owned();
    [cfg_path.to_string(), default].into_iter().find(|d| !d.is_empty() && Path::new(d).join("tokens.txt").is_file()).unwrap_or_default()
}

/// The buddies.model value that picks Windows' own recognizer even when a model folder exists.
const WINDOWS: &str = "windows";

/// The Parakeet folder to use, or "" for Windows' own recognizer: asked for, or no model folder found.
fn parakeet_dir(cfg_path: &str) -> String {
    if cfg_path == WINDOWS { String::new() } else { model_dir(cfg_path) }
}

/// 16-bit mono 16 kHz PCM WAV bytes (what `read_wav` reads and Windows' recognizer takes).
fn wav_bytes(audio: &[f32]) -> Vec<u8> {
    let data = audio.len() as u32 * 2;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes()); b.extend(1u16.to_le_bytes()); b.extend(1u16.to_le_bytes());
    b.extend(RATE.to_le_bytes()); b.extend((RATE * 2).to_le_bytes()); b.extend(2u16.to_le_bytes()); b.extend(16u16.to_le_bytes());
    b.extend(b"data"); b.extend(data.to_le_bytes());
    for s in audio { b.extend(((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()); }
    b
}

/// Windows' built-in recognizer (System.Speech dictation, runs on this PC). The file name goes in through an
/// environment variable, not the command line, so a path can't inject script.
/// shortcut: PowerShell start-up adds ~1 s; move to SAPI through the windows crate if it bothers.
fn windows_transcribe(audio: &[f32]) -> Result<String, String> {
    let file = std::env::temp_dir().join(format!("zorui-listen-{}.wav", std::process::id()));
    std::fs::write(&file, wav_bytes(audio)).map_err(|e| format!("{}: {e}", file.display()))?;
    let script = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Speech; \
                  $r = New-Object System.Speech.Recognition.SpeechRecognitionEngine; \
                  $r.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar)); \
                  $r.SetInputToWaveFile($env:ZORUI_WAV); $x = $r.Recognize(); if ($x) { $x.Text }";
    let run = hidden("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", script]).env("ZORUI_WAV", &file).output();
    let _ = std::fs::remove_file(&file);
    let o = run.map_err(|e| format!("could not start Windows speech recognition: {e}"))?;
    if !o.status.success() {
        let why = String::from_utf8_lossy(&o.stderr);
        return Err(format!("Windows speech recognition is not available: {}", why.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error").trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn listen(args: &[String], out: &Sink) {
    let wav = arg(args, "--wav");
    let dir = parakeet_dir(arg(args, "--model"));
    out.emit(&json!({ "state": "starting" }));
    // the model takes seconds to load: do it while the microphone is already recording
    let loader = (!dir.is_empty()).then(|| std::thread::spawn(move || Recognizer::new(&dir)));
    // with a model, a second thread reads the model back as soon as it is loaded and decodes the audio so far
    // every so often, for the pill's live text; it hands the model back when the recording ends
    let (live, stop) = (Mutex::new(Vec::<f32>::new()), AtomicBool::new(false));
    std::thread::scope(|sc| {
        let worker = loader.map(|l| sc.spawn(|| partials(l, &live, &stop, out)));
        let audio = if wav.is_empty() {
            record(arg(args, "--mic"), dictate_mode(arg(args, "--mode")), out, worker.is_some().then_some(&live))
        } else {
            out.emit(&json!({ "state": "listening" }));
            read_wav(Path::new(wav))
        };
        stop.store(true, Ordering::Relaxed);
        let audio = match audio {
            Ok(a) => a,
            Err(e) => { out.emit(&json!({ "error": e })); return; }
        };
        if out.stopped() { return; }
        let Some(audio) = audio else { out.emit(&json!({ "text": "" })); return };
        out.emit(&json!({ "state": "thinking" }));
        let text = match worker {
            Some(w) => w.join().map_err(|_| "the speech model thread crashed".to_string()).and_then(|r| r).and_then(|r| r.transcribe(&audio)),
            None => windows_transcribe(&audio),
        };
        out.emit(&match text { Ok(t) => json!({ "text": t }), Err(e) => json!({ "error": e }) });
    });
}

/// Longest stretch of audio (seconds) decoded for a live line; older speech just scrolls off it.
const PARTIAL_WINDOW_S: usize = 20;

/// Where the audio decoded for a live line starts: the last `PARTIAL_WINDOW_S` seconds of `len` samples.
fn partial_start(len: usize) -> usize { len.saturating_sub(PARTIAL_WINDOW_S * RATE as usize) }

/// Time for another live decode: at least a second of audio, and 0.7 s more than the last decode saw.
fn partial_due(have: usize, last: usize) -> bool { have >= RATE as usize && have >= last + RATE as usize * 7 / 10 }

/// Waits for the model, then sends `{"partial": text}` for the audio recorded so far until `stop`. Returns the model.
fn partials(loader: std::thread::JoinHandle<Result<Recognizer, String>>, live: &Mutex<Vec<f32>>, stop: &AtomicBool, out: &Sink) -> Result<Recognizer, String> {
    let rec = loader.join().map_err(|_| "the speech model thread crashed".to_string())??;
    let mut last = 0;
    while !stop.load(Ordering::Relaxed) && !out.stopped() {
        std::thread::sleep(Duration::from_millis(100));
        let audio = {
            let Ok(a) = live.lock() else { break };
            if !partial_due(a.len(), last) { continue; }
            last = a.len();
            a[partial_start(a.len())..].to_vec()
        };
        match rec.transcribe(&audio) {
            Ok(t) if !t.is_empty() && !stop.load(Ordering::Relaxed) => { out.emit(&json!({ "partial": t })); }
            _ => {}
        }
    }
    Ok(rec)
}

// ===================================================================== recording

/// Loudness that counts as speech: 3x the quietest of the first chunks (the mic's own noise), at least 0.006.
fn threshold(noise: &[f32]) -> f32 {
    // capped: starting to talk during the first 0.3 s must not raise the bar above speech itself, but a
    // steadily noisy room (floor above 0.02) still gets a bar above its own noise
    let Some(low) = noise.iter().copied().reduce(f32::min) else { return 0.015 };
    (0.03f32).max(1.5 * low).min((0.006f32).max(3.0 * low))
}

/// Cuts 0.1 s chunks into a recording: waits up to `wait` s for speech, stops after `quiet` s of silence
/// or `longest` s of speech. The chunk before the first loud one is kept so word starts survive.
struct Vad { wait: f32, quiet: f32, longest: f32, got: Vec<f32>, chunks: u32, silent: f32, heard: bool, n: u32, noise: Vec<f32>, level: f32, prev: Option<Vec<f32>> }

impl Vad {
    /// Keeps everything from the first chunk and ends only at MANUAL_LONGEST (Toggle and Hold stop it from outside).
    fn manual() -> Vad { Vad { wait: f32::MAX, quiet: f32::MAX, longest: MANUAL_LONGEST, heard: true, ..Vad::new() } }

    fn new() -> Vad { Vad { wait: 6.0, quiet: 1.0, longest: 12.0, got: vec![], chunks: 0, silent: 0.0, heard: false, n: 0, noise: vec![], level: 0.015, prev: None } }

    /// Feeds one chunk: (its loudness, whether the recording is over).
    fn push(&mut self, a: &[f32]) -> (f32, bool) {
        self.n += 1;
        let rms = (a.iter().map(|x| x * x).sum::<f32>() / a.len().max(1) as f32).sqrt();
        if self.noise.len() < 3 { // the first 0.3 s measures the mic's noise floor
            self.noise.push(rms);
            self.level = threshold(&self.noise);
        }
        if rms > self.level && self.noise.len() >= 3 {
            if !self.heard { if let Some(p) = self.prev.take() { self.got.extend(p); self.chunks += 1; } }
            self.heard = true;
            self.silent = 0.0;
        } else if self.heard {
            self.silent += 0.1;
        }
        if self.heard { self.got.extend_from_slice(a); self.chunks += 1; }
        self.prev = Some(a.to_vec());
        let done = (!self.heard && self.n as f32 * 0.1 >= self.wait) || (self.heard && (self.silent >= self.quiet || self.chunks as f32 * 0.1 >= self.longest));
        (rms, done)
    }
}

fn friendly(d: &IMMDevice) -> String {
    unsafe {
        let Ok(store) = d.OpenPropertyStore(STGM_READ) else { return String::new() };
        let Ok(mut v) = store.GetValue(&PKEY_Device_FriendlyName) else { return String::new() };
        let name = PropVariantToStringAlloc(&v).ok().and_then(|p| { let s = p.to_string().ok(); CoTaskMemFree(Some(p.0 as _)); s }).unwrap_or_default();
        let _ = PropVariantClear(&mut v);
        name
    }
}

/// The first input device whose name contains `name` (any case); the Windows default for "" or no match.
pub(super) unsafe fn pick_mic(en: &IMMDeviceEnumerator, name: &str) -> windows::core::Result<IMMDevice> {
    let n = name.trim().to_lowercase();
    if !n.is_empty() {
        let list = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
        for i in 0..list.GetCount()? {
            let d = list.Item(i)?;
            if friendly(&d).to_lowercase().contains(&n) { return Ok(d); }
        }
    }
    en.GetDefaultAudioEndpoint(eCapture, eConsole)
}

/// Mono 16 kHz audio; None when nobody spoke. Err when the microphone cannot be opened.
/// `live` collects every chunk too, for the live text.
fn record(mic: &str, mode: Dictate, out: &Sink, live: Option<&Mutex<Vec<f32>>>) -> Result<Option<Vec<f32>>, String> {
    let held = if mode == Dictate::Hold { keys_down() } else { vec![] };
    // released before we even started: nothing to hold, so stop on silence like Auto
    let mode = if mode == Dictate::Hold && held.is_empty() { Dictate::Auto } else { mode };
    let _ = std::fs::remove_file(stop_file());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let e = |what: &str| { let what = what.to_string(); move |err: windows::core::Error| format!("{what}: {err}") };
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(e("microphone"))?;
        let dev = pick_mic(&en, mic).map_err(e("no microphone"))?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(e("microphone"))?;
        // shared mode converts to the format asked for, so the mic's own rate and channels don't matter
        let fmt = WAVEFORMATEX { wFormatTag: 3 /* WAVE_FORMAT_IEEE_FLOAT */, nChannels: 1, nSamplesPerSec: RATE, nAvgBytesPerSec: RATE * 4, nBlockAlign: 4, wBitsPerSample: 32, cbSize: 0 };
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 10_000_000, 0, &fmt, None).map_err(e("microphone"))?;
        let cap: IAudioCaptureClient = client.GetService().map_err(e("microphone"))?;
        client.Start().map_err(e("microphone"))?;
        out.emit(&json!({ "state": "listening" }));
        let (mut vad, mut pending) = (if mode == Dictate::Auto { Vad::new() } else { Vad::manual() }, Vec::<f32>::new());
        let (chunk, started) = ((RATE / 10) as usize, std::time::Instant::now());
        let mut stalled = false;
        let result = (|| -> windows::core::Result<()> {
            while !out.stopped() {
                std::thread::sleep(Duration::from_millis(10));
                if stop_wanted(mode, &held) { return Ok(()); }
                if vad.n == 0 && started.elapsed() > Duration::from_secs(3) { stalled = true; break; } // a muted or unplugged mic delivers nothing
                while cap.GetNextPacketSize()? > 0 {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                    cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                    if silent { pending.resize(pending.len() + frames as usize, 0.0); }
                    else { pending.extend_from_slice(std::slice::from_raw_parts(data as *const f32, frames as usize)); }
                    cap.ReleaseBuffer(frames)?;
                }
                while pending.len() >= chunk {
                    let a: Vec<f32> = pending.drain(..chunk).collect();
                    if let Some(l) = live { if let Ok(mut l) = l.lock() { l.extend_from_slice(&a); } }
                    let (rms, done) = vad.push(&a);
                    let peak = a.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                    out.emit(&json!({ "level": (rms * 1e4).round() / 1e4, "peak": (peak * 1e4).round() / 1e4, "floor": (vad.level * 1e4).round() / 1e4 })); // the pill's waveform, clipping and silence
                    if done { return Ok(()); }
                }
            }
            Ok(())
        })();
        let _ = client.Stop();
        result.map_err(e("microphone"))?;
        if stalled { return Err("No audio is coming from the microphone. Check that it is plugged in, not muted, and allowed under Settings > Privacy > Microphone.".into()); }
        Ok((!vad.got.is_empty()).then_some(vad.got))
    }
}

/// A 16-bit PCM WAV as mono 16 kHz floats (linear resample, good enough for speech); `--wav` uses it.
fn read_wav(path: &Path) -> Result<Option<Vec<f32>>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let u16at = |i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32at = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    if b.get(0..4) != Some(b"RIFF") || b.get(8..12) != Some(b"WAVE") { return Err("not a WAV file".into()); }
    let (mut i, mut fmt, mut data) = (12, None, None);
    while let (Some(id), Some(len)) = (b.get(i..i + 4), u32at(i + 4)) {
        let body = i + 8..(i + 8 + len as usize).min(b.len());
        match id { b"fmt " => fmt = Some((u16at(body.start), u16at(body.start + 2), u32at(body.start + 4), u16at(body.start + 14))), b"data" => data = Some(body.clone()), _ => {} }
        i = body.end + (len as usize & 1);
    }
    let (Some((Some(tag), Some(ch), Some(rate), Some(bits))), Some(data)) = (fmt, data) else { return Err("WAV has no fmt or data".into()) };
    if tag != 1 || bits != 16 || ch == 0 { return Err("only 16-bit PCM WAV files are supported".into()); }
    let mono: Vec<f32> = b[data].chunks_exact(2 * ch as usize)
        .map(|f| f.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).sum::<f32>() / ch as f32).collect();
    Ok((!mono.is_empty()).then(|| resample(&mono, rate)))
}

fn resample(a: &[f32], rate: u32) -> Vec<f32> {
    if rate == RATE || a.len() < 2 { return a.to_vec(); }
    let n = (a.len() as f64 * RATE as f64 / rate as f64) as usize;
    (0..n).map(|k| {
        let x = k as f64 * (a.len() - 1) as f64 / (n.max(2) - 1) as f64;
        let (i, f) = (x.floor() as usize, (x - x.floor()) as f32);
        a[i] + (a[(i + 1).min(a.len() - 1)] - a[i]) * f
    }).collect()
}

// ===================================================================== sherpa-onnx

type Fn1 = unsafe extern "C" fn(*const c_void) -> *const c_void;
struct Api {
    create: Fn1, destroy: unsafe extern "C" fn(*const c_void), stream: unsafe extern "C" fn(*const c_void) -> *const c_void,
    destroy_stream: unsafe extern "C" fn(*const c_void), accept: unsafe extern "C" fn(*const c_void, i32, *const f32, i32),
    decode: unsafe extern "C" fn(*const c_void, *const c_void), result: unsafe extern "C" fn(*const c_void) -> *const *const c_char,
    destroy_result: unsafe extern "C" fn(*const c_void),
}

fn sherpa_dir() -> PathBuf { root().join("sherpa") }

/// Downloads the sherpa-onnx DLLs once (tar.exe ships with Windows 10+ and reads .tar.bz2).
fn fetch_sherpa(dir: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("zorui-sherpa");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let tarball = tmp.join("s.tar.bz2");
    std::fs::write(&tarball, get_bytes(SHERPA_URL, &[], 600).ok_or("could not download the speech engine (sherpa-onnx) from GitHub")?).map_err(|e| e.to_string())?;
    let ok = hidden("tar.exe").arg("-xjf").arg(&tarball).arg("-C").arg(&tmp).status().is_ok_and(|s| s.success());
    if !ok { return Err("could not unpack the speech engine".into()); }
    let lib = std::fs::read_dir(&tmp).map_err(|e| e.to_string())?.filter_map(Result::ok).map(|e| e.path().join("lib")).find(|p| p.is_dir()).ok_or("the speech engine archive has no lib folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for d in DLLS { std::fs::copy(lib.join(d), dir.join(d)).map_err(|e| format!("{d}: {e}"))?; }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// `dir` holds every file the recognizer loads.
fn complete_model(dir: &Path) -> bool { MODEL_FILES.iter().all(|f| dir.join(f).is_file()) }

/// A model Settings offers to download. Both are 0.6b NeMo transducers, the one layout the recognizer below reads
/// (128 mel features, encoder/decoder/joiner int8 files). Other sherpa-onnx families (whisper, zipformer, paraformer,
/// SenseVoice, the 110m Parakeets with 80 features) would need their own recognizer code.
pub struct Model { pub id: &'static str, pub name: &'static str, pub note: &'static str, pub mb: u32, archive: &'static str, bytes: u64 }

/// `mb` is the size on disk (what Orca lists); `bytes` is the archive, only for the progress bar.
pub static MODELS: [Model; 2] = [
    Model { id: "v3", name: "Parakeet TDT v3", note: "Recommended. Highest accuracy for 25 European languages. Punctuation and capitalization.", mb: 670,
        archive: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2", bytes: 487_211_443 },
    Model { id: "v2", name: "Parakeet TDT v2", note: "English only. Faster than v3 with similar accuracy.", mb: 661,
        archive: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2", bytes: 482_440_461 },
];

impl Model {
    /// The folder it unpacks to in the speech-models folder: the archive name without its prefix and extension.
    pub fn folder(&self) -> String { self.archive.trim_end_matches(".tar.bz2").trim_start_matches("sherpa-onnx-nemo-").to_string() }
    pub fn path(&self) -> PathBuf { crate::conf::speech_dir().join(self.folder()) }
    pub fn installed(&self) -> bool { complete_model(&self.path()) }
}

/// Deletes the model folder `folder` under `base`; fine if it is already gone.
fn remove_model_dir(base: &Path, folder: &str) -> Result<(), String> {
    if folder.is_empty() || folder.contains(['/', '\\', '.']) { return Err("not a model folder".into()); }
    let d = base.join(folder);
    if !d.exists() { return Ok(()); }
    std::fs::remove_dir_all(&d).map_err(|e| e.to_string())
}

pub fn delete_model(id: &str) -> Result<(), String> {
    let m = MODELS.iter().find(|m| m.id == id).ok_or("unknown speech model")?;
    remove_model_dir(&crate::conf::speech_dir(), &m.folder())
}

/// How a voice command records. Auto stops when you stop talking. Toggle records until the hotkey is pressed again.
/// Hold records while the keys that started it are still down.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dictate { Auto, Toggle, Hold }

pub fn dictate_mode(s: &str) -> Dictate { match s { "toggle" => Dictate::Toggle, "hold" => Dictate::Hold, _ => Dictate::Auto } }

/// Longest recording in Toggle and Hold mode, in seconds.
const MANUAL_LONGEST: f32 = 120.0;

fn stop_file() -> PathBuf { std::env::temp_dir().join("zorui-listen-stop") }

/// Tells a recording to stop and transcribe (the hotkey pressed a second time in Toggle mode, or the pill's stop button).
pub fn request_finish() { let _ = std::fs::write(stop_file(), b""); }

fn key_down(k: u16) -> bool { unsafe { GetAsyncKeyState(k as i32) < 0 } }

/// Keyboard keys down right now (mouse buttons are 1..6, so they are skipped), plus the ones keymap.rs's hook ate: Windows never saw those go down.
fn keys_down() -> Vec<u16> {
    let mut v: Vec<u16> = (7u16..255).filter(|k| key_down(*k)).collect();
    let eaten: Vec<u16> = crate::keymap::eaten_held().into_iter().filter(|k| !v.contains(k)).collect();
    v.extend(eaten);
    v
}

fn all_released(held: &[u16], down: impl Fn(u16) -> bool) -> bool { held.iter().all(|k| !down(*k)) }

/// A recording should end now: someone asked (the hotkey again in Toggle mode, the pill's stop button in any mode),
/// or Hold's keys came up.
fn stop_wanted(mode: Dictate, held: &[u16]) -> bool { stop_wanted_with(mode, held, &stop_file()) }

/// `stop_wanted` with the stop-request file named, so a test can use its own.
fn stop_wanted_with(mode: Dictate, held: &[u16], file: &Path) -> bool {
    file.exists() || (mode == Dictate::Hold && { let eaten = crate::keymap::eaten_held(); all_released(held, |k| key_down(k) || eaten.contains(&k)) })
}

/// The first folder inside `dir` that holds a whole model (the archive's top folder has a different name).
fn find_model(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| complete_model(p))
}

/// What the model download is doing, for the settings row: 0 idle, 1 downloading, 2 unpacking.
static PHASE: AtomicU8 = AtomicU8::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// The archive's size on the release page, only for the progress bar (a wrong value just skews the bar).
const MODEL_BYTES: u64 = 487_211_443;
static TOTAL: AtomicU64 = AtomicU64::new(MODEL_BYTES);
/// What `download_model` returns when `cancel_download` stopped it.
pub const CANCELLED: &str = "cancelled";

pub fn cancel_download() { CANCEL.store(true, Ordering::Relaxed); }

/// (bar fraction 0..1, text) for a download in `phase` that has `done` of `total` bytes.
fn describe(phase: u8, done: u64, total: u64) -> (f32, String) {
    match phase {
        1 => ((done as f32 / total.max(1) as f32).min(0.99), format!("Downloading {} of {} MB", done >> 20, total >> 20)),
        2 => (1.0, "Unpacking...".into()),
        _ => (0.0, String::new()),
    }
}

/// Progress of the running `download_model`.
pub fn progress() -> (f32, String) { describe(PHASE.load(Ordering::Relaxed), DONE.load(Ordering::Relaxed), TOTAL.load(Ordering::Relaxed)) }

/// Runs `c` to the end, calling `tick` every 200 ms; kills it and returns `CANCELLED` when `cancel_download` is called.
fn run_cancellable(mut c: std::process::Command, tick: impl Fn()) -> Result<(), String> {
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    loop {
        if CANCEL.load(Ordering::Relaxed) { let _ = child.kill(); let _ = child.wait(); return Err(CANCELLED.into()); }
        match child.try_wait() {
            Ok(Some(st)) => return if st.success() { Ok(()) } else { Err("the command failed".into()) },
            Ok(None) => { tick(); std::thread::sleep(Duration::from_millis(200)); }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn unpack_model(tmp: &Path, base: &Path, url: &str, folder: &str) -> Result<(), String> {
    let tarball = tmp.join("m.tar.bz2");
    let mut get = hidden("curl.exe");
    get.args(["-s", "-f", "-L", "--max-time", "1800", "-A", "zor-ui", "-o"]).arg(&tarball).arg(url);
    PHASE.store(1, Ordering::Relaxed);
    run_cancellable(get, || DONE.store(std::fs::metadata(&tarball).map_or(0, |m| m.len()), Ordering::Relaxed))
        .map_err(|e| if e == CANCELLED { e } else { "could not download the Parakeet model from GitHub".into() })?;
    PHASE.store(2, Ordering::Relaxed);
    let mut untar = hidden("tar.exe");
    untar.arg("-xjf").arg(&tarball).arg("-C").arg(tmp);
    run_cancellable(untar, || {}).map_err(|e| if e == CANCELLED { e } else { "could not unpack the Parakeet model".into() })?;
    let src = find_model(tmp).ok_or("the Parakeet archive has no model folder")?;
    let dest = base.join(folder);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(src, dest).map_err(|e| e.to_string())
}

/// Downloads model `id` (see MODELS) into the speech-models folder, and the engine DLLs so the first voice command isn't
/// slow. Does nothing for a model that is already there. Blocks for minutes; call it from a thread.
pub fn download_model(id: &str) -> Result<(), String> {
    let m = MODELS.iter().find(|m| m.id == id).ok_or("unknown speech model")?;
    CANCEL.store(false, Ordering::Relaxed);
    DONE.store(0, Ordering::Relaxed);
    TOTAL.store(m.bytes, Ordering::Relaxed);
    if !m.installed() {
        let base = crate::conf::speech_dir();
        let tmp = base.join(".download"); // inside `base`, so the final rename stays on one drive
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let r = unpack_model(&tmp, &base, &format!("{DOWNLOADS}{}", m.archive), &m.folder());
        PHASE.store(0, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&tmp);
        r?;
    }
    if !DLLS.iter().all(|d| sherpa_dir().join(d).is_file()) { fetch_sherpa(&sherpa_dir())?; }
    Ok(())
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let dir = sherpa_dir();
        if !DLLS.iter().all(|d| dir.join(d).is_file()) { fetch_sherpa(&dir)?; }
        let dll = LoadLibraryExW(PCWSTR(HSTRING::from(dir.join(DLLS[0]).as_os_str()).as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH)
            .map_err(|e| format!("could not load the speech engine: {e}"))?;
        let sym = |n: &str| { let c = CString::new(n).unwrap(); GetProcAddress(dll, windows::core::PCSTR(c.as_ptr() as _)).ok_or(format!("speech engine is missing {n}")) };
        Ok(Api {
            create: std::mem::transmute(sym("SherpaOnnxCreateOfflineRecognizer")?), destroy: std::mem::transmute(sym("SherpaOnnxDestroyOfflineRecognizer")?),
            stream: std::mem::transmute(sym("SherpaOnnxCreateOfflineStream")?), destroy_stream: std::mem::transmute(sym("SherpaOnnxDestroyOfflineStream")?),
            accept: std::mem::transmute(sym("SherpaOnnxAcceptWaveformOffline")?), decode: std::mem::transmute(sym("SherpaOnnxDecodeOfflineStream")?),
            result: std::mem::transmute(sym("SherpaOnnxGetOfflineStreamResult")?), destroy_result: std::mem::transmute(sym("SherpaOnnxDestroyOfflineRecognizerResult")?),
        })
    }).as_ref().map_err(Clone::clone)
}

struct Recognizer { api: &'static Api, ptr: *const c_void }
unsafe impl Send for Recognizer {}

/// Byte offsets inside SherpaOnnxOfflineRecognizerConfig for sherpa-onnx v2.0.1 (64-bit): feat_config 0,
/// model_config.transducer 8 (encoder, decoder, joiner), then paraformer, nemo_ctc, whisper (48..96), tdnn.
const SAMPLE_RATE: usize = 0;
const FEATURE_DIM: usize = 4;
const ENCODER: usize = 8;
const DECODER: usize = 16;
const JOINER: usize = 24;
const TOKENS: usize = 104;
const NUM_THREADS: usize = 112;
const MODEL_TYPE: usize = 128;

impl Recognizer {
    fn new(dir: &str) -> Result<Recognizer, String> {
        let api = api()?;
        let file = |n: &str| CString::new(Path::new(dir).join(n).to_string_lossy().as_bytes()).map_err(|e| e.to_string());
        let strs = [file("encoder.int8.onnx")?, file("decoder.int8.onnx")?, file("joiner.int8.onnx")?, file("tokens.txt")?, CString::new("nemo_transducer").unwrap()];
        // the real struct is a few hundred bytes of pointers and ints; the rest of this zeroed buffer reads as "unset"
        let mut cfg = vec![0u64; 512];
        let base = cfg.as_mut_ptr() as *mut u8;
        unsafe {
            let put_i32 = |off: usize, v: i32| (base.add(off) as *mut i32).write(v);
            let put_str = |off: usize, s: &CString| (base.add(off) as *mut *const c_char).write(s.as_ptr());
            put_i32(SAMPLE_RATE, RATE as i32);
            put_i32(FEATURE_DIM, 128);
            put_str(ENCODER, &strs[0]); put_str(DECODER, &strs[1]); put_str(JOINER, &strs[2]);
            put_str(TOKENS, &strs[3]); put_i32(NUM_THREADS, 4); put_str(MODEL_TYPE, &strs[4]);
            let ptr = (api.create)(base as *const c_void);
            if ptr.is_null() { return Err("could not load the Parakeet model (see the model folder in Settings)".into()); }
            Ok(Recognizer { api, ptr })
        }
    }

    fn transcribe(&self, audio: &[f32]) -> Result<String, String> {
        unsafe {
            let s = (self.api.stream)(self.ptr);
            if s.is_null() { return Err("the speech model refused the audio".into()); }
            (self.api.accept)(s, RATE as i32, audio.as_ptr(), audio.len() as i32);
            (self.api.decode)(self.ptr, s);
            let r = (self.api.result)(s);
            let text = if r.is_null() || (*r).is_null() { String::new() } else { CStr::from_ptr(*r).to_string_lossy().trim().to_string() };
            if !r.is_null() { (self.api.destroy_result)(r as *const c_void); }
            (self.api.destroy_stream)(s);
            Ok(text)
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) { unsafe { (self.api.destroy)(self.ptr) } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_matches_python() {
        assert_eq!(threshold(&[]), 0.015);
        assert!((threshold(&[0.001, 0.002, 0.004]) - 0.006).abs() < 1e-6); // quiet room: floor 0.006
        assert!((threshold(&[0.01, 0.02, 0.03]) - 0.03).abs() < 1e-6);     // 3x low = 0.03 beats 1.5x
        assert!((threshold(&[0.05, 0.06, 0.07]) - 0.075).abs() < 1e-6);    // noisy room: 1.5x low
    }

    fn tone(amp: f32) -> Vec<f32> { vec![amp; 1600] }

    #[test]
    fn vad_gives_up_when_nobody_speaks() {
        let mut v = Vad::new();
        let done = (0..70).map(|_| v.push(&tone(0.001)).1).position(|d| d);
        assert_eq!(done, Some(59)); // 6 s of 0.1 s chunks
        assert!(v.got.is_empty());
    }

    #[test]
    fn vad_keeps_the_chunk_before_speech_and_stops_after_quiet() {
        let mut v = Vad::new();
        for _ in 0..5 { v.push(&tone(0.001)); }
        for _ in 0..5 { v.push(&tone(0.2)); }
        let done = (0..20).map(|_| v.push(&tone(0.001)).1).position(|d| d);
        assert_eq!(done, Some(9)); // 1 s of silence
        assert_eq!(v.chunks, 1 + 5 + 10); // the lead-in chunk, the speech, the silence it waited through
    }

    #[test]
    fn vad_caps_long_speech() {
        let mut v = Vad::new();
        for _ in 0..3 { v.push(&tone(0.001)); }
        assert_eq!((0..200).map(|_| v.push(&tone(0.2)).1).position(|d| d), Some(118)); // the lead-in chunk counts toward the 12 s
    }

    #[test]
    fn resample_halves_and_keeps_16k() {
        assert_eq!(resample(&[0.0, 1.0], 16000), vec![0.0, 1.0]);
        assert_eq!(resample(&vec![0.5; 32000], 32000).len(), 16000);
    }

    #[test]
    fn reads_a_stereo_wav() {
        let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        b.extend([16, 0, 0, 0, 1, 0, 2, 0]); b.extend(16000u32.to_le_bytes()); b.extend(64000u32.to_le_bytes()); b.extend([4, 0, 16, 0]);
        b.extend(b"data"); b.extend(8u32.to_le_bytes());
        for s in [16384i16, -16384, 32767, 0] { b.extend(s.to_le_bytes()); }
        let p = std::env::temp_dir().join("zorui-listen-test.wav");
        std::fs::write(&p, b).unwrap();
        let a = read_wav(&p).unwrap().unwrap();
        assert_eq!(a.len(), 2);
        assert!(a[0].abs() < 1e-6 && (a[1] - 0.49998).abs() < 1e-3);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn model_dir_needs_tokens() {
        assert_eq!(model_dir(r"C:\definitely\not\here"), model_dir(""));
    }

    #[test]
    fn a_model_needs_all_its_files_and_is_found_under_any_folder_name() {
        let dir = std::env::temp_dir().join(format!("zorui-dl-{}", std::process::id()));
        let inner = dir.join("sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8");
        std::fs::create_dir_all(&inner).unwrap();
        for f in &MODEL_FILES[..3] { std::fs::write(inner.join(f), "").unwrap(); }
        assert!(!complete_model(&inner) && find_model(&dir).is_none()); // tokens.txt missing
        std::fs::write(inner.join("tokens.txt"), "").unwrap();
        assert!(complete_model(&inner));
        assert_eq!(find_model(&dir), Some(inner));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_catalog_has_distinct_folders_and_v3_keeps_its_folder() {
        assert_eq!(MODELS[0].folder(), MODEL);
        assert_eq!(MODELS[1].folder(), "parakeet-tdt-0.6b-v2-int8");
        assert_ne!(MODELS[0].path(), MODELS[1].path());
        assert!(MODELS.iter().all(|m| m.mb > 0 && !m.note.is_empty()));
        assert!(download_model("nope").is_err() && delete_model("nope").is_err());
    }

    #[test]
    fn deleting_a_model_removes_only_its_folder_and_refuses_odd_names() {
        let base = std::env::temp_dir().join(format!("zorui-del-{}", std::process::id()));
        std::fs::create_dir_all(base.join("m1")).unwrap();
        std::fs::create_dir_all(base.join("m2")).unwrap();
        assert!(remove_model_dir(&base, "m1").is_ok());
        assert!(!base.join("m1").exists() && base.join("m2").exists());
        assert!(remove_model_dir(&base, "m1").is_ok()); // already gone
        for bad in ["", "..", "a/b", "a\\b", "m2.old"] { assert!(remove_model_dir(&base, bad).is_err(), "{bad}"); }
        assert!(base.join("m2").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dictation_modes_parse_and_hold_waits_for_every_key() {
        assert_eq!([dictate_mode("toggle"), dictate_mode("hold"), dictate_mode("auto"), dictate_mode("")], [Dictate::Toggle, Dictate::Hold, Dictate::Auto, Dictate::Auto]);
        assert!(!all_released(&[0x12, 0x42], |k| k == 0x42)); // B still down
        assert!(all_released(&[0x12, 0x42], |_| false));
        assert!(all_released(&[], |_| true));
        // the live app's own stop file in %TEMP% must not decide this, so the test names its own
        let file = std::env::temp_dir().join(format!("zorui-test-stop-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert!(!stop_wanted_with(Dictate::Auto, &[], &file));
        std::fs::write(&file, b"").unwrap(); // the pill's stop button ends a recording in any mode
        assert!(stop_wanted_with(Dictate::Auto, &[], &file) && stop_wanted_with(Dictate::Toggle, &[], &file));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn live_text_waits_for_a_second_of_audio_and_reads_only_the_recent_part() {
        let r = RATE as usize;
        assert!(!partial_due(r - 1, 0) && partial_due(r, 0));
        assert!(!partial_due(2 * r, 2 * r + 100) && !partial_due(2 * r + r / 2, 2 * r) && partial_due(2 * r + r * 7 / 10, 2 * r));
        assert_eq!(partial_start(5 * r), 0);
        assert_eq!(partial_start(30 * r), 10 * r);
    }

    #[test]
    fn a_manual_recording_keeps_silence_and_only_stops_at_the_cap() {
        let mut v = Vad::manual();
        for _ in 0..300 { assert!(!v.push(&vec![0.0; 1600]).1); } // 30 s of silence
        assert_eq!(v.got.len(), 300 * 1600);
        let mut auto = Vad::new();
        assert!((0..100).any(|_| auto.push(&vec![0.0; 1600]).1)); // Auto gives up waiting for speech
        let mut v = Vad::manual();
        assert!((0..1300).any(|_| v.push(&vec![0.0; 1600]).1)); // the 120 s cap
    }

    #[test]
    fn progress_text_and_bar_follow_the_phase() {
        assert_eq!(describe(0, 5, 10), (0.0, String::new()));
        let (f, t) = describe(1, 100 << 20, 400 << 20);
        assert!((f - 0.25).abs() < 1e-6 && t == "Downloading 100 of 400 MB");
        assert_eq!(describe(1, 900, 100).0, 0.99); // a bar never fills before the unpack is done
        assert_eq!(describe(1, 5, 0).0, 0.99); // a zero total does not divide by zero
        assert_eq!(describe(2, 0, 10), (1.0, "Unpacking...".to_string()));
    }

    #[test]
    fn cancel_kills_the_running_command() {
        CANCEL.store(true, Ordering::Relaxed);
        let mut c = hidden("ping.exe");
        c.args(["-n", "30", "127.0.0.1"]);
        let t = std::time::Instant::now();
        assert_eq!(run_cancellable(c, || {}), Err(CANCELLED.to_string()));
        assert!(t.elapsed().as_secs() < 5);
        CANCEL.store(false, Ordering::Relaxed);
    }

    #[test]
    fn windows_is_used_when_asked_for_or_when_no_model_exists() {
        let dir = std::env::temp_dir().join(format!("zorui-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tokens.txt"), "").unwrap();
        let d = dir.to_string_lossy().into_owned();
        assert_eq!(parakeet_dir(&d), d); // a real model folder is used
        assert_eq!(parakeet_dir(WINDOWS), ""); // "windows" wins over any folder
        assert_eq!(parakeet_dir(r"C:\definitely\not\here"), model_dir("")); // a bad folder falls back like automatic
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wav_bytes_read_back_the_same_audio() {
        let audio = [0.0, 0.5, -0.5, 1.0, -1.0, 2.0];
        let p = std::env::temp_dir().join(format!("zorui-wavout-{}.wav", std::process::id()));
        std::fs::write(&p, wav_bytes(&audio)).unwrap();
        let back = read_wav(&p).unwrap().unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(back.len(), audio.len());
        for (a, b) in audio.iter().zip(&back) { assert!((a.clamp(-1.0, 1.0) - b).abs() < 1e-3); } // out-of-range samples clip
        assert_eq!(wav_bytes(&[]).len(), 44);
    }
}
