//! ElevenLabs (cloud): speech, its voice list, and voice cloning from a recording. Speech comes back as raw
//! 16-bit PCM at 24 kHz (output_format=pcm_24000), so no audio decoder is needed. The key is the saved
//! "elevenlabs" entry from the Services page. Calls were written against the public API docs and are not yet
//! verified against a live account.

use super::audio;
use super::contract::*;
use super::http::HttpClient;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ID: &str = "elevenlabs";
pub const BASE: &str = "https://api.elevenlabs.io/v1";
pub const KEY_NAME: &str = "elevenlabs";
pub const MODELS: [&str; 2] = ["eleven_multilingual_v2", "eleven_turbo_v2_5"];
const VOICE_LIST_TTL: Duration = Duration::from_secs(600);

pub type KeySource = Box<dyn Fn() -> Option<String> + Send + Sync>;

pub struct ElevenLabsTts {
    http: Arc<dyn HttpClient>,
    key: KeySource,
    /// The voice list, kept for ten minutes: the Settings page asks for it on every paint.
    voices: Mutex<Option<(Instant, Vec<VoiceInfo>)>>,
}

impl ElevenLabsTts {
    pub fn new(http: Arc<dyn HttpClient>, key: KeySource) -> ElevenLabsTts {
        ElevenLabsTts { http, key, voices: Mutex::new(None) }
    }

    fn api_key(&self) -> Result<String, ProviderError> {
        (self.key)().ok_or_else(|| ProviderError::Auth("no ElevenLabs API key saved".into()))
    }

    fn headers(key: &str) -> Vec<(String, String)> {
        vec![("xi-api-key".to_string(), key.to_string())]
    }

    fn forget_voices(&self) {
        if let Ok(mut c) = self.voices.lock() { *c = None; }
    }
}

/// Speed the API accepts: 0.7 to 1.2.
pub fn speed_for(speed: f32) -> f32 { speed.clamp(0.7, 1.2) }

/// The JSON body of one text-to-speech request.
pub fn body(text: &str, model: &str, speed: f32) -> Value {
    json!({ "text": text, "model_id": model, "voice_settings": { "speed": speed_for(speed) } })
}

/// Voices from a list reply: {"voices": [{"voice_id", "name", "labels": {"language"}}]}.
pub fn parse_voices(reply: &[u8]) -> Result<Vec<VoiceInfo>, ProviderError> {
    let v: Value = serde_json::from_slice(reply).map_err(|e| ProviderError::Other(format!("bad voice list: {e}")))?;
    Ok(v["voices"].as_array().into_iter().flatten().filter_map(|x| Some(VoiceInfo {
        id: x["voice_id"].as_str()?.to_string(),
        name: x["name"].as_str().unwrap_or_default().to_string(),
        language: x["labels"]["language"].as_str().unwrap_or_default().to_string(),
    })).collect())
}

/// Little-endian 16-bit PCM as f32 samples.
pub fn pcm_samples(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
}

/// The error an error reply means. ElevenLabs puts the reason under "detail".
pub fn error_for(status: u16, reply: &[u8]) -> ProviderError {
    let v: Value = serde_json::from_slice(reply).unwrap_or(Value::Null);
    let message = v["detail"]["message"].as_str().or(v["detail"].as_str()).map(String::from)
        .unwrap_or_else(|| format!("HTTP {status}"));
    match status {
        401 | 403 => ProviderError::Auth(message),
        429 => ProviderError::RateLimited { retry_after_secs: None },
        500..=599 => ProviderError::Network(message),
        _ => ProviderError::Other(message),
    }
}

fn safe_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl TtsProvider for ElevenLabsTts {
    fn id(&self) -> &'static str { ID }
    fn name(&self) -> &'static str { "ElevenLabs (cloud)" }
    fn kind(&self) -> ProviderKind { ProviderKind::Remote }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { cloning: true, voice: VoiceCapabilities { speed: true, ..Default::default() }, ..Default::default() }
    }

    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        if let Ok(c) = self.voices.lock() {
            if let Some((at, list)) = c.as_ref() { if at.elapsed() < VOICE_LIST_TTL { return Ok(list.clone()); } }
        }
        let key = self.api_key()?;
        let r = self.http.get(&format!("{BASE}/voices"), &Self::headers(&key), &Cancel::new())?;
        if r.status != 200 { return Err(error_for(r.status, &r.body)); }
        let list = parse_voices(&r.body)?;
        if let Ok(mut c) = self.voices.lock() { *c = Some((Instant::now(), list.clone())); }
        Ok(list)
    }

    /// Checks that a key is saved; a live call would spend the user's characters.
    fn health(&self) -> Result<(), ProviderError> {
        self.api_key().map(|_| ())
    }

    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        if !safe_id(&req.voice_id) { return Err(ProviderError::Other("that is not an ElevenLabs voice id".into())); }
        let key = self.api_key()?;
        let model = req.model_id.as_deref().unwrap_or(MODELS[0]);
        let url = format!("{BASE}/text-to-speech/{}?output_format=pcm_24000", req.voice_id);
        let mut headers = Self::headers(&key);
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
        let r = self.http.post(&url, &headers, body(&req.text, model, req.settings.speed).to_string().as_bytes(), cancel)?;
        if r.status != 200 { return Err(error_for(r.status, &r.body)); }
        let samples = audio::normalize(&pcm_samples(&r.body), audio::INTERNAL_RATE, 1)?;
        Ok(SynthesisResult { audio: AudioOutput::Pcm { samples, rate: audio::INTERNAL_RATE, channels: 1 } })
    }

    /// Instant cloning: uploads the recording and returns the new voice. The list cache is dropped so it shows.
    fn clone_voice(&self, req: &CloneVoiceRequest) -> Result<VoiceInfo, ProviderError> {
        let key = self.api_key()?;
        if !req.reference.path.is_file() { return Err(ProviderError::InvalidAudio("the recording file is missing".into())); }
        let fields = vec![("name".to_string(), req.name.clone())];
        let files = vec![("files".to_string(), req.reference.path.clone())];
        let r = self.http.post_form(&format!("{BASE}/voices/add"), &Self::headers(&key), &fields, &files, &Cancel::new())?;
        if r.status != 200 { return Err(error_for(r.status, &r.body)); }
        let v: Value = serde_json::from_slice(&r.body).map_err(|e| ProviderError::Other(format!("bad clone reply: {e}")))?;
        let id = v["voice_id"].as_str().ok_or_else(|| ProviderError::Other("clone reply has no voice_id".into()))?;
        self.forget_voices();
        Ok(VoiceInfo { id: id.into(), name: req.name.clone(), language: String::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::HttpResponse;
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;

    #[derive(Debug, Default)]
    struct Sent { method: &'static str, url: String, headers: Vec<(String, String)>, body: String, fields: Vec<(String, String)>, files: Vec<(String, PathBuf)> }

    /// Records every request; answers from a queue, one reply per request.
    #[derive(Default)]
    struct Fake { replies: Mutex<VecDeque<HttpResponse>>, sent: Mutex<Vec<Sent>> }

    impl Fake {
        fn answering(replies: Vec<HttpResponse>) -> Arc<Fake> {
            Arc::new(Fake { replies: Mutex::new(replies.into()), sent: Mutex::new(vec![]) })
        }
        fn next(&self) -> Result<HttpResponse, ProviderError> {
            self.replies.lock().unwrap().pop_front().ok_or(ProviderError::Network("no canned reply".into()))
        }
    }

    impl HttpClient for Fake {
        fn post(&self, url: &str, headers: &[(String, String)], body: &[u8], _: &Cancel) -> Result<HttpResponse, ProviderError> {
            self.sent.lock().unwrap().push(Sent { method: "POST", url: url.into(), headers: headers.to_vec(), body: String::from_utf8_lossy(body).into_owned(), ..Default::default() });
            self.next()
        }
        fn post_form(&self, url: &str, headers: &[(String, String)], fields: &[(String, String)], files: &[(String, PathBuf)], _: &Cancel) -> Result<HttpResponse, ProviderError> {
            self.sent.lock().unwrap().push(Sent { method: "FORM", url: url.into(), headers: headers.to_vec(), fields: fields.to_vec(), files: files.to_vec(), ..Default::default() });
            self.next()
        }
        fn get(&self, url: &str, headers: &[(String, String)], _: &Cancel) -> Result<HttpResponse, ProviderError> {
            self.sent.lock().unwrap().push(Sent { method: "GET", url: url.into(), headers: headers.to_vec(), ..Default::default() });
            self.next()
        }
    }

    fn key(k: Option<&str>) -> KeySource {
        let k = k.map(String::from);
        Box::new(move || k.clone())
    }

    fn ok(body: Vec<u8>) -> HttpResponse { HttpResponse { status: 200, body } }

    fn req(voice: &str) -> SynthesisRequest {
        SynthesisRequest { voice_id: voice.into(), model_id: None, text: "Hello.".into(), settings: SpeechSettings::default() }
    }

    #[test]
    fn voices_are_read_from_the_list_and_cached() {
        let list = br#"{"voices":[{"voice_id":"v1","name":"Rachel","labels":{"language":"en"}}]}"#.to_vec();
        let fake = Fake::answering(vec![ok(list)]);
        let p = ElevenLabsTts::new(fake.clone(), key(Some("k")));
        assert_eq!(p.voices().unwrap()[0].name, "Rachel");
        assert_eq!(p.voices().unwrap()[0].id, "v1");
        assert_eq!(fake.sent.lock().unwrap().len(), 1, "the second call reads the cache");
        assert_eq!(fake.sent.lock().unwrap()[0].headers, vec![("xi-api-key".to_string(), "k".to_string())]);
    }

    #[test]
    fn speech_is_pcm_at_24k_and_the_key_goes_in_a_header() {
        let pcm: Vec<u8> = [0i16, 16384, -16384, 0].iter().flat_map(|s| s.to_le_bytes()).collect();
        let fake = Fake::answering(vec![ok(pcm)]);
        let p = ElevenLabsTts::new(fake.clone(), key(Some("k")));
        let r = p.synthesize(&req("v1"), &Cancel::new()).unwrap();
        match r.audio {
            AudioOutput::Pcm { samples, rate, .. } => { assert_eq!(rate, audio::INTERNAL_RATE); assert!((samples[1] - 0.5).abs() < 1e-4); }
            other => panic!("expected PCM, got {other:?}"),
        }
        let sent = fake.sent.lock().unwrap();
        assert!(sent[0].url.contains("/text-to-speech/v1?output_format=pcm_24000"));
        assert!(sent[0].body.contains("\"model_id\":\"eleven_multilingual_v2\""));
    }

    #[test]
    fn cloning_uploads_the_recording_and_returns_the_new_voice() {
        let path = std::env::temp_dir().join(format!("zorui-el-{}.wav", super::super::profile::new_id()));
        std::fs::write(&path, b"RIFF").unwrap();
        let fake = Fake::answering(vec![ok(br#"{"voice_id":"newv"}"#.to_vec())]);
        let p = ElevenLabsTts::new(fake.clone(), key(Some("k")));
        let r = CloneVoiceRequest { name: "Me".into(), reference: VoiceReference { path: path.clone(), transcript: "hi".into(), duration_secs: 4.0 } };
        assert_eq!(p.clone_voice(&r).unwrap().id, "newv");
        let sent = fake.sent.lock().unwrap();
        assert!(sent[0].url.ends_with("/voices/add"));
        assert_eq!(sent[0].fields, vec![("name".to_string(), "Me".to_string())]);
        assert_eq!(sent[0].files[0].1, path);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn no_key_means_no_request_at_all() {
        let fake = Fake::answering(vec![]);
        let p = ElevenLabsTts::new(fake.clone(), key(None));
        assert!(matches!(p.health(), Err(ProviderError::Auth(_))));
        assert!(matches!(p.voices(), Err(ProviderError::Auth(_))));
        assert!(fake.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn errors_keep_their_kind_and_the_api_message() {
        let msg = br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key"}}"#;
        assert_eq!(error_for(401, msg), ProviderError::Auth("Invalid API key".into()));
        assert!(matches!(error_for(429, b""), ProviderError::RateLimited { .. }));
    }

    #[test]
    fn speed_is_held_to_what_the_api_takes() {
        assert_eq!(speed_for(3.0), 1.2);
        assert_eq!(speed_for(0.1), 0.7);
    }

    #[test]
    fn a_voice_id_that_could_change_the_url_is_refused() {
        let fake = Fake::answering(vec![]);
        let p = ElevenLabsTts::new(fake.clone(), key(Some("k")));
        assert!(p.synthesize(&req("../voices"), &Cancel::new()).is_err());
        assert!(fake.sent.lock().unwrap().is_empty());
    }
}
