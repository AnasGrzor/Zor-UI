//! Rust port of scripts/buddy_listen.py (the `buddy-listen` stream): records the microphone through
//! WASAPI until you stop talking, then transcribes it on this PC with the Parakeet model that Orca
//! downloads. The recognizer is sherpa-onnx's C API, loaded at run time from %LOCALAPPDATA%\zor-ui\sherpa
//! (downloaded once from the sherpa-onnx release). Prints the same JSON lines as the script:
//! {"state": "starting" | "listening" | "thinking"}, {"level": rms}, then {"text"} or {"error"}.

use super::{common::*, Sink};
use serde_json::json;
use std::{
    ffi::{c_char, c_void, CStr, CString},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};
use windows::{
    core::{PCWSTR, HSTRING},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::*,
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
const DLLS: [&str; 3] = ["sherpa-onnx-c-api.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll"];

pub fn stream(_cfg: &crate::conf::Config, _name: &str, args: &[String], _stdin: &str, out: &Sink) {
    listen(args, out);
}

fn arg<'a>(args: &'a [String], flag: &str) -> &'a str {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("")
}

/// The model folder: `cfg_path` (buddies.model) or Orca's own copy; "" when neither has tokens.txt.
fn model_dir(cfg_path: &str) -> String {
    let orca = std::env::var("APPDATA").map(|a| format!("{a}\\orca\\speech-models\\{MODEL}")).unwrap_or_default();
    [cfg_path.to_string(), orca].into_iter().find(|d| !d.is_empty() && Path::new(d).join("tokens.txt").is_file()).unwrap_or_default()
}

fn listen(args: &[String], out: &Sink) {
    let wav = arg(args, "--wav");
    let dir = model_dir(arg(args, "--model"));
    if dir.is_empty() {
        out.emit(&json!({ "error": "Parakeet model not found. Open Orca once and enable voice dictation, or set buddies.model." }));
        return;
    }
    out.emit(&json!({ "state": "starting" }));
    // the model takes seconds to load: do it while the microphone is already recording
    let loader = std::thread::spawn(move || Recognizer::new(&dir));
    let audio = if wav.is_empty() {
        record(arg(args, "--mic"), out)
    } else {
        out.emit(&json!({ "state": "listening" }));
        read_wav(Path::new(wav))
    };
    let audio = match audio {
        Ok(a) => a,
        Err(e) => { out.emit(&json!({ "error": e })); return; }
    };
    if out.stopped() { return; }
    let Some(audio) = audio else { out.emit(&json!({ "text": "" })); return };
    out.emit(&json!({ "state": "thinking" }));
    let text = loader.join().map_err(|_| "the speech model thread crashed".to_string()).and_then(|r| r).and_then(|r| r.transcribe(&audio));
    out.emit(&match text { Ok(t) => json!({ "text": t }), Err(e) => json!({ "error": e }) });
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
unsafe fn pick_mic(en: &IMMDeviceEnumerator, name: &str) -> windows::core::Result<IMMDevice> {
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
fn record(mic: &str, out: &Sink) -> Result<Option<Vec<f32>>, String> {
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
        let (mut vad, mut pending) = (Vad::new(), Vec::<f32>::new());
        let (chunk, started) = ((RATE / 10) as usize, std::time::Instant::now());
        let mut stalled = false;
        let result = (|| -> windows::core::Result<()> {
            while !out.stopped() {
                std::thread::sleep(Duration::from_millis(10));
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
                    let (rms, done) = vad.push(&a);
                    out.emit(&json!({ "level": (rms * 1e4).round() / 1e4 })); // the pill's waveform
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
}
