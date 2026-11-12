//! Rust port of end4_bridge.py's notification and AI-agent commands: `notifications`,
//! `notifications-dismiss`, `notifications-watch`, `agents`, `agents-watch`.

use super::common::*;
use super::*;
use serde_json::json;
use std::{
    collections::BTreeSet,
    ffi::{c_char, c_void, CStr, CString},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Streaming commands this module runs.
pub const STREAMS: &[&str] = &["notifications-watch", "agents-watch"];

/// One-shot command `name`; None if this module doesn't port it.
pub fn call(_cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    match name {
        "notifications" => Some(read_notifications(&wpndb(), &dismissed_path())),
        "notifications-dismiss" => Some(dismiss(args, &wpndb(), &dismissed_path())),
        "agents" => Some(json!({ "agents": agents_summary(&agents_dir(), now_s()) })),
        "agents-dismiss" => Some(json!({ "cleared": agents_dismiss(&agents_dir(), args.first().copied(), now_s()) })),
        _ => None,
    }
}

/// Runs streaming command `name` (one of STREAMS) until it ends or `out.stopped()`.
pub fn stream(_cfg: &Config, name: &str, _args: &[String], _stdin: &str, out: &Sink) {
    // shortcut: no "one copy for all bars" mutex (Python's `skip`), the caller owns how many streams it starts.
    match name {
        "notifications-watch" => notifications_watch(out),
        "agents-watch" => agents_watch(out),
        _ => {}
    }
}

fn now_s() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }
fn dismissed_path() -> PathBuf { root().join("notifications-dismissed.json") }
fn agents_dir() -> PathBuf { root().join("agents") }

/// Sleeps `secs` in short slices; false if the stream was stopped meanwhile.
fn nap(out: &Sink, secs: f64) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if out.stopped() { return false; }
        std::thread::sleep(Duration::from_millis(100).min(end.saturating_duration_since(Instant::now())));
    }
    !out.stopped()
}

// ===================================================================== notifications

/// Windows' toast database (ZORUI_WPNDB overrides it, for tests).
fn wpndb() -> PathBuf {
    std::env::var_os("ZORUI_WPNDB").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Microsoft/Windows/Notifications/wpndatabase.db")
    })
}

/// FILETIME (100 ns ticks since 1601) as unix seconds.
fn filetime_to_unix(ft: i64) -> f64 { ft as f64 / 10_000_000.0 - 11_644_473_600.0 }

/// A readable app name from an AppUserModelID.
fn pretty_app(aumid: &str) -> String {
    let mut a = aumid.split('!').next().unwrap_or("");
    let b = a.as_bytes();
    // package family hash: "_" + 13 chars of [a-z0-9]
    if b.len() >= 14 && b[b.len() - 14] == b'_' && b[b.len() - 13..].iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        a = &a[..a.len() - 14];
    }
    let known = match a {
        "MSEdge" => "Microsoft Edge",
        "Chrome" => "Google Chrome",
        "Microsoft.Windows.Explorer" => "File Explorer",
        "windows.immersivecontrolpanel" => "Settings",
        "Microsoft.WindowsTerminal" => "Terminal",
        _ => "",
    };
    if !known.is_empty() { return known.into(); }
    let tail = if a.contains(['.', '\\']) { a.rsplit(['.', '\\']).next().unwrap_or("") } else { a };
    let mut s = String::new();
    let mut prev = ' ';
    for c in tail.chars() {
        if prev.is_ascii_lowercase() && c.is_ascii_uppercase() { s.push(' '); }
        s.push(c);
        prev = c;
    }
    let s = s.trim();
    if s.is_empty() { "App".into() } else { s.into() }
}

/// html.unescape for the entities toasts actually use (named basics plus numeric).
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let ch = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''), "nbsp" => Some('\u{a0}'),
                _ => name.strip_prefix('#').and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => n.parse().ok(),
                }).and_then(char::from_u32),
            };
            c.map(|c| (c, e + 1))
        });
        match ch {
            Some((c, n)) => { out.push(c); rest = &rest[n..]; }
            None => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out + rest
}

/// (title, body) of a toast payload: the text of its <text> elements, first one and the rest.
fn toast_text(payload: &[u8]) -> (String, String) {
    let p = String::from_utf8_lossy(payload);
    let mut texts = Vec::new();
    let mut rest: &str = &p;
    while let Some(i) = rest.find("<text") {
        let Some(gt) = rest[i..].find('>') else { break };
        let from = i + gt + 1;
        let Some(end) = rest[from..].find("</text>") else { break };
        let inner = &rest[from..from + end];
        rest = &rest[from + end + 7..];
        // strip tags: <[^>]+>
        let mut t = String::new();
        let mut s = inner;
        while let Some(lt) = s.find('<') {
            match s[lt + 1..].find('>') {
                Some(g) if g > 0 => { t.push_str(&s[..lt]); s = &s[lt + g + 2..]; }
                _ => { t.push_str(&s[..lt + 1]); s = &s[lt + 1..]; }
            }
        }
        t.push_str(s);
        let t = t.trim();
        if !t.is_empty() { texts.push(unescape(t)); }
    }
    match texts.split_first() {
        Some((t, r)) => (t.clone(), r.join("\n")),
        None => (String::new(), String::new()),
    }
}

/// What a click on the toast should ShellExecute: the toast's protocol link (`ms-phone:...`) when it has
/// one, else the owning app by AUMID, else "".
// shortcut: opaque (non-protocol) launch args need COM activation, so those toasts only open the app.
fn open_target(payload: &[u8], aumid: &str) -> String {
    let p = String::from_utf8_lossy(payload);
    let tag = p.find("<toast").and_then(|i| p[i..].find('>').map(|e| &p[i..i + e]));
    let attr = |name: &str| tag.and_then(|t| {
        let key = format!(" {name}=\"");
        let from = t.find(&key)? + key.len();
        Some(unescape(&t[from..from + t[from..].find('"')?]))
    });
    if attr("activationType").as_deref() == Some("protocol") {
        if let Some(l) = attr("launch") {
            // a scheme of 2+ letters, so "C:\..." paths are not mistaken for links
            if l.split_once(':').is_some_and(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))) { return l; }
        }
    }
    if aumid.is_empty() { String::new() } else { format!("shell:AppsFolder\\{aumid}") }
}

/// Ids the user dismissed (strings only, like the ids we write).
fn load_dismissed(path: &Path) -> BTreeSet<String> {
    load_json(path, json!([])).as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Notification history, newest first: {"items": [...]} or {"error": ..., "items": []}.
fn read_notifications(db: &Path, dismissed_file: &Path) -> Value {
    if !db.exists() { return json!({ "error": "Notification history not found", "items": [] }); }
    let dismissed = load_dismissed(dismissed_file);
    let rows = match query_toasts(db) {
        Ok(r) => r,
        Err(e) => return json!({ "error": format!("Couldn't read notifications: {e}"), "items": [] }),
    };
    let mut items = Vec::new();
    for r in rows {
        if dismissed.contains(&r.id) { continue; }
        let (title, body) = toast_text(&r.payload);
        if title.is_empty() && body.is_empty() { continue; }
        items.push(json!({ "id": r.id, "app": pretty_app(&r.aumid), "title": title, "body": body, "time": r.arrival.map_or(0.0, filetime_to_unix), "open": open_target(&r.payload, &r.aumid) }));
    }
    json!({ "items": items })
}

/// Marks ids (or "all" current ones) as dismissed; keeps the newest 2000.
fn dismiss(ids: &[&str], db: &Path, dismissed_file: &Path) -> Value {
    let mut d = load_dismissed(dismissed_file);
    if ids == ["all"] {
        let cur = read_notifications(db, dismissed_file);
        d.extend(cur["items"].as_array().into_iter().flatten().filter_map(|i| i["id"].as_str().map(String::from)));
    } else {
        d.extend(ids.iter().map(|s| s.to_string()));
    }
    let key = |x: &String| if !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit()) { x.parse::<u128>().unwrap_or(0) } else { 0 };
    let mut v: Vec<String> = d.into_iter().collect();
    v.sort_by_key(key);
    let v = &v[v.len().saturating_sub(2000)..];
    save_json(dismissed_file, &json!(v));
    json!({ "ok": true })
}

/// Streams {"new": notification} for toasts that arrive; re-reads only when the db or its WAL changes.
fn notifications_watch(out: &Sink) {
    let (db, dis) = (wpndb(), dismissed_path());
    let wal = db.with_file_name(format!("{}-wal", db.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let items = |v: &Value| v["items"].as_array().cloned().unwrap_or_default();
    let mut seen: BTreeSet<String> = items(&read_notifications(&db, &dis)).iter().filter_map(|n| n["id"].as_str().map(String::from)).collect();
    if !out.emit(&json!({ "started": true })) { return; }
    let mut stamp = None;
    loop {
        let now = (mtime(&db), mtime(&wal));
        if Some(now) != stamp {
            stamp = Some(now);
            for n in items(&read_notifications(&db, &dis)).into_iter().rev() {   // oldest first
                if let Some(id) = n["id"].as_str() {
                    if seen.insert(id.to_string()) && !out.emit(&json!({ "new": n })) { return; }
                }
            }
        }
        if !nap(out, 2.0) { return; }
    }
}

// ===================================================================== SQLite through Windows' own winsqlite3.dll

struct Row { id: String, payload: Vec<u8>, arrival: Option<i64>, aumid: String }

type P = *mut c_void;
struct Lib {
    open: unsafe extern "system" fn(*const c_char, *mut P, i32, *const c_char) -> i32,
    close: unsafe extern "system" fn(P) -> i32,
    prepare: unsafe extern "system" fn(P, *const c_char, i32, *mut P, *mut *const c_char) -> i32,
    step: unsafe extern "system" fn(P) -> i32,
    finalize: unsafe extern "system" fn(P) -> i32,
    ctype: unsafe extern "system" fn(P, i32) -> i32,
    int64: unsafe extern "system" fn(P, i32) -> i64,
    blob: unsafe extern "system" fn(P, i32) -> *const u8,
    bytes: unsafe extern "system" fn(P, i32) -> i32,
    errmsg: unsafe extern "system" fn(P) -> *const c_char,
}

impl Lib {
    fn load() -> Option<Lib> {
        use windows::{core::{w, PCSTR}, Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW}};
        unsafe {
            let h = LoadLibraryW(w!("winsqlite3.dll")).ok()?;
            macro_rules! f { ($n:literal, $t:ty) => { std::mem::transmute::<unsafe extern "system" fn() -> isize, $t>(GetProcAddress(h, PCSTR(concat!($n, "\0").as_ptr()))?) }; }
            Some(Lib {
                open: f!("sqlite3_open_v2", unsafe extern "system" fn(*const c_char, *mut P, i32, *const c_char) -> i32), close: f!("sqlite3_close", unsafe extern "system" fn(P) -> i32), prepare: f!("sqlite3_prepare_v2", unsafe extern "system" fn(P, *const c_char, i32, *mut P, *mut *const c_char) -> i32), step: f!("sqlite3_step", unsafe extern "system" fn(P) -> i32),
                finalize: f!("sqlite3_finalize", unsafe extern "system" fn(P) -> i32), ctype: f!("sqlite3_column_type", unsafe extern "system" fn(P, i32) -> i32), int64: f!("sqlite3_column_int64", unsafe extern "system" fn(P, i32) -> i64),
                blob: f!("sqlite3_column_blob", unsafe extern "system" fn(P, i32) -> *const u8), bytes: f!("sqlite3_column_bytes", unsafe extern "system" fn(P, i32) -> i32), errmsg: f!("sqlite3_errmsg", unsafe extern "system" fn(P) -> *const c_char),
            })
        }
    }
}

/// The newest 80 toasts. The db (+ WAL/SHM) is copied to a temp dir first so we never lock Windows' live database.
fn query_toasts(src: &Path) -> Result<Vec<Row>, String> {
    let tmp = std::env::temp_dir().join(format!("zor-ui-wpn-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let db = tmp.join("wpn.db");
    let r = (|| {
        for suffix in ["", "-wal", "-shm"] {
            let s = PathBuf::from(format!("{}{suffix}", src.display()));
            if s.exists() { std::fs::copy(&s, format!("{}{suffix}", db.display())).map_err(|e| e.to_string())?; }
        }
        query_db(&db)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

fn query_db(db: &Path) -> Result<Vec<Row>, String> {
    let lib = Lib::load().ok_or("winsqlite3.dll not available")?;
    let path = CString::new(db.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let sql = c"SELECT n.Id, n.Payload, n.ArrivalTime, h.PrimaryId FROM Notification n LEFT JOIN NotificationHandler h ON h.RecordId = n.HandlerId WHERE n.Type = 'toast' ORDER BY n.ArrivalTime DESC LIMIT 80";
    unsafe {
        let mut con: P = std::ptr::null_mut();
        let rc = (lib.open)(path.as_ptr(), &mut con, 2, std::ptr::null()); // READWRITE, like sqlite3.connect
        let msg = |con: P| if con.is_null() { "out of memory".to_string() } else { CStr::from_ptr((lib.errmsg)(con)).to_string_lossy().into_owned() };
        if rc != 0 { let m = msg(con); if !con.is_null() { (lib.close)(con); } return Err(m); }
        let mut st: P = std::ptr::null_mut();
        if (lib.prepare)(con, sql.as_ptr(), -1, &mut st, std::ptr::null_mut()) != 0 { let m = msg(con); (lib.close)(con); return Err(m); }
        let bytes = |i: i32| -> Vec<u8> {
            if (lib.ctype)(st, i) == 5 { return Vec::new(); }
            let (p, n) = ((lib.blob)(st, i), (lib.bytes)(st, i));
            if p.is_null() || n <= 0 { Vec::new() } else { std::slice::from_raw_parts(p, n as usize).to_vec() }
        };
        let int = |i: i32| match (lib.ctype)(st, i) {
            1 => Some((lib.int64)(st, i)),
            5 => None,
            _ => String::from_utf8_lossy(&bytes(i)).trim().parse().ok(),
        };
        let mut rows = Vec::new();
        let result = loop {
            match (lib.step)(st) {
                100 => rows.push(Row {
                    id: if (lib.ctype)(st, 0) == 1 { (lib.int64)(st, 0).to_string() } else { String::from_utf8_lossy(&bytes(0)).into_owned() },
                    payload: bytes(1),
                    arrival: int(2),
                    aumid: String::from_utf8_lossy(&bytes(3)).into_owned(),
                }),
                101 => break Ok(rows),
                _ => break Err(msg(con)),
            }
        };
        (lib.finalize)(st);
        (lib.close)(con);
        result
    }
}

// ===================================================================== AI agents (end4_agent.py writes, the island reads)

fn agent_label(id: &str) -> String {
    match id {
        "claude" => "Claude".into(), "codex" => "Codex".into(), "gemini" => "Gemini".into(), "opencode" => "OpenCode".into(), "cursor" => "Cursor".into(),
        _ => { // Python's str.title()
            let mut up = true;
            id.chars().map(|c| { let r = if up { c.to_uppercase().collect::<String>() } else { c.to_lowercase().collect() }; up = !c.is_alphabetic(); r }).collect()
        }
    }
}

/// Python's `float(v or 0)`.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        Value::Bool(true) => 1.0,
        _ => 0.0,
    }
}

/// `v` unless it is null, else "" (Python's `.get(key, "")` on our JSON).
fn or_empty(v: &Value) -> Value { if v.is_null() { json!("") } else { v.clone() } }

/// Sort order of a session in the island: waiting, then working, then the rest.
fn rank(s: &Value) -> u8 { match s["state"].as_str() { Some("waiting") => 0, Some("working") => 1, _ => 2 } }

/// One session as the island draws it: what it is doing now, its subagents and its spend.
fn session_json(s: &Value) -> Value {
    let subs: Vec<Value> = s["subs"].as_object().into_iter().flatten()
        .map(|(k, v)| json!({ "id": k, "type": or_empty(&v["type"]), "tool": or_empty(&v["tool"]), "since": num(&v["since"]) })).collect();
    json!({
        "id": or_empty(&s["id"]), "state": or_empty(&s["state"]), "message": or_empty(&s["message"]), "cwd": or_empty(&s["cwd"]),
        "model": or_empty(&s["model"]), "prompt": or_empty(&s["prompt"]), "tool": or_empty(&s["tool"]), "since": num(&s["since"]),
        "at": num(&s["at"]), "subs": subs, "spend": s["spend"].clone(),
    })
}

/// One entry per agent: the session that needs you (if any), what's running, and usage windows.
/// Deletes session files whose agent quit without a SessionEnd (12 h old).
pub(super) fn agents_summary(dir: &Path, now: f64) -> Vec<Value> {
    let mut res = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return res };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    for d in dirs {
        let id = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&d).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json") && p.file_name().is_some_and(|n| n != "usage.json")).collect();
        files.sort();
        let mut sessions: Vec<Value> = Vec::new();
        for f in files {
            let mut s = load_json(&f, json!({}));
            if !s.is_object() { s = json!({}); }
            let age = now - num(&s["at"]);
            if age > 12.0 * 3600.0 { let _ = std::fs::remove_file(&f); continue; }
            if s["state"] == "working" && age > 15.0 * 60.0 { // crashed mid-turn: nothing is running any more
                s["state"] = json!("done");
                s["tool"] = json!("");
                s["subs"] = json!({});
            }
            sessions.push(s);
        }
        let u = load_json(&d.join("usage.json"), json!({}));
        let windows: serde_json::Map<String, Value> = u["windows"].as_object().into_iter().flatten()
            .filter(|(_, v)| num(&v["resets_at"]) == 0.0 || num(&v["resets_at"]) > now)
            .map(|(k, v)| (k.clone(), v.clone())).collect();
        if sessions.is_empty() && windows.is_empty() { continue; }
        let mut waiting: Vec<&Value> = sessions.iter().filter(|s| s["state"] == "waiting").collect();
        waiting.sort_by(|a, b| num(&b["at"]).total_cmp(&num(&a["at"])));
        let working: Vec<&Value> = sessions.iter().filter(|s| s["state"] == "working").collect();
        let first = waiting.first().or(working.first());
        // every session, the ones that need you first, then running, then idle; newest first within each
        let mut ranked: Vec<&Value> = sessions.iter().collect();
        ranked.sort_by(|a, b| rank(a).cmp(&rank(b)).then(num(&b["at"]).total_cmp(&num(&a["at"]))));
        res.push(json!({
            "id": id, "label": agent_label(&id),
            "state": if !waiting.is_empty() { "waiting" } else if !working.is_empty() { "working" } else { "idle" },
            "message": waiting.first().map_or(json!(""), |s| or_empty(&s["message"])),
            // the newest prompt that asks for permission: the island offers Allow and Deny for it (permit.rs)
            // only agents whose hook waits for the answer (permit.rs PERMIT_AGENTS) get Allow and Deny
            "permit": waiting.iter().find(|s| s["ask"] == "permission").filter(|_| super::permit::PERMIT_AGENTS.contains(&id.as_str())).map_or(Value::Null, |s| json!({ "session": or_empty(&s["id"]), "message": or_empty(&s["message"]), "at": num(&s["at"]) })),
            "cwd": first.map_or(json!(""), |s| or_empty(&s["cwd"])),
            "waiting": waiting.len(), "working": working.len(),
            "sessions": ranked.into_iter().map(session_json).collect::<Vec<_>>(),
            "usage": windows, "model": or_empty(&u["model"]), "usageAt": u["at"].clone(),
        }));
    }
    res
}

/// Marks every `waiting` session of `agent` (all agents if None) as `done`, for a prompt the agent never
/// cleared itself. The file stays, so the agent's next hook event overwrites it. Returns how many it cleared.
pub(super) fn agents_dismiss(dir: &Path, agent: Option<&str>, now: f64) -> usize {
    let mut n = 0;
    for d in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        if agent.is_some_and(|a| d.file_name().is_none_or(|f| f != a)) { continue; }
        for f in std::fs::read_dir(&d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json") && p.file_name().is_some_and(|n| n != "usage.json")) {
            let mut s = load_json(&f, json!({}));
            if s["state"] != "waiting" { continue; }
            s["state"] = json!("done");
            s["at"] = json!(now);
            if std::fs::write(&f, s.to_string()).is_ok() { n += 1; }
        }
    }
    n
}

/// Streams {"agents": [...]} when an agent reports (and every 30 s, for resets and stale sessions).
fn agents_watch(out: &Sink) {
    let dir = agents_dir();
    let stamps = || -> Vec<(PathBuf, Option<SystemTime>)> {
        let mut v = Vec::new();
        for d in std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|d| d.path().is_dir()) {
            for e in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
                if e.file_name().to_string_lossy().ends_with(".json") { v.push((e.path(), e.metadata().and_then(|m| m.modified()).ok())); }
            }
        }
        v.sort();
        v
    };
    let (mut stamp, mut last, mut sent) = (None, None, Instant::now());
    let mut first = true;
    loop {
        let now = stamps();
        if first || Some(&now) != stamp.as_ref() || sent.elapsed() > Duration::from_secs(30) {
            first = false;
            stamp = Some(now);
            sent = Instant::now();
            let cur = agents_summary(&dir, now_s());
            if Some(&cur) != last.as_ref() {
                if !out.emit(&json!({ "agents": cur })) { return; }
                last = Some(cur);
            }
        }
        if !nap(out, 0.5) { return; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zor-ui-notify-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn app_names() {
        assert_eq!(pretty_app("MSEdge"), "Microsoft Edge");
        assert_eq!(pretty_app("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"), "Terminal");
        assert_eq!(pretty_app("5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App"), "Whats App Desktop");
        assert_eq!(pretty_app("C:\\Apps\\discord.exe"), "exe");
        assert_eq!(pretty_app(""), "App");
        assert_eq!(pretty_app("Spotify"), "Spotify");
    }

    #[test]
    fn toast_parsing() {
        let p = br#"<toast><visual><binding><text id="1">Hi &amp; <b>bye</b></text><text/><text>Line &#65;</text><text>  </text></binding></visual></toast>"#;
        let (t, b) = toast_text(p);
        assert_eq!(t, "Hi & bye");
        assert_eq!(b, "Line A");
        assert_eq!(toast_text(b""), (String::new(), String::new()));
    }

    #[test]
    fn click_target() {
        let app = "MSEdge";
        let open = |p: &str| open_target(p.as_bytes(), app);
        assert_eq!(open(r#"<toast activationType="protocol" launch="ms-phone:messages?id=1&amp;thread=5"><visual/></toast>"#), "ms-phone:messages?id=1&thread=5");
        assert_eq!(open(r#"<toast hint-people="tel:1" launch="windowsdefender://threat" activationType="protocol">"#), "windowsdefender://threat");
        // opaque launch args, a drive path, a missing launch and no payload all fall back to the app
        let fallback = "shell:AppsFolder\\MSEdge";
        assert_eq!(open(r#"<toast launch="identifier=abc"><visual/></toast>"#), fallback);
        assert_eq!(open(r#"<toast activationType="protocol" launch="C:\x.exe">"#), fallback);
        assert_eq!(open("<toast><visual/></toast>"), fallback);
        assert_eq!(open(""), fallback);
        assert_eq!(open_target(b"", ""), "");
    }

    #[test]
    fn filetime() {
        assert_eq!(filetime_to_unix(116_444_736_000_000_000), 0.0);
    }

    #[test]
    fn dismiss_file() {
        let d = tmp("dismiss");
        let (f, db) = (d.join("dismissed.json"), d.join("none.db"));
        assert_eq!(dismiss(&["10", "9", "abc"], &db, &f), json!({ "ok": true }));
        assert_eq!(load_json(&f, json!(null)), json!(["abc", "9", "10"]));
        dismiss(&["9", "11"], &db, &f);
        assert_eq!(load_json(&f, json!(null)), json!(["abc", "9", "10", "11"]));
        let many: Vec<String> = (0..2100).map(|i| i.to_string()).collect();
        dismiss(&many.iter().map(String::as_str).collect::<Vec<_>>(), &db, &f);
        let v = load_json(&f, json!([]));
        assert_eq!(v.as_array().map(Vec::len), Some(2000));
        assert_eq!(v[1999], "2099");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_db() {
        assert_eq!(read_notifications(Path::new("Z:/nope.db"), Path::new("Z:/x.json")), json!({ "error": "Notification history not found", "items": [] }));
    }

    #[test]
    fn agents() {
        let d = tmp("agents");
        let now = 1_000_000.0;
        let a = d.join("claude");
        std::fs::create_dir_all(&a).unwrap();
        let w = |n: &str, v: Value| std::fs::write(a.join(n), v.to_string()).unwrap();
        w("1.json", json!({ "state": "waiting", "at": now - 10.0, "message": "ok?", "cwd": "C:/x" }));
        w("2.json", json!({ "state": "working", "at": now - 20.0, "cwd": "C:/y" }));
        w("3.json", json!({ "state": "working", "at": now - 16.0 * 60.0 }));
        w("4.json", json!({ "state": "working", "at": now - 13.0 * 3600.0 }));
        w("usage.json", json!({ "model": "m", "at": 5, "windows": { "5h": { "resets_at": now + 5.0 }, "old": { "resets_at": now - 5.0 }, "none": {} } }));
        let r = agents_summary(&d, now);
        assert_eq!(r.len(), 1);
        let mut agent = r[0].clone();
        let sessions = agent.as_object_mut().unwrap().remove("sessions").unwrap();
        assert_eq!(agent, json!({ "id": "claude", "label": "Claude", "state": "waiting", "message": "ok?", "permit": null, "cwd": "C:/x", "waiting": 1, "working": 1,
            "usage": { "5h": { "resets_at": now + 5.0 }, "none": {} }, "model": "m", "usageAt": 5 }));
        // every session, the one that needs you first; the crashed one (working for 16 min) is done now
        let states: Vec<&str> = sessions.as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect();
        assert_eq!(states, ["waiting", "working", "done"]);
        assert!(!a.join("4.json").exists());
        assert_eq!(agent_label("my-agent"), "My-Agent");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dismiss_clears_waiting_only() {
        let d = tmp("agents-dismiss");
        let now = 1_000_000.0;
        for id in ["claude", "codex"] {
            std::fs::create_dir_all(d.join(id)).unwrap();
            std::fs::write(d.join(id).join("1.json"), json!({ "state": "waiting", "at": now - 10.0, "message": "ok?" }).to_string()).unwrap();
        }
        std::fs::write(d.join("claude/2.json"), json!({ "state": "working", "at": now - 5.0 }).to_string()).unwrap();
        assert_eq!(agents_dismiss(&d, Some("claude"), now), 1);
        let r = agents_summary(&d, now);
        assert_eq!((r[0]["id"].as_str(), r[0]["state"].as_str()), (Some("claude"), Some("working")));
        assert_eq!(r[1]["state"], "waiting"); // codex untouched
        assert_eq!(agents_dismiss(&d, None, now), 1);
        assert_eq!(agents_dismiss(&d, None, now), 0);
        assert!(agents_summary(&d, now).iter().all(|a| a["state"] != "waiting"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn agents_that_wait_for_the_answer_offer_allow_and_deny() {
        let d = tmp("agents-permit");
        let now = 1_000_000.0;
        for id in ["claude", "codex"] {
            std::fs::create_dir_all(d.join(id)).unwrap();
            std::fs::write(d.join(id).join("s.json"), json!({ "state": "waiting", "at": now - 1.0, "ask": "permission", "id": "s", "message": "Allow?" }).to_string()).unwrap();
        }
        let r = agents_summary(&d, now);
        assert_eq!(r[0]["permit"]["session"], "s", "claude gets Allow and Deny");
        assert_eq!(r[1]["permit"]["session"], "s", "codex gets Allow and Deny");
        assert_eq!(r[1]["state"], "waiting");
        let g = d.join("gemini");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("s.json"), json!({ "state": "waiting", "at": now - 1.0, "ask": "permission", "id": "s" }).to_string()).unwrap();
        let gem = agents_summary(&d, now).into_iter().find(|a| a["id"] == "gemini").unwrap();
        assert!(gem["permit"].is_null(), "gemini has no permission hook, so no buttons it cannot answer");
        let _ = std::fs::remove_dir_all(&d);
    }
}
