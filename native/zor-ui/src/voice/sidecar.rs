//! A local sidecar: an executable the app starts and talks to over stdin and stdout, one JSON object per line.
//! It starts on first use, is restarted after a crash or cancel, and is asked to shut down on `shutdown`.
//!
//! Requests: {"id", "op": "health" | "voices" | "synthesize" | "clone" | "shutdown", ...}.
//! Replies:  {"id", "ok": true, "result": {...}} or {"id", "ok": false, "error": {"kind", "message"}}.
//! Synthesis writes a WAV to the "out" path the request names; the app reads it and deletes it.

use super::audio;
use super::contract::*;
use super::manifest::Manifest;
use super::{profile::new_id, wav};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

/// How long a sidecar may take to answer one request before it is treated as hung.
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// One connection to a sidecar process (or a stand-in for it in tests).
pub trait Transport: Send {
    /// Sends one request line and returns the reply line. Returns Cancelled when `cancel` is set while waiting.
    fn request(&mut self, line: &str, cancel: &Cancel) -> Result<String, ProviderError>;
}

pub type Launcher = Box<dyn Fn() -> Result<Box<dyn Transport>, ProviderError> + Send + Sync>;

pub struct ProcessTransport {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl ProcessTransport {
    pub fn spawn(exe: &Path, args: &[String], cwd: &Path) -> Result<ProcessTransport, ProviderError> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = Command::new(exe)
            .args(args)
            .current_dir(cwd)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn()
            .map_err(|e| ProviderError::ModelLoad(format!("could not start {}: {e}", exe.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| ProviderError::Other("no stdin on sidecar".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Other("no stdout on sidecar".into()))?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // Only JSON lines are replies; anything else the process prints is ignored.
                if line.trim_start().starts_with('{') && tx.send(line).is_err() { break; }
            }
        });
        Ok(ProcessTransport { child, stdin, lines })
    }
}

impl Transport for ProcessTransport {
    fn request(&mut self, line: &str, cancel: &Cancel) -> Result<String, ProviderError> {
        writeln!(self.stdin, "{line}").and_then(|_| self.stdin.flush())
            .map_err(|e| ProviderError::Network(format!("sidecar stopped accepting requests: {e}")))?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            if cancel.is_cancelled() {
                let _ = self.child.kill();
                return Err(ProviderError::Cancelled);
            }
            match self.lines.recv_timeout(Duration::from_millis(25)) {
                Ok(reply) => return Ok(reply),
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() > deadline => {
                    let _ = self.child.kill();
                    return Err(ProviderError::Network("sidecar did not reply in time".into()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(ProviderError::Network("sidecar closed its output".into())),
            }
        }
    }
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The launcher that starts a manifest's executable from its install folder.
pub fn process_launcher(dir: PathBuf, m: &Manifest) -> Launcher {
    let exe = dir.join(&m.command);
    let args = m.args.clone();
    Box::new(move || Ok(Box::new(ProcessTransport::spawn(&exe, &args, &dir)?) as Box<dyn Transport>))
}

pub struct SidecarProvider {
    id: &'static str,
    name: &'static str,
    version: &'static str,
    caps: ProviderCapabilities,
    launcher: Launcher,
    /// None until first use, and again after a crash, cancel or shutdown.
    running: Mutex<Option<Box<dyn Transport>>>,
    next_id: AtomicU64,
    /// Where synthesis output is written before it is read back and deleted.
    tmp: PathBuf,
}

impl SidecarProvider {
    /// shortcut: ids, names and versions are leaked so the trait can return 'static strings; one small string per installed provider, bounded.
    pub fn new(m: &Manifest, launcher: Launcher, tmp: PathBuf) -> SidecarProvider {
        SidecarProvider {
            id: Box::leak(m.id.clone().into_boxed_str()),
            name: Box::leak(m.name.clone().into_boxed_str()),
            version: Box::leak(m.version.clone().into_boxed_str()),
            caps: m.capabilities,
            launcher,
            running: Mutex::new(None),
            next_id: AtomicU64::new(1),
            tmp,
        }
    }

    /// One request and reply. Starts the sidecar if it is not running. Any transport failure drops the
    /// process, so the next call starts a fresh one.
    fn call(&self, op: &str, fields: Value, cancel: &Cancel) -> Result<Value, ProviderError> {
        let mut slot = self.running.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() { *slot = Some((self.launcher)()?); }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut req = json!({ "id": id, "op": op });
        if let (Value::Object(r), Value::Object(f)) = (&mut req, fields) { r.extend(f); }
        let transport = slot.as_mut().expect("started above");
        let line = match transport.request(&req.to_string(), cancel) {
            Ok(line) => line,
            Err(e) => { *slot = None; return Err(e); }
        };
        let reply: Value = serde_json::from_str(&line).map_err(|e| { *slot = None; ProviderError::Other(format!("bad reply from sidecar: {e}")) })?;
        if reply.get("id").and_then(Value::as_u64) != Some(id) {
            *slot = None;
            return Err(ProviderError::Other("sidecar answered out of order".into()));
        }
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(error_from(reply.get("error").unwrap_or(&Value::Null)))
    }
}

/// Maps a sidecar's error object to the app's error kinds.
fn error_from(e: &Value) -> ProviderError {
    let msg = e.get("message").and_then(Value::as_str).unwrap_or("sidecar error").to_string();
    match e.get("kind").and_then(Value::as_str) {
        Some("auth") => ProviderError::Auth(msg),
        Some("rate_limited") => ProviderError::RateLimited { retry_after_secs: e.get("retry_after").and_then(Value::as_u64) },
        Some("network") => ProviderError::Network(msg),
        Some("invalid_audio") => ProviderError::InvalidAudio(msg),
        Some("model_load") => ProviderError::ModelLoad(msg),
        _ => ProviderError::Other(msg),
    }
}

impl TtsProvider for SidecarProvider {
    fn id(&self) -> &'static str { self.id }
    fn name(&self) -> &'static str { self.name }
    fn kind(&self) -> ProviderKind { ProviderKind::Sidecar }
    fn version(&self) -> &'static str { self.version }
    fn capabilities(&self) -> ProviderCapabilities { self.caps }

    fn voices(&self) -> Result<Vec<VoiceInfo>, ProviderError> {
        let r = self.call("voices", json!({}), &Cancel::new())?;
        Ok(r.get("voices").and_then(Value::as_array).into_iter().flatten().filter_map(|v| Some(VoiceInfo {
            id: v.get("id")?.as_str()?.to_string(),
            name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            language: v.get("language").and_then(Value::as_str).unwrap_or_default().to_string(),
        })).collect())
    }

    fn health(&self) -> Result<(), ProviderError> {
        self.call("health", json!({}), &Cancel::new()).map(|_| ())
    }

    fn synthesize(&self, req: &SynthesisRequest, cancel: &Cancel) -> Result<SynthesisResult, ProviderError> {
        let out = self.tmp.join(format!("{}.wav", new_id()));
        let fields = json!({
            "voice": req.voice_id, "model": req.model_id, "text": req.text, "out": out.to_string_lossy(),
            "speed": req.settings.speed, "pitch": req.settings.pitch,
            "expressiveness": req.settings.expressiveness, "emotion": req.settings.emotion,
        });
        let reply = self.call("synthesize", fields, cancel);
        // The file is read and removed whether or not the call succeeded.
        let bytes = std::fs::read(&out);
        std::fs::remove_file(&out).ok();
        reply?;
        let bytes = bytes.map_err(|_| ProviderError::InvalidAudio("sidecar wrote no audio file".into()))?;
        let w = wav::parse(&bytes)?;
        let samples = audio::normalize(&w.samples, w.rate, w.channels)?;
        Ok(SynthesisResult { audio: AudioOutput::Pcm { samples, rate: audio::INTERNAL_RATE, channels: 1 } })
    }

    fn clone_voice(&self, req: &CloneVoiceRequest) -> Result<VoiceInfo, ProviderError> {
        if !self.caps.cloning { return Err(ProviderError::Unsupported("voice cloning")); }
        let r = self.call("clone", json!({
            "name": req.name, "reference": req.reference.path.to_string_lossy(), "transcript": req.reference.transcript,
        }), &Cancel::new())?;
        let id = r.get("id").and_then(Value::as_str).ok_or_else(|| ProviderError::Other("clone reply has no voice id".into()))?;
        Ok(VoiceInfo { id: id.into(), name: req.name.clone(), language: String::new() })
    }

    /// Asks the sidecar to exit, then drops the process. Does nothing if it was not running.
    fn shutdown(&self) {
        let mut slot = self.running.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(mut t) = slot.take() {
            let _ = t.request(&json!({ "id": 0, "op": "shutdown" }).to_string(), &Cancel::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest;
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    type Handler = Box<dyn FnMut(&Value) -> Value + Send>;

    /// A sidecar in memory. `crash` makes its first request fail as if the process died.
    struct Fake { handler: Handler, crash: bool }

    impl Transport for Fake {
        fn request(&mut self, line: &str, cancel: &Cancel) -> Result<String, ProviderError> {
            if cancel.is_cancelled() { return Err(ProviderError::Cancelled); }
            if std::mem::take(&mut self.crash) { return Err(ProviderError::Network("pipe closed".into())); }
            let req: Value = serde_json::from_str(line).unwrap();
            let mut reply = (self.handler)(&req);
            reply["id"] = req["id"].clone();
            Ok(reply.to_string())
        }
    }

    fn provider(handler: fn() -> Handler, launches: Arc<AtomicUsize>, crash_first: bool) -> SidecarProvider {
        let m = manifest::parse(&manifest::sample()).unwrap();
        let launcher: Launcher = Box::new(move || {
            let n = launches.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(Fake { handler: handler(), crash: crash_first && n == 0 }) as Box<dyn Transport>)
        });
        let tmp = std::env::temp_dir().join(format!("zorui-sidecar-test-{}", new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        SidecarProvider::new(&m, launcher, tmp)
    }

    fn ok_handler() -> Handler {
        Box::new(|req: &Value| match req["op"].as_str() {
            Some("voices") => json!({ "ok": true, "result": { "voices": [{ "id": "sam", "name": "Sam", "language": "en" }] } }),
            Some("synthesize") => {
                let out = req["out"].as_str().unwrap();
                std::fs::write(out, wav::encode(22_050, 1, &vec![0i16; 2205])).unwrap(); // 0.1 s
                json!({ "ok": true, "result": {} })
            }
            Some("fail") => json!({ "ok": false, "error": { "kind": "auth", "message": "bad key" } }),
            _ => json!({ "ok": true, "result": {} }),
        })
    }

    #[test]
    fn discovery_and_health_go_through_one_started_sidecar() {
        let launches = Arc::new(AtomicUsize::new(0));
        let p = provider(ok_handler, launches.clone(), false);
        p.health().unwrap();
        let voices = p.voices().unwrap();
        assert_eq!(voices[0].id, "sam");
        assert_eq!(launches.load(Ordering::Relaxed), 1, "started once and reused");
        assert_eq!(p.kind(), ProviderKind::Sidecar);
        assert!(p.capabilities().voice.speed, "capabilities come from the manifest");
    }

    #[test]
    fn synthesis_reads_the_written_wav_into_the_app_format_and_deletes_it() {
        let p = provider(ok_handler, Arc::new(AtomicUsize::new(0)), false);
        let r = p.synthesize(&SynthesisRequest { voice_id: "sam".into(), model_id: None, text: "hi".into(), settings: SpeechSettings::default() }, &Cancel::new()).unwrap();
        match r.audio {
            AudioOutput::Pcm { samples, rate, channels } => {
                assert_eq!((rate, channels), (audio::INTERNAL_RATE, 1));
                assert_eq!(samples.len(), 2400, "0.1 s at 22.05 kHz is 0.1 s at 24 kHz");
            }
            other => panic!("expected PCM, got {other:?}"),
        }
        assert_eq!(std::fs::read_dir(&p.tmp).unwrap().count(), 0, "temporary audio removed");
    }

    #[test]
    fn a_sidecar_error_keeps_its_kind() {
        let p = provider(ok_handler, Arc::new(AtomicUsize::new(0)), false);
        assert_eq!(p.call("fail", json!({}), &Cancel::new()), Err(ProviderError::Auth("bad key".into())));
    }

    #[test]
    fn a_crashed_sidecar_is_started_again_on_the_next_call() {
        let launches = Arc::new(AtomicUsize::new(0));
        let p = provider(ok_handler, launches.clone(), true);
        assert!(p.health().is_err(), "the first process dies");
        p.health().unwrap();
        assert_eq!(launches.load(Ordering::Relaxed), 2, "a fresh process replaced it");
    }

    #[test]
    fn a_cancelled_synthesis_reports_cancelled_and_leaves_no_file() {
        let p = provider(ok_handler, Arc::new(AtomicUsize::new(0)), false);
        let cancel = Cancel::new();
        cancel.cancel();
        let r = p.synthesize(&SynthesisRequest { voice_id: "sam".into(), model_id: None, text: "hi".into(), settings: SpeechSettings::default() }, &cancel);
        assert_eq!(r.unwrap_err(), ProviderError::Cancelled);
        assert_eq!(std::fs::read_dir(&p.tmp).unwrap().count(), 0);
    }

    #[test]
    fn shutdown_asks_the_sidecar_to_stop_and_the_next_call_starts_one_again() {
        let launches = Arc::new(AtomicUsize::new(0));
        let p = provider(ok_handler, launches.clone(), false);
        p.health().unwrap();
        p.shutdown();
        assert!(p.running.lock().unwrap().is_none());
        p.health().unwrap();
        assert_eq!(launches.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_real_process_round_trips_a_line() {
        // A PowerShell echo loop stands in for a sidecar: every line it reads, it writes back.
        let script = "while($l=[Console]::In.ReadLine()){[Console]::Out.WriteLine($l);[Console]::Out.Flush()}";
        let mut t = ProcessTransport::spawn(Path::new("powershell.exe"), &["-NoProfile".into(), "-Command".into(), script.into()], &std::env::temp_dir()).unwrap();
        let reply = t.request("{\"id\":7,\"op\":\"health\"}", &Cancel::new()).unwrap();
        assert_eq!(reply, "{\"id\":7,\"op\":\"health\"}");
    }
}
