//! The audio half of screen recording: `zorui.exe --bridge record-audio --mode mic|system|both [--mic name] --out file.pcm`
//! captures the microphone and/or what the speakers play (WASAPI loopback), mixes them and writes raw
//! 48 kHz stereo f32 samples to the file until the process is killed. `media::record` starts it next to
//! ffmpeg and muxes the file into the video when the recording stops (ffmpeg has no loopback input).
//! The writer paces itself on the clock, so a silent source (loopback delivers nothing while nothing
//! plays) still fills its share of the timeline and the audio stays as long as the video.

use super::{listen::pick_mic, Sink};
use serde_json::json;
use std::{
    collections::VecDeque,
    io::Write,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};
use windows::{
    core::Result as WinResult,
    Win32::{
        Media::Audio::*,
        System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
    },
};

pub const STREAMS: &[&str] = &["record-audio"];
/// Sample rate of the .pcm file (stereo f32le); the mux step reads it with these numbers.
pub const RATE: u32 = 48000;

type Queue = Arc<Mutex<VecDeque<f32>>>;

/// The `record.audio` choices: (value, label, icon). Settings and the sidebar's right-click menu list these.
pub const MODES: [(&str, &str, &str); 4] = [
    ("none", "No sound", "volume_off"),
    ("mic", "Microphone", "mic"),
    ("system", "System sound", "speaker"),
    ("both", "Microphone and system sound", "graphic_eq"),
];

/// The Screen record tile's subtitle while idle: what the next recording captures.
pub fn short(mode: &str) -> &'static str {
    match mode { "mic" => "Mic", "system" => "System sound", "both" => "Mic + system", _ => "No sound" }
}

/// (microphone, system sound) for a `record.audio` value; anything else records no audio.
pub fn wanted(mode: &str) -> (bool, bool) {
    match mode { "mic" => (true, false), "system" => (false, true), "both" => (true, true), _ => (false, false) }
}

pub fn stream(_cfg: &crate::conf::Config, _name: &str, args: &[String], _stdin: &str, out: &Sink) {
    let arg = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("");
    let (mic, system) = wanted(arg("--mode"));
    let file = match std::fs::File::create(arg("--out")) {
        Ok(f) => f,
        Err(e) => { out.emit(&json!({ "error": format!("{}: {e}", arg("--out")) })); return; }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let mut queues = vec![];
    for (on, loopback) in [(mic, false), (system, true)] {
        if !on { continue; }
        let (q, stop, name) = (Queue::default(), stop.clone(), arg("--mic").to_string());
        queues.push(q.clone());
        // a source that cannot open stays silent; the other one and the file length are unaffected
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            if let Err(e) = capture(loopback, &name, &q, &stop) { eprintln!("capture loopback={loopback}: {e}"); }
        });
    }
    let (mut f, t0, mut written) = (std::io::BufWriter::new(file), Instant::now(), 0u64);
    while !out.stopped() {
        std::thread::sleep(Duration::from_millis(20));
        let want = (t0.elapsed().as_secs_f64() * RATE as f64) as u64;
        if want <= written { continue; }
        let bytes: Vec<u8> = mix(&queues, (want - written) as usize * 2).iter().flat_map(|s| s.to_le_bytes()).collect();
        if f.write_all(&bytes).and_then(|_| f.flush()).is_err() { break; }
        written = want;
    }
    stop.store(true, Ordering::Relaxed);
}

/// `n` samples: every queue's oldest samples added together (a short queue counts as silence), clamped.
fn mix(queues: &[Queue], n: usize) -> Vec<f32> {
    let mut out = vec![0f32; n];
    for q in queues {
        let Ok(mut q) = q.lock() else { continue };
        let k = n.min(q.len());
        for (o, s) in out.iter_mut().zip(q.drain(..k)) { *o += s; }
    }
    out.iter_mut().for_each(|s| *s = s.clamp(-1.0, 1.0));
    out
}

/// Feeds `q` with stereo 48 kHz samples from the mic named `mic` (default mic when empty or unmatched)
/// or from the default speakers until `stop`. shortcut: follows no device change, a recording that
/// outlives a headset swap goes silent on that source; start a new recording.
unsafe fn capture(loopback: bool, mic: &str, q: &Queue, stop: &AtomicBool) -> WinResult<()> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = if loopback { en.GetDefaultAudioEndpoint(eRender, eConsole)? } else { pick_mic(&en, mic)? };
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    // shared mode converts to this format, so the device's own rate and channels don't matter
    let fmt = WAVEFORMATEX { wFormatTag: 3 /* WAVE_FORMAT_IEEE_FLOAT */, nChannels: 2, nSamplesPerSec: RATE, nAvgBytesPerSec: RATE * 8, nBlockAlign: 8, wBitsPerSample: 32, cbSize: 0 };
    let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback { flags |= AUDCLNT_STREAMFLAGS_LOOPBACK; }
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, &fmt, None)?;
    let cap: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    let result = (|| -> WinResult<()> {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            while cap.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize * 2;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                if let Ok(mut q) = q.lock() {
                    if silent { q.extend(std::iter::repeat(0.0).take(n)); } else { q.extend(std::slice::from_raw_parts(data as *const f32, n)); }
                    let over = q.len().saturating_sub(RATE as usize * 4);   // the writer fell 2 s behind: drop the oldest
                    q.drain(..over);
                }
                cap.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    })();
    let _ = client.Stop();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(v: &[f32]) -> Queue { Arc::new(Mutex::new(v.iter().copied().collect())) }

    #[test]
    fn every_listed_mode_is_understood() {
        for (key, _, _) in MODES { assert_eq!(wanted(key) != (false, false), key != "none", "{key}"); }
    }

    #[test]
    fn tile_subtitle() {
        assert_eq!(["none", "mic", "system", "both", ""].map(short), ["No sound", "Mic", "System sound", "Mic + system", "No sound"]);
    }

    #[test]
    fn modes() {
        assert_eq!([wanted("mic"), wanted("system"), wanted("both"), wanted("none"), wanted("")], [(true, false), (false, true), (true, true), (false, false), (false, false)]);
    }

    #[test]
    fn mix_pads_adds_and_clamps() {
        let (a, b) = (q(&[0.25, 0.5, 0.75]), q(&[0.5, 0.75]));
        assert_eq!(mix(&[a.clone(), b], 4), [0.75, 1.0, 0.75, 0.0]);   // 0.5+0.75 clamps at 1.0, the short queue counts as silence
        assert_eq!(a.lock().unwrap().len(), 0);
        let c = q(&[0.1, 0.2, 0.3]);
        assert_eq!(mix(&[c.clone()], 2), [0.1, 0.2]);
        assert_eq!(c.lock().unwrap().len(), 1);   // unread samples stay queued
        assert_eq!(mix(&[], 3), [0.0, 0.0, 0.0]);
    }
}
