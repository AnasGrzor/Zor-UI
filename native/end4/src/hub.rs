//! State shared between the bar, popups and desk cards (timer, recording, agents, which monitor a popup
//! opens on...), kept in shared.json so it survives a restart.

use serde_json::{json, Map, Value};
use std::{
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

static STATE: OnceLock<Mutex<Map<String, Value>>> = OnceLock::new();

fn file() -> std::path::PathBuf { crate::conf::data_dir().join("shared.json") }

fn state() -> &'static Mutex<Map<String, Value>> {
    STATE.get_or_init(|| {
        let m = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Mutex::new(m)
    })
}

fn save(m: &Map<String, Value>) {
    let tmp = file().with_extension("tmp");
    if std::fs::write(&tmp, Value::Object(m.clone()).to_string()).is_ok() { let _ = std::fs::rename(&tmp, file()); }
}

pub fn now_ms() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64) }

/// localStorage.getItem(key).
pub fn get(key: &str) -> Option<String> { state().lock().ok()?.get(key).and_then(Value::as_str).map(String::from) }

/// localStorage.setItem / removeItem (None).
pub fn set(key: &str, v: Option<String>) {
    let Ok(mut m) = state().lock() else { return };
    match v { Some(v) => { m.insert(key.into(), Value::String(v)); } None => { m.remove(key); } }
    save(&m);
}

pub fn get_json(key: &str) -> Value { get(key).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null) }

// ------------------------------------------------------------------ pomodoro + stopwatch (lib/timer.js)

fn fresh() -> Value {
    json!({ "phase": "focus", "cycle": 0, "running": false, "endsAt": 0, "left": null, "sw": { "running": false, "start": 0, "acc": 0, "laps": [] } })
}

pub fn timer() -> Value {
    let mut t = fresh();
    if let (Some(o), Value::Object(saved)) = (t.as_object_mut(), get_json("zor-ui.timer")) { for (k, v) in saved { o.insert(k, v); } }
    t
}

fn minutes(cfg: &crate::conf::Config, phase: &str) -> f64 {
    cfg.get(&format!("timer.{phase}")).as_f64().unwrap_or(match phase { "short" => 5.0, "long" => 15.0, _ => 25.0 })
}

/// Seconds left in the pomodoro phase.
pub fn left(t: &Value, cfg: &crate::conf::Config) -> f64 {
    if t["running"].as_bool() == Some(true) { return ((t["endsAt"].as_f64().unwrap_or(0.0) - now_ms()) / 1000.0).max(0.0); }
    t["left"].as_f64().unwrap_or_else(|| minutes(cfg, t["phase"].as_str().unwrap_or("focus")) * 60.0)
}

pub fn sw_elapsed(t: &Value) -> f64 {
    let sw = &t["sw"];
    (sw["acc"].as_f64().unwrap_or(0.0) + if sw["running"].as_bool() == Some(true) { now_ms() - sw["start"].as_f64().unwrap_or(0.0) } else { 0.0 }) / 1000.0
}

/// Ends a finished phase and returns its name, else None (timer.tick).
pub fn tick(cfg: &crate::conf::Config) -> Option<String> {
    let mut t = timer();
    if t["running"].as_bool() != Some(true) || now_ms() < t["endsAt"].as_f64().unwrap_or(0.0) { return None; }
    let done = t["phase"].as_str().unwrap_or("focus").to_string();
    if done == "focus" {
        let cycle = t["cycle"].as_i64().unwrap_or(0) + 1;
        let cycles = cfg.get("timer.cycles").as_i64().filter(|&c| c > 0).unwrap_or(4);
        t["cycle"] = json!(cycle);
        t["phase"] = json!(if cycle % cycles == 0 { "long" } else { "short" });
    } else { t["phase"] = json!("focus"); }
    t["running"] = json!(false);
    t["left"] = Value::Null;
    set("zor-ui.timer", Some(t.to_string()));
    Some(done)
}

/// timer.clock(): "m:ss" or "h:mm:ss".
pub fn clock(sec: f64) -> String {
    let s = sec.max(0.0).floor() as u64;
    let (h, m, x) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{x:02}") } else { format!("{m}:{x:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clocks() {
        assert_eq!(clock(65.9), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-3.0), "0:00");
    }
}
