//! How the shell calls the helper commands: toggles / switchwall (`run`), the bridge commands
//! (`bridge`, `stream`), all implemented in `crate::bridge`.

use crate::conf::Config;
use serde_json::Value;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

/// run() in core.js: toggles or switchwall with their -Name value arguments. Null for anything else.
pub fn run(cfg: &Config, script: &str, args: &[&str]) -> Value {
    crate::log::line(&format!("run {script} {}", args.join(" ")));
    let r = crate::bridge::script(cfg, script, args).unwrap_or(Value::Null);
    crate::log::line(&format!("run {script} -> {}", r["error"].as_str().map_or("ok".to_string(), |e| format!("error: {e}"))));
    r
}

/// Fire and forget, on a thread (clicks must never wait for the command).
pub fn run_bg(cfg: &Config, script: &str, args: &[&str]) {
    let (cfg, script, args) = (cfg.clone(), script.to_string(), args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    std::thread::spawn(move || {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        run(&cfg, &script, &a);
    });
}

/// bridge() in core.js: one bridge command, its JSON answer (Null for an unknown command).
pub fn bridge(cfg: &Config, name: &str, args: &[&str]) -> Value {
    crate::bridge::call(cfg, name, args).unwrap_or(Value::Null)
}

/// A running bridge stream; dropping it asks the command to stop.
pub struct Stream(Arc<AtomicBool>);
impl Drop for Stream {
    fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); }
}

/// bridgeStream() in core.js: a long-running bridge command; `on_line` gets each line (on its own thread).
pub fn stream(cfg: &Config, name: &str, args: &[&str], on_line: impl Fn(&str) + Send + Sync + 'static) -> Option<Stream> {
    crate::bridge::stream(cfg, name, args, "", on_line, || {}).map(Stream)
}

/// `stream` for commands that read a JSON conversation (ai-chat): `on_exit` runs once the command ends.
pub fn stream_stdin(cfg: &Config, name: &str, args: &[&str], stdin: &str, on_line: impl Fn(&str) + Send + Sync + 'static, on_exit: impl FnOnce() + Send + 'static) -> Option<Stream> {
    crate::bridge::stream(cfg, name, args, stdin, on_line, on_exit).map(Stream)
}
