//! Rust ports of the pack's helper scripts: scripts/end4_bridge.py (+ football.py, end4_theme.py),
//! toggles.ps1 and switchwall.ps1. `proc` asks here first; a command no module claims (None) still
//! runs the script, so the port can land one command at a time. Each command prints the same JSON
//! as the script did, so callers don't change. `END4_PYTHON_BRIDGE=1` sends everything to the
//! scripts again (for comparing the two).

use crate::conf::Config;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

mod common;
mod football;
mod media;
mod notify;
mod sys;
mod theme;
mod toggles;
mod we;
mod web;

/// Where a streaming command writes its lines; `stopped()` turns true when the caller drops the stream.
pub struct Sink {
    line: Box<dyn Fn(&str) + Send + Sync>,
    stop: Arc<AtomicBool>,
}

impl Sink {
    pub fn new(line: impl Fn(&str) + Send + Sync + 'static, stop: Arc<AtomicBool>) -> Sink { Sink { line: Box::new(line), stop } }
    /// One JSON line. False once the caller has stopped listening (end the command then).
    pub fn emit(&self, v: &Value) -> bool {
        if self.stopped() { return false; }
        (self.line)(&v.to_string());
        true
    }
    /// One raw text line (the visualizer's "0.12 0.34 ..." levels, which aren't JSON).
    pub fn emit_line(&self, s: &str) -> bool {
        if self.stopped() { return false; }
        (self.line)(s);
        true
    }
    pub fn stopped(&self) -> bool { self.stop.load(Ordering::Relaxed) }
}

fn python_only() -> bool { std::env::var("END4_PYTHON_BRIDGE").is_ok_and(|v| v == "1") }

/// One-shot end4_bridge.py command `name`. None: not ported, run the script.
pub fn call(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    if python_only() { return None; }
    sys::call(cfg, name, args)
        .or_else(|| web::call(cfg, name, args))
        .or_else(|| football::call(cfg, name, args))
        .or_else(|| notify::call(cfg, name, args))
        .or_else(|| we::call(cfg, name, args))
        .or_else(|| media::call(cfg, name, args))
}

type Streamer = fn(&Config, &str, &[String], &str, &Sink);

/// The module that streams `name` (a long-running bridge command), if any.
fn streamer(name: &str) -> Option<Streamer> {
    if python_only() { return None; }
    [(web::STREAMS, web::stream as Streamer), (notify::STREAMS, notify::stream), (we::STREAMS, we::stream), (media::STREAMS, media::stream)]
        .into_iter().find(|(names, _)| names.contains(&name)).map(|(_, f)| f)
}

/// A Rust port handles streaming command `name`.
pub fn ported_stream(name: &str) -> bool { streamer(name).is_some() }

/// Starts streaming command `name` on a thread: `on_line` gets each line, `on_exit` runs when it ends.
/// None: not ported. Setting the returned flag asks the command to stop.
pub fn stream(cfg: &Config, name: &str, args: &[&str], stdin: &str, on_line: impl Fn(&str) + Send + Sync + 'static, on_exit: impl FnOnce() + Send + 'static) -> Option<Arc<AtomicBool>> {
    let f = streamer(name)?;
    let stop = Arc::new(AtomicBool::new(false));
    let (cfg, name, args, stdin, flag) = (cfg.clone(), name.to_string(), args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), stdin.to_string(), stop.clone());
    std::thread::spawn(move || {
        f(&cfg, &name, &args, &stdin, &Sink::new(on_line, flag));
        on_exit();
    });
    Some(stop)
}

/// PowerShell helper `script` (toggles / switchwall) with its -Name value arguments. None: not ported.
pub fn script(cfg: &Config, script: &str, args: &[&str]) -> Option<Value> {
    if python_only() { return None; }
    match script {
        "toggles" => toggles::run(cfg, args),
        // the port writes GlazeWM / Flow / Terminal configs and hasn't re-themed a live PC yet: opt in
        "switchwall" if Config::load().bool("native.theme") => theme::switchwall(cfg, args),
        _ => None,
    }
}

/// `end4.exe --bridge <command> [args]` / `--script <toggles|switchwall> [args]`: runs a command from the
/// command line (GlazeWM keybinds use this) and prints what the script would; anything not ported runs
/// the script itself. stdin feeds streaming commands such as ai-chat.
pub fn cli(kind: &str, args: &[String]) -> i32 {
    use std::io::{Read, Write};
    let cfg = Config::load();
    let Some((name, rest)) = args.split_first() else { eprintln!("usage: end4 {kind} <command> [args]"); return 2 };
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let print = |v: &Value| { let mut o = std::io::stdout(); let _ = writeln!(o, "{v}"); let _ = o.flush(); };
    if kind == "--script" {
        if !matches!(name.as_str(), "toggles" | "switchwall") { eprintln!("unknown script {name}"); return 2; }
        print(&crate::proc::run(&cfg, name, &rest));
        return 0;
    }
    if let Some(v) = call(&cfg, name, &rest) { print(&v); return 0; }
    if let Some(f) = streamer(name) {
        let mut stdin = String::new();
        if name == "ai-chat" { let _ = std::io::stdin().read_to_string(&mut stdin); }
        let args: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
        f(&cfg, name, &args, &stdin, &Sink::new(|l| { let mut o = std::io::stdout(); let _ = writeln!(o, "{l}"); let _ = o.flush(); }, Arc::new(AtomicBool::new(false))));
        return 0;
    }
    print(&crate::proc::bridge(&cfg, name, &rest)); // not ported: the Python script
    0
}
