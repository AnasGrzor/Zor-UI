//! Settings' side effects: config-set writes (one at a time, in order), bridge / PowerShell calls on
//! threads, and the inbox that carries their results back to the view.

use super::Settings;
use crate::{app::Ctx, conf::Config, proc};
use serde_json::{json, Value};
use std::{
    os::windows::process::CommandExt,
    sync::{mpsc, Arc, Mutex, OnceLock},
};

type Fx = Box<dyn FnOnce(&mut Settings) + Send>;

/// Closures threads leave for the view; it runs them at the next paint or input.
#[derive(Clone, Default)]
pub struct Inbox(Arc<Mutex<Vec<Fx>>>);

impl Inbox {
    pub fn send(&self, f: impl FnOnce(&mut Settings) + Send + 'static) {
        if let Ok(mut q) = self.0.lock() { q.push(Box::new(f)); }
        crate::app::post(Box::new(|_| {})); // wakes the app so it repaints
    }
    pub fn take(&self) -> Vec<Fx> { self.0.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default() }
}

/// A Config that only carries what bridge / run need, to move into a thread.
pub fn bcfg(cx: &Ctx) -> Config {
    Config::parse(&json!({ "scriptsDir": cx.cfg.str("scriptsDir") }).to_string())
}

/// The message to toast when a bridge / script answer failed.
pub fn failure(r: &Value) -> Option<String> {
    if r.is_null() { return Some("No answer from the helper".into()); }
    r["error"].as_str().map(String::from).or_else(|| (r["ok"] == false).then(|| "Failed".into()))
}

type Write = (Config, String, String, Inbox);
static QUEUE: OnceLock<Mutex<mpsc::Sender<Write>>> = OnceLock::new();

/// bridge('config-set', path, JSON) like set() in settings.js. The bridge rewrites config.json read-modify-write,
/// so writes go through one worker thread, in click order.
pub fn config_set(cfg: Config, path: &str, v: &Value, inbox: &Inbox) {
    if cfg!(test) { return; } // tests must never write the real config.json
    let tx = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Write>();
        std::thread::spawn(move || {
            for (cfg, path, json, inbox) in rx {
                if let Some(e) = failure(&proc::bridge(&cfg, "config-set", &[&path, &json])) { inbox.send(move |s| s.toast(&e)); }
            }
        });
        Mutex::new(tx)
    });
    if let Ok(tx) = tx.lock() { let _ = tx.send((cfg, path.to_string(), v.to_string(), inbox.clone())); }
}

/// bridge(name, ...args) on a thread, then `then` on the view with the answer.
pub fn bridge(cx: &Ctx, inbox: &Inbox, name: &str, args: Vec<String>, then: impl FnOnce(&mut Settings, Value) + Send + 'static) {
    let (cfg, inbox, name) = (bcfg(cx), inbox.clone(), name.to_string());
    std::thread::spawn(move || {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::log::line(&format!("bridge {name} started"));
        let r = proc::bridge(&cfg, &name, &a);
        crate::log::line(&format!("bridge {name} done: {}", outcome(&r)));
        inbox.send(move |s| then(s, r));
    });
}

/// "ok", or the error text of a bridge / script answer, for the log.
fn outcome(r: &Value) -> String {
    match (r["ok"].as_bool(), r["error"].as_str()) { (_, Some(e)) => format!("error: {e}"), (Some(false), None) => "not ok".into(), _ => "ok".into() }
}

/// run(script, ...args) (PowerShell helper) on a thread, then `then` on the view.
pub fn script(cx: &Ctx, inbox: &Inbox, name: &str, args: Vec<String>, then: impl FnOnce(&mut Settings, Value) + Send + 'static) {
    let (cfg, inbox, name) = (bcfg(cx), inbox.clone(), name.to_string());
    std::thread::spawn(move || {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::log::line(&format!("script {name} {} started", a.join(" ")));
        let r = proc::run(&cfg, &name, &a);
        crate::log::line(&format!("script {name} done: {}", outcome(&r)));
        inbox.send(move |s| then(s, r));
    });
}

/// openUrl() in core.js: the default browser.
pub fn open_url(url: &str) {
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).creation_flags(0x0800_0000).spawn();
}

/// The GitHub page of a community preset folder.
pub fn preset_url(name: &str) -> String {
    let enc: String = name.bytes().map(|b| match b {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect();
    format!("https://github.com/pctrade/end4-pCpresets/tree/main/presets/{enc}")
}

/// A GET's bytes through curl.exe (util::http_get only returns text), for preset previews.
pub fn curl_bytes(url: &str) -> Option<Vec<u8>> {
    if !crate::util::web_url(url) { return None; }
    let out = std::process::Command::new("curl.exe").args(["-s", "-f", "-L", "--max-time", "20", "--", url]).creation_flags(0x0800_0000).output().ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preset_page_urls() {
        assert_eq!(preset_url("Zed"), "https://github.com/pctrade/end4-pCpresets/tree/main/presets/Zed");
        assert_eq!(preset_url("a b/ü"), "https://github.com/pctrade/end4-pCpresets/tree/main/presets/a%20b%2F%C3%BC");
    }
    #[test]
    fn failures() {
        assert_eq!(failure(&json!({ "ok": true })), None);
        assert_eq!(failure(&json!({ "ok": false })), Some("Failed".into()));
        assert_eq!(failure(&json!({ "error": "nope" })), Some("nope".into()));
        assert!(failure(&Value::Null).is_some());
    }
}
