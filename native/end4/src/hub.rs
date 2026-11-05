//! State the Zebar widgets keep in localStorage and the native bar needs too (timer, recording,
//! agents, which monitor a popup opens on...). The native app holds the copy everyone shares:
//! core.js mirrors these keys over ws://127.0.0.1:6133/state and gets every change pushed back.

use serde_json::{json, Map, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Same list as SHARED in lib/core.js.
pub const KEYS: &[&str] = &["zor-ui.timer", "zor-ui.rec", "zor-ui.timerDone", "end4.agents", "end4.openOn",
    "end4.islandAt", "zor-ui.rightTab", "zor-ui.todo", "zor-ui.notes", "end4.closedAt.island-panel", "end4.closedAt.overview",
    "end4.closedAt.sidebar-left", "end4.closedAt.sidebar-right", "end4.closedAt.settings"];
pub const PORT: u16 = 6133;

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

/// localStorage.setItem / removeItem (None). Open widgets get the change pushed.
pub fn set(key: &str, v: Option<String>) {
    let Ok(mut m) = state().lock() else { return };
    match v { Some(v) => { m.insert(key.into(), Value::String(v)); } None => { m.remove(key); } }
    save(&m);
    drop(m);
    broadcast();
}

pub fn get_json(key: &str) -> Value { get(key).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null) }

/// Origins of Zebar's widget pages. Browsers send Origin on WebSocket and cross-site requests, so
/// web pages can't read agent messages or move the timer; local programs (no Origin) could anyway.
fn allowed(origin: &str) -> bool {
    ["http://127.0.0.1:6124", "http://localhost:6124", "http://asset.localhost", "http://tauri.localhost", "tauri://localhost"]
        .iter().any(|o| origin.eq_ignore_ascii_case(o))
}

/// Widgets connected over WebSocket; each gets the whole state after every change.
/// Each socket sits behind its own lock so a ping reply never lands in the middle of a state frame.
type Sub = Arc<Mutex<TcpStream>>;
static SUBS: Mutex<Vec<Sub>> = Mutex::new(Vec::new());
const MAX_SUBS: usize = 32;
const MAX_MSG: usize = 1 << 20;

fn snapshot() -> String { state().lock().map(|m| Value::Object(m.clone()).to_string()).unwrap_or_default() }

fn broadcast() {
    let text = snapshot();
    let subs: Vec<Sub> = SUBS.lock().map(|g| g.clone()).unwrap_or_default();
    // written outside the SUBS lock: a client that stopped reading only costs its own write timeout
    let dead: Vec<Sub> = subs.into_iter().filter(|s| !s.lock().is_ok_and(|mut g| ws_send(&mut g, &text).is_ok())).collect();
    if !dead.is_empty() { if let Ok(mut g) = SUBS.lock() { g.retain(|s| !dead.iter().any(|d| Arc::ptr_eq(s, d))); } }
}

/// Serves ws://127.0.0.1:6133/state (and a plain GET for debugging), a thread per connection.
/// `changed` runs after a widget changes something.
pub fn serve(changed: impl Fn() + Send + Sync + 'static) {
    let _ = state();
    let changed = std::sync::Arc::new(changed);
    std::thread::spawn(move || {
        let Ok(l) = TcpListener::bind(("127.0.0.1", PORT)) else { return };
        for s in l.incoming().flatten() {
            let changed = changed.clone();
            std::thread::spawn(move || { let _ = handle(s, &*changed); });
        }
    });
}

const CRLF2: &[u8] = b"\r\n\r\n";

fn handle(mut s: TcpStream, changed: &dyn Fn()) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = s.read(&mut chunk)?;
        if n == 0 { return Ok(()); }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == CRLF2) { break i + 4; }
        if buf.len() > 16 * 1024 { return Ok(()); }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let header = |name: &str| head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    if header("origin").is_some_and(|o| !allowed(&o)) {
        let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return Ok(());
    }
    if header("host").is_some_and(|h| !host_ok(&h)) {
        let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return Ok(());
    }
    if let Some(key) = header("sec-websocket-key") {
        if SUBS.lock().map_or(true, |g| g.len() >= MAX_SUBS) { return Ok(()); }
        write!(s, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept(&key))?;
        s.set_read_timeout(None)?;
        s.set_write_timeout(Some(Duration::from_secs(2)))?;
        let out: Sub = Arc::new(Mutex::new(s.try_clone()?));
        if let Ok(mut g) = out.lock() { ws_send(&mut g, &snapshot())?; }
        if let Ok(mut subs) = SUBS.lock() { subs.push(out.clone()); }
        while let Some(msg) = ws_read(&mut s, &out) {
            let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&msg) else { continue };
            if apply(&o) { broadcast(); changed(); }
        }
        return Ok(());
    }
    let body = snapshot();
    write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

/// Browsers send the page's own host; a DNS-rebinding page arrives with its attacker's name instead.
fn host_ok(host: &str) -> bool {
    ["127.0.0.1", "localhost"].iter().any(|h| host.eq_ignore_ascii_case(&format!("{h}:{PORT}")) || host.eq_ignore_ascii_case(h))
}

/// Sets or removes (non-string values) shared keys; true if anything changed.
fn apply(o: &Map<String, Value>) -> bool {
    let Ok(mut m) = state().lock() else { return false };
    let mut changed = false;
    for (k, v) in o {
        if !KEYS.contains(&k.as_str()) { continue; }
        let old = m.get(k).cloned();
        match v { Value::String(_) => { m.insert(k.clone(), v.clone()); } _ => { m.remove(k); } }
        changed |= m.get(k) != old.as_ref();
    }
    if changed { save(&m); }
    changed
}

/// Sec-WebSocket-Accept: base64(sha1(key + the RFC 6455 GUID)).
fn accept(key: &str) -> String {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA1_ALG_HANDLE};
    let mut h = [0u8; 20];
    let input = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    unsafe { let _ = BCryptHash(BCRYPT_SHA1_ALG_HANDLE, None, input.as_bytes(), &mut h); }
    base64(&h)
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 { out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' }); }
    }
    out
}

/// Server frame: unmasked text.
fn ws_send(s: &mut TcpStream, text: &str) -> std::io::Result<()> {
    let p = text.as_bytes();
    let mut f = vec![0x81u8];
    match p.len() {
        n if n < 126 => f.push(n as u8),
        n if n < 65536 => { f.push(126); f.extend_from_slice(&(n as u16).to_be_bytes()); }
        n => { f.push(127); f.extend_from_slice(&(n as u64).to_be_bytes()); }
    }
    f.extend_from_slice(p);
    s.write_all(&f)
}

/// Client frames are masked. Joins fragments, answers pings; None when the socket closes.
fn ws_read(s: &mut TcpStream, out: &Mutex<TcpStream>) -> Option<String> {
    let mut msg = Vec::new();
    loop {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).ok()?;
        let (fin, op) = (h[0] & 0x80 != 0, h[0] & 0x0F);
        let mut len = (h[1] & 0x7F) as u64;
        if len == 126 { let mut b = [0u8; 2]; s.read_exact(&mut b).ok()?; len = u16::from_be_bytes(b) as u64; }
        if len == 127 { let mut b = [0u8; 8]; s.read_exact(&mut b).ok()?; len = u64::from_be_bytes(b); }
        if len > MAX_MSG as u64 { return None; }
        let mut mask = [0u8; 4];
        if h[1] & 0x80 != 0 { s.read_exact(&mut mask).ok()?; }
        let mut p = vec![0u8; len as usize];
        s.read_exact(&mut p).ok()?;
        for (i, b) in p.iter_mut().enumerate() { *b ^= mask[i % 4]; }
        match op {
            8 => return None,
            9 => { let mut f = vec![0x8A, p.len().min(125) as u8]; f.extend_from_slice(&p[..p.len().min(125)]); out.lock().ok()?.write_all(&f).ok()?; }
            0 | 1 => { if msg.len() + p.len() > MAX_MSG { return None; } msg.extend_from_slice(&p); if fin { return Some(String::from_utf8_lossy(&msg).into_owned()); } }
            _ => {}
        }
    }
}

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
    #[test]
    fn websocket_accept() {
        assert_eq!(accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="); // RFC 6455 example
        assert_eq!(base64(b"ab"), "YWI=");
    }
    #[test]
    fn hosts() {
        assert!(host_ok("127.0.0.1:6133") && host_ok("LOCALHOST:6133") && host_ok("localhost"));
        assert!(!host_ok("evil.example:6133") && !host_ok("127.0.0.1.evil.example"));
    }
    #[test]
    fn a_runaway_fragmented_message_is_dropped() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (mut srv, _) = l.accept().unwrap();
        let out = Mutex::new(srv.try_clone().unwrap());
        let w = std::thread::spawn(move || {
            // masked text frame without FIN, then continuation frames of 512 KiB
            let mut frame = vec![0x01u8, 0x80 | 127];
            frame.extend_from_slice(&(512u64 * 1024).to_be_bytes());
            frame.extend_from_slice(&[0; 4]);
            frame.extend(std::iter::repeat(b'a').take(512 * 1024));
            for _ in 0..4 { if c.write_all(&frame).is_err() { break; } frame[0] = 0x00; }
        });
        assert_eq!(ws_read(&mut srv, &out), None);
        let _ = w.join();
    }
    #[test]
    fn origins() {
        assert!(allowed("http://127.0.0.1:6124"));
        assert!(!allowed("https://evil.example"));
    }
}
