//! Several Claude Code logins on one PC, and a switch between them (Settings, Services, Claude accounts).
//!
//! A login is two things on disk: `claudeAiOauth` in `~\.claude\.credentials.json` (the tokens) and `oauthAccount`
//! in `~\.claude.json` (who it is). Each saved login is a slot in `%LOCALAPPDATA%\zor-ui\claude-accounts.json`
//! whose tokens are encrypted with Windows DPAPI, like the API keys in secrets.json. Switching takes the same lock
//! directories Claude Code takes, copies the live tokens back into the slot they came from (Claude Code rotates
//! them), then replaces just those two values in the two files. Nothing else in either file is touched: the text
//! around the value is kept byte for byte, because .claude.json holds all of Claude Code's other state.
//!
//! The layout is Claude Code's own and not a public API. A file that doesn't look as expected is never written.
//!
//! Saved logins that aren't in use get their access token renewed here (the active one is Claude Code's to renew),
//! so their usage can be read and a switch lands on a working login. A renewal gives a new refresh token and the old
//! one stops working, so the new tokens are written to the slot at once. When the server says the refresh token is
//! no longer valid the slot is marked `dead` until that login is saved again.
//!
//! Auto-switch (`auto_tick`, from the usage loop in app.rs): when the active login passes a threshold and another has
//! room, a banner says so and the switch happens 20 seconds later unless the banner is clicked.
//! shortcut: CLAUDE_CONFIG_DIR is not honoured (claude_usage.rs doesn't either); add it if anyone sets it.

use super::{common::{err, hidden, root, save_json}, web::{b64_decode, b64_encode, dpapi}};
use crate::conf::Config;
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// One writer of the slot file at a time (Settings, the usage loop and auto-switch all write it).
static STORE: Mutex<()> = Mutex::new(());
fn store_lock() -> std::sync::MutexGuard<'static, ()> { STORE.lock().unwrap_or_else(|e| e.into_inner()) }

/// Where the files live. Tests point both at a temp folder.
pub struct Paths { home: PathBuf, store: PathBuf }

impl Paths {
    pub fn live() -> Paths { Paths { home: crate::conf::home(), store: root().join("claude-accounts.json") } }
    fn creds(&self) -> PathBuf { self.home.join(".claude").join(".credentials.json") }
    fn config(&self) -> PathBuf { self.home.join(".claude.json") }
    /// Claude Code's lock directories (proper-lockfile) and when each counts as abandoned, in the order it takes them.
    fn locks(&self) -> [(PathBuf, Duration); 3] {
        [(self.home.join(".claude").join(".oauth_refresh.lock"), Duration::from_secs(60)),
         (self.home.join(".claude.lock"), Duration::from_secs(60)),
         (self.home.join(".claude.json.lock"), Duration::from_secs(10))]
    }
}

// ------------------------------------------------------------------ locks

struct Lock(Vec<PathBuf>);

impl Drop for Lock {
    fn drop(&mut self) { for d in self.0.iter().rev() { let _ = std::fs::remove_dir(d); } }
}

/// Creates the lock directory, waiting up to `budget` for another holder and breaking one older than `stale`.
fn take(dir: &Path, stale: Duration, budget: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        match std::fs::create_dir(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let age = dir.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
                if age.is_some_and(|a| a > stale) {
                    std::fs::remove_dir(dir).map_err(|e| format!("can't clear the old lock {}: {e}", dir.display()))?;
                    continue;
                }
                if start.elapsed() >= budget { return Err("Claude Code is busy with its login files. Try again in a moment.".into()); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("can't lock {}: {e}", dir.display())),
        }
    }
}

fn lock_all(p: &Paths, budget: Duration) -> Result<Lock, String> {
    let mut held = Lock(vec![]);
    for (dir, stale) in p.locks() {
        take(&dir, stale, budget)?; // an early return drops `held`, which frees the ones already taken
        held.0.push(dir);
    }
    Ok(held)
}

// ------------------------------------------------------------------ editing one value in a JSON text

/// Where the value of top-level `key` starts and ends in the text of a JSON object.
fn value_span(text: &str, key: &str) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let (mut i, mut depth) = (0, 0i32);
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < b.len() && b[i] != b'"' { if b[i] == b'\\' { i += 1; } i += 1; }
                if depth == 1 && text.get(start..i) == Some(key) {
                    let mut j = i + 1;
                    while j < b.len() && b[j].is_ascii_whitespace() { j += 1; }
                    if b.get(j) == Some(&b':') {
                        j += 1;
                        while j < b.len() && b[j].is_ascii_whitespace() { j += 1; }
                        return Some((j, value_end(b, j)?));
                    }
                }
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

fn value_end(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0;
    while i < b.len() {
        match b[i] {
            b'"' => { i += 1; while i < b.len() && b[i] != b'"' { if b[i] == b'\\' { i += 1; } i += 1; } }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                if depth == 0 { return Some(i); }
                depth -= 1;
                if depth == 0 { return Some(i + 1); }
            }
            b',' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// `text` with the value of top-level `key` replaced by `new` (JSON text); None when the key isn't there.
fn splice(text: &str, key: &str, new: &str) -> Option<String> {
    let (s, e) = value_span(text, key)?;
    Some(format!("{}{}{}", &text[..s], new, &text[e..]))
}

/// A JSON object file read strictly: a missing, unreadable or non-object file is an error, never an empty default,
/// so a damaged file can't be "repaired" into an empty one.
fn read_obj(path: &Path) -> Result<(String, Value), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))?;
    if !v.is_object() { return Err(format!("{} isn't a JSON object", path.display())); }
    Ok((text, v))
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("zor-tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("can't write {}: {e}", path.display()))
}

// ------------------------------------------------------------------ logins and slots

/// Who a login is: the pair Claude Code itself tells accounts apart by.
#[derive(Clone, Debug, PartialEq)]
struct Ident { email: String, org: String }

fn ident(account: &Value) -> Option<Ident> {
    let email = account["emailAddress"].as_str().filter(|s| !s.is_empty())?;
    Some(Ident { email: email.into(), org: account["organizationUuid"].as_str().unwrap_or("").into() })
}

fn slot_ident(s: &Value) -> Option<Ident> {
    Some(Ident { email: s["email"].as_str()?.into(), org: s["org"].as_str().unwrap_or("").into() })
}

/// The login Claude Code is using now.
struct Live { oauth: Value, account: Value }

fn live_of(creds: &Value, config: &Value) -> Option<Live> {
    let (oauth, account) = (creds["claudeAiOauth"].clone(), config["oauthAccount"].clone());
    (oauth["accessToken"].as_str().is_some_and(|t| !t.is_empty()) && ident(&account).is_some()).then_some(Live { oauth, account })
}

fn live(p: &Paths) -> Option<Live> {
    live_of(&read_obj(&p.creds()).ok()?.1, &read_obj(&p.config()).ok()?.1)
}

fn load(p: &Paths) -> Vec<Value> {
    super::common::load_json(&p.store, json!({}))["slots"].as_array().cloned().unwrap_or_default()
}

fn store(p: &Paths, slots: &[Value]) -> Result<(), String> {
    if save_json(&p.store, &json!({ "slots": slots })) { Ok(()) } else { Err(format!("can't write {}", p.store.display())) }
}

fn seal(oauth: &Value, account: &Value) -> Option<String> {
    dpapi(json!({ "claudeAiOauth": oauth, "oauthAccount": account }).to_string().as_bytes(), true).map(|b| b64_encode(&b))
}

fn open(slot: &Value) -> Option<(Value, Value)> {
    let v: Value = serde_json::from_slice(&dpapi(&b64_decode(slot["blob"].as_str()?), false)?).ok()?;
    Some((v["claudeAiOauth"].clone(), v["oauthAccount"].clone()))
}

/// Puts a live login into its slot, adding the slot when it's new. The slot number.
fn upsert(slots: &mut Vec<Value>, l: &Live) -> Result<u64, String> {
    let id = ident(&l.account).ok_or("Claude Code isn't signed in")?;
    let blob = seal(&l.oauth, &l.account).ok_or("Windows couldn't encrypt the login (DPAPI)")?;
    let plan = l.oauth["subscriptionType"].as_str().unwrap_or("");
    if let Some(s) = slots.iter_mut().find(|s| slot_ident(s).as_ref() == Some(&id)) {
        s["blob"] = json!(blob);
        s["plan"] = json!(plan);
        s["dead"] = json!(false); // a fresh sign-in revives a slot marked dead
        return Ok(s["n"].as_u64().unwrap_or(0));
    }
    let n = slots.iter().filter_map(|s| s["n"].as_u64()).max().unwrap_or(0) + 1;
    slots.push(json!({ "n": n, "email": id.email, "org": id.org, "orgName": l.account["organizationName"], "plan": plan, "blob": blob }));
    Ok(n)
}

/// Copies the live tokens into the slot of the same login when they differ (Claude Code rotates them as it runs).
/// True when a slot changed; false when nothing differed or no slot matches.
fn sync_live(slots: &mut [Value], l: &Live) -> bool {
    let Some(id) = ident(&l.account) else { return false };
    let Some(s) = slots.iter_mut().find(|s| slot_ident(s).as_ref() == Some(&id)) else { return false };
    if open(s).is_some_and(|(o, a)| o == l.oauth && a == l.account) { return false; }
    match seal(&l.oauth, &l.account) { Some(b) => { s["blob"] = json!(b); true } None => false }
}

/// Saves the login Claude Code is signed in with.
pub fn save_current(p: &Paths) -> Result<u64, String> {
    let l = live(p).ok_or("Claude Code isn't signed in. Run `claude`, log in, then save.")?;
    let _w = store_lock();
    let mut slots = load(p);
    let n = upsert(&mut slots, &l)?;
    store(p, &slots)?;
    Ok(n)
}

/// Makes slot `n` the login Claude Code uses.
pub fn switch(p: &Paths, n: u64) -> Result<(), String> {
    let _w = store_lock();
    let slots = load(p);
    let target = slots.iter().find(|s| s["n"].as_u64() == Some(n)).ok_or("No such account")?;
    if target["dead"] == true { return Err("That login has expired. Sign in to it with `claude`, then save it again.".into()); }
    let (oauth, account) = open(target).ok_or("Can't decrypt that login. It was saved by another Windows user or PC.")?;
    let tid = ident(&account).ok_or("The saved login has no email")?;
    let _held = lock_all(p, Duration::from_secs(9))?;
    // read under the lock, so what Claude Code wrote a moment ago is what we replace
    let (ctext, cv) = read_obj(&p.creds())?;
    let (jtext, jv) = read_obj(&p.config())?;
    let mut slots = load(p);
    if let Some(l) = live_of(&cv, &jv) {
        if ident(&l.account).as_ref() == Some(&tid) { return Ok(()); }
        if !slots.iter().any(|s| slot_ident(s) == ident(&l.account)) {
            return Err("The login you're using isn't saved yet. Save it first, or switching away would lose it.".into());
        }
        if sync_live(&mut slots, &l) { store(p, &slots)?; }
    }
    let new_c = splice(&ctext, "claudeAiOauth", &oauth.to_string()).ok_or("The credentials file has no claudeAiOauth to replace")?;
    let new_j = splice(&jtext, "oauthAccount", &account.to_string()).ok_or(".claude.json has no oauthAccount to replace")?;
    write_atomic(&p.creds(), &new_c)?;
    if let Err(e) = write_atomic(&p.config(), &new_j) {
        let _ = write_atomic(&p.creds(), &ctext); // never leave the tokens of one account next to the name of another
        return Err(e);
    }
    Ok(())
}

pub fn remove(p: &Paths, n: u64) -> Result<(), String> {
    let _w = store_lock();
    let mut slots = load(p);
    slots.retain(|s| s["n"].as_u64() != Some(n));
    store(p, &slots)
}

// ------------------------------------------------------------------ usage and the list for Settings

fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// A token Claude Code still accepts (expiresAt is in milliseconds); one a minute from expiry is as good as expired.
fn token_ok(oauth: &Value, now_ms: u64) -> Option<String> {
    let t = oauth["accessToken"].as_str().filter(|t| !t.is_empty())?;
    (oauth["expiresAt"].as_u64().unwrap_or(0) > now_ms + 60_000).then(|| t.to_string())
}

/// The accounts for Settings: one row each, with the last usage that could be read (kept when a read fails).
pub fn list(p: &Paths) -> Value {
    let lv = live(p);
    let lid = lv.as_ref().and_then(|l| ident(&l.account));
    let now = now_secs();
    let mut slots = {
        let _w = store_lock();
        let mut slots = load(p);
        if let Some(l) = &lv { if sync_live(&mut slots, l) { let _ = store(p, &slots); } }
        slots
    };
    // renew and read every login side by side (one request or two each)
    let work: Vec<Option<Work>> = slots.iter().map(|s| {
        if s["dead"] == true { return None; }
        let active = lid.is_some() && slot_ident(s) == lid;
        let (oauth, account) = if active { let l = lv.as_ref()?; (l.oauth.clone(), l.account.clone()) } else { open(s)? };
        Some(Work { n: s["n"].as_u64().unwrap_or(0), oauth, account, active })
    }).collect();
    let outcomes: Vec<Option<Outcome>> = std::thread::scope(|sc| {
        let jobs: Vec<_> = work.into_iter().map(|w| sc.spawn(move || w.map(|w| renew_and_read(w, now * 1000)))).collect();
        jobs.into_iter().map(|j| j.join().ok().flatten()).collect()
    });
    if outcomes.iter().flatten().any(|o| o.renewed.is_some() || o.dead || o.usage.is_some()) {
        let _w = store_lock();
        let mut fresh = load(p); // read again: a switch or save may have run while the requests were out
        for o in outcomes.iter().flatten() { apply(&mut fresh, o, now); }
        let _ = store(p, &fresh);
        slots = fresh;
    }
    let rows: Vec<Value> = slots.iter().map(|s| {
        let u = &s["usage"];
        json!({
            "n": s["n"], "email": s["email"], "orgName": s["orgName"], "plan": s["plan"],
            "active": lid.is_some() && slot_ident(s) == lid,
            "dead": s["dead"] == true,
            "session": u["session"], "week": u["week"],
            "ageMin": u["at"].as_u64().map(|at| now.saturating_sub(at) / 60),
        })
    }).collect();
    json!({ "accounts": rows })
}

// ------------------------------------------------------------------ token renewal

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"; // Claude Code's public OAuth client

/// A login to renew and read; `active` ones are Claude Code's to renew.
struct Work { n: u64, oauth: Value, account: Value, active: bool }

/// What came back: renewed tokens to store, whether the refresh token is no longer valid, the usage read.
struct Outcome { n: u64, account: Value, renewed: Option<Value>, dead: bool, usage: Option<crate::claude_usage::Usage> }

#[derive(Debug, PartialEq)]
enum Renewal { New(Value), Dead, Failed }

fn refresh_body(refresh_token: &str) -> String {
    json!({ "grant_type": "refresh_token", "refresh_token": refresh_token, "client_id": CLIENT_ID }).to_string()
}

/// The token endpoint's answer as new `claudeAiOauth` values (the old ones with the new tokens and expiry laid over).
fn read_renewal(body: &str, old: &Value, now_ms: u64) -> Renewal {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if let Some(t) = v["access_token"].as_str().filter(|t| !t.is_empty()) {
        let mut o = old.clone();
        o["accessToken"] = json!(t);
        if let Some(r) = v["refresh_token"].as_str().filter(|r| !r.is_empty()) { o["refreshToken"] = json!(r); }
        o["expiresAt"] = json!(now_ms + v["expires_in"].as_u64().unwrap_or(3600) * 1000);
        return Renewal::New(o);
    }
    if v["error"] == "invalid_grant" { Renewal::Dead } else { Renewal::Failed }
}

/// POSTs the refresh. The body goes in through stdin, so the token never shows in the process list.
fn renew(old: &Value, now_ms: u64) -> Renewal {
    let Some(rt) = old["refreshToken"].as_str().filter(|r| !r.is_empty()) else { return Renewal::Dead };
    let spawned = hidden("curl.exe")
        .args(["-s", "--max-time", "20", "-X", "POST", "-H", "Content-Type: application/json", "-d", "@-", TOKEN_URL])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn();
    let Ok(mut child) = spawned else { return Renewal::Failed };
    let sent = child.stdin.take().map(|mut si| si.write_all(refresh_body(rt).as_bytes()).is_ok()).unwrap_or(false); // dropping stdin ends the body
    if !sent { let _ = child.kill(); return Renewal::Failed; }
    match child.wait_with_output() {
        Ok(out) => read_renewal(&String::from_utf8_lossy(&out.stdout), old, now_ms),
        Err(_) => Renewal::Failed,
    }
}

fn renew_and_read(w: Work, now_ms: u64) -> Outcome {
    let (mut oauth, mut renewed, mut dead) = (w.oauth, None, false);
    if !w.active && token_ok(&oauth, now_ms).is_none() {
        match renew(&oauth, now_ms) {
            Renewal::New(o) => { renewed = Some(o.clone()); oauth = o; }
            Renewal::Dead => dead = true,
            Renewal::Failed => {}
        }
    }
    let usage = if dead { None } else { token_ok(&oauth, now_ms).and_then(crate::claude_usage::fetch_with) };
    Outcome { n: w.n, account: w.account, renewed, dead, usage }
}

/// Lays one outcome over the slots as they are on disk now.
fn apply(slots: &mut [Value], o: &Outcome, now: u64) {
    let Some(s) = slots.iter_mut().find(|s| s["n"].as_u64() == Some(o.n)) else { return };
    if let Some(r) = &o.renewed { if let Some(b) = seal(r, &o.account) { s["blob"] = json!(b); } }
    if o.dead { s["dead"] = json!(true); }
    if let Some(u) = &o.usage { s["usage"] = json!({ "session": u.session, "week": u.week, "at": now }); }
}

// ------------------------------------------------------------------ auto-switch

/// What a click on the auto-switch banner carries (bar.rs sees it and calls `cancel`).
pub const CANCEL: &str = "zorui-claude-cancel";
const COUNTDOWN_SECS: u64 = 20;
const COOLDOWN_SECS: u64 = 600;
/// A reading older than this isn't trusted when picking where to go.
const FRESH_MIN: u64 = 15;
static CANCELLED: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);

pub fn cancel() { CANCELLED.store(true, Ordering::Relaxed); }

fn worst(r: &Value) -> Option<u64> { Some(r["session"].as_u64()?.max(r["week"].as_u64()?)) }

/// Where to switch: the active login is at or past `threshold`; the pick is the live login with the least used
/// that is under it, going by a fresh reading. None when the active one is fine, unread, or nothing has room.
fn pick(rows: &[Value], threshold: u64) -> Option<(u64, String)> {
    if worst(rows.iter().find(|r| r["active"] == true)?)? < threshold { return None; }
    rows.iter()
        .filter(|r| r["active"] != true && r["dead"] != true && r["ageMin"].as_u64().is_some_and(|a| a <= FRESH_MIN))
        .filter_map(|r| Some((worst(r).filter(|w| *w < threshold)?, r)))
        .min_by_key(|(w, _)| *w)
        .map(|(_, r)| (r["n"].as_u64().unwrap_or(0), r["email"].as_str().unwrap_or("").to_string()))
}

fn cooled(now: u64, last: u64) -> bool { last == 0 || now.saturating_sub(last) >= COOLDOWN_SECS }

/// Whether the usage loop should keep reading Claude usage: auto-switch is on, or the bar shows the chip.
pub fn wanted(cfg: &Config) -> bool {
    cfg.get("claude.autoSwitch").as_bool() == Some(true) || crate::bar::layout(cfg).iter().any(|m| m.0 == "claude" && m.1 != "hidden")
}

fn banner(text: String, open: &str, secs: u64) {
    let open = open.to_string();
    crate::app::post(Box::new(move |cx| cx.notif = Some(crate::app::Notif { app: "Zor UI".into(), text, until: crate::hub::now_ms() + secs as f64 * 1000.0, open })));
}

/// One check of auto-switch; blocks for the countdown when it decides to switch. Runs on the usage loop's thread.
pub fn auto_tick(cfg: &Config) {
    if cfg.get("claude.autoSwitch").as_bool() != Some(true) || !cooled(now_secs(), LAST.load(Ordering::Relaxed)) { return; }
    let threshold = cfg.get("claude.threshold").as_u64().unwrap_or(95).clamp(50, 100);
    let p = Paths::live();
    let Some((n, email)) = pick(list(&p)["accounts"].as_array().map(Vec::as_slice).unwrap_or(&[]), threshold) else { return };
    CANCELLED.store(false, Ordering::Relaxed);
    banner(format!("Claude passed {threshold}%. Switching to {email} in {COUNTDOWN_SECS} s. Click to cancel."), CANCEL, COUNTDOWN_SECS);
    for _ in 0..COUNTDOWN_SECS {
        std::thread::sleep(Duration::from_secs(1));
        if CANCELLED.load(Ordering::Relaxed) || crate::conf::Config::load().get("claude.autoSwitch").as_bool() != Some(true) {
            LAST.store(now_secs(), Ordering::Relaxed); // a "no" holds for the cooldown, so it isn't asked again at once
            return;
        }
    }
    LAST.store(now_secs(), Ordering::Relaxed);
    match switch(&p, n) {
        Ok(()) => {
            banner(format!("Switched Claude to {email}"), "", 6);
            if let Some(u) = crate::claude_usage::fetch() { crate::app::post(Box::new(move |cx| cx.claude = Some(u))); }
        }
        Err(e) => banner(format!("Couldn't switch Claude: {e}"), "", 8),
    }
}

/// The grey line under an account: plan, the two usage windows, and how old that reading is.
pub(crate) fn summary(row: &Value) -> String {
    if row["dead"] == true { return "Login expired. Sign in with `claude`, then save it again".into(); }
    let mut parts = vec![];
    if let Some(plan) = row["plan"].as_str().filter(|p| !p.is_empty()) {
        let mut c = plan.chars();
        parts.push(c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default());
    }
    match (row["session"].as_u64(), row["week"].as_u64()) {
        (Some(s), Some(w)) => {
            parts.push(format!("5h {s}% · 7d {w}%"));
            parts.push(match row["ageMin"].as_u64().unwrap_or(0) { 0 => "just now".into(), m if m < 60 => format!("{m} min ago"), m => format!("{} h ago", m / 60) });
        }
        _ => parts.push("usage unavailable".into()),
    }
    parts.join(" · ")
}

fn done(r: Result<(), String>) -> Value { match r { Ok(()) => json!({ "ok": true }), Err(e) => err(e) } }

pub fn call(_cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    let p = Paths::live();
    let n = || args.first().and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
    Some(match name {
        "claude-accounts" => list(&p),
        "claude-save" => done(save_current(&p).map(|_| ())),
        "claude-switch" => {
            let r = switch(&p, n());
            // the bar and Gojo show the active login's usage: read it now instead of at the next 3 minute tick
            if r.is_ok() { if let Some(u) = crate::claude_usage::fetch() { crate::app::post(Box::new(move |cx| cx.claude = Some(u))); } }
            done(r)
        }
        "claude-remove" => done(remove(&p, n())),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
  "numStartups": 7,
  "projects": { "c:/Code/x": {"a": 1}, "C:/Code/x": {"a": 2} },
  "oauthAccount": { "emailAddress": "a@x.com", "organizationUuid": "org-a", "note": "}, \"quoted\"" },
  "tail": [1, 2]
}"#;

    fn creds(token: &str, expires: u64) -> String {
        json!({ "mcpOAuth": { "srv": { "k": 1 } }, "claudeAiOauth": { "accessToken": token, "refreshToken": "r", "expiresAt": expires, "subscriptionType": "pro" } }).to_string()
    }

    fn account(email: &str, org: &str) -> String { json!({ "emailAddress": email, "organizationUuid": org, "organizationName": "Org" }).to_string() }

    /// A fake home with Claude Code signed in as `email`.
    fn setup(tag: &str, email: &str, org: &str, token: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("zorui-acct-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home").join(".claude")).unwrap();
        let p = Paths { home: dir.join("home"), store: dir.join("data").join("claude-accounts.json") };
        let config = CONFIG.replace(&account_of_config(), &account(email, org));
        std::fs::write(p.config(), config).unwrap();
        std::fs::write(p.creds(), creds(token, u64::MAX / 2)).unwrap();
        p
    }

    fn account_of_config() -> String {
        let (s, e) = value_span(CONFIG, "oauthAccount").unwrap();
        CONFIG[s..e].to_string()
    }

    /// What Claude Code would see as the signed-in email and token.
    fn current(p: &Paths) -> (String, String) {
        let l = live(p).unwrap();
        (l.account["emailAddress"].as_str().unwrap().into(), l.oauth["accessToken"].as_str().unwrap().into())
    }

    #[test]
    fn finds_and_replaces_one_top_level_value() {
        let (s, e) = value_span(CONFIG, "oauthAccount").unwrap();
        assert!(CONFIG[s..e].starts_with('{') && CONFIG[s..e].ends_with('}') && CONFIG[s..e].contains("quoted"), "braces inside a string don't end it");
        let out = splice(CONFIG, "oauthAccount", r#"{"emailAddress":"b@x.com"}"#).unwrap();
        assert!(out.contains("b@x.com") && !out.contains("a@x.com"));
        assert_eq!(out.replace(r#"{"emailAddress":"b@x.com"}"#, &CONFIG[s..e]), CONFIG, "everything else is byte for byte the same");
        assert_eq!(splice(CONFIG, "numStartups", "9").unwrap().matches("\"numStartups\": 9,").count(), 1, "a number value ends at the comma");
        assert!(splice(CONFIG, "email", "1").is_none() && splice(CONFIG, "emailAddress", "1").is_none(), "nested keys aren't top level");
        assert!(splice(r#"{"a":1}"#, "missing", "1").is_none());
        assert!(serde_json::from_str::<Value>(&out).is_ok(), "the result is still JSON, with both case-different project keys");
    }

    #[test]
    fn identity_needs_an_email() {
        assert_eq!(ident(&json!({ "emailAddress": "a@x.com", "organizationUuid": "o" })), Some(Ident { email: "a@x.com".into(), org: "o".into() }));
        assert_eq!(ident(&json!({ "emailAddress": "" })), None);
        assert_eq!(ident(&json!({})), None);
        assert!(live_of(&json!({ "claudeAiOauth": { "accessToken": "t" } }), &json!({})).is_none());
        assert!(live_of(&json!({}), &json!({ "oauthAccount": { "emailAddress": "a@x.com" } })).is_none());
    }

    #[test]
    fn strict_reads_refuse_damaged_files() {
        let p = setup("strict", "a@x.com", "o", "t");
        std::fs::write(p.config(), "{ not json").unwrap();
        assert!(read_obj(&p.config()).is_err() && live(&p).is_none());
        std::fs::write(p.config(), "[1]").unwrap();
        assert!(read_obj(&p.config()).is_err());
        assert!(read_obj(&p.home.join("missing.json")).is_err());
    }

    #[test]
    fn locks_wait_then_give_up_and_break_old_ones() {
        let dir = std::env::temp_dir().join(format!("zorui-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let l = dir.join("a.lock");
        take(&l, Duration::from_secs(60), Duration::ZERO).unwrap();
        assert!(take(&l, Duration::from_secs(60), Duration::from_millis(150)).is_err(), "held by someone else");
        assert!(take(&l, Duration::ZERO, Duration::from_millis(150)).is_ok(), "an old lock is taken over");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_guard_frees_every_directory() {
        let p = setup("guard", "a@x.com", "o", "t");
        { let _g = lock_all(&p, Duration::from_millis(200)).unwrap(); assert!(p.locks().iter().all(|(d, _)| d.is_dir())); }
        assert!(p.locks().iter().all(|(d, _)| !d.exists()), "released on drop");
        std::fs::create_dir(&p.locks()[2].0).unwrap();
        assert!(lock_all(&p, Duration::from_millis(150)).is_err());
        assert!(!p.locks()[0].0.exists() && !p.locks()[1].0.exists(), "a failure frees the ones already taken");
    }

    #[test]
    fn save_and_switch_round_trip() {
        let p = setup("swap", "a@x.com", "org-a", "token-a");
        let (a_cfg, a_cred) = (std::fs::read_to_string(p.config()).unwrap(), std::fs::read_to_string(p.creds()).unwrap());
        assert_eq!(save_current(&p).unwrap(), 1);
        assert_eq!(save_current(&p).unwrap(), 1, "the same login keeps its slot");
        // Claude Code signs in as B
        std::fs::write(p.config(), CONFIG.replace(&account_of_config(), &account("b@x.com", "org-b"))).unwrap();
        std::fs::write(p.creds(), creds("token-b", u64::MAX / 2)).unwrap();
        assert_eq!(save_current(&p).unwrap(), 2);
        let slots = load(&p);
        assert!(!std::fs::read_to_string(&p.store).unwrap().contains("token-"), "tokens are encrypted in the store");
        assert_eq!(slots.len(), 2);

        switch(&p, 1).unwrap();
        assert_eq!(current(&p), ("a@x.com".into(), "token-a".into()));
        let (cfg, cred) = (std::fs::read_to_string(p.config()).unwrap(), std::fs::read_to_string(p.creds()).unwrap());
        assert!(cfg.contains("\"numStartups\": 7") && cfg.contains("\"tail\": [1, 2]") && cfg.contains("\"C:/Code/x\""), "the rest of .claude.json is kept");
        assert!(cred.contains("mcpOAuth") && cred.contains("\"srv\""), "MCP logins stay");
        assert_eq!(serde_json::from_str::<Value>(&cfg).unwrap()["oauthAccount"]["emailAddress"], "a@x.com");
        assert_eq!(serde_json::from_str::<Value>(&a_cfg).unwrap()["projects"], serde_json::from_str::<Value>(&cfg).unwrap()["projects"]);
        assert_eq!(serde_json::from_str::<Value>(&a_cred).unwrap()["claudeAiOauth"], serde_json::from_str::<Value>(&cred).unwrap()["claudeAiOauth"]);

        switch(&p, 2).unwrap();
        assert_eq!(current(&p), ("b@x.com".into(), "token-b".into()));
        assert!(p.locks().iter().all(|(d, _)| !d.exists()), "locks are released");
    }

    #[test]
    fn switching_keeps_tokens_claude_code_rotated() {
        let p = setup("rotate", "a@x.com", "org-a", "old-a");
        save_current(&p).unwrap();
        std::fs::write(p.config(), CONFIG.replace(&account_of_config(), &account("b@x.com", "org-b"))).unwrap();
        std::fs::write(p.creds(), creds("token-b", u64::MAX / 2)).unwrap();
        save_current(&p).unwrap();
        switch(&p, 1).unwrap();
        std::fs::write(p.creds(), creds("new-a", u64::MAX / 2)).unwrap(); // Claude Code refreshed A's token
        switch(&p, 2).unwrap();
        switch(&p, 1).unwrap();
        assert_eq!(current(&p).1, "new-a", "the rotated token came back, not the one saved first");
    }

    #[test]
    fn refuses_to_drop_an_unsaved_login_or_write_a_damaged_file() {
        let p = setup("refuse", "a@x.com", "org-a", "token-a");
        save_current(&p).unwrap();
        std::fs::write(p.config(), CONFIG.replace(&account_of_config(), &account("c@x.com", "org-c"))).unwrap();
        let before = std::fs::read_to_string(p.config()).unwrap();
        assert!(switch(&p, 1).unwrap_err().contains("isn't saved"));
        assert_eq!(std::fs::read_to_string(p.config()).unwrap(), before, "nothing was written");
        save_current(&p).unwrap();
        std::fs::write(p.config(), "{ not json").unwrap();
        assert!(switch(&p, 1).is_err());
        assert_eq!(std::fs::read_to_string(p.config()).unwrap(), "{ not json", "a damaged file is left as it is");
        assert!(switch(&p, 99).unwrap_err().contains("No such"));
    }

    #[test]
    fn remove_forgets_a_slot() {
        let p = setup("remove", "a@x.com", "org-a", "token-a");
        save_current(&p).unwrap();
        remove(&p, 1).unwrap();
        assert!(load(&p).is_empty());
    }

    #[test]
    fn token_needs_a_minute_left() {
        let o = json!({ "accessToken": "t", "expiresAt": 200_000u64 });
        assert_eq!(token_ok(&o, 100_000), Some("t".into()));
        assert_eq!(token_ok(&o, 150_000), None);
        assert_eq!(token_ok(&json!({ "accessToken": "", "expiresAt": u64::MAX / 2 }), 0), None);
        assert_eq!(token_ok(&json!({}), 0), None);
    }

    #[test]
    fn renewal_keeps_old_fields_and_takes_the_new_tokens() {
        let old = json!({ "accessToken": "old", "refreshToken": "r-old", "expiresAt": 1, "subscriptionType": "pro", "scopes": ["a"] });
        let Renewal::New(o) = read_renewal(r#"{"access_token":"new","refresh_token":"r-new","expires_in":100}"#, &old, 5_000) else { panic!("renewed") };
        assert_eq!((o["accessToken"].as_str(), o["refreshToken"].as_str(), o["expiresAt"].as_u64()), (Some("new"), Some("r-new"), Some(105_000)));
        assert_eq!((o["subscriptionType"].as_str(), o["scopes"].clone()), (Some("pro"), json!(["a"])));
        let Renewal::New(o) = read_renewal(r#"{"access_token":"new"}"#, &old, 0) else { panic!("renewed") };
        assert_eq!((o["refreshToken"].as_str(), o["expiresAt"].as_u64()), (Some("r-old"), Some(3_600_000)), "no new refresh token keeps the old, no expiry means an hour");
        assert_eq!(read_renewal(r#"{"error":"invalid_grant"}"#, &old, 0), Renewal::Dead);
        assert_eq!(read_renewal(r#"{"error":"server_error"}"#, &old, 0), Renewal::Failed);
        assert_eq!(read_renewal("<html>", &old, 0), Renewal::Failed, "a network error page isn't a dead login");
        let b: Value = serde_json::from_str(&refresh_body("rt")).unwrap();
        assert_eq!((b["grant_type"].as_str(), b["refresh_token"].as_str(), b["client_id"].as_str()), (Some("refresh_token"), Some("rt"), Some(CLIENT_ID)));
    }

    #[test]
    fn outcomes_land_on_the_slot_as_it_is_on_disk() {
        let p = setup("apply", "a@x.com", "org-a", "token-a");
        save_current(&p).unwrap();
        let account: Value = serde_json::from_str(&account("a@x.com", "org-a")).unwrap();
        let oauth = json!({ "accessToken": "renewed", "refreshToken": "r2", "expiresAt": 9 });
        let mut slots = load(&p);
        let o = Outcome { n: 1, account: account.clone(), renewed: Some(oauth.clone()), dead: false, usage: Some(crate::claude_usage::Usage { session: 5, week: 6 }) };
        apply(&mut slots, &o, 1000);
        assert_eq!(open(&slots[0]).unwrap().0, oauth, "the new tokens are what's stored");
        assert_eq!(slots[0]["usage"], json!({ "session": 5, "week": 6, "at": 1000 }));
        apply(&mut slots, &Outcome { n: 1, account, renewed: None, dead: true, usage: None }, 2000);
        assert_eq!((slots[0]["dead"].clone(), slots[0]["usage"]["at"].clone()), (json!(true), json!(1000)), "dead is marked, the last reading stays");
        apply(&mut slots, &Outcome { n: 7, account: json!({}), renewed: None, dead: true, usage: None }, 0); // a slot removed meanwhile is ignored
        assert_eq!(slots.len(), 1);
    }

    #[test]
    fn dead_slots_refuse_a_switch_until_saved_again() {
        let p = setup("dead", "a@x.com", "org-a", "token-a");
        save_current(&p).unwrap();
        std::fs::write(p.config(), CONFIG.replace(&account_of_config(), &account("b@x.com", "org-b"))).unwrap();
        std::fs::write(p.creds(), creds("token-b", u64::MAX / 2)).unwrap();
        save_current(&p).unwrap();
        let mut slots = load(&p);
        slots[0]["dead"] = json!(true);
        store(&p, &slots).unwrap();
        assert!(switch(&p, 1).unwrap_err().contains("expired"));
        std::fs::write(p.config(), CONFIG.replace(&account_of_config(), &account("a@x.com", "org-a"))).unwrap();
        std::fs::write(p.creds(), creds("token-a2", u64::MAX / 2)).unwrap();
        save_current(&p).unwrap();
        assert_eq!(load(&p)[0]["dead"], json!(false), "signing in again revives it");
    }

    fn row(n: u64, active: bool, s: u64, w: u64, age: u64) -> Value { json!({ "n": n, "email": format!("{n}@x.com"), "active": active, "session": s, "week": w, "ageMin": age }) }

    #[test]
    fn picks_the_roomiest_fresh_login_when_the_active_one_is_full() {
        let rows = [row(1, true, 96, 10, 0), row(2, false, 40, 30, 2), row(3, false, 10, 20, 1), row(4, false, 5, 5, 60)];
        assert_eq!(pick(&rows, 95), Some((3, "3@x.com".into())), "lowest worst window wins; a reading an hour old is skipped");
        assert_eq!(pick(&[row(1, true, 90, 10, 0), row(2, false, 1, 1, 0)], 95), None, "the active one still has room");
        assert_eq!(pick(&[row(1, true, 10, 97, 0), row(2, false, 1, 1, 0)], 95), Some((2, "2@x.com".into())), "the week counts too");
        assert_eq!(pick(&[row(1, true, 99, 99, 0), row(2, false, 95, 0, 0)], 95), None, "nothing under the threshold");
        let mut dead = row(2, false, 1, 1, 0);
        dead["dead"] = json!(true);
        assert_eq!(pick(&[row(1, true, 99, 0, 0), dead], 95), None, "a dead login is never chosen");
        assert_eq!(pick(&[json!({ "n": 1, "active": true, "session": null, "week": null }), row(2, false, 1, 1, 0)], 95), None, "no reading for the active one");
        assert_eq!(pick(&[row(2, false, 1, 1, 0)], 95), None, "no active login");
    }

    #[test]
    fn cooldown_is_ten_minutes() {
        assert!(cooled(5, 0));
        assert!(!cooled(1000, 500));
        assert!(cooled(1100, 500));
    }

    #[test]
    fn summary_line() {
        assert_eq!(summary(&json!({ "plan": "pro", "dead": true, "session": 1, "week": 1 })), "Login expired. Sign in with `claude`, then save it again");
        assert_eq!(summary(&json!({ "plan": "pro", "session": 23, "week": 4, "ageMin": 0 })), "Pro · 5h 23% · 7d 4% · just now");
        assert_eq!(summary(&json!({ "plan": "max", "session": 1, "week": 2, "ageMin": 12 })), "Max · 5h 1% · 7d 2% · 12 min ago");
        assert_eq!(summary(&json!({ "session": 1, "week": 2, "ageMin": 190 })), "5h 1% · 7d 2% · 3 h ago");
        assert_eq!(summary(&json!({ "plan": "pro", "session": null, "week": null })), "Pro · usage unavailable");
    }
}
