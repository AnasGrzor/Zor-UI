//! OpenAI's text-to-speech API (POST /v1/audio/speech). Remote: each line is a request over HTTPS, billed
//! per character. The key is read from the credential store on every call, so saving a new one takes effect
//! without a restart.

use super::audio;
use super::contract::*;
use super::http::HttpClient;
use super::wav;
use serde_json::{json, Value};
use std::sync::Arc;

pub const ID: &str = "openai";
/// Name of the saved key in the credential store (see secrets.rs).
pub const KEY_NAME: &str = "openai";
pub const URL: &str = "https://api.openai.com/v1/audio/speech";
pub const DEFAULT_MODEL: &str = "tts-1";
/// The voices the API offers. Fixed by OpenAI, so no request is needed to list them.
pub const VOICES: [&str; 6] = ["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

pub type KeySource = Box<dyn Fn() -> Option<String> + Send + Sync>;

pub struct OpenAiTts {
    http: Arc<dyn HttpClient>,
    key: KeySource,
}

impl OpenAiTts {
    pub fn new(http: Arc<dyn HttpClient>, key: KeySource) -> OpenAiTts { OpenAiTts { http, key } }

    fn api_key(&self) -> Result<String, ProviderError> {
        (self.key)().ok_or_else(|| ProviderError::Auth("no OpenAI API key saved".into()))
    }
}

/// The JSON body for one speech request. Speed is clamped to the API's 0.25 to 4.0 range.
pub fn body(model: &str, voice: &str, text: &str, speed: f32) -> Value {
    json!({ "model": model, "voice": voice, "input": text, "response_format": "wav", "speed": speed.clamp(0.25, 4.0) })
}

/// The error an error reply means, from its status and the API's message.
pub fn error_for(status: u16, reply: &[u8]) -> ProviderError {
    let message = serde_json::from_slice::<Value>(reply).ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| format!("HTTP {status}"));
    match status {
        401 | 403 => ProviderError::Auth(message),
        429 => ProviderError::RateLimited { retry_after_secs: None },
        500..=599 => ProviderError::Network(message),
        _ => ProviderError::Other(message),
    }
}

impl TtsProvider for OpenAiTts {
    fn id(&self) -> &'static str { ID }
    fn name(&self) -> &'static str { "OpenAI (cloud)" }
    fn kind(&self) -> ProviderKind { ProviderKind::Remote }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { voice: VoiceCapabilities { speed: true, ..Default::default() }, ..Default::default() }
    }

    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        Ok(VOICES.iter().map(|v| VoiceInfo { id: v.to_string(), name: v.to_string(), language: String::new() }).collect())
    }

    /// Only checks that a key is saved. A live call would bill the user on every health check.
    fn health(&self) -> Result<(), ProviderError> {
        self.api_key().map(|_| ())
    }

    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        if !VOICES.contains(&req.voice_id.as_str()) {
            return Err(ProviderError::Other(format!("OpenAI has no voice called '{}'", req.voice_id)));
        }
        let key = self.api_key()?;
        let model = req.model_id.as_deref().unwrap_or(DEFAULT_MODEL);
        let payload = body(model, &req.voice_id, &req.text, req.settings.speed);
        let headers = [
            ("Authorization".to_string(), format!("Bearer {key}")),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let reply = self.http.post(URL, &headers, payload.to_string().as_bytes(), cancel)?;
        if reply.status != 200 { return Err(error_for(reply.status, &reply.body)); }
        let w = wav::parse(&reply.body)?;
        let samples = audio::normalize(&w.samples, w.rate, w.channels)?;
        Ok(SynthesisResult { audio: AudioOutput::Pcm { samples, rate: audio::INTERNAL_RATE, channels: 1 } })
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::HttpResponse;
    use super::*;
    use std::sync::Mutex;

    /// Records each request and answers with a canned reply.
    struct Fake {
        reply: Mutex<Option<HttpResponse>>,
        sent: Mutex<Vec<(String, Vec<(String, String)>, String)>>,
    }

    impl Fake {
        fn answering(status: u16, body: Vec<u8>) -> Arc<Fake> {
            Arc::new(Fake { reply: Mutex::new(Some(HttpResponse { status, body })), sent: Mutex::new(vec![]) })
        }
    }

    impl HttpClient for Fake {
        fn post(&self, url: &str, headers: &[(String, String)], body: &[u8], _: &Cancel) -> Result<HttpResponse, ProviderError> {
            self.sent.lock().unwrap().push((url.into(), headers.to_vec(), String::from_utf8_lossy(body).into_owned()));
            self.reply.lock().unwrap().take().ok_or(ProviderError::Network("no canned reply".into()))
        }
    }

    fn key(k: Option<&str>) -> KeySource {
        let k = k.map(String::from);
        Box::new(move || k.clone())
    }

    fn req(voice: &str) -> SynthesisRequest {
        SynthesisRequest { voice_id: voice.into(), model_id: None, text: "Hello.".into(), settings: SpeechSettings::default() }
    }

    #[test]
    fn a_wav_reply_becomes_app_format_audio_and_the_request_is_right() {
        let fake = Fake::answering(200, wav::encode(24_000, 1, &vec![0i16; 2400]));
        let p = OpenAiTts::new(fake.clone(), key(Some("sk-test")));
        let r = p.synthesize(&req("alloy"), &Cancel::new()).unwrap();
        match r.audio {
            AudioOutput::Pcm { samples, rate, .. } => { assert_eq!(rate, audio::INTERNAL_RATE); assert_eq!(samples.len(), 2400); }
            other => panic!("expected PCM, got {other:?}"),
        }
        let sent = fake.sent.lock().unwrap();
        assert_eq!(sent[0].0, URL);
        assert!(sent[0].1.contains(&("Authorization".to_string(), "Bearer sk-test".to_string())));
        assert!(sent[0].2.contains("\"response_format\":\"wav\"") && sent[0].2.contains("\"voice\":\"alloy\""));
    }

    #[test]
    fn no_key_is_an_auth_error_and_no_request_is_sent() {
        let fake = Fake::answering(200, vec![]);
        let p = OpenAiTts::new(fake.clone(), key(None));
        assert!(matches!(p.health(), Err(ProviderError::Auth(_))));
        assert!(matches!(p.synthesize(&req("alloy"), &Cancel::new()), Err(ProviderError::Auth(_))));
        assert!(fake.sent.lock().unwrap().is_empty(), "nothing leaves the machine without a key");
    }

    #[test]
    fn error_replies_map_to_the_matching_kinds() {
        let msg = br#"{"error":{"message":"Incorrect API key provided"}}"#;
        assert_eq!(error_for(401, msg), ProviderError::Auth("Incorrect API key provided".into()));
        assert_eq!(error_for(429, b""), ProviderError::RateLimited { retry_after_secs: None });
        assert!(matches!(error_for(503, b"down"), ProviderError::Network(_)));
        assert!(matches!(error_for(400, b"not json"), ProviderError::Other(m) if m == "HTTP 400"));
    }

    #[test]
    fn an_unknown_voice_is_refused_before_any_request() {
        let fake = Fake::answering(200, vec![]);
        let p = OpenAiTts::new(fake.clone(), key(Some("sk")));
        assert!(matches!(p.synthesize(&req("ghost"), &Cancel::new()), Err(ProviderError::Other(_))));
        assert!(fake.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn speed_is_clamped_in_the_request_body() {
        assert_eq!(body("tts-1", "nova", "x", 9.0)["speed"], 4.0);
        assert_eq!(body("tts-1", "nova", "x", 0.01)["speed"], 0.25);
    }

    #[test]
    fn health_checks_the_key_without_calling_the_api() {
        let fake = Fake::answering(200, vec![]);
        assert!(OpenAiTts::new(fake.clone(), key(Some("sk"))).health().is_ok());
        assert!(fake.sent.lock().unwrap().is_empty(), "health must not bill the user");
    }
}
