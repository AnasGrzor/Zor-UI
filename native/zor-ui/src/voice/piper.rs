//! Piper, a local neural TTS. One `piper.exe` runs with --json-input and stays up between lines, so the voice
//! loads once (about 0.5 s) instead of on every line. The process is restarted when the voice or the speed
//! changes, after a crash, or after a cancel. It stops after a minute with nothing to say, which frees its memory.
//!
//! Piper's --length_scale is fixed when the process starts; a per-line length_scale is ignored (checked).
//! Each line is {"text", "output_file"}; Piper prints the output path on stdout once the WAV is written.

use super::audio;
use super::contract::*;
use super::{profile::new_id, wav};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const ID: &str = "piper";
/// How long a session stays up with nothing to say.
pub const IDLE: Duration = Duration::from_secs(60);
/// How long one line may take before the session is treated as stuck.
const LINE_TIMEOUT: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|p| p.into_inner()) }

/// A running Piper process. Dropping it kills the process.
pub struct Session {
    id: u64,
    key: String,
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    last_used: Instant,
}

impl Session {
    /// Starts `exe` with `args`: requests on stdin, replies on stdout.
    pub fn spawn(id: u64, key: String, exe: &Path, args: &[String]) -> Result<Session, ProviderError> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new(exe);
        // a bare program name (found on PATH) has an empty parent folder, so only set one that exists
        if let Some(dir) = exe.parent().filter(|d| !d.as_os_str().is_empty()) { cmd.current_dir(dir); }
        let mut child = cmd
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn()
            .map_err(|e| ProviderError::ModelLoad(format!("could not start Piper: {e}")))?;
        let stdin = child.stdin.take().ok_or_else(|| ProviderError::Other("no stdin on Piper".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Other("no stdout on Piper".into()))?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() { break; }
            }
        });
        Ok(Session { id, key, child, stdin, lines, last_used: Instant::now() })
    }

    /// Sends one request and waits for the reply naming `out`. Cancel returns Cancelled; the caller drops the session.
    pub fn request(&mut self, line: &str, out: &str, cancel: &Cancel, timeout: Duration) -> Result<(), ProviderError> {
        writeln!(self.stdin, "{line}").and_then(|_| self.stdin.flush())
            .map_err(|_| ProviderError::Other("Piper stopped".into()))?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
            match self.lines.recv_timeout(Duration::from_millis(25)) {
                Ok(reply) if reply.trim() == out => return Ok(()),
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if Instant::now() > deadline { return Err(ProviderError::Other("Piper did not finish the line".into())); }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ProviderError::Other("Piper stopped".into())),
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The request line for one utterance. Text is JSON-escaped, so it can never end the line early.
pub fn json_line(text: &str, out: &Path) -> String {
    json!({ "text": text, "output_file": out.to_string_lossy() }).to_string()
}

/// Piper's length_scale for a speed: speed 2.0 means half the length. Clamped to the range Piper handles well.
pub fn scale_for(speed: f32) -> String { format!("{:.3}", 1.0 / speed.clamp(0.25, 4.0)) }

/// The arguments that start a session. The text is never among them; it goes on each request line.
pub fn session_args(model: &Path, speed: f32) -> Vec<String> {
    vec!["-m".into(), model.to_string_lossy().into_owned(), "--length_scale".into(), scale_for(speed), "--json-input".into()]
}

/// What a running session was started for: a session is reused only for the same model and speed.
pub fn session_key(model: &Path, speed: f32) -> String { format!("{}|{}", model.display(), scale_for(speed)) }

/// Whether a session unused since `last_used` should stop now.
pub fn expired(last_used: Instant, idle: Duration) -> bool { last_used.elapsed() >= idle }

/// Stops the session in `slot` once it has been idle for `idle`. Ends early if the slot holds another session
/// (a restart replaced this one) or is empty. Synthesis holds the slot's lock, so it is never stopped mid-line.
fn watch(slot: Arc<Mutex<Option<Session>>>, id: u64, idle: Duration) {
    let tick = (idle / 4).clamp(Duration::from_millis(10), Duration::from_millis(500));
    std::thread::spawn(move || loop {
        std::thread::sleep(tick);
        let mut g = lock(&slot);
        let stop = match g.as_ref() {
            Some(s) if s.id == id => expired(s.last_used, idle),
            _ => return,
        };
        if stop {
            *g = None;
            return;
        }
    });
}

pub struct PiperTts {
    exe: PathBuf,
    /// voice-models/piper: one folder per voice.
    models: PathBuf,
    tmp: PathBuf,
    version: &'static str,
    slot: Arc<Mutex<Option<Session>>>,
    next_id: AtomicU64,
    idle: Duration,
}

impl PiperTts {
    /// shortcut: the version string is leaked once so the trait can return 'static; one short string per install.
    pub fn new(exe: PathBuf, models: PathBuf, tmp: PathBuf, version: &str) -> PiperTts {
        PiperTts {
            exe,
            models,
            tmp,
            version: Box::leak(version.to_string().into_boxed_str()),
            slot: Arc::new(Mutex::new(None)),
            next_id: AtomicU64::new(1),
            idle: IDLE,
        }
    }

    /// The same provider with another idle time (tests use a short one).
    pub fn with_idle(mut self, idle: Duration) -> PiperTts { self.idle = idle; self }

    /// Whether a Piper process is up right now.
    pub fn running(&self) -> bool { lock(&self.slot).is_some() }

    /// The .onnx model of voice `id`, if the voice is complete (its .onnx.json config is there too).
    fn model_for(&self, id: &str) -> Option<PathBuf> {
        if !super::manifest::valid_model_id(id) { return None; }
        let dir = self.models.join(id);
        let onnx = std::fs::read_dir(&dir).ok()?
            .filter_map(Result::ok).map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "onnx"))?;
        let config = PathBuf::from(format!("{}.json", onnx.display()));
        config.is_file().then_some(onnx)
    }
}

impl TtsProvider for PiperTts {
    fn id(&self) -> &'static str { ID }
    fn name(&self) -> &'static str { "Piper (local)" }
    fn kind(&self) -> ProviderKind { ProviderKind::Sidecar }
    fn version(&self) -> &'static str { self.version }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities { voice: VoiceCapabilities { speed: true, ..Default::default() }, ..Default::default() }
    }

    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        let Ok(rd) = std::fs::read_dir(&self.models) else { return Ok(vec![]) };
        let mut voices: Vec<VoiceInfo> = rd.filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|id| self.model_for(id).is_some())
            .map(|id| VoiceInfo { name: id.clone(), language: language_of(&self.models.join(&id)), id })
            .collect();
        voices.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(voices)
    }

    fn health(&self) -> Result<(), ProviderError> {
        if self.exe.is_file() { Ok(()) } else { Err(ProviderError::ModelLoad("Piper is not installed".into())) }
    }

    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        let model = self.model_for(&req.voice_id)
            .ok_or_else(|| ProviderError::Other(format!("no Piper voice installed called '{}'", req.voice_id)))?;
        if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
        let out = self.tmp.join(format!("{}.wav", new_id()));
        let key = session_key(&model, req.settings.speed);
        let outcome = {
            // held for the whole line: the idle watcher waits for it, and a second line waits its turn
            let mut slot = lock(&self.slot);
            if !slot.as_ref().is_some_and(|s| s.key == key) {
                *slot = None;
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                *slot = Some(Session::spawn(id, key.clone(), &self.exe, &session_args(&model, req.settings.speed))?);
                watch(self.slot.clone(), id, self.idle);
            }
            let s = slot.as_mut().expect("started above");
            let r = s.request(&json_line(&req.text, &out), &out.to_string_lossy(), cancel, LINE_TIMEOUT);
            if r.is_ok() { s.last_used = Instant::now(); }
            if r.is_err() { *slot = None; }
            r
        };
        let bytes = std::fs::read(&out);
        std::fs::remove_file(&out).ok();
        outcome?;
        let w = wav::parse(&bytes.map_err(|_| ProviderError::InvalidAudio("Piper wrote no audio".into()))?)?;
        let samples = audio::normalize(&w.samples, w.rate, w.channels)?;
        Ok(SynthesisResult { audio: AudioOutput::Pcm { samples, rate: audio::INTERNAL_RATE, channels: 1 } })
    }

    /// Stops the Piper process now. The next line starts it again.
    fn shutdown(&self) { *lock(&self.slot) = None; }
}

/// The language code from a voice's .onnx.json, e.g. "en_US". Empty when the file lacks one.
fn language_of(dir: &Path) -> String {
    let config = std::fs::read_dir(dir).ok()
        .and_then(|rd| rd.filter_map(Result::ok).map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(".onnx.json")));
    config.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["language"]["code"].as_str().map(String::from))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zorui-piper-{name}-{}", new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn voice(models: &Path, id: &str, with_config: bool) {
        let d = models.join(id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{id}.onnx")), b"onnx").unwrap();
        if with_config {
            std::fs::write(d.join(format!("{id}.onnx.json")), r#"{"language":{"code":"en_US"}}"#).unwrap();
        }
    }

    /// A stand-in for Piper: prints each request's output_file back, as Piper prints the path it wrote.
    fn echo_args() -> Vec<String> {
        let script = "while($l=[Console]::In.ReadLine()){ $o=($l|ConvertFrom-Json).output_file; [Console]::Out.WriteLine($o); [Console]::Out.Flush() }";
        vec!["-NoProfile".into(), "-Command".into(), script.into()]
    }

    fn echo_session(id: u64) -> Session {
        Session::spawn(id, "echo".into(), Path::new("powershell.exe"), &echo_args()).unwrap()
    }

    #[test]
    fn a_request_line_is_one_json_object_with_the_text_and_output_path_escaped() {
        let line = json_line("He said \"hi\"\nthen left", Path::new(r"C:\out\a.wav"));
        assert!(!line.contains('\n'), "one line per request");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["text"], "He said \"hi\"\nthen left");
        assert_eq!(v["output_file"], r"C:\out\a.wav");
    }

    #[test]
    fn speed_maps_to_length_scale_and_is_clamped() {
        assert_eq!(scale_for(2.0), "0.500", "twice as fast is half the length");
        assert_eq!(scale_for(100.0), "0.250", "clamped to 4x");
        assert_eq!(scale_for(1.0), "1.000");
    }

    #[test]
    fn the_session_arguments_fix_model_and_speed_and_never_carry_the_text() {
        let a = session_args(Path::new("m.onnx"), 2.0);
        assert!(a.contains(&"--json-input".to_string()));
        assert_eq!(a[a.iter().position(|x| x == "--length_scale").unwrap() + 1], "0.500");
        assert!(!a.iter().any(|x| x.contains("hello")));
    }

    #[test]
    fn a_session_is_reused_only_for_the_same_voice_and_speed() {
        assert_eq!(session_key(Path::new("a.onnx"), 1.0), session_key(Path::new("a.onnx"), 1.0));
        assert_ne!(session_key(Path::new("a.onnx"), 1.0), session_key(Path::new("b.onnx"), 1.0), "another voice restarts");
        assert_ne!(session_key(Path::new("a.onnx"), 1.0), session_key(Path::new("a.onnx"), 2.0), "another speed restarts");
    }

    #[test]
    fn idle_time_is_measured_against_the_last_use() {
        assert!(!expired(Instant::now(), IDLE), "just used");
        assert!(expired(Instant::now() - Duration::from_secs(61), IDLE), "a minute and a bit unused");
    }

    #[test]
    fn a_session_answers_several_lines_from_one_process() {
        let mut s = echo_session(1);
        let cancel = Cancel::new();
        for n in 0..3 {
            let out = format!(r"C:\out\line-{n}.wav");
            s.request(&json_line("x", Path::new(&out)), &out, &cancel, Duration::from_secs(30)).unwrap();
        }
        assert!(s.child.try_wait().unwrap().is_none(), "still the same running process");
    }

    #[test]
    fn a_cancelled_line_returns_cancelled_at_once() {
        let mut s = echo_session(2);
        let cancel = Cancel::new();
        cancel.cancel();
        let r = s.request(&json_line("x", Path::new(r"C:\out\c.wav")), r"C:\out\c.wav", &cancel, Duration::from_secs(30));
        assert_eq!(r, Err(ProviderError::Cancelled));
    }

    #[test]
    fn an_idle_session_is_stopped_by_the_watcher() {
        let slot = Arc::new(Mutex::new(Some(echo_session(3))));
        watch(slot.clone(), 3, Duration::from_millis(300));
        std::thread::sleep(Duration::from_millis(1500));
        assert!(lock(&slot).is_none(), "stopped after its idle time");
    }

    #[test]
    fn health_needs_the_executable_and_synthesis_needs_an_installed_voice() {
        let models = temp("none");
        let tmp = temp("tmp2");
        let missing = PiperTts::new(PathBuf::from("does-not-exist.exe"), models.clone(), tmp.clone(), "t");
        assert!(matches!(missing.health(), Err(ProviderError::ModelLoad(_))));
        let req = SynthesisRequest { voice_id: "ghost".into(), model_id: None, text: "hi".into(), settings: SpeechSettings::default() };
        assert!(matches!(missing.synthesize(&req, &Cancel::new()), Err(ProviderError::Other(_))));
        std::fs::remove_dir_all(&models).ok();
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn only_complete_voices_are_listed_with_their_language() {
        let models = temp("voices");
        voice(&models, "en_US-lessac-medium", true);
        voice(&models, "half-downloaded", false);
        let p = PiperTts::new(PathBuf::from("piper.exe"), models.clone(), temp("tmp"), "t");
        let v = p.voices().unwrap();
        std::fs::remove_dir_all(&models).ok();
        assert_eq!(v.len(), 1, "a voice without its config is not offered");
        assert_eq!(v[0].id, "en_US-lessac-medium");
        assert_eq!(v[0].language, "en_US");
    }

    /// Runs the real Piper: three lines through one process. Set ZORUI_PIPER_EXE and ZORUI_PIPER_MODEL first.
    #[test]
    #[ignore = "needs a real Piper install: set ZORUI_PIPER_EXE and ZORUI_PIPER_MODEL"]
    fn real_piper_keeps_the_voice_loaded_between_lines() {
        let (Ok(exe), Ok(model)) = (std::env::var("ZORUI_PIPER_EXE"), std::env::var("ZORUI_PIPER_MODEL")) else { return };
        let model = PathBuf::from(model);
        let models = model.parent().unwrap().parent().unwrap().to_path_buf();
        let p = PiperTts::new(PathBuf::from(exe), models, temp("warm"), "warm");
        let voice = model.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let say = |text: &str| {
            let t = std::time::Instant::now();
            let r = p.synthesize(&SynthesisRequest { voice_id: voice.clone(), model_id: None, text: text.into(), settings: SpeechSettings::default() }, &Cancel::new()).unwrap();
            assert!(matches!(r.audio, AudioOutput::Pcm { .. }));
            t.elapsed()
        };
        let first = say("The first line loads the voice.");
        let second = say("The second line reuses the loaded voice.");
        let third = say("And the third line does too.");
        eprintln!("first {first:?}, second {second:?}, third {third:?}");
        assert!(second < first && third < first, "later lines skip the model load");
        assert!(p.running(), "still running between lines");
        p.shutdown();
        assert!(!p.running(), "shutdown stops it");
    }

    #[test]
    #[ignore = "needs a real Piper install: set ZORUI_PIPER_EXE and ZORUI_PIPER_MODEL"]
    fn real_piper_makes_pcm() {
        let (Ok(exe), Ok(model)) = (std::env::var("ZORUI_PIPER_EXE"), std::env::var("ZORUI_PIPER_MODEL")) else { return };
        let model = PathBuf::from(model);
        let models = model.parent().unwrap().parent().unwrap().to_path_buf();
        let p = PiperTts::new(PathBuf::from(exe), models, temp("real"), "real");
        let voice = model.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let r = p.synthesize(&SynthesisRequest { voice_id: voice, model_id: None, text: "Hello there.".into(), settings: SpeechSettings::default() }, &Cancel::new()).unwrap();
        match r.audio {
            AudioOutput::Pcm { samples, rate, .. } => { assert_eq!(rate, audio::INTERNAL_RATE); assert!(samples.len() > 1000); }
            other => panic!("expected PCM, got {other:?}"),
        }
    }
}
