//! GlazeWM over its IPC WebSocket (ws://127.0.0.1:6123), like Zebar's glazewm provider: subscribe
//! to every event and re-query the monitor tree, focus, binding modes and pause state.

use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ws { pub name: String, pub display: String, pub displayed: bool, pub occupied: bool }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Monitor { pub id: String, pub x: i32, pub y: i32, pub w: i32, pub focus: bool, pub workspaces: Vec<Ws> }

/// A tiled / floating window (overview, dock): `ws` is the workspace name it sits on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GWin { pub id: String, pub title: String, pub process: String, pub focus: bool, pub floating: bool, pub minimized: bool, pub ws: String }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wm {
    pub monitors: Vec<Monitor>,
    /// Every window on every workspace, in tree order.
    pub windows: Vec<GWin>,
    /// The workspace shown on the focused monitor.
    pub workspace: String,
    /// Focused window (title, process), None when a workspace (desktop) has focus.
    pub window: Option<(String, String)>,
    pub modes: Vec<(String, String)>,
    pub paused: bool,
}

impl Wm {
    /// The GlazeWM monitor a bar at physical x/y sits on.
    pub fn monitor_at(&self, x: i32, y: i32) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.x == x && m.y == y).or_else(|| self.monitors.iter().min_by_key(|m| (m.x - x).abs() + (m.y - y).abs()))
    }
}

static OUT: OnceLock<Mutex<Option<TcpStream>>> = OnceLock::new();

/// Runs a GlazeWM command ("focus --workspace 3").
pub fn command(c: &str) {
    if let Some(Ok(mut s)) = OUT.get().map(|m| m.lock()) {
        if let Some(st) = s.as_mut() { if send(st, &format!("command {c}")).is_err() { *s = None; } }
    }
}

/// GlazeWM's config with workspaces 1..=`main` bound to the monitor workspace 1 is bound to and the
/// rest to the monitor workspace 10 is bound to. None when the config has no such pair, or both
/// are on one monitor (single screen), or nothing would change.
pub fn bind_split(yaml: &str, main: u32) -> Option<String> {
    let ws_of = |l: &str| l.trim().strip_prefix("- name:").map(|n| n.trim().trim_matches(|c| c == '\'' || c == '"').parse::<u32>().ok());
    let bound = |l: &str| l.trim().strip_prefix("bind_to_monitor:").map(|v| v.trim().to_string());
    let (mut cur, mut first, mut last) = (None, None, None);
    for l in yaml.lines() {
        if let Some(n) = ws_of(l) { cur = n; }
        if let (Some(n), Some(v)) = (cur, bound(l)) {
            if n == 1 { first = Some(v.clone()); }
            if n == 10 { last = Some(v); }
        }
    }
    let (first, last) = (first?, last?);
    if first == last { return None; }
    let mut cur = None;
    let out: String = yaml.split_inclusive('\n').map(|l| {
        if let Some(n) = ws_of(l) { cur = n; }
        match (cur, bound(l)) {
            (Some(n), Some(_)) => {
                let indent = &l[..l.len() - l.trim_start().len()];
                let eol = &l[l.trim_end().len()..];
                format!("{indent}bind_to_monitor: {}{eol}", if n <= main { &first } else { &last })
            }
            _ => l.to_string(),
        }
    }).collect();
    (out != yaml).then_some(out)
}

/// Rebinds the workspaces in the user's GlazeWM config so `main` of them sit on the main monitor, and reloads it.
pub fn apply_split(main: u32) {
    let path = crate::conf::home().join(r".glzr\glazewm\config.yaml");
    let Ok(old) = std::fs::read_to_string(&path) else { return };
    let Some(new) = bind_split(&old, main) else { return };
    if std::fs::write(&path, new).is_ok() { command("wm-reload-config"); }
}

/// The same on one container ("close" or "move --workspace 3" for window `id`, "focus" to focus it).
/// shortcut: `command --id <id> <cmd>` is GlazeWM's CLI syntax; not tried against a live GlazeWM.
pub fn command_on(id: &str, c: &str) { command(&format!("--id {id} {c}")) }

/// "focus --container-id" only switches the workspace for a minimized window; set-tiling restores it.
pub fn focus_cmds(id: &str, minimized: bool) -> Vec<String> {
    let mut v = vec![format!("focus --container-id {id}")];
    if minimized { v.push(format!("--id {id} set-tiling")); }
    v
}

pub fn focus(id: &str, minimized: bool) { for c in focus_cmds(id, minimized) { command(&c); } }

#[cfg(test)]
mod focus_tests {
    #[test]
    fn restores_minimized_only() {
        assert_eq!(super::focus_cmds("a", false), vec!["focus --container-id a"]);
        assert_eq!(super::focus_cmds("a", true), vec!["focus --container-id a", "--id a set-tiling"]);
    }
}

fn send(s: &mut TcpStream, text: &str) -> std::io::Result<()> {
    // client frames are masked (RFC 6455); a zero mask leaves the bytes as they are
    let p = text.as_bytes();
    let mut f = vec![0x81u8];
    match p.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n if n < 65536 => { f.push(0x80 | 126); f.extend_from_slice(&(n as u16).to_be_bytes()); }
        n => { f.push(0x80 | 127); f.extend_from_slice(&(n as u64).to_be_bytes()); }
    }
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(p);
    s.write_all(&f)
}

/// One whole message (text frames joined); answers pings. None when the socket closes.
fn read_message(r: &mut impl Read, w: &mut TcpStream) -> Option<String> {
    let mut msg = Vec::new();
    loop {
        let mut h = [0u8; 2];
        r.read_exact(&mut h).ok()?;
        let (fin, op) = (h[0] & 0x80 != 0, h[0] & 0x0F);
        let mut len = (h[1] & 0x7F) as u64;
        if len == 126 { let mut b = [0u8; 2]; r.read_exact(&mut b).ok()?; len = u16::from_be_bytes(b) as u64; }
        if len == 127 { let mut b = [0u8; 8]; r.read_exact(&mut b).ok()?; len = u64::from_be_bytes(b); }
        let mask = if h[1] & 0x80 != 0 { let mut m = [0u8; 4]; r.read_exact(&mut m).ok()?; Some(m) } else { None };
        if len > 64 << 20 { return None; }
        let mut p = vec![0u8; len as usize];
        r.read_exact(&mut p).ok()?;
        if let Some(m) = mask { for (i, b) in p.iter_mut().enumerate() { *b ^= m[i % 4]; } }
        match op {
            8 => return None,
            9 => { let mut f = vec![0x8A, 0x80 | p.len().min(125) as u8, 0, 0, 0, 0]; f.extend_from_slice(&p[..p.len().min(125)]); w.write_all(&f).ok()?; }
            0 | 1 => { msg.extend_from_slice(&p); if fin { return Some(String::from_utf8_lossy(&msg).into_owned()); } }
            _ => {}
        }
    }
}

const QUERIES: [&str; 4] = ["query monitors", "query focused", "query binding-modes", "query paused"];

/// Connects (and reconnects) on a thread; `on` gets the new state after every change.
pub fn start(on: impl Fn(Wm) + Send + 'static) {
    let _ = OUT.set(Mutex::new(None));
    std::thread::spawn(move || loop {
        if let Err(()) = session(&on) { std::thread::sleep(Duration::from_secs(2)); }
    });
}

fn session(on: &impl Fn(Wm)) -> Result<(), ()> {
    let mut s = TcpStream::connect(("127.0.0.1", 6123)).map_err(|_| ())?;
    s.set_nodelay(true).ok();
    s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1:6123\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ZW5kNC1wQy1uYXRpdmUhIQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").map_err(|_| ())?;
    let mut r = BufReader::new(s.try_clone().map_err(|_| ())?);
    let mut line = String::new();
    loop { line.clear(); if r.read_line(&mut line).map_err(|_| ())? == 0 { return Err(()); } if line == "\r\n" { break; } }
    *OUT.get().ok_or(())?.lock().map_err(|_| ())? = s.try_clone().ok();
    send(&mut s, "sub --events all").map_err(|_| ())?;
    for q in QUERIES { send(&mut s, q).map_err(|_| ())?; }
    let mut wm = Wm::default();
    while let Some(m) = read_message(&mut r, &mut s) {
        let Ok(v) = serde_json::from_str::<Value>(&m) else { continue };
        if v["messageType"] == "event_subscription" {
            for q in QUERIES { send(&mut s, q).map_err(|_| ())?; }
            continue;
        }
        let before = wm.clone();
        apply(&mut wm, v["clientMessage"].as_str().unwrap_or(""), &v["data"]);
        if wm != before { on(wm.clone()); }
    }
    Err(())
}

fn apply(wm: &mut Wm, query: &str, d: &Value) {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    match query {
        "query monitors" => {
            wm.windows.clear();
            for w in d["monitors"].as_array().into_iter().flatten().flat_map(|m| m["children"].as_array()).flatten() { collect(w, &s(&w["name"]), &mut wm.windows); }
            wm.monitors = monitors(d);
            wm.workspace = wm.monitors.iter().find(|m| m.focus).and_then(|m| m.workspaces.iter().find(|w| w.displayed)).map(|w| w.name.clone()).unwrap_or_default();
        }
        "query focused" => {
            let f = &d["focused"];
            wm.window = (f["type"] == "window").then(|| (s(&f["title"]), s(&f["processName"])));
        }
        "query binding-modes" => wm.modes = d["bindingModes"].as_array().map(|a| a.iter().map(|m| (s(&m["name"]),
            m["displayName"].as_str().map(String::from).unwrap_or_else(|| s(&m["name"])))).collect()).unwrap_or_default(),
        "query paused" => wm.paused = d["paused"].as_bool().or_else(|| d.as_bool()).unwrap_or(false),
        _ => {}
    }
}

/// Windows under `v` (a workspace or a split container, which nest).
fn collect(v: &Value, ws: &str, out: &mut Vec<GWin>) {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    for c in v["children"].as_array().into_iter().flatten() {
        if c["type"] == "window" {
            out.push(GWin { id: s(&c["id"]), title: s(&c["title"]), process: s(&c["processName"]), focus: c["hasFocus"].as_bool() == Some(true),
                floating: c["state"]["type"] == "floating", minimized: c["state"]["type"] == "minimized", ws: ws.into() });
        } else { collect(c, ws, out); }
    }
}

fn monitors(d: &Value) -> Vec<Monitor> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    d["monitors"].as_array().map(|ms| ms.iter().map(|m| Monitor {
        id: s(&m["id"]), x: m["x"].as_i64().unwrap_or(0) as i32, y: m["y"].as_i64().unwrap_or(0) as i32,
        w: m["width"].as_i64().unwrap_or(0) as i32, focus: m["hasFocus"].as_bool().unwrap_or(false),
        workspaces: m["children"].as_array().map(|ws| ws.iter().map(|w| Ws {
            name: s(&w["name"]), display: w["displayName"].as_str().map(String::from).unwrap_or_else(|| s(&w["name"])),
            displayed: w["isDisplayed"].as_bool().unwrap_or(false),
            occupied: w["children"].as_array().is_some_and(|c| !c.is_empty()),
        }).collect()).unwrap_or_default(),
    }).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn rebinds_workspaces_between_monitors() {
        let ws = |m: &[&str]| (1..=10).map(|i| format!("  - name: '{i}'\r\n    bind_to_monitor: {}\r\n", m[(i > 5) as usize])).collect::<String>();
        let yaml = format!("workspaces:\r\n{}\r\nwindow_rules:\r\n", ws(&["1", "0"]));
        let out = super::bind_split(&yaml, 3).unwrap();
        let want = (1..=10).map(|i| format!("  - name: '{i}'\r\n    bind_to_monitor: {}\r\n", if i <= 3 { "1" } else { "0" })).collect::<String>();
        assert_eq!(out, format!("workspaces:\r\n{want}\r\nwindow_rules:\r\n"), "keeps CRLF and everything else");
        assert_eq!(super::bind_split(&yaml, 5), None, "already 5/5");
        assert_eq!(super::bind_split(&yaml.replace("bind_to_monitor: 0", "bind_to_monitor: 1"), 3), None, "one monitor");
        assert_eq!(super::bind_split("general:\n  x: 1\n", 3), None, "no workspaces");
        assert!(super::bind_split(&out, 9).unwrap().contains("- name: '9'\r\n    bind_to_monitor: 1"));
    }

    use super::*;
    #[test]
    fn parses_tree() {
        let mut wm = Wm::default();
        let d: Value = serde_json::from_str(r#"{"monitors":[{"id":"m1","x":0,"y":0,"width":1920,"hasFocus":true,
            "children":[{"name":"1","isDisplayed":true,"children":[{"type":"split","children":[{"type":"window","id":"w1","title":"A","processName":"code","hasFocus":true,"state":{"type":"floating"}}]}]},{"name":"2","displayName":"Web","children":[]}]}]}"#).unwrap();
        apply(&mut wm, "query monitors", &d);
        assert_eq!(wm.monitors[0].workspaces[0], Ws { name: "1".into(), display: "1".into(), displayed: true, occupied: true });
        assert_eq!(wm.monitors[0].workspaces[1].display, "Web");
        assert_eq!(wm.windows, vec![GWin { id: "w1".into(), title: "A".into(), process: "code".into(), focus: true, floating: true, minimized: false, ws: "1".into() }]);
        assert_eq!(wm.workspace, "1");
        apply(&mut wm, "query focused", &serde_json::json!({"focused": {"type": "window", "title": "T", "processName": "code"}}));
        assert_eq!(wm.window, Some(("T".into(), "code".into())));
        assert_eq!(wm.monitor_at(5, 0).map(|m| m.id.as_str()), Some("m1"));
    }
}
