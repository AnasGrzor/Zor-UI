//! A provider that needs no model or network, for tests of the registry, queue and Buddy path.

use super::contract::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct MockTts {
    id: &'static str,
    fail_health: bool,
    fail_preview: bool,
    cloning: bool,
    delay: Duration,
    /// Every text that reached `synthesize`, in order. Shared so a test can read it after the mock moves.
    pub spoken: Arc<Mutex<Vec<String>>>,
}

impl MockTts {
    pub fn new(id: &'static str) -> MockTts {
        MockTts { id, fail_health: false, fail_preview: false, cloning: false, delay: Duration::ZERO, spoken: Arc::new(Mutex::new(vec![])) }
    }
    pub fn failing_health(mut self) -> Self { self.fail_health = true; self }
    pub fn failing_preview(mut self) -> Self { self.fail_preview = true; self }
    pub fn with_cloning(mut self) -> Self { self.cloning = true; self }
    pub fn with_delay(mut self, d: Duration) -> Self { self.delay = d; self }
}

impl TtsProvider for MockTts {
    fn id(&self) -> &'static str { self.id }
    fn name(&self) -> &'static str { "Mock" }
    fn kind(&self) -> ProviderKind { ProviderKind::Native }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { cloning: self.cloning, ..Default::default() }
    }
    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        Ok(vec![VoiceInfo { id: "v1".into(), name: "Voice one".into(), language: "en".into() }])
    }
    fn health(&self) -> Result<(), ProviderError> {
        if self.fail_health { Err(ProviderError::ModelLoad("mock model missing".into())) } else { Ok(()) }
    }
    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        if req.text == PREVIEW_TEXT && self.fail_preview {
            return Err(ProviderError::Network("mock preview failed".into()));
        }
        self.spoken.lock().unwrap().push(req.text.clone());
        let mut waited = Duration::ZERO;
        while waited < self.delay {
            if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
            std::thread::sleep(Duration::from_millis(5));
            waited += Duration::from_millis(5);
        }
        if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
        Ok(SynthesisResult { audio: AudioOutput::Played })
    }
    fn clone_voice(&self, req: &CloneVoiceRequest) -> Result<VoiceInfo, ProviderError> {
        if !self.cloning { return Err(ProviderError::Unsupported("voice cloning")); }
        Ok(VoiceInfo { id: format!("clone-{}", req.name), name: req.name.clone(), language: "en".into() })
    }
}
