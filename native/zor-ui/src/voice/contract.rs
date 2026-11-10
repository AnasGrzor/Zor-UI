//! What every voice provider speaks: the trait, its requests and results, and the error kinds.
//! Optional features are default methods that answer `Unsupported`, so a provider only writes what it has.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Where the provider runs. Native: in-process Rust or a model runtime. Sidecar: a local executable or
/// Python service the app starts. Remote: HTTP or WebSocket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind { Native, Sidecar, Remote }

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderError {
    /// The provider does not offer this feature (the name of the capability).
    Unsupported(&'static str),
    Auth(String),
    RateLimited { retry_after_secs: Option<u64> },
    Network(String),
    InvalidAudio(String),
    ModelLoad(String),
    /// The caller cancelled the synthesis. Not a failure to report to the user.
    Cancelled,
    Other(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Unsupported(what) => write!(f, "this voice provider does not support {what}"),
            ProviderError::Auth(m) => write!(f, "the provider rejected the credentials: {m}"),
            ProviderError::RateLimited { retry_after_secs: Some(s) } => write!(f, "rate limited, retry in {s} s"),
            ProviderError::RateLimited { retry_after_secs: None } => write!(f, "rate limited"),
            ProviderError::Network(m) => write!(f, "network error: {m}"),
            ProviderError::InvalidAudio(m) => write!(f, "invalid audio: {m}"),
            ProviderError::ModelLoad(m) => write!(f, "the model could not load: {m}"),
            ProviderError::Cancelled => write!(f, "cancelled"),
            ProviderError::Other(m) => write!(f, "{m}"),
        }
    }
}

/// Cancel flag shared by the queue and one running synthesis. Cheap to clone; all clones see the same flag.
#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel { Cancel::default() }
    pub fn cancel(&self) { self.0.store(true, Ordering::Relaxed); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Relaxed) }
}

/// Voice-level features. A provider reports which ones its voices honour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceCapabilities {
    pub speed: bool,
    pub pitch: bool,
    pub emotion: bool,
}

/// Provider-level features. `voice` is what each of its voices can do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub cloning: bool,
    pub design: bool,
    pub streaming: bool,
    pub voice: VoiceCapabilities,
}

impl ProviderCapabilities {
    /// The first feature `need` asks for that this provider lacks, or None when it has them all.
    pub fn missing(&self, need: &ProviderCapabilities) -> Option<&'static str> {
        if need.cloning && !self.cloning { return Some("voice cloning"); }
        if need.design && !self.design { return Some("voice design"); }
        if need.streaming && !self.streaming { return Some("streaming"); }
        if need.voice.speed && !self.voice.speed { return Some("speed control"); }
        if need.voice.pitch && !self.voice.pitch { return Some("pitch control"); }
        if need.voice.emotion && !self.voice.emotion { return Some("emotion control"); }
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
}

/// Speech controls. Defaults are the normal voice; a provider without a control ignores it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechSettings {
    /// 1.0 is normal speed.
    pub speed: f32,
    /// Semitones; 0.0 is the voice's own pitch.
    pub pitch: f32,
    /// 0 to 1; how much the voice expresses.
    pub expressiveness: f32,
    pub emotion: Option<String>,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        SpeechSettings { speed: 1.0, pitch: 0.0, expressiveness: 0.5, emotion: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SynthesisRequest {
    /// The provider's own voice id (from `VoiceInfo::id`).
    pub voice_id: String,
    pub model_id: Option<String>,
    pub text: String,
    pub settings: SpeechSettings,
}

/// Recorded reference audio the user authorised for cloning. Kept outside the executable.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceReference {
    pub path: PathBuf,
    pub transcript: String,
    pub duration_secs: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloneVoiceRequest {
    pub name: String,
    pub reference: VoiceReference,
}

/// Audio in the app's internal format (see `audio::normalize`), or `Played` when the provider played it
/// itself (the Windows voice does).
#[derive(Clone, Debug, PartialEq)]
pub enum AudioOutput {
    Played,
    Pcm { samples: Vec<f32>, rate: u32, channels: u16 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SynthesisResult {
    pub audio: AudioOutput,
}

/// A text-to-speech backend. Buddy never calls this; it goes through `voice::speak`.
/// Synthesis runs on the speech worker thread, never the UI thread. Check `cancel` while working and return
/// `ProviderError::Cancelled` once it is set.
pub trait TtsProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn kind(&self) -> ProviderKind;
    fn version(&self) -> &'static str { "1" }
    fn capabilities(&self) -> ProviderCapabilities;
    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError>;
    /// Cheap check that the provider can work right now (installed, reachable, credentials present).
    fn health(&self) -> Result<(), ProviderError>;
    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError>;
    /// A short sample in one voice, used to test a provider before it becomes active.
    fn preview(&self, voice_id: &str, settings: &SpeechSettings, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        self.synthesize(&SynthesisRequest { voice_id: voice_id.into(), model_id: None, text: PREVIEW_TEXT.into(), settings: settings.clone() }, cancel)
    }
    fn clone_voice(&self, _req: &CloneVoiceRequest) -> Result<VoiceInfo, ProviderError> {
        Err(ProviderError::Unsupported("voice cloning"))
    }
    fn design_voice(&self, _description: &str) -> Result<VoiceInfo, ProviderError> {
        Err(ProviderError::Unsupported("voice design"))
    }
    fn shutdown(&self) {}
}

pub type SharedProvider = Arc<dyn TtsProvider>;

pub const PREVIEW_TEXT: &str = "This is how I sound.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_names_the_first_feature_the_provider_lacks() {
        let have = ProviderCapabilities { cloning: false, ..Default::default() };
        let need = ProviderCapabilities { cloning: true, voice: VoiceCapabilities { speed: true, ..Default::default() }, ..Default::default() };
        assert_eq!(have.missing(&need), Some("voice cloning"));
        assert_eq!(have.missing(&ProviderCapabilities::default()), None, "asking for nothing is always met");
    }

    #[test]
    fn cancel_is_shared_between_clones() {
        let c = Cancel::new();
        let worker = c.clone();
        assert!(!worker.is_cancelled());
        c.cancel();
        assert!(worker.is_cancelled());
    }

    #[test]
    fn default_optional_methods_report_unsupported() {
        struct Plain;
        impl TtsProvider for Plain {
            fn id(&self) -> &'static str { "plain" }
            fn name(&self) -> &'static str { "Plain" }
            fn kind(&self) -> ProviderKind { ProviderKind::Native }
            fn capabilities(&self) -> ProviderCapabilities { ProviderCapabilities::default() }
            fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> { Ok(vec![]) }
            fn health(&self) -> Result<(), ProviderError> { Ok(()) }
            fn synthesize(&self, _: &SynthesisRequest, _: &Cancel) -> Result<SynthesisResult, ProviderError> { Ok(SynthesisResult { audio: AudioOutput::Played }) }
        }
        let p = Plain;
        let req = CloneVoiceRequest { name: "x".into(), reference: VoiceReference { path: PathBuf::from("a.wav"), transcript: String::new(), duration_secs: 1.0 } };
        assert_eq!(p.clone_voice(&req), Err(ProviderError::Unsupported("voice cloning")));
        assert_eq!(p.design_voice("warm"), Err(ProviderError::Unsupported("voice design")));
        assert!(p.preview("v", &SpeechSettings::default(), &Cancel::new()).is_ok(), "preview falls back to synthesize");
    }
}
