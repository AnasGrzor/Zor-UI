//! Reference recordings for voice cloning. A recording is checked before it is kept, then copied to
//! voice-references/<id>/reference.wav, outside the executable, so the voice can be recreated on another
//! provider later (VoicePlan sections 5 and 6). Only the user's own recordings, or ones they have permission to use.

use super::contract::VoiceReference;
use super::wav;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MIN_SECS: f32 = 3.0;
pub const MAX_SECS: f32 = 120.0;
pub const MAX_BYTES: u64 = 60 * 1024 * 1024;
pub const MAX_TRANSCRIPT: usize = 1000;

/// What the check measured, for the Studio's note.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub duration_secs: f32,
    pub rate: u32,
    pub channels: u16,
}

/// Accepts a 16-bit PCM WAV of 3 to 120 seconds at 16 kHz or more, not clipping, not silent.
pub fn check(bytes: &[u8]) -> Result<Check, String> {
    let w = wav::parse(bytes).map_err(|e| e.to_string())?;
    if w.rate < 16_000 { return Err(format!("the sample rate is {} Hz; use 16000 Hz or more", w.rate)); }
    let frames = w.samples.len() / w.channels as usize;
    let secs = frames as f32 / w.rate as f32;
    if secs < MIN_SECS { return Err(format!("{secs:.1} s is too short; record at least {MIN_SECS:.0} s")); }
    if secs > MAX_SECS { return Err(format!("{secs:.0} s is too long; keep it under {MAX_SECS:.0} s")); }
    let clipped = w.samples.iter().filter(|s| s.abs() >= 0.999).count() as f32 / w.samples.len() as f32;
    if clipped > 0.001 { return Err("the recording clips (too loud); record it quieter".into()); }
    let rms = (w.samples.iter().map(|s| s * s).sum::<f32>() / w.samples.len() as f32).sqrt();
    if rms < 0.003 { return Err("the recording is almost silent".into()); }
    Ok(Check { duration_secs: secs, rate: w.rate, channels: w.channels })
}

/// Reads and checks the file at `path`, refusing anything over MAX_BYTES before reading it.
pub fn inspect(path: &Path) -> Result<(Vec<u8>, Check), String> {
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len > MAX_BYTES { return Err("the file is over 60 MB".into()); }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let c = check(&bytes)?;
    Ok((bytes, c))
}

/// Where kept recordings live.
pub fn root() -> PathBuf { crate::conf::data_dir().join("voice-references") }

/// Keeps a checked recording under `root/<id>/reference.wav` with its transcript, and returns its reference.
pub fn keep(root: &Path, id: &str, bytes: &[u8], check: &Check, transcript: &str) -> Result<VoiceReference, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') { return Err("bad recording id".into()); }
    let t = transcript.trim();
    if t.is_empty() { return Err("type what is said in the recording".into()); }
    if t.chars().count() > MAX_TRANSCRIPT { return Err(format!("the transcript can be at most {MAX_TRANSCRIPT} characters")); }
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("reference.wav");
    let part = dir.join("reference.part");
    std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(VoiceReference { path, transcript: t.into(), duration_secs: check.duration_secs })
}

/// The Studio's current recording: the file, what the check found, and the note the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub path: Option<PathBuf>,
    pub check: Option<Check>,
    pub note: String,
}

static DRAFT: Mutex<Option<Draft>> = Mutex::new(None);

pub fn draft() -> Draft { DRAFT.lock().ok().and_then(|d| d.clone()).unwrap_or_default() }

pub fn set_draft(d: Draft) { if let Ok(mut g) = DRAFT.lock() { *g = Some(d); } }

#[cfg(test)]
mod tests {
    use super::*;

    /// `secs` of a sine at `amp`, mono, at `rate`.
    fn tone(secs: f32, rate: u32, amp: f32) -> Vec<u8> {
        let n = (secs * rate as f32) as usize;
        let pcm: Vec<i16> = (0..n).map(|i| ((i as f32 * 0.05).sin() * amp * 32767.0) as i16).collect();
        wav::encode(rate, 1, &pcm)
    }

    #[test]
    fn a_clear_recording_of_normal_length_passes() {
        let c = check(&tone(5.0, 22_050, 0.3)).unwrap();
        assert!((c.duration_secs - 5.0).abs() < 0.01);
        assert_eq!((c.rate, c.channels), (22_050, 1));
    }

    #[test]
    fn too_short_too_long_and_low_rate_are_refused_with_the_reason() {
        assert!(check(&tone(1.0, 22_050, 0.3)).unwrap_err().contains("too short"));
        assert!(check(&tone(MAX_SECS + 5.0, 8_000, 0.3)).is_err_and(|e| e.contains("sample rate") || e.contains("too long")));
        assert!(check(&tone(5.0, 8_000, 0.3)).unwrap_err().contains("sample rate"));
        assert!(check(&tone(MAX_SECS + 5.0, 16_000, 0.3)).unwrap_err().contains("too long"));
    }

    #[test]
    fn silence_and_clipping_are_refused() {
        assert!(check(&tone(4.0, 22_050, 0.0)).unwrap_err().contains("silent"));
        assert!(check(&tone(4.0, 22_050, 1.0)).unwrap_err().contains("clips"));
    }

    #[test]
    fn a_file_that_is_not_wav_is_refused() {
        assert!(check(b"mp3 bytes, not a wav").is_err());
    }

    #[test]
    fn keeping_writes_the_file_outside_the_app_with_its_transcript() {
        let root = std::env::temp_dir().join(format!("zorui-refs-{}", super::super::profile::new_id()));
        let bytes = tone(4.0, 22_050, 0.3);
        let c = check(&bytes).unwrap();
        let r = keep(&root, "abc-123", &bytes, &c, "  Hello there.  ").unwrap();
        assert_eq!(std::fs::read(&r.path).unwrap(), bytes, "the original recording is kept byte for byte");
        assert_eq!(r.transcript, "Hello there.", "transcript trimmed");
        assert!(!root.join("abc-123").join("reference.part").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keeping_refuses_empty_transcripts_and_unsafe_ids() {
        let root = std::env::temp_dir().join("zorui-refs-refuse");
        let bytes = tone(4.0, 22_050, 0.3);
        let c = check(&bytes).unwrap();
        assert!(keep(&root, "ok-1", &bytes, &c, "   ").is_err());
        assert!(keep(&root, "../escape", &bytes, &c, "hi").is_err(), "no path separators in the id");
        assert!(!root.exists() || std::fs::read_dir(&root).unwrap().count() == 0);
    }
}
