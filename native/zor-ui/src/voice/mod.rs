//! The Voice API. Buddy and the rest of the app speak through `speak` and `stop`; they never name a provider.
//! Providers live in the registry, lines wait in the speech queue, and one worker thread synthesizes them, so
//! speech never blocks the UI.
//!
//! Stages 3-7 of docs/VoicePlan.md (sidecar and remote adapters, the Settings studio, cloning) are not wired
//! yet. Their contracts are here and tested; `allow(dead_code)` keeps the build quiet until they are used.
#![allow(dead_code)]

pub mod audio;
pub mod config;
pub mod contract;
pub mod profile;
pub mod queue;
pub mod registry;
pub mod windows;
#[cfg(test)]
pub mod mock;

use contract::{AudioOutput, ProviderCapabilities, ProviderError, SpeechSettings, SynthesisRequest};
use queue::{Job, Priority, Push, Queue};
use registry::Registry;
use config::VoiceConfig;
use serde_json::Value;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

/// Lines waiting behind the one playing. Ambient remarks past this are dropped, not stacked.
const MAX_PENDING: usize = 4;

pub struct Engine {
    registry: Mutex<Registry>,
    queue: Mutex<Queue>,
    wake: Condvar,
    config: Mutex<VoiceConfig>,
}

/// Locks, and reads through a poisoned lock: a panic in one synthesis must not silence every later line.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|p| p.into_inner()) }

impl Engine {
    /// An engine with `providers` registered and `windows` active, plus a worker thread that drains the queue.
    pub fn start(providers: Vec<contract::SharedProvider>, config: VoiceConfig) -> Arc<Engine> {
        let mut registry = Registry::new();
        for p in providers { let _ = registry.register(p); }
        registry.activate_builtin(windows::ID);
        let e = Arc::new(Engine { registry: Mutex::new(registry), queue: Mutex::new(Queue::new(MAX_PENDING)), wake: Condvar::new(), config: Mutex::new(config) });
        let worker = e.clone();
        std::thread::Builder::new().name("zor-voice".into()).spawn(move || worker.run()).ok();
        e
    }

    fn run(&self) {
        loop {
            let (job, cancel) = {
                let mut q = lock(&self.queue);
                loop {
                    if let Some(next) = q.take() { break next; }
                    q = self.wake.wait(q).unwrap_or_else(|p| p.into_inner());
                }
            };
            let active = lock(&self.registry).active();
            let outcome = match active {
                Some(p) => p.synthesize(&job.req, &cancel),
                None => Err(ProviderError::Other("no voice provider is active".into())),
            };
            match outcome {
                Ok(r) => if let AudioOutput::Pcm { samples, .. } = r.audio {
                    // No playback device path yet: normalized audio is produced but not sent to the speakers.
                    crate::log::line(&format!("voice: {} samples ready, playback not wired", samples.len()));
                },
                Err(ProviderError::Cancelled) => {}
                Err(e) => crate::log::line(&format!("voice: {e}")),
            }
            lock(&self.queue).finish();
        }
    }

    /// Queues `text` for the voice of Buddy `slot` (0 = a, 1 = b). Returns what the queue did with it.
    pub fn speak(&self, slot: usize, text: &str, priority: Priority) -> Option<Push> {
        if text.trim().is_empty() { return None; }
        let provider_id = lock(&self.registry).active().map(|p| p.id())?;
        let (voice, settings) = {
            let cfg = lock(&self.config);
            let voice_id = cfg.buddies[slot.min(1)].clone();
            match resolve(&cfg, &voice_id, provider_id) {
                Ok(found) => found,
                Err(why) => { crate::log::line(&format!("voice: {why}")); return None; }
            }
        };
        let job = Job { priority, req: SynthesisRequest { voice_id: voice, model_id: None, text: text.into(), settings } };
        let push = lock(&self.queue).push(job);
        self.wake.notify_one();
        Some(push)
    }

    pub fn stop(&self) {
        lock(&self.queue).stop();
    }

    /// Applies a config "voice" section. The provider changes only through `Registry::switch`, so a provider
    /// that fails its checks leaves the working one in place.
    pub fn configure(&self, next: VoiceConfig) -> Result<(), String> {
        let voice = {
            let cfg = &next;
            let profile = cfg.profiles.iter().find(|p| p.id == cfg.buddies[0]).ok_or("the first Buddy's voice is missing")?;
            profile.binding_for(&cfg.provider).map(|b| b.voice.clone())
                .ok_or_else(|| format!("'{}' has no binding for {}", profile.name, cfg.provider))?
        };
        let mut reg = lock(&self.registry);
        let already = reg.active().map(|p| p.id()) == Some(next.provider.as_str());
        if !already {
            reg.switch(&next.provider, &voice, &ProviderCapabilities::default()).map_err(|e| format!("{e:?}"))?;
        }
        *lock(&self.config) = next;
        Ok(())
    }
}

/// The provider voice and speech settings a logical voice uses on provider `provider`.
pub fn resolve(cfg: &VoiceConfig, voice_id: &str, provider: &str) -> Result<(String, SpeechSettings), String> {
    let profile = cfg.profiles.iter().find(|p| p.id == voice_id).ok_or_else(|| format!("no voice '{voice_id}'"))?;
    let binding = profile.binding_for(provider).ok_or_else(|| {
        format!("voice '{}' has no {provider} binding; recreate it from its reference recording", profile.name)
    })?;
    Ok((binding.voice.clone(), profile.settings.clone()))
}

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// The app-wide engine, started on first use with the built-in providers.
fn engine() -> &'static Arc<Engine> {
    ENGINE.get_or_init(|| Engine::start(vec![Arc::new(windows::WindowsTts)], VoiceConfig::load(&Value::Null, "gojo", "geto")))
}

/// Speaks `text` as Buddy slot `slot`. Non-blocking: the line is queued and synthesized on the voice thread.
pub fn speak(slot: usize, text: &str, priority: Priority) -> Option<Push> { engine().speak(slot, text, priority) }

/// Silences the voice and drops queued lines.
pub fn stop() { engine().stop() }

/// Applies the config's "voice" section (see `config::VoiceConfig::load`) and logs a failure.
pub fn configure(section: &Value, a_char: &str, b_char: &str) {
    let next = VoiceConfig::load(section, a_char, b_char);
    if let Err(why) = engine().configure(next) { crate::log::line(&format!("voice: could not apply settings: {why}")); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::SharedProvider;
    use mock::MockTts;
    use profile::{builtin_id, builtin_profiles, ProviderBinding, VoiceProfile};
    use std::time::{Duration, Instant};

    fn cfg_for(provider: &str, voice: &str) -> VoiceConfig {
        let mut profiles = builtin_profiles();
        profiles.push(VoiceProfile {
            id: "user-1".into(), name: "Mine".into(), description: String::new(), tags: vec![],
            settings: SpeechSettings::default(), reference: None,
            bindings: vec![ProviderBinding { provider: provider.into(), voice: voice.into(), model: None, ext: serde_json::json!({ "v": 1 }) }],
        });
        VoiceConfig { provider: provider.into(), buddies: ["user-1".into(), builtin_id("geto")], profiles }
    }

    fn wait_for(spoken: &Arc<Mutex<Vec<String>>>, text: &str) -> bool {
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if lock(spoken).iter().any(|s| s == text) { return true; }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_line_reaches_the_active_provider_with_its_bound_voice() {
        let mock = MockTts::new("mock");
        let spoken = mock.spoken.clone();
        let e = Engine::start(vec![Arc::new(windows::WindowsTts), Arc::new(mock)], cfg_for("mock", "v1"));
        e.configure(cfg_for("mock", "v1")).unwrap();
        assert!(e.speak(0, "hello there", Priority::Reply).is_some());
        assert!(wait_for(&spoken, "hello there"), "the mock provider spoke the line");
    }

    #[test]
    fn a_failed_switch_keeps_the_working_provider() {
        let bad: SharedProvider = Arc::new(MockTts::new("bad").failing_health());
        let e = Engine::start(vec![Arc::new(windows::WindowsTts), bad], cfg_for("windows", "male"));
        let err = e.configure(cfg_for("bad", "v1"));
        assert!(err.is_err(), "the failing provider is refused");
        assert_eq!(lock(&e.registry).active().unwrap().id(), "windows", "the built-in voice is still active");
    }

    #[test]
    fn a_voice_with_no_binding_for_the_active_provider_is_not_replaced_silently() {
        let cfg = cfg_for("elevenlabs", "x");
        let err = resolve(&cfg, "user-1", "windows").unwrap_err();
        assert!(err.contains("recreate it from its reference recording"), "{err}");
        assert!(resolve(&cfg, "user-1", "elevenlabs").is_ok());
    }

    #[test]
    fn a_newer_reply_cuts_one_still_being_synthesized() {
        let slow = MockTts::new("mock").with_delay(Duration::from_secs(5));
        let spoken = slow.spoken.clone();
        let e = Engine::start(vec![Arc::new(slow)], cfg_for("mock", "v1"));
        e.configure(cfg_for("mock", "v1")).unwrap();
        let started = Instant::now();
        e.speak(0, "first", Priority::Reply);
        assert!(wait_for(&spoken, "first"));
        assert_eq!(e.speak(0, "second", Priority::Reply), Some(Push::Cut));
        assert!(wait_for(&spoken, "second"));
        assert!(started.elapsed() < Duration::from_secs(4), "the first line was cut, not waited out");
    }

    #[test]
    fn empty_text_is_not_queued_and_stop_clears_the_queue() {
        let e = Engine::start(vec![Arc::new(MockTts::new("mock"))], cfg_for("mock", "v1"));
        assert_eq!(e.speak(0, "   ", Priority::Reply), None);
        e.stop();
        assert!(lock(&e.queue).is_idle());
    }
}
