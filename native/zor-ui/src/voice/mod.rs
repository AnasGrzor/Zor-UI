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
pub mod lifecycle;
pub mod manifest;
pub mod playback;
pub mod process;
pub mod profile;
pub mod queue;
pub mod reference;
pub mod registry;
pub mod sha256;
pub mod sidecar;
pub mod windows;
pub mod wav;
pub mod catalog;
pub mod elevenlabs;
pub mod endpoint;
pub mod http;
pub mod library;
pub mod record;
pub mod openai;
pub mod piper;
#[cfg(test)]
pub mod mock;

use contract::{AudioOutput, ProviderCapabilities, ProviderError, SharedProvider, SpeechSettings, SynthesisRequest};
use queue::{Job, Priority, Push, Queue};
use registry::Registry;
use config::VoiceConfig;
use serde_json::Value;
use std::path::PathBuf;
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
        Engine::start_with(providers, config, windows::ID)
    }

    /// Like `start`, with `fallback` as the provider that speaks until a config switches it.
    pub fn start_with(providers: Vec<contract::SharedProvider>, config: VoiceConfig, fallback: &str) -> Arc<Engine> {
        let mut registry = Registry::new();
        for p in providers { let _ = registry.register(p); }
        registry.activate_builtin(fallback);
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
                Ok(r) => if let AudioOutput::Pcm { samples, rate, .. } = r.audio {
                    if let Err(e) = playback::play(&samples, rate, &cancel) {
                        if e != ProviderError::Cancelled { crate::log::line(&format!("voice: {e}")); }
                    }
                },
                Err(ProviderError::Cancelled) => {}
                Err(e) => crate::log::line(&format!("voice: {e}")),
            }
            lock(&self.queue).finish();
        }
    }

    /// Queues `text` for the voice of Buddy `slot` (0 = a, 1 = b). Returns what the queue did with it.
    pub fn speak(&self, slot: usize, text: &str, priority: Priority) -> Option<Push> {
        let voice = lock(&self.config).buddies[slot.min(1)].clone();
        match self.speak_as(&voice, text, priority) {
            Ok(push) => Some(push),
            Err(why) => { crate::log::line(&format!("voice: {why}")); None }
        }
    }

    /// Queues `text` in voice profile `voice_id` on the active provider. Errors say why it cannot speak.
    pub fn speak_as(&self, voice_id: &str, text: &str, priority: Priority) -> Result<Push, String> {
        if text.trim().is_empty() { return Err("there is nothing to say".into()); }
        let provider_id = lock(&self.registry).active().map(|p| p.id()).ok_or("no voice provider is active")?;
        let (voice, model, settings) = resolve(&lock(&self.config), voice_id, provider_id)?;
        let job = Job { priority, req: SynthesisRequest { voice_id: voice, model_id: model, text: text.into(), settings } };
        let push = lock(&self.queue).push(job);
        self.wake.notify_one();
        Ok(push)
    }

    pub fn stop(&self) {
        lock(&self.queue).stop();
    }

    /// Applies a config "voice" section. The provider changes only through `Registry::switch`, so a provider
    /// that fails its checks leaves the working one in place.
    /// Switches provider when `next` names another one. The slow checks run with no lock held, so speech
    /// (and the UI thread calling it) keeps going while a switch is in progress.
    /// Adds a provider after start-up (an install this session). Its id must not be registered already.
    pub fn register(&self, p: contract::SharedProvider) -> Result<(), String> {
        lock(&self.registry).register(p)
    }

    pub fn configure(&self, next: VoiceConfig) -> Result<(), String> {
        if let Ok(mut s) = endpoint_setup().lock() { *s = next.endpoint.clone(); }
        let voice = {
            let cfg = &next;
            let profile = cfg.profiles.iter().find(|p| p.id == cfg.buddies[0]).ok_or("the first Buddy's voice is missing")?;
            profile.binding_for(&cfg.provider).map(|b| b.voice.clone())
                .ok_or_else(|| format!("'{}' has no binding for {}", profile.name, cfg.provider))?
        };
        // both Buddies must have a voice the provider can speak, or the switch would leave one silent
        for slot in 0..2 {
            let id = &next.buddies[slot];
            let profile = next.profiles.iter().find(|p| p.id == *id).ok_or_else(|| format!("Buddy {} has no voice", ["A", "B"][slot]))?;
            if profile.binding_for(&next.provider).is_none() {
                return Err(format!("'{}' has no binding for {}", profile.name, next.provider));
            }
        }
        let already = lock(&self.registry).active().map(|p| p.id()) == Some(next.provider.as_str());
        if !already {
            let candidate = lock(&self.registry).get(&next.provider).ok_or_else(|| format!("no provider '{}'", next.provider))?;
            registry::check(&candidate, &voice, &ProviderCapabilities::default()).map_err(|e| format!("{e:?}"))?;
            let old = lock(&self.registry).activate(candidate);
            if let Some(old) = old.filter(|o| o.id() != next.provider) { old.shutdown(); }
        }
        *lock(&self.config) = next;
        Ok(())
    }
}

/// The provider voice and speech settings a logical voice uses on provider `provider`.
pub fn resolve(cfg: &VoiceConfig, voice_id: &str, provider: &str) -> Result<(String, Option<String>, SpeechSettings), String> {
    let profile = cfg.profiles.iter().find(|p| p.id == voice_id).ok_or_else(|| format!("no voice '{voice_id}'"))?;
    let binding = profile.binding_for(provider).ok_or_else(|| {
        format!("voice '{}' has no {provider} binding; recreate it from its reference recording", profile.name)
    })?;
    Ok((binding.voice.clone(), binding.model.clone(), profile.settings.clone()))
}

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// Clones the recording of `p` with the first provider that can clone, and records that provider's voice as
/// a binding. Returns the provider's id. Fails without changing `p` when no provider can clone.
pub fn clone_with(providers: &[contract::SharedProvider], p: &mut profile::VoiceProfile) -> Result<String, String> {
    let reference = p.reference.clone().ok_or("this voice has no recording to clone from")?;
    let provider = providers.iter().find(|q| q.capabilities().cloning)
        .ok_or("No installed provider can clone voices yet")?;
    provider.health().map_err(|e| e.to_string())?;
    let req = contract::CloneVoiceRequest { name: p.name.clone(), reference };
    let info = provider.clone_voice(&req).map_err(|e| e.to_string())?;
    p.bindings.retain(|b| b.provider != provider.id());
    p.bindings.push(profile::ProviderBinding { provider: provider.id().into(), voice: info.id, model: None, ext: serde_json::json!({ "v": profile::EXT_VERSION }) });
    Ok(provider.id().into())
}

/// Clones `p` with the installed providers (see `clone_with`).
pub fn clone_profile(p: &mut profile::VoiceProfile) -> Result<String, String> {
    // the provider speaking now is tried first, so a clone lands where the voice will be used
    let (mut all, active) = { let r = lock(&engine().registry); (r.all(), r.active().map(|a| a.id())) };
    if let Some(a) = active { all.sort_by_key(|p| p.id() != a); }
    clone_with(&all, p)
}

/// Names of the providers that can clone and are set up (their health check passes), e.g. ElevenLabs with a key.
pub fn cloning_providers() -> Vec<String> {
    lock(&engine().registry).all().iter().filter(|p| p.capabilities().cloning && p.health().is_ok()).map(|p| p.name().to_string()).collect()
}

/// What provider `id` can do, for the Settings page. None when it is not registered.
pub fn provider_caps(id: &str) -> Option<contract::ProviderCapabilities> {
    lock(&engine().registry).get(id).map(|p| p.capabilities())
}

/// Speaks the test sentence in voice `id` on the active provider. The error says why it cannot.
pub fn preview_voice(id: &str) -> Result<(), String> {
    engine().speak_as(id, contract::PREVIEW_TEXT, Priority::Reply).map(|_| ())
}

/// Whether a voice in `profiles` (the list the Settings page shows) can speak on the active provider. The page
/// checks its own list because a save reaches the engine only after the config watcher runs.
pub fn can_speak_in(profiles: &[Value], id: &str) -> Result<(), String> {
    let provider = lock(&engine().registry).active().map(|p| p.id()).ok_or("no voice provider is active")?;
    let profile = profile::builtin_profiles().into_iter()
        .chain(profiles.iter().filter_map(profile::VoiceProfile::from_json))
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no voice '{id}'"))?;
    if profile.binding_for(provider).is_some() { Ok(()) } else { Err(format!("'{}' has no binding for {provider}", profile.name)) }
}

/// Whether voice `id` can speak on the active provider (it has a binding for it).
pub fn can_speak(id: &str) -> Result<(), String> {
    let e = engine();
    let provider = lock(&e.registry).active().map(|p| p.id()).ok_or("no voice provider is active")?;
    resolve(&lock(&e.config), id, provider).map(|_| ())
}

/// The voices provider `id` offers (empty when it is unknown or fails to list them).
pub fn provider_voices(id: &str) -> Vec<contract::VoiceInfo> {
    let p = lock(&engine().registry).get(id);
    p.and_then(|p| p.voices().ok()).unwrap_or_default()
}

/// Where installed providers live: one folder each, with provider.json and its executable.
pub fn providers_root() -> PathBuf { crate::conf::data_dir().join("voice-providers") }
/// Where model files live: voice-models/<provider>/<model>/.
pub fn models_root() -> PathBuf { crate::conf::data_dir().join("voice-models") }
/// The record of what is installed.
pub fn store_path() -> PathBuf { crate::conf::data_dir().join("voice-install.json") }

/// Sidecars found in the providers folder. A folder with a bad manifest is logged and skipped.
fn installed_providers() -> Vec<SharedProvider> {
    let tmp = crate::conf::data_dir().join("voice-tmp");
    std::fs::create_dir_all(&tmp).ok();
    manifest::discover(&providers_root()).into_iter().filter_map(|(dir, found)| match found {
        Ok(m) => Some(match m.adapter {
            manifest::Adapter::JsonSidecar => Arc::new(sidecar::SidecarProvider::new(&m, sidecar::process_launcher(dir.clone(), &m), tmp.clone())) as SharedProvider,
            manifest::Adapter::PiperCli => Arc::new(piper::PiperTts::new(dir.join(&m.command), models_root().join(&m.id), tmp.clone(), &m.version)) as SharedProvider,
        }),
        Err(why) => { crate::log::line(&format!("voice: skipped provider in {}: {why}", dir.display())); None }
    }).collect()
}

/// Downloads through curl, as the rest of the app does. Used by the install step.
pub fn fetch(url: &str) -> Option<Vec<u8>> { crate::bridge::common::get_bytes(url, &[], 900) }

/// Installs a catalog provider: its package and the models named, each with its approval (see
/// `lifecycle::install_manifest`). Nothing is downloaded unless every item has one.
pub fn install_catalog(id: &str, approvals: &[lifecycle::Approval]) -> Result<(), String> {
    let raw = catalog::find(id).ok_or_else(|| format!("no provider '{id}' in the catalog"))?;
    let m = manifest::parse(&raw)?;
    let mut store = lifecycle::Store::load(&store_path());
    lifecycle::install_manifest(&mut store, &providers_root(), &models_root(), &m, &raw, approvals, &fetch, &lifecycle::extract_with_tar)
        .map_err(|e| e.to_string())?;
    store.save(&store_path()).map_err(|e| e.to_string())
}

/// The endpoint's setup, shared with the engine, which refreshes it from config on every change.
fn endpoint_setup() -> &'static endpoint::Setup {
    static SETUP: OnceLock<endpoint::Setup> = OnceLock::new();
    SETUP.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// Models offered for a provider's voices in the Settings page. Empty when the provider has one model.
pub fn models_for(provider: &str) -> &'static [&'static str] {
    match provider {
        "openai" => &["tts-1", "tts-1-hd"],
        "elevenlabs" => &elevenlabs::MODELS,
        _ => &[],
    }
}

/// The id of the provider speaking now, if one is active.
pub fn active_provider() -> Option<&'static str> { lock(&engine().registry).active().map(|p| p.id()) }

/// The app-wide engine: Windows, OpenAI (which reports a missing key until one is saved), ElevenLabs and the custom
/// endpoint (both report what is missing until set up), and installed providers.
fn engine() -> &'static Arc<Engine> {
    ENGINE.get_or_init(|| {
        // the keys the Services page saves; read on each call
        let key: openai::KeySource = Box::new(|| Some(crate::bridge::web::secret_get(openai::KEY_NAME)).filter(|k| !k.is_empty()));
        let eleven_key: elevenlabs::KeySource = Box::new(|| Some(crate::bridge::web::secret_get(elevenlabs::KEY_NAME)).filter(|k| !k.is_empty()));
        let endpoint_key: Box<dyn Fn() -> Option<String> + Send + Sync> = Box::new(|| Some(crate::bridge::web::secret_get(endpoint::KEY_NAME)).filter(|k| !k.is_empty()));
        let mut providers: Vec<SharedProvider> = vec![
            Arc::new(windows::WindowsTts),
            Arc::new(openai::OpenAiTts::new(Arc::new(http::Curl), key)),
            Arc::new(elevenlabs::ElevenLabsTts::new(Arc::new(http::Curl), eleven_key)),
            Arc::new(endpoint::EndpointTts::new(Arc::new(http::Curl), endpoint_key, endpoint_setup().clone())),
        ];
        providers.extend(installed_providers());
        Engine::start(providers, VoiceConfig::load(&Value::Null, "gojo", "geto"))
    })
}

/// Speaks `text` as Buddy slot `slot`. Non-blocking: the line is queued and synthesized on the voice thread.
pub fn speak(slot: usize, text: &str, priority: Priority) -> Option<Push> { engine().speak(slot, text, priority) }

/// Silences the voice and drops queued lines.
pub fn stop() { engine().stop() }

/// What the last switch said, for the Settings page: "Using piper", or why it could not switch.
static STATUS: Mutex<String> = Mutex::new(String::new());

pub fn status() -> String { STATUS.lock().map(|s| s.clone()).unwrap_or_default() }

/// Applies the config's "voice" section (see `config::VoiceConfig::load`). A provider switch runs a test
/// preview, so it happens on its own thread, not the UI thread that calls this.
/// shortcut: two config saves in a row can race their switches; a queue of switches fixes it if it ever shows.
pub fn configure(section: &Value, a_char: &str, b_char: &str) {
    let next = VoiceConfig::load(section, a_char, b_char);
    let e = engine().clone();
    std::thread::spawn(move || {
        let provider = next.provider.clone();
        let result = e.configure(next);
        let text = match &result {
            Ok(()) => format!("Using {provider}"),
            Err(why) => { crate::log::line(&format!("voice: could not apply settings: {why}")); why.clone() }
        };
        if let Ok(mut s) = STATUS.lock() { *s = text; }
    });
}

/// Registers installed providers the engine does not know yet, e.g. after an install this session.
pub fn refresh_providers() {
    let e = engine();
    let known = lock(&e.registry).ids();
    for p in installed_providers() {
        if !known.contains(&p.id()) { let _ = lock(&e.registry).register(p); }
    }
}

/// Whether provider `id` is installed (recorded in voice-install.json).
pub fn installed(id: &str) -> bool {
    lifecycle::Store::load(&store_path()).providers.iter().any(|p| p.id == id)
}

/// Whether an OpenAI key is saved for the Services page.
pub fn openai_key_saved() -> bool { !crate::bridge::web::secret_get(openai::KEY_NAME).is_empty() }

/// Approvals for every item a catalog provider downloads (package and models), with the sizes the prompt shows.
pub fn approvals_for(id: &str) -> Result<Vec<lifecycle::Approval>, String> {
    let raw = catalog::find(id).ok_or_else(|| format!("no provider '{id}' in the catalog"))?;
    let m = manifest::parse(&raw)?;
    let mut out: Vec<lifecycle::Approval> = m.package.iter().map(|p| lifecycle::Approval { what: m.id.clone(), size_bytes: p.size_bytes }).collect();
    out.extend(m.models.iter().map(|md| lifecycle::Approval { what: md.id.clone(), size_bytes: md.size_bytes() }));
    Ok(out)
}

/// Total download size of a catalog provider in bytes, or None when it is not in the catalog.
pub fn download_bytes(id: &str) -> Option<u64> {
    approvals_for(id).ok().map(|a| a.iter().map(|x| x.size_bytes).sum())
}

/// Removes an installed provider's files and its record. Its voices stay on disk for a reinstall.
pub fn remove(id: &str) -> Result<(), String> {
    let mut store = lifecycle::Store::load(&store_path());
    lifecycle::remove_provider(&mut store, &providers_root(), &models_root(), id, false).map_err(|e| e.to_string())?;
    store.save(&store_path()).map_err(|e| e.to_string())
}

/// Speaks the test sentence as Buddy 0, so a chosen voice can be heard without waiting for a reply.
pub fn test_voice() { speak(0, contract::PREVIEW_TEXT, Priority::Reply); }

#[cfg(test)]
mod tests {
    use super::*;
    use contract::SharedProvider;
    use mock::MockTts;
    use profile::{builtin_profiles, ProviderBinding, VoiceProfile};
    use std::time::{Duration, Instant};

    fn cfg_for(provider: &str, voice: &str) -> VoiceConfig {
        let mut profiles = builtin_profiles();
        profiles.push(VoiceProfile {
            id: "user-1".into(), name: "Mine".into(), description: String::new(), tags: vec![],
            settings: SpeechSettings::default(), reference: None,
            bindings: vec![ProviderBinding { provider: provider.into(), voice: voice.into(), model: None, ext: serde_json::json!({ "v": 1 }) }],
        });
        // both Buddies on the custom voice: the switch checks each Buddy has a voice for the provider
        VoiceConfig { provider: provider.into(), buddies: ["user-1".into(), "user-1".into()], profiles, endpoint: None }
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
    fn a_slow_provider_switch_does_not_block_speech() {
        // The fallback speaks while "slow" checks its health for a second. speak() is what the UI thread calls.
        let fast = MockTts::new("fast");
        let slow = MockTts::new("slow").with_health_delay(Duration::from_secs(1));
        let e = Engine::start_with(vec![Arc::new(fast), Arc::new(slow)], cfg_for("fast", "v1"), "fast");
        let switching = e.clone();
        let switch = std::thread::spawn(move || switching.configure(cfg_for("slow", "v1")));
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        let pushed = e.speak(0, "during the switch", Priority::Reply);
        let took = started.elapsed();
        assert!(pushed.is_some(), "the line is still accepted");
        assert!(took < Duration::from_millis(200), "speak waited {took:?} for the switch");
        assert!(switch.join().unwrap().is_ok());
        assert_eq!(lock(&e.registry).active().unwrap().id(), "slow", "the switch then completes");
    }

    #[test]
    fn a_catalog_download_is_the_sum_of_its_files_with_one_approval_each() {
        assert_eq!(download_bytes("piper"), Some(22_477_236 + 63_201_294 + 4_885));
        let a = approvals_for("piper").unwrap();
        assert_eq!(a.len(), 2, "the package and the voice");
        assert!(download_bytes("nope").is_none());
    }

    #[test]
    fn a_failed_switch_reads_as_a_sentence_for_the_settings_page() {
        assert_eq!(registry::SwitchError::UnknownProvider("x".into()).to_string(), "there is no provider 'x'");
        assert_eq!(registry::SwitchError::NoVoice("v".into()).to_string(), "the provider has no voice 'v'");
    }

    #[test]
    fn a_choice_made_before_its_provider_was_installed_applies_once_it_is() {
        // the Settings page saves "piper" before Piper is installed; the switch fails, then succeeds after register
        let e = Engine::start(vec![Arc::new(windows::WindowsTts)], cfg_for("windows", "male"));
        let late = MockTts::new("late");
        assert_eq!(e.configure(cfg_for("late", "v1")).unwrap_err(), "no provider 'late'");
        assert_eq!(lock(&e.registry).active().unwrap().id(), "windows", "still on the working voice");
        e.register(Arc::new(late)).unwrap();
        e.configure(cfg_for("late", "v1")).unwrap();
        assert_eq!(lock(&e.registry).active().unwrap().id(), "late", "the saved choice applies now");
    }

    #[test]
    fn a_switch_is_refused_when_buddy_b_has_no_voice_for_the_provider() {
        let fast = MockTts::new("fast");
        let other = MockTts::new("other");
        let e = Engine::start_with(vec![Arc::new(fast), Arc::new(other)], cfg_for("fast", "v1"), "fast");
        let mut next = cfg_for("other", "v1");
        next.buddies[1] = "user-missing".into();
        let err = e.configure(next).unwrap_err();
        assert!(err.contains("Buddy B has no voice"), "{err}");
        assert_eq!(lock(&e.registry).active().unwrap().id(), "fast", "nothing switched");
    }

    /// Installs Piper for real (an 86 MB download, checked against its hashes) into a temp data folder, then speaks.
    /// Run: cargo test --release piper_installs_end_to_end -- --ignored
    #[test]
    #[ignore = "downloads Piper (about 86 MB); run with --ignored"]
    fn piper_installs_end_to_end() {
        use contract::{Cancel, TtsProvider};
        let base = std::env::temp_dir().join(format!("zorui-e2e-{}", profile::new_id()));
        std::env::set_var("LOCALAPPDATA", &base);
        install_catalog("piper", &approvals_for("piper").unwrap()).expect("install");
        assert!(installed("piper"), "recorded as installed");
        let tmp = base.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let p = piper::PiperTts::new(providers_root().join("piper").join("piper").join("piper.exe"), models_root().join("piper"), tmp, "t");
        let voices = p.voices().unwrap();
        assert!(voices.iter().any(|v| v.id == "en_US-lessac-medium"), "voices: {voices:?}");
        let r = p.synthesize(&SynthesisRequest {
            voice_id: "en_US-lessac-medium".into(), model_id: None,
            text: "Hello there. This is a test of the installed voice.".into(), settings: SpeechSettings::default(),
        }, &Cancel::new()).expect("speak");
        match r.audio {
            AudioOutput::Pcm { samples, rate, .. } => {
                assert_eq!(rate, audio::INTERNAL_RATE);
                assert!(samples.len() > 24_000, "more than a second of speech: {} samples", samples.len());
            }
            other => panic!("expected PCM, got {other:?}"),
        }
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn cloning_adds_the_providers_voice_and_refuses_when_none_can_clone() {
        use profile::VoiceProfile;
        let mut p = VoiceProfile {
            id: "r1".into(), name: "Mine".into(), description: String::new(), tags: vec![], settings: SpeechSettings::default(),
            reference: Some(contract::VoiceReference { path: "x.wav".into(), transcript: "hi".into(), duration_secs: 5.0 }),
            bindings: vec![],
        };
        let none: Vec<contract::SharedProvider> = vec![Arc::new(MockTts::new("plain"))];
        assert!(clone_with(&none, &mut p).unwrap_err().contains("No installed provider can clone"));
        assert!(p.bindings.is_empty(), "nothing changed when refused");
        let cloner: Vec<contract::SharedProvider> = vec![Arc::new(MockTts::new("cloner").with_cloning())];
        assert_eq!(clone_with(&cloner, &mut p).unwrap(), "cloner");
        assert_eq!(p.binding_for("cloner").unwrap().voice, "clone-Mine");
        assert_eq!(clone_with(&cloner, &mut p).unwrap(), "cloner");
        assert_eq!(p.bindings.len(), 1, "cloning again replaces the binding");
    }

    #[test]
    fn a_voice_without_a_recording_cannot_be_cloned() {
        let mut p = profile::VoiceProfile { id: "x".into(), name: "x".into(), description: String::new(), tags: vec![], settings: SpeechSettings::default(), reference: None, bindings: vec![] };
        let cloner: Vec<contract::SharedProvider> = vec![Arc::new(MockTts::new("cloner").with_cloning())];
        assert!(clone_with(&cloner, &mut p).is_err());
    }

    #[test]
    fn empty_text_is_not_queued_and_stop_clears_the_queue() {
        let e = Engine::start(vec![Arc::new(MockTts::new("mock"))], cfg_for("mock", "v1"));
        assert_eq!(e.speak(0, "   ", Priority::Reply), None);
        e.stop();
        assert!(lock(&e.queue).is_idle());
    }
}
