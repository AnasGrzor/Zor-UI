//! Right sidebar data and logic with no drawing: what the background threads fetch, the quick-toggle
//! definitions, notification grouping, uptime and calendar maths, and the timer / to-do edits on hub state.

use crate::{app, conf::Config, hub, proc, util};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    os::windows::process::CommandExt,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex, MutexGuard},
    time::Duration,
};

#[derive(Clone, Default, Debug)]
pub struct Notif { pub id: String, pub app: String, pub title: String, pub body: String, pub time: f64 }

#[derive(Clone, Default, Debug)]
pub struct Mix { pub app: String, pub volume: f32, pub muted: bool }

/// Everything background threads fill in; the view clones it per paint.
#[derive(Clone, Default)]
pub struct Shared {
    /// Ids just dismissed: a load that started before the dismiss was saved must not bring them back.
    pub gone: Vec<String>,
    pub loaded: bool,
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
    pub caffeine: bool,
    pub brightness: Option<f32>,
    pub night: bool,
    pub night_err: String,
    pub plan_loaded: bool,
    pub plan: Option<String>,
    pub notifs: Vec<Notif>,
    pub notif_err: String,
    pub mixer: Vec<Mix>,
    pub mixer_open: bool,
    pub os: String,
    /// The default output device ("Speakers (Realtek)"); empty until read.
    pub out_name: String,
    pub busy: HashSet<String>,
}
pub type Sh = Arc<Mutex<Shared>>;

pub fn lock(sh: &Sh) -> MutexGuard<'_, Shared> { sh.lock().unwrap_or_else(|e| e.into_inner()) }

/// Asks the app for a repaint (a job that does nothing bumps the data version).
pub fn repaint() { app::post(Box::new(|_| {})); }

// ------------------------------------------------------------------ fetching

pub fn parse_notifs(v: &Value) -> (Vec<Notif>, String) {
    let s = |i: &Value, k: &str| i[k].as_str().unwrap_or("").to_string();
    let items: Vec<Notif> = v["items"].as_array().map(|a| a.iter().map(|i| Notif {
        id: s(i, "id"), app: s(i, "app"), title: s(i, "title"), body: s(i, "body"), time: i["time"].as_f64().unwrap_or(0.0),
    }).collect()).unwrap_or_default();
    let err = if items.is_empty() { v["error"].as_str().unwrap_or("").to_string() } else { String::new() };
    (items, err)
}

pub fn parse_mixer(v: &Value) -> Vec<Mix> {
    v["apps"].as_array().map(|a| a.iter().map(|m| Mix {
        app: m["app"].as_str().unwrap_or("").to_string(), volume: m["volume"].as_f64().unwrap_or(0.0) as f32, muted: m["muted"].as_bool().unwrap_or(false),
    }).collect()).unwrap_or_default()
}

pub fn parse_plan(v: &Value) -> Option<String> {
    v["plans"].as_array()?.iter().find(|p| p["active"].as_bool() == Some(true))?["name"].as_str().map(String::from)
}

/// Empties the list and dismisses exactly the ids that were showing. "all" would re-read Windows'
/// toast database first, and when that read fails or is slow nothing is saved and the list refills.
pub fn clear_notifs(cfg: Option<Arc<Config>>, sh: &Sh) {
    let ids: Vec<String> = { let mut s = lock(sh); let ids: Vec<String> = s.notifs.iter().map(|n| n.id.clone()).collect(); s.notifs.clear(); s.gone.extend(ids.iter().cloned()); ids };
    if ids.is_empty() { return; }
    if let Some(cfg) = cfg { std::thread::spawn(move || { let a: Vec<&str> = ids.iter().map(String::as_str).collect(); proc::bridge(&cfg, "notifications-dismiss", &a); }); }
}

pub fn load_notifs(cfg: &Config, sh: &Sh) {
    let (items, err) = parse_notifs(&proc::bridge(cfg, "notifications", &[]));
    let mut s = lock(sh);
    // forget ids the database no longer lists (the dismiss was saved); hide the rest
    let listed: Vec<String> = items.iter().map(|n| n.id.clone()).collect();
    s.gone.retain(|g| listed.contains(g));
    let mut items = items;
    items.retain(|n| !s.gone.contains(&n.id));
    s.notifs = items;
    s.notif_err = err;
}

pub fn load_mixer(cfg: &Config, sh: &Sh) { let m = parse_mixer(&proc::bridge(cfg, "mixer", &[])); lock(sh).mixer = m; }

/// Starts the one-off loads and the poller (notifications every 5 s, the mixer every 3 s while open).
pub fn start(cfg: Arc<Config>, sh: Sh, alive: Arc<AtomicBool>) {
    let (c, s) = (cfg.clone(), sh.clone());
    std::thread::spawn(move || {
        let r = proc::bridge(&c, "nightlight", &["status"]);
        lock(&s).out_name = crate::media::output_name().unwrap_or_default();
        lock(&s).night = r["on"].as_bool() == Some(true);
        let p = parse_plan(&proc::run(&c, "toggles", &["-Action", "powerplan"]));
        { let mut g = lock(&s); g.plan = p; g.plan_loaded = true; }
        repaint();
        let st = proc::run(&c, "toggles", &["-Action", "status", "-Monitor", "active"]);
        {
            let mut g = lock(&s);
            if st["error"].is_null() {
                g.wifi = st["wifi"].as_bool();
                g.bluetooth = st["bluetooth"].as_bool();
                g.caffeine = st["caffeine"].as_bool() == Some(true);
                g.brightness = st["brightness"].as_f64().map(|b| b as f32);
            }
            g.loaded = true;
        }
        repaint();
    });
    let s = sh.clone();
    std::thread::spawn(move || {
        let out = std::process::Command::new("reg").args(["query", r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion"])
            .creation_flags(0x0800_0000).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        lock(&s).os = os_name(&out);
        repaint();
    });
    std::thread::spawn(move || {
        let mut n = 0u32;
        while alive.load(Ordering::Relaxed) {
            if n % 10 == 0 { load_notifs(&cfg, &sh); repaint(); }
            if n % 6 == 0 && lock(&sh).mixer_open { load_mixer(&cfg, &sh); repaint(); }
            if n % 6 == 0 { let o = crate::media::output_name().unwrap_or_default(); if o != lock(&sh).out_name { lock(&sh).out_name = o; repaint(); } }
            std::thread::sleep(Duration::from_millis(500));
            n = n.wrapping_add(1);
        }
    });
}

// ------------------------------------------------------------------ header text

/// "Windows 11 Pro 24H2" from `reg query ...\CurrentVersion` output (ProductName still says Windows 10 on 11).
pub fn os_name(reg: &str) -> String {
    let val = |name: &str| reg.lines().find_map(|l| {
        l.trim().strip_prefix(name).filter(|r| r.starts_with(char::is_whitespace)).and_then(|r| r.split_once("REG_SZ")).map(|(_, v)| v.trim().to_string())
    }).unwrap_or_default();
    let (mut product, display) = (val("ProductName"), val("DisplayVersion"));
    if val("CurrentBuild").parse::<u32>().unwrap_or(0) >= 22000 { product = product.replacen("Windows 10", "Windows 11", 1); }
    format!("{product} {display}").trim().to_string()
}

/// "Up 3h 12m · Windows 11 Pro 24H2" (no hours under an hour).
pub fn uptime_text(ms: u64, os: &str) -> String {
    if ms == 0 { return String::new(); }
    let up = ms / 60_000;
    let (h, m) = (up / 60, up % 60);
    format!("Up {}{m}m · {os}", if h > 0 { format!("{h}h ") } else { String::new() })
}

// ------------------------------------------------------------------ quick toggles

pub struct Def { pub id: &'static str, pub icon: &'static str, pub label: &'static str, pub on: bool, pub sub: String, pub settings: &'static str }

pub struct DefIn<'a> {
    pub s: &'a Shared,
    pub audio: Option<crate::media::Audio>,
    pub ssid: &'a str,
    pub dark: bool,
    /// None: GlazeWM isn't running.
    pub paused: Option<bool>,
    pub kelvin: i64,
    pub target: &'a str,
    /// Unix ms when screen recording started, 0 if not recording.
    pub rec: f64,
    pub now: f64,
}

fn on_off(loaded: bool, v: Option<bool>) -> String {
    (if !loaded { "…" } else if v.is_none() { "Unavailable" } else if v == Some(true) { "On" } else { "Off" }).to_string()
}

/// toggleDefs() in sidebar-right.js.
pub fn defs(i: &DefIn) -> Vec<Def> {
    let s = i.s;
    let a = i.audio;
    let mic = a.and_then(|a| a.mic_muted);
    let plan = s.plan.as_deref().unwrap_or("").to_lowercase();
    let rec = i.rec > 0.0;
    let d = |id, icon, label, on, sub: String, settings| Def { id, icon, label, on, sub, settings };
    vec![
        d("wifi", if s.wifi == Some(false) { "signal_wifi_off" } else { "wifi" }, "Wi-Fi", s.wifi == Some(true),
            if s.wifi == Some(true) { if i.ssid.is_empty() { "On".into() } else { i.ssid.to_string() } } else { on_off(s.loaded, s.wifi) }, "network-wifi"),
        d("bluetooth", if s.bluetooth == Some(true) { "bluetooth" } else { "bluetooth_disabled" }, "Bluetooth", s.bluetooth == Some(true), on_off(s.loaded, s.bluetooth), "bluetooth"),
        d("sound", if a.is_some_and(|a| a.muted) { "volume_off" } else { "volume_up" }, "Sound", a.is_some_and(|a| !a.muted),
            match a { Some(a) if a.muted => "Muted".into(), Some(a) => format!("{}%", a.volume.round()), None => "No device".into() }, "sound"),
        d("output", if ["headphone", "headset"].iter().any(|k| s.out_name.to_lowercase().contains(k)) { "headphones" } else { "speaker" }, "Output", false,
            if s.out_name.is_empty() { "No device".into() } else { s.out_name.clone() }, "sound"),
        d("mic", if mic == Some(true) { "mic_off" } else { "mic" }, "Microphone", mic == Some(false),
            match mic { Some(true) => "Muted".into(), Some(false) => "Listening".into(), None => "No device".into() }, "sound"),
        d("dark", if i.dark { "dark_mode" } else { "light_mode" }, "Dark mode", i.dark, (if i.dark { "Dark" } else { "Light" }).into(), "personalization-colors"),
        d("caffeine", "coffee", "Keep awake", s.caffeine, (if s.caffeine { "Screen stays on" } else { "Off" }).into(), "powersleep"),
        d("tiling", "grid_view", "Tiling", i.paused == Some(false),
            match i.paused { Some(true) => "Paused", Some(false) => "GlazeWM", None => "Not running" }.into(), ""),
        d("nightlight", "nightlight", "Night light", s.night,
            if !s.night_err.is_empty() { s.night_err.clone() } else if s.night { format!("{}K", i.kelvin) } else { "Off".into() }, "nightlight"),
        d("snip", "screenshot_region", "Screen snip", false, "Region capture".into(), ""),
        d("osk", "keyboard", "Keyboard", false, "On-screen".into(), ""),
        d("gamemode", "sports_esports", "Game mode", false, "Stops the shell".into(), ""),
        d("power", if plan.contains("saver") { "energy_savings_leaf" } else if plan.contains("perform") { "bolt" } else { "balance" }, "Power plan",
            s.plan.is_some() && !plan.contains("saver"), if !s.plan_loaded { "…".into() } else { s.plan.clone().unwrap_or_else(|| "Unavailable".into()) }, "powersleep"),
        d("colorpick", "colorize", "Color picker", false, "Copies the hex".into(), ""),
        d("screentranslate", "translate", "Screen translate", false, format!("Into {}", i.target), ""),
        d("record", if rec { "stop_circle" } else { "screen_record" }, "Screen record", rec,
            if rec { format!("Recording {}", hub::clock((i.now - i.rec) / 1000.0)) } else { "Primary monitor".into() }, ""),
    ]
}

// ------------------------------------------------------------------ notifications

pub struct Group { pub app: String, pub items: Vec<Notif> }

/// Groups by app in order of first appearance.
pub fn group(n: &[Notif]) -> Vec<Group> {
    let mut g: Vec<Group> = vec![];
    for x in n {
        match g.iter_mut().find(|g| g.app == x.app) { Some(g) => g.items.push(x.clone()), None => g.push(Group { app: x.app.clone(), items: vec![x.clone()] }) }
    }
    g
}

/// "now", "5m", "3h" (Ok), or Err(days back) for a date. `since`: seconds ago; `sod`: seconds since local midnight.
pub fn ago(since: f64, sod: f64) -> Result<String, i64> {
    Ok(if since < 60.0 { "now".into() } else if since < 3600.0 { format!("{}m", (since / 60.0) as u64) } else if since < 86400.0 { format!("{}h", (since / 3600.0) as u64) }
       else { return Err((((since - sod).max(0.0) + 86399.0) / 86400.0) as i64) })
}

pub fn ago_text(time: f64) -> String {
    let now = util::now();
    let sod = now.wHour as f64 * 3600.0 + now.wMinute as f64 * 60.0 + now.wSecond as f64;
    match ago(hub::now_ms() / 1000.0 - time, sod) {
        Ok(s) => s,
        Err(days) => util::date_fmt(&util::add_days(&now, -days), "d MMM"),
    }
}

// ------------------------------------------------------------------ calendar

#[derive(Debug, PartialEq)]
pub struct Cell { pub d: u32, pub out: bool, pub today: bool }

/// Monday = 0 (Sakamoto's day-of-week).
fn monday_index(y: i32, m: u32, d: u32) -> u32 {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    (((y + y / 4 - y / 100 + y / 400 + t[m as usize - 1] + d as i32) % 7) as u32 + 6) % 7
}

/// monthGrid() in core.js: whole weeks, Monday first. `today` = (year, month, day).
pub fn month_grid(y: i32, m: u32, today: (i32, u32, u32)) -> Vec<Cell> {
    let offset = monday_index(y, m, 1);
    let days = util::days_in_month(y as u16, m as u16) as u32;
    let prev = if m == 1 { util::days_in_month((y - 1) as u16, 12) } else { util::days_in_month(y as u16, m as u16 - 1) } as u32;
    let mut cells: Vec<Cell> = (0..offset).rev().map(|i| Cell { d: prev - i, out: true, today: false }).collect();
    cells.extend((1..=days).map(|d| Cell { d, out: false, today: (y, m, d) == today }));
    while cells.len() % 7 != 0 { let d = cells.len() as u32 - offset - days + 1; cells.push(Cell { d, out: true, today: false }); }
    cells
}

pub fn shift_month((y, m): (i32, u32), by: i32) -> (i32, u32) {
    let i = y * 12 + m as i32 - 1 + by;
    (i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
}

// ------------------------------------------------------------------ timer (lib/timer.js on the hub's copy)

pub fn phase_name(p: &str) -> &'static str { match p { "short" => "Short break", "long" => "Long break", _ => "Focus" } }

fn minutes(cfg: &Config, phase: &str) -> f64 {
    cfg.get(&format!("timer.{phase}")).as_f64().unwrap_or(match phase { "short" => 5.0, "long" => 15.0, _ => 25.0 })
}
pub fn cycles(cfg: &Config) -> i64 { cfg.get("timer.cycles").as_i64().filter(|&c| c > 0).unwrap_or(4) }
pub fn total(t: &Value, cfg: &Config) -> f64 { minutes(cfg, t["phase"].as_str().unwrap_or("focus")) * 60.0 }

pub fn left_at(t: &Value, cfg: &Config, now: f64) -> f64 {
    if t["running"].as_bool() == Some(true) { return ((t["endsAt"].as_f64().unwrap_or(0.0) - now) / 1000.0).max(0.0); }
    t["left"].as_f64().unwrap_or_else(|| total(t, cfg))
}

/// "Focus · 1/4" under the dial.
pub fn pomo_label(t: &Value, cfg: &Config) -> String {
    let cycle = t["cycle"].as_i64().unwrap_or(0);
    let n = cycles(cfg);
    let c = if t["phase"].as_str() == Some("focus") { cycle } else { cycle - 1 };
    format!("{} · {}/{n}", phase_name(t["phase"].as_str().unwrap_or("focus")), c.rem_euclid(n) + 1)
}

pub fn pomo_toggle(t: &mut Value, cfg: &Config, now: f64) {
    let left = left_at(t, cfg, now);
    if t["running"].as_bool() == Some(true) { t["left"] = json!(left); t["running"] = json!(false); }
    else { t["endsAt"] = json!(now + left * 1000.0); t["left"] = Value::Null; t["running"] = json!(true); }
}

pub fn pomo_reset(t: &mut Value) {
    let sw = t["sw"].clone();
    *t = json!({ "phase": "focus", "cycle": 0, "running": false, "endsAt": 0, "left": null, "sw": sw });
}

pub fn pomo_skip(t: &mut Value, cfg: &Config) {
    if t["phase"].as_str() == Some("focus") {
        let cycle = t["cycle"].as_i64().unwrap_or(0) + 1;
        t["cycle"] = json!(cycle);
        t["phase"] = json!(if cycle % cycles(cfg) == 0 { "long" } else { "short" });
    } else { t["phase"] = json!("focus"); }
    t["running"] = json!(false);
    t["left"] = Value::Null;
}

pub fn sw_toggle(t: &mut Value, now: f64) {
    let sw = &mut t["sw"];
    if sw["running"].as_bool() == Some(true) { sw["acc"] = json!(sw["acc"].as_f64().unwrap_or(0.0) + now - sw["start"].as_f64().unwrap_or(now)); sw["running"] = json!(false); }
    else { sw["start"] = json!(now); sw["running"] = json!(true); }
}

pub fn sw_elapsed_at(t: &Value, now: f64) -> f64 {
    let sw = &t["sw"];
    (sw["acc"].as_f64().unwrap_or(0.0) + if sw["running"].as_bool() == Some(true) { now - sw["start"].as_f64().unwrap_or(now) } else { 0.0 }) / 1000.0
}

pub fn sw_lap(t: &mut Value, now: f64) {
    if t["sw"]["running"].as_bool() != Some(true) { return; }
    let e = sw_elapsed_at(t, now);
    if let Some(l) = t["sw"]["laps"].as_array_mut() { l.insert(0, json!(e)); } else { t["sw"]["laps"] = json!([e]); }
}

pub fn sw_reset(t: &mut Value) { t["sw"] = json!({ "running": false, "start": 0, "acc": 0, "laps": [] }); }

/// "1:05.3" for the stopwatch and laps.
pub fn sw_text(sec: f64) -> String { format!("{}.{}", hub::clock(sec), ((sec % 1.0) * 10.0) as u32) }

/// Applies `f` to the shared timer state and writes it back.
pub fn edit_timer(f: impl FnOnce(&mut Value)) {
    let mut t = hub::timer();
    f(&mut t);
    hub::set("zor-ui.timer", Some(t.to_string()));
}

// ------------------------------------------------------------------ to-do

pub fn todos() -> Vec<Value> { hub::get_json("zor-ui.todo").as_array().cloned().unwrap_or_default() }
pub fn save_todos(t: &[Value]) { hub::set("zor-ui.todo", Some(Value::Array(t.to_vec()).to_string())); }

#[cfg(test)]
mod tests {
    #[test]
    fn a_stale_load_cannot_refill_cleared_notifications() {
        let sh = Sh::default();
        let n = |id: &str| Notif { id: id.into(), app: "a".into(), title: "t".into(), body: String::new(), time: 0.0 };
        lock(&sh).notifs = vec![n("1"), n("2")];
        super::clear_notifs(None, &sh);
        assert_eq!(lock(&sh).gone, ["1", "2"]);
        // what load_notifs does with a read that still lists them
        let mut s = lock(&sh);
        let mut items = vec![n("1"), n("2"), n("3")];
        items.retain(|x| !s.gone.contains(&x.id));
        assert_eq!(items.len(), 1);
        s.gone.retain(|g| ["3"].contains(&g.as_str()));
        assert!(s.gone.is_empty());
    }

    #[test]
    fn clear_empties_the_list_without_a_config() {
        let sh = Sh::default();
        lock(&sh).notifs = vec![Notif { id: "1".into(), app: "a".into(), title: "t".into(), body: String::new(), time: 0.0 }];
        super::clear_notifs(None, &sh);
        assert!(lock(&sh).notifs.is_empty());
    }
    use super::*;

    fn cfg() -> Config { Config::parse("{}") }
    fn input(s: &Shared) -> DefIn<'_> {
        DefIn { s, audio: None, ssid: "", dark: true, paused: Some(false), kelvin: 4500, target: "en", rec: 0.0, now: 0.0 }
    }
    fn sub(d: &[Def], id: &str) -> String { d.iter().find(|d| d.id == id).unwrap().sub.clone() }

    #[test]
    fn toggle_subtitles() {
        let mut s = Shared::default();
        let d = defs(&input(&s));
        assert_eq!(d.len(), 16);
        assert_eq!(sub(&d, "output"), "No device");
        assert_eq!(sub(&d, "gamemode"), "Stops the shell");
        assert_eq!((sub(&d, "wifi"), sub(&d, "power"), sub(&d, "sound"), sub(&d, "tiling")), ("…".into(), "…".into(), "No device".into(), "GlazeWM".into()));
        s.loaded = true; s.plan_loaded = true; s.wifi = Some(true); s.bluetooth = None; s.night = true; s.plan = Some("Power saver".into());
        let mut i = input(&s);
        i.ssid = "Home"; i.audio = Some(crate::media::Audio { volume: 61.6, muted: false, mic_muted: Some(true), mic_volume: None }); i.paused = Some(true); i.rec = 1000.0; i.now = 66_000.0;
        let d = defs(&i);
        assert_eq!((sub(&d, "wifi"), sub(&d, "bluetooth"), sub(&d, "sound"), sub(&d, "mic")), ("Home".into(), "Unavailable".into(), "62%".into(), "Muted".into()));
        assert_eq!((sub(&d, "tiling"), sub(&d, "nightlight"), sub(&d, "record"), sub(&d, "screentranslate")), ("Paused".into(), "4500K".into(), "Recording 1:05".into(), "Into en".into()));
        let p = d.iter().find(|d| d.id == "power").unwrap();
        assert_eq!((p.icon, p.on, p.sub.as_str()), ("energy_savings_leaf", false, "Power saver"));
        assert!(d.iter().find(|d| d.id == "record").unwrap().on);
    }

    #[test]
    fn notification_grouping() {
        let n = |id: &str, app: &str| Notif { id: id.into(), app: app.into(), ..Default::default() };
        let g = group(&[n("1", "Mail"), n("2", "Chat"), n("3", "Mail")]);
        assert_eq!(g.iter().map(|g| (g.app.as_str(), g.items.len())).collect::<Vec<_>>(), [("Mail", 2), ("Chat", 1)]);
        let (items, err) = parse_notifs(&json!({ "items": [{ "id": "7", "app": "A", "title": "T", "body": "", "time": 5 }] }));
        assert_eq!((items[0].id.as_str(), items[0].time, err.as_str()), ("7", 5.0, ""));
        assert_eq!(parse_notifs(&json!({ "error": "no db" })).1, "no db");
        assert_eq!(ago(30.0, 0.0), Ok("now".into()));
        assert_eq!(ago(300.0, 0.0), Ok("5m".into()));
        assert_eq!(ago(7300.0, 0.0), Ok("2h".into()));
        assert_eq!(ago(90_000.0, 10_000.0), Err(1)); // 25 h ago, 2.8 h into today: yesterday
        assert_eq!(ago(90_000.0, 80_000.0), Err(1));
        assert_eq!(ago(200_000.0, 3_600.0), Err(3));
    }

    #[test]
    fn uptime_and_os() {
        assert_eq!(uptime_text(3 * 3_600_000 + 12 * 60_000, "Windows 11 Pro"), "Up 3h 12m · Windows 11 Pro");
        assert_eq!(uptime_text(5 * 60_000, "X"), "Up 5m · X");
        assert_eq!(uptime_text(0, "X"), "");
        let reg = "    CurrentBuild    REG_SZ    26200\r\n    DisplayVersion    REG_SZ    25H2\r\n    ProductName    REG_SZ    Windows 10 Pro\r\n";
        assert_eq!(os_name(reg), "Windows 11 Pro 25H2");
    }

    #[test]
    fn calendar() {
        assert_eq!(monday_index(2026, 11, 4), 2); // a Wednesday
        let g = month_grid(2026, 11, (2026, 11, 4));
        assert_eq!(g.len() % 7, 0);
        assert_eq!((g[0].d, g[0].out), (26, true)); // Mon 26 Oct
        assert_eq!(g.iter().filter(|c| c.today).count(), 1);
        assert_eq!(g.iter().find(|c| c.today).unwrap().d, 4);
        assert_eq!(g.last().map(|c| (c.d, c.out)), Some((6, true))); // Sun 6 Dec
        assert_eq!(shift_month((2026, 1), -1), (2025, 12));
        assert_eq!(shift_month((2026, 12), 1), (2027, 1));
    }

    #[test]
    fn pomodoro_and_stopwatch() {
        let c = cfg();
        let mut t = json!({ "phase": "focus", "cycle": 0, "running": false, "endsAt": 0, "left": null, "sw": { "running": false, "start": 0, "acc": 0, "laps": [] } });
        assert_eq!(left_at(&t, &c, 0.0), 1500.0);
        assert_eq!(pomo_label(&t, &c), "Focus · 1/4");
        pomo_toggle(&mut t, &c, 1000.0);
        assert_eq!(left_at(&t, &c, 11_000.0), 1490.0);
        pomo_toggle(&mut t, &c, 11_000.0);
        assert_eq!((t["running"].as_bool(), t["left"].as_f64()), (Some(false), Some(1490.0)));
        pomo_skip(&mut t, &c);
        assert_eq!((t["phase"].as_str(), t["cycle"].as_i64()), (Some("short"), Some(1)));
        assert_eq!(pomo_label(&t, &c), "Short break · 1/4");
        assert_eq!(left_at(&t, &c, 0.0), 300.0);
        t["cycle"] = json!(3);
        t["phase"] = json!("focus");
        pomo_skip(&mut t, &c);
        assert_eq!(t["phase"].as_str(), Some("long"));
        sw_toggle(&mut t, 5000.0);
        sw_lap(&mut t, 7500.0);
        sw_toggle(&mut t, 9000.0);
        assert_eq!(sw_elapsed_at(&t, 99_999.0), 4.0);
        assert_eq!(t["sw"]["laps"], json!([2.5]));
        pomo_reset(&mut t);
        assert_eq!(t["cycle"], json!(0));
        assert_eq!(t["sw"]["laps"], json!([2.5])); // reset keeps the stopwatch
        sw_reset(&mut t);
        assert_eq!(sw_text(65.37), "1:05.3");
        assert_eq!(sw_text(0.0), "0:00.0");
    }
}
