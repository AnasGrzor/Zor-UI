//! Child processes: the pack's PowerShell helpers, the Python bridge (one-shot and streaming),
//! and Zebar widgets that aren't native yet (sidebars, overview, island panel).

use crate::conf::Config;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader},
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stderr(Stdio::null());
    c
}

fn last_json(out: &[u8]) -> Value {
    let text = String::from_utf8_lossy(out);
    text.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str(l).ok()).unwrap_or(Value::Null)
}

/// run() in core.js: scriptsDir\<script>.ps1 with args, in the background. Returns its JSON line.
pub fn run(cfg: &Config, script: &str, args: &[&str]) -> Value {
    let dir = cfg.str("scriptsDir");
    if dir.is_empty() { return Value::Null; }
    let file = format!("{dir}\\{script}.ps1");
    let mut c = cmd("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", &file]).args(args);
    c.output().map(|o| last_json(&o.stdout)).unwrap_or(Value::Null)
}

/// Fire and forget, on a thread (clicks must never wait for PowerShell).
pub fn run_bg(cfg: &Config, script: &str, args: &[&str]) {
    let (dir, script, args) = (cfg.str("scriptsDir").to_string(), script.to_string(), args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    std::thread::spawn(move || {
        let c = Config::parse(&serde_json::json!({ "scriptsDir": dir }).to_string());
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        run(&c, &script, &a);
    });
}

fn bridge_cmd(cfg: &Config, name: &str, args: &[&str]) -> Option<Command> {
    let (py, br) = (cfg.str("pythonPath"), cfg.str("bridgePath"));
    if py.is_empty() || br.is_empty() { return None; }
    let mut c = cmd(py);
    c.args(["-X", "utf8", br, name]).args(args);
    Some(c)
}

/// bridge() in core.js: one end4_bridge.py command, its JSON line.
pub fn bridge(cfg: &Config, name: &str, args: &[&str]) -> Value {
    bridge_cmd(cfg, name, args).and_then(|mut c| c.output().ok()).map(|o| last_json(&o.stdout)).unwrap_or(Value::Null)
}

/// bridgeStream() in core.js: a long-running bridge command; `on_line` gets each stdout line
/// (on a reader thread). The child is killed when the returned handle drops.
pub struct Stream(Child);
impl Drop for Stream { fn drop(&mut self) { let _ = self.0.kill(); } }

pub fn stream(cfg: &Config, name: &str, args: &[&str], on_line: impl Fn(&str) + Send + 'static) -> Option<Stream> {
    let mut child = bridge_cmd(cfg, name, args)?.stdout(Stdio::piped()).spawn().ok()?;
    let out = child.stdout.take()?;
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if !line.trim().is_empty() { on_line(&line); }
        }
    });
    Some(Stream(child))
}

/// Opens a Zebar widget the way openHere() does: a fresh copy with the preset's placement (Zebar
/// ignores starting a preset that's already open; the new copy tells the open one to close).
pub fn zebar_widget(name: &str) {
    let zpack: Value = std::fs::read_to_string(crate::conf::pack_dir().join("zpack.json")).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let Some(p) = zpack["widgets"].as_array().and_then(|w| w.iter().find(|w| w["name"] == name)).map(|w| &w["presets"][0]) else { return };
    let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
    let mt = p["monitorSelection"]["type"].as_str().unwrap_or("primary").to_string();
    let _ = cmd("zebar").args(["start-widget", "--pack", "end4-pC", "--widget-name", name, "--anchor", &s("anchor"),
        "--offset-x", &s("offsetX"), "--offset-y", &s("offsetY"), "--width", &s("width"), "--height", &s("height"), "--monitor-type", &mt])
        .stdout(Stdio::null()).spawn();
}

/// zebar.startWidgetPreset(name, 'default').
pub fn zebar_preset(name: &str) {
    let _ = cmd("zebar").args(["start-widget-preset", "--pack", "end4-pC", "--widget-name", name, "--preset", "default"]).stdout(Stdio::null()).spawn();
}


/// `stream` for commands that read one JSON line on stdin (ai-chat): writes `stdin`, closes it, and calls
/// `on_exit` (reader thread) once stdout ends. Dropping the handle kills the child.
pub fn stream_stdin(cfg: &Config, name: &str, args: &[&str], stdin: &str, on_line: impl Fn(&str) + Send + 'static, on_exit: impl FnOnce() + Send + 'static) -> Option<Stream> {
    use std::io::Write;
    let mut child = bridge_cmd(cfg, name, args)?.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    let mut input = child.stdin.take()?;
    let _ = writeln!(input, "{stdin}");
    drop(input);
    let out = child.stdout.take()?;
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if !line.trim().is_empty() { on_line(&line); }
        }
        on_exit();
    });
    Some(Stream(child))
}
