//! Windows' built-in voice (System.Speech, run through PowerShell). This is the voice Buddy always used, now
//! behind the provider contract, and the fallback every install keeps.

use super::contract::*;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

pub const ID: &str = "windows";
/// The one voice this adapter offers: the male voice the Buddies always used.
pub const MALE: &str = "male";

const SCRIPT: &str = "Add-Type -AssemblyName System.Speech; $v = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                      $v.SelectVoiceByHints('Male'); $v.Speak($env:ZOR_SAY)";

pub struct WindowsTts;

impl TtsProvider for WindowsTts {
    fn id(&self) -> &'static str { ID }
    fn name(&self) -> &'static str { "Windows voice" }
    fn kind(&self) -> ProviderKind { ProviderKind::Native }
    fn capabilities(&self) -> ProviderCapabilities { ProviderCapabilities::default() }

    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        Ok(vec![VoiceInfo { id: MALE.into(), name: "Male voice".into(), language: String::new() }])
    }

    fn health(&self) -> Result<(), ProviderError> {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let ps = std::path::Path::new(&root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        if ps.exists() { Ok(()) } else { Err(ProviderError::Other("PowerShell is not installed".into())) }
    }

    /// The text goes in through an environment variable, not the command line, so a track title can't inject script.
    /// shortcut: PowerShell start-up adds ~0.5 s before each line; move to SAPI through the windows crate if it matters.
    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        if req.voice_id != MALE {
            return Err(ProviderError::Other(format!("no Windows voice called '{}'", req.voice_id)));
        }
        if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .env("ZOR_SAY", &req.text)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ProviderError::Other(format!("could not start Windows speech: {e}")))?;
        super::process::wait_cancellable(&mut child, cancel, "Windows speech")?;
        Ok(SynthesisResult { audio: AudioOutput::Played })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_the_male_voice_and_no_optional_features() {
        let p = WindowsTts;
        assert_eq!(p.voices().unwrap()[0].id, MALE);
        assert_eq!(p.capabilities(), ProviderCapabilities::default());
        assert_eq!(p.clone_voice(&CloneVoiceRequest {
            name: "x".into(),
            reference: VoiceReference { path: "a.wav".into(), transcript: String::new(), duration_secs: 1.0 },
        }), Err(ProviderError::Unsupported("voice cloning")));
    }

    #[test]
    fn an_unknown_voice_is_refused_before_anything_starts() {
        let r = WindowsTts.synthesize(&SynthesisRequest { voice_id: "ghost".into(), model_id: None, text: "hi".into(), settings: SpeechSettings::default() }, &Cancel::new());
        assert!(matches!(r, Err(ProviderError::Other(_))));
    }

    #[test]
    fn a_cancelled_line_is_not_spoken() {
        let cancel = Cancel::new();
        cancel.cancel();
        let r = WindowsTts.synthesize(&SynthesisRequest { voice_id: MALE.into(), model_id: None, text: "hi".into(), settings: SpeechSettings::default() }, &cancel);
        assert_eq!(r, Err(ProviderError::Cancelled));
    }
}
