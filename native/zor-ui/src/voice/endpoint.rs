//! A custom OpenAI-compatible speech endpoint (local servers, or another vendor's OpenAI-style API). The user
//! sets the base URL, the model, the voice names and, when the server needs one, a key (the "endpoint" entry on
//! the Services page). Requests go to <base>/audio/speech in the same shape as OpenAI's.

use super::audio;
use super::contract::*;
use super::http::HttpClient;
use super::openai;
use super::wav;
use std::sync::{Arc, Mutex};

pub const ID: &str = "endpoint";
pub const KEY_NAME: &str = "endpoint";

/// What the user set up. The engine refreshes it from config on every change; the provider reads it per call.
#[derive(Clone, Debug, PartialEq)]
pub struct EndpointCfg {
    pub url: String,
    pub model: String,
    pub voices: Vec<String>,
}

pub type Setup = Arc<Mutex<Option<EndpointCfg>>>;

/// https, or plain http to this machine only (a local server). Anything else could send the key in the clear.
pub fn valid_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.contains(char::is_whitespace) { return Err("the address has spaces in it".into()); }
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"].iter().any(|p| url.starts_with(p));
    if url.starts_with("https://") || local { Ok(()) } else { Err("use an https:// address (plain http is only for localhost)".into()) }
}

pub struct EndpointTts {
    http: Arc<dyn HttpClient>,
    key: Box<dyn Fn() -> Option<String> + Send + Sync>,
    setup: Setup,
}

impl EndpointTts {
    pub fn new(http: Arc<dyn HttpClient>, key: Box<dyn Fn() -> Option<String> + Send + Sync>, setup: Setup) -> EndpointTts {
        EndpointTts { http, key, setup }
    }

    fn current(&self) -> Option<EndpointCfg> { self.setup.lock().ok().and_then(|c| c.clone()) }

    fn configured(&self) -> Result<EndpointCfg, ProviderError> {
        self.current().ok_or_else(|| ProviderError::Other("no endpoint set up yet. Add its address in the Voice settings".into()))
    }
}

impl TtsProvider for EndpointTts {
    fn id(&self) -> &'static str { ID }
    fn name(&self) -> &'static str { "Custom endpoint" }
    fn kind(&self) -> ProviderKind { ProviderKind::Remote }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { voice: VoiceCapabilities { speed: true, ..Default::default() }, ..Default::default() }
    }

    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        Ok(self.current().map(|c| c.voices).unwrap_or_default().into_iter()
            .map(|v| VoiceInfo { name: v.clone(), id: v, language: String::new() }).collect())
    }

    fn health(&self) -> Result<(), ProviderError> {
        let c = self.configured()?;
        valid_url(&c.url).map_err(ProviderError::Other)
    }

    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        let c = self.configured()?;
        valid_url(&c.url).map_err(ProviderError::Other)?;
        if !c.voices.contains(&req.voice_id) {
            return Err(ProviderError::Other(format!("the endpoint has no voice called '{}'", req.voice_id)));
        }
        let model = req.model_id.clone().unwrap_or(c.model.clone());
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if let Some(key) = (self.key)().filter(|k| !k.is_empty()) { headers.push(("Authorization".to_string(), format!("Bearer {key}"))); }
        let url = format!("{}/audio/speech", c.url.trim_end_matches('/'));
        let payload = openai::body(&model, &req.voice_id, &req.text, req.settings.speed);
        let reply = self.http.post(&url, &headers, payload.to_string().as_bytes(), cancel)?;
        if reply.status != 200 { return Err(openai::error_for(reply.status, &reply.body)); }
        let w = wav::parse(&reply.body)?;
        let samples = audio::normalize(&w.samples, w.rate, w.channels)?;
        Ok(SynthesisResult { audio: AudioOutput::Pcm { samples, rate: audio::INTERNAL_RATE, channels: 1 } })
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::HttpResponse;
    use super::*;
    use std::sync::Mutex as M;

    struct Fake { reply: M<Option<HttpResponse>>, sent: M<Vec<(String, Vec<(String, String)>)>> }

    impl HttpClient for Fake {
        fn post(&self, url: &str, headers: &[(String, String)], _: &[u8], _: &Cancel) -> Result<HttpResponse, ProviderError> {
            self.sent.lock().unwrap().push((url.into(), headers.to_vec()));
            self.reply.lock().unwrap().take().ok_or(ProviderError::Network("no reply".into()))
        }
    }

    fn fake(status: u16, body: Vec<u8>) -> Arc<Fake> {
        Arc::new(Fake { reply: M::new(Some(HttpResponse { status, body })), sent: M::new(vec![]) })
    }

    fn setup(url: &str, voices: &[&str]) -> Setup {
        Arc::new(Mutex::new(Some(EndpointCfg { url: url.into(), model: "tts-1".into(), voices: voices.iter().map(|v| v.to_string()).collect() })))
    }

    fn provider(http: Arc<dyn HttpClient>, key: Option<&str>, s: Setup) -> EndpointTts {
        let key = key.map(String::from);
        EndpointTts::new(http, Box::new(move || key.clone()), s)
    }

    fn speak(voice: &str) -> SynthesisRequest {
        SynthesisRequest { voice_id: voice.into(), model_id: None, text: "hi".into(), settings: SpeechSettings::default() }
    }

    #[test]
    fn only_https_or_localhost_addresses_are_accepted() {
        assert!(valid_url("https://tts.example.com/v1").is_ok());
        assert!(valid_url("http://localhost:8880/v1").is_ok());
        assert!(valid_url("http://127.0.0.1:5000").is_ok());
        assert!(valid_url("http://tts.example.com").is_err(), "no plain http off this machine");
        assert!(valid_url("https://x y").is_err());
    }

    #[test]
    fn a_speech_request_goes_to_the_base_plus_audio_speech_without_a_key_header() {
        let f = fake(200, wav::encode(24_000, 1, &[0i16; 240]));
        let p = provider(f.clone(), None, setup("http://localhost:8880/v1/", &["af_sky"]));
        let r = p.synthesize(&speak("af_sky"), &Cancel::new()).unwrap();
        assert!(matches!(r.audio, AudioOutput::Pcm { .. }));
        let sent = f.sent.lock().unwrap();
        assert_eq!(sent[0].0, "http://localhost:8880/v1/audio/speech", "no double slash");
        assert!(!sent[0].1.iter().any(|(k, _)| k == "Authorization"), "no key saved: none sent");
    }

    #[test]
    fn a_saved_key_is_sent_as_a_bearer_token_and_unknown_voices_are_refused() {
        let f = fake(200, wav::encode(24_000, 1, &[0i16; 8]));
        let p = provider(f.clone(), Some("sk-1"), setup("https://tts.example.com", &["alloy"]));
        assert!(p.synthesize(&speak("nope"), &Cancel::new()).is_err());
        assert!(f.sent.lock().unwrap().is_empty(), "an unknown voice sends nothing");
        p.synthesize(&speak("alloy"), &Cancel::new()).unwrap();
        assert!(f.sent.lock().unwrap()[0].1.contains(&("Authorization".to_string(), "Bearer sk-1".to_string())));
    }

    #[test]
    fn health_needs_a_setup_and_a_safe_address() {
        let f = fake(200, vec![]);
        let remote_plain = provider(f.clone(), None, setup("http://example.com", &["a"]));
        assert!(remote_plain.health().is_err(), "plain http to a remote host is refused");
        let nothing = provider(f, None, Arc::new(Mutex::new(None)));
        assert!(nothing.health().is_err(), "nothing set up");
    }
}
