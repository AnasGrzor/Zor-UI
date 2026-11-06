//! The helper commands (system, web, football, notifications, Wallpaper Engine, media, color picker, screen
//! translate, voice listening, toggles and switchwall). `proc` is how the shell calls them; `zorui.exe
//! --bridge <command>` and `--script <toggles|switchwall>` run them from a terminal. Each prints JSON.

use crate::conf::Config;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(crate) mod agent_hook;
mod claude_cli;
mod common;
mod football;
mod listen;
mod media;
mod sub_cli;
mod notify;
mod picker;
pub(crate) mod rec_audio;
mod screen_translate;
pub(crate) mod sys;
mod theme;
mod toggles;
pub(crate) mod update;
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

/// One-shot end4_bridge.py command `name`. None: not a command.
pub fn call(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    sys::call(cfg, name, args)
        .or_else(|| web::call(cfg, name, args))
        .or_else(|| football::call(cfg, name, args))
        .or_else(|| notify::call(cfg, name, args))
        .or_else(|| we::call(cfg, name, args))
        .or_else(|| media::call(cfg, name, args))
        .or_else(|| picker::call(cfg, name, args))
        .or_else(|| screen_translate::call(cfg, name, args))
}

type Streamer = fn(&Config, &str, &[String], &str, &Sink);

/// The module that streams `name` (a long-running bridge command), if any.
fn streamer(name: &str) -> Option<Streamer> {
    [(web::STREAMS, web::stream as Streamer), (notify::STREAMS, notify::stream), (we::STREAMS, we::stream), (media::STREAMS, media::stream), (listen::STREAMS, listen::stream), (rec_audio::STREAMS, rec_audio::stream)]
        .into_iter().find(|(names, _)| names.contains(&name)).map(|(_, f)| f)
}

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
    match script {
        "toggles" => toggles::run(cfg, args),
        "switchwall" => theme::switchwall(cfg, args),
        _ => None,
    }
}

/// `zorui.exe --bridge <command> [args]` / `--script <toggles|switchwall> [args]`: runs a command from the
/// command line (GlazeWM keybinds use this) and prints what the script would; an unknown command exits with 2. stdin feeds streaming commands such as ai-chat.
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
    eprintln!("unknown command {name}");
    2
}
