//! Records the microphone for a voice reference. It starts the app's own record-audio bridge, which writes raw
//! 48 kHz stereo f32 until it is killed. On stop, that file becomes a 16-bit WAV that reference::check accepts.

use super::wav;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;

pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

struct Recording { child: Child, pcm: PathBuf }

static REC: Mutex<Option<Recording>> = Mutex::new(None);

pub fn recording() -> bool { REC.lock().map(|r| r.is_some()).unwrap_or(false) }

/// Starts recording from the microphone. Fails if a recording is already running.
pub fn start() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut slot = REC.lock().map_err(|_| "recording is unavailable")?;
    if slot.is_some() { return Err("already recording".into()); }
    let exe = bridge_exe()?;
    let pcm = std::env::temp_dir().join(format!("zorui-rec-{}.pcm", super::profile::new_id()));
    let child = Command::new(exe)
        .args(["--bridge", "record-audio", "--mode", "mic", "--out"])
        .arg(&pcm)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("could not start recording: {e}"))?;
    *slot = Some(Recording { child, pcm });
    Ok(())
}

/// The program that runs the record-audio bridge: ZORUI_EXE when set (tests point it at the release build),
/// else this program.
fn bridge_exe() -> Result<PathBuf, String> {
    match std::env::var_os("ZORUI_EXE") {
        Some(p) => Ok(PathBuf::from(p)),
        None => std::env::current_exe().map_err(|e| e.to_string()),
    }
}

/// Stops the recording and returns the WAV file it made, in the temp folder.
pub fn stop() -> Result<PathBuf, String> {
    let rec = REC.lock().map_err(|_| "recording is unavailable")?.take().ok_or("not recording")?;
    let mut child = rec.child;
    child.kill().ok();
    child.wait().ok();
    let raw = std::fs::read(&rec.pcm).map_err(|e| format!("no audio was captured: {e}"));
    std::fs::remove_file(&rec.pcm).ok();
    let wav_bytes = pcm_to_wav(&raw?)?;
    let out = std::env::temp_dir().join(format!("zorui-rec-{}.wav", super::profile::new_id()));
    std::fs::write(&out, wav_bytes).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Raw stereo f32le at RATE, as the recorder writes it, to a 16-bit WAV. A whole frame is kept or dropped.
pub fn pcm_to_wav(raw: &[u8]) -> Result<Vec<u8>, String> {
    let frame = 4 * CHANNELS as usize;
    let whole = raw.len() - raw.len() % frame;
    if whole == 0 { return Err("no audio was captured. Check the microphone in Settings".into()); }
    let pcm: Vec<i16> = raw[..whole].chunks_exact(4)
        .map(|b| (f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();
    Ok(wav::encode(RATE, CHANNELS, &pcm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_stereo_float_becomes_a_16_bit_wav_that_reads_back() {
        let raw: Vec<u8> = [0.5f32, -0.5, 2.0, -2.0].iter().flat_map(|s| s.to_le_bytes()).collect();
        let w = wav::parse(&pcm_to_wav(&raw).unwrap()).unwrap();
        assert_eq!((w.rate, w.channels), (RATE, CHANNELS));
        assert_eq!(w.samples.len(), 4);
        assert!((w.samples[0] - 0.5).abs() < 1e-3);
        assert!((w.samples[2] - 1.0).abs() < 1e-3, "over-range samples are clamped, not wrapped");
    }

    #[test]
    fn a_partial_frame_is_dropped_and_an_empty_capture_is_refused() {
        let mut raw: Vec<u8> = [0.1f32, 0.1].iter().flat_map(|s| s.to_le_bytes()).collect();
        raw.extend_from_slice(&[1, 2, 3]);
        assert_eq!(wav::parse(&pcm_to_wav(&raw).unwrap()).unwrap().samples.len(), 2);
        assert!(pcm_to_wav(&[1, 2, 3]).is_err());
    }

    /// Records for two seconds through the release build. Set ZORUI_EXE to zorui.exe first.
    #[test]
    #[ignore = "records 2 s from the microphone; set ZORUI_EXE to the release zorui.exe"]
    fn records_a_wav_through_the_bridge() {
        assert!(std::env::var_os("ZORUI_EXE").is_some(), "set ZORUI_EXE to the release zorui.exe");
        start().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let path = stop().unwrap();
        let w = wav::parse(&std::fs::read(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!((w.rate, w.channels), (RATE, CHANNELS));
        assert!(w.samples.len() > RATE as usize / 2, "about 2 s captured, got {} samples", w.samples.len());
    }

    #[test]
    fn stopping_when_not_recording_is_an_error() {
        assert!(stop().is_err());
        assert!(!recording());
    }
}
