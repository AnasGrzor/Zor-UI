//! Several Claude Code logins on one PC, and a switch between them (Settings, Services, Claude accounts).
//!
//! Each saved login has its own Claude Code config folder, `claude-profiles\<n>\home`, used as CLAUDE_CONFIG_DIR
//! (see claude_profile.rs). The Default login is the user's own `~\.claude`. A switch sets which folder new terminals
//! get, so open sessions keep the account they started with. zor-ui only writes a login file to renew an expired
//! token, under Claude Code's refresh lock for that folder.
//!
//! `claude-profiles\state.json` keeps the last usage reading of each account and whether its refresh token was
//! refused (`dead`). Signing in again clears `dead` once the next read sees a valid token.
//!
//! Auto-switch (`auto_tick`, from the usage loop in app.rs): when the active login passes a threshold and another has
//! room, a banner says so and the switch happens 20 seconds later unless the banner is clicked.
//! The old one-file store (`claude-accounts.json`, DPAPI-sealed tokens) is moved into folders once, by `migrate`.

use super::{common::{err, hidden, load_json, save_json}, web::{b64_decode, dpapi}};
use super::claude_profile::{self as prof, Root, Files};
use crate::conf::Config;
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// One writer of state.json (and of the migration) at a time: Settings, the usage loop and auto-switch all write it.
static STORE: Mutex<()> = Mutex::new(());
fn store_lock() -> std::sync::MutexGuard<'static, ()> { STORE.lock().unwrap_or_else(|e| e.into_inner()) }

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

// ------------------------------------------------------------------ who a login is

/// Who a login is: the pair Claude Code itself tells accounts apart by.
#[derive(Clone, Debug, PartialEq)]
struct Ident { email: String, org: String }

fn ident(account: &Value) -> Option<Ident> {
    let email = account["emailAddress"].as_str().filter(|s| !s.is_empty())?;
    Some(Ident { email: email.into(), org: account["organizationUuid"].as_str().unwrap_or("").into() })
}

fn marker_ident(m: &Value) -> Option<Ident> {
    let email = m["email"].as_str().filter(|s| !s.is_empty())?;
    Some(Ident { email: email.into(), org: m["org"].as_str().unwrap_or("").into() })
}

/// The marker written for profile `n`.
fn marker(n: u64, id: &Ident, account: &Value, oauth: &Value) -> Value {
    json!({ "n": n, "email": id.email, "org": id.org, "orgName": account["organizationName"], "plan": oauth["subscriptionType"] })
}

/// Every login already saved, by row number: the Default login (0) and each profile's marker.
fn identities(root: &Root) -> Vec<(u64, Ident)> {
    let mut v = vec![];
    if let Some((_, account)) = prof::login_of(&Files::of(root, 0)) {
        if let Some(id) = ident(&account) { v.push((0, id)); }
    }
    for n in root.numbers() {
        if let Some(id) = prof::read_marker(root, n).as_ref().and_then(marker_ident) { v.push((n, id)); }
    }
    v
}

/// The row, other than `except`, that already holds this login (same email and organization).
fn taken_by(rows: &[(u64, Ident)], id: &Ident, except: u64) -> Option<u64> {
    rows.iter().find(|(n, r)| *n != except && r == id).map(|(n, _)| *n)
}

// ------------------------------------------------------------------ state.json

fn read_state(root: &Root) -> Value { load_json(&root.dir.join("state.json"), json!({})) }

fn write_state(root: &Root, v: &Value) -> Result<(), String> {
    let p = root.dir.join("state.json");
    if save_json(&p, v) { Ok(()) } else { Err(format!("can't write {}", p.display())) }
}

/// What one account's read or renewal found, as it goes into state.json.
struct Outcome { n: u64, dead: bool, usage: Option<crate::claude_usage::Usage> }

/// Lays one outcome over the state: `dead` always, and the usage only when a reading was taken (the last one stays otherwise).
fn apply_outcome(state: &mut Value, o: &Outcome, now: u64) {
    let s = &mut state[o.n.to_string()];
    s["dead"] = json!(o.dead);
    if let Some(u) = &o.usage { s["usage"] = json!({ "session": u.session, "week": u.week, "at": now }); }
}

// ------------------------------------------------------------------ token renewal

const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"; // Claude Code's public OAuth client

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

/// A token Claude Code still accepts (expiresAt is in milliseconds); one a minute from expiry is as good as expired.
fn token_ok(oauth: &Value, now_ms: u64) -> Option<String> {
    let t = oauth["accessToken"].as_str().filter(|t| !t.is_empty())?;
    (oauth["expiresAt"].as_u64().unwrap_or(0) > now_ms + 60_000).then(|| t.to_string())
}

/// Renews an expired login (when it isn't already known dead) and reads its usage. The renewal holds Claude Code's
/// refresh lock for that folder, and re-reads the login under it, so one side spends a refresh token at a time.
fn renew_and_read(n: u64, f: &Files, was_dead: bool, now_ms: u64) -> Outcome {
    let mut oauth = prof::login_of(f).map(|(o, _)| o);
    let mut dead = was_dead;
    if !was_dead && oauth.as_ref().is_some_and(|o| token_ok(o, now_ms).is_none())
        && take(&f.lock, Duration::from_secs(60), Duration::from_secs(5)).is_ok()
    {
        let _held = Lock(vec![f.lock.clone()]);
        if let Some((cur, account)) = prof::login_of(f) { // the other side may have renewed it while we waited
            if token_ok(&cur, now_ms).is_some() {
                oauth = Some(cur);
            } else {
                match renew(&cur, now_ms) {
                    Renewal::New(o) => { let _ = prof::write_login(f, &o, &account); oauth = Some(o); }
                    Renewal::Dead => dead = true,
                    Renewal::Failed => {}
                }
            }
        }
    }
    let token = oauth.as_ref().and_then(|o| token_ok(o, now_ms));
    // a refused login that now has a valid token (signed in again) is no longer dead
    // tests use fake tokens, which must never be sent to the real endpoint
    let dead = dead && token.is_none();
    let usage = if cfg!(test) { None } else { token.and_then(crate::claude_usage::fetch_with) };
    Outcome { n, dead, usage }
}

// ------------------------------------------------------------------ the list for Settings

fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Who a row is: the Default login's account, or a profile's marker.
fn who(root: &Root, n: u64, f: &Files) -> Value {
    if n > 0 {
        let m = prof::read_marker(root, n).unwrap_or(Value::Null);
        return json!({ "email": m["email"], "orgName": m["orgName"], "plan": m["plan"] });
    }
    match prof::login_of(f) {
        Some((oauth, account)) => json!({ "email": account["emailAddress"], "orgName": account["organizationName"], "plan": oauth["subscriptionType"] }),
        None => json!({ "email": null, "orgName": null, "plan": null }),
    }
}

/// The accounts for Settings: Default first, then each profile. Renews expired tokens and reads usage.
pub fn list(root: &Root) -> Value {
    migrate(root, &legacy_file(root));
    list_with(root, prof::active(root))
}

/// `list` with the active account passed in, so tests never read the registry.
fn list_with(root: &Root, active: u64) -> Value {
    let now = now_secs();
    let ns: Vec<u64> = std::iter::once(0).chain(root.numbers()).collect();
    let files: Vec<Files> = ns.iter().map(|&n| Files::of(root, n)).collect();
    let was_dead: Vec<bool> = {
        let _w = store_lock();
        let st = read_state(root);
        ns.iter().map(|&n| st[n.to_string()]["dead"] == true).collect()
    };
    // renew and read every login side by side (one request or two each)
    let outcomes: Vec<Outcome> = std::thread::scope(|sc| {
        let jobs: Vec<_> = ns.iter().zip(&files).zip(&was_dead)
            .map(|((&n, f), &d)| (n, sc.spawn(move || renew_and_read(n, f, d, now * 1000)))).collect();
        jobs.into_iter().map(|(n, j)| j.join().unwrap_or(Outcome { n, dead: false, usage: None })).collect()
    });
    let state = {
        let _w = store_lock();
        let mut st = read_state(root);
        let current = root.numbers(); // a profile removed while the requests were out must not come back
        for o in outcomes.iter().filter(|o| o.n == 0 || current.contains(&o.n)) { apply_outcome(&mut st, o, now); }
        let _ = write_state(root, &st);
        st
    };
    let rows: Vec<Value> = ns.iter().zip(&files).map(|(&n, f)| {
        let w = who(root, n, f);
        let u = &state[n.to_string()]["usage"];
        json!({
            "n": n, "default": n == 0, "email": w["email"], "orgName": w["orgName"], "plan": w["plan"],
            "active": active == n, "dead": state[n.to_string()]["dead"] == true,
            "session": u["session"], "week": u["week"],
            "ageMin": u["at"].as_u64().map(|at| now.saturating_sub(at) / 60),
        })
    }).collect();
    json!({ "accounts": rows })
}

// ------------------------------------------------------------------ migration from the one-file store

/// The old store, beside the profiles folder. Derived from `root` so tests never reach the real one.
fn legacy_file(root: &Root) -> PathBuf { root.dir.parent().unwrap_or(&root.dir).join("claude-accounts.json") }

/// Moves the old one-file store into account folders, once. A slot that fails stays in the old file, so the next
/// list tries again. A slot whose login is already saved (Default or a profile) counts as done.
fn migrate(root: &Root, file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    let Ok(v) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else { return };
    let _w = store_lock();
    let mut all_ok = true;
    for slot in v["slots"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        all_ok &= migrate_slot(root, slot).is_ok();
    }
    if all_ok { let _ = std::fs::rename(file, file.with_file_name("claude-accounts.json.migrated")); }
}

fn migrate_slot(root: &Root, slot: &Value) -> Result<(), String> {
    let (oauth, account) = open(slot).ok_or("Can't decrypt a saved login. It was saved by another Windows user or PC.")?;
    let id = ident(&account).ok_or("A saved login has no email")?;
    if taken_by(&identities(root), &id, u64::MAX).is_some() { return Ok(()); }
    let n = root.next();
    let made = (|| -> Result<(), String> {
        let home = prof::create(root, n)?;
        prof::write_login(&Files::of_profile(&home), &oauth, &account)?;
        prof::write_marker(root, n, &marker(n, &id, &account, &oauth))
    })();
    if made.is_err() { let _ = prof::remove(root, n); } // never leave a half-made folder behind
    made
}

/// DPAPI-decrypts a slot of the old store. Only migration reads it now.
fn open(slot: &Value) -> Option<(Value, Value)> {
    let v: Value = serde_json::from_slice(&dpapi(&b64_decode(slot["blob"].as_str()?), false)?).ok()?;
    Some((v["claudeAiOauth"].clone(), v["oauthAccount"].clone()))
}

// ------------------------------------------------------------------ add, relogin, remove, switch

/// Creates the next account folder and opens a terminal to sign in there. A watcher then keeps the login as a row.
pub fn add(root: &Root) -> Result<(), String> {
    let n = root.next();
    let made = prof::create(root, n).and_then(|_| prof::open_login(root, n));
    if let Err(e) = made { let _ = prof::remove(root, n); return Err(e); }
    std::thread::spawn(move || watch_login(n));
    Ok(())
}

/// Waits up to five minutes for the sign-in in account `n`'s folder, then keeps it as a row.
fn watch_login(n: u64) {
    let root = Root::live();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_secs(2));
        if let Some((oauth, account)) = prof::login_of(&Files::of(&root, n)) {
            finish_add(&root, n, &oauth, &account);
            return;
        }
    }
    let _ = prof::remove(&root, n);
}

fn finish_add(root: &Root, n: u64, oauth: &Value, account: &Value) {
    let Some(id) = ident(account) else { return };
    if taken_by(&identities(root), &id, n).is_some() {
        let _ = prof::remove(root, n);
        banner("That account is already saved".into(), "", 8);
        return;
    }
    match prof::write_marker(root, n, &marker(n, &id, account, oauth)) {
        Ok(()) => banner(format!("Claude account added: {}", id.email), "", 6),
        Err(e) => {
            let _ = prof::remove(root, n);
            banner(format!("Couldn't save that account: {e}"), "", 8);
        }
    }
}

/// Opens a terminal to sign in again in account `n`'s folder (0 = Default).
pub fn relogin(root: &Root, n: u64) -> Result<(), String> {
    if n != 0 && !root.numbers().contains(&n) { return Err("No such account".into()); }
    prof::open_login(root, n)
}

/// Why account `n` can't be switched to, if it can't.
fn switch_error(root: &Root, n: u64, dead: bool) -> Option<String> {
    if n != 0 && !root.numbers().contains(&n) { return Some("No such account".into()); }
    if dead { return Some("That login has expired. Use Sign in again.".into()); }
    if n != 0 && prof::login_of(&Files::of(root, n)).is_none() { return Some("That account isn't signed in. Use Sign in again.".into()); }
    None
}

/// Makes account `n` (0 = Default) the one new terminals get.
pub fn switch(root: &Root, n: u64) -> Result<(), String> {
    let dead = { let _w = store_lock(); read_state(root)[n.to_string()]["dead"] == true };
    if let Some(e) = switch_error(root, n, dead) { return Err(e); }
    prof::select(root, n)
}

/// Deletes profile `n` (never Default). If it is the active account, Default is selected first.
pub fn remove(root: &Root, n: u64) -> Result<(), String> {
    if n == 0 || !root.numbers().contains(&n) { return Err("No such account".into()); }
    let _w = store_lock();
    if prof::active(root) == n { prof::select(root, 0)?; }
    prof::remove(root, n)?;
    let mut st = read_state(root);
    if let Some(o) = st.as_object_mut() { o.remove(&n.to_string()); }
    write_state(root, &st)
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

pub(super) fn banner(text: String, open: &str, secs: u64) {
    let open = open.to_string();
    crate::app::post(Box::new(move |cx| cx.notif = Some(crate::app::Notif { app: "Zor UI".into(), text, until: crate::hub::now_ms() + secs as f64 * 1000.0, open })));
}

/// One check of auto-switch; blocks for the countdown when it decides to switch. Runs on the usage loop's thread.
pub fn auto_tick(cfg: &Config) {
    if cfg.get("claude.autoSwitch").as_bool() != Some(true) || !cooled(now_secs(), LAST.load(Ordering::Relaxed)) { return; }
    let threshold = cfg.get("claude.threshold").as_u64().unwrap_or(95).clamp(50, 100);
    let root = Root::live();
    let Some((n, email)) = pick(list(&root)["accounts"].as_array().map(Vec::as_slice).unwrap_or(&[]), threshold) else { return };
    CANCELLED.store(false, Ordering::Relaxed);
    banner(format!("Claude passed {threshold}%. New sessions will use {email} in {COUNTDOWN_SECS} s. Click to cancel."), CANCEL, COUNTDOWN_SECS);
    for _ in 0..COUNTDOWN_SECS {
        std::thread::sleep(Duration::from_secs(1));
        if CANCELLED.load(Ordering::Relaxed) || crate::conf::Config::load().get("claude.autoSwitch").as_bool() != Some(true) {
            LAST.store(now_secs(), Ordering::Relaxed); // a "no" holds for the cooldown, so it isn't asked again at once
            return;
        }
    }
    LAST.store(now_secs(), Ordering::Relaxed);
    match switch(&root, n) {
        Ok(()) => {
            let restarted = if restart_wanted(cfg) { format!(" {}", restart_sessions(cfg, &root, n)) } else { String::new() };
            banner(format!("New Claude sessions will use {email}.{restarted}"), "", 8);
            if let Some(u) = crate::claude_usage::fetch() { crate::app::post(Box::new(move |cx| cx.claude = Some(u))); }
        }
        Err(e) => banner(format!("Couldn't switch Claude: {e}"), "", 8),
    }
}

/// The grey line under an account: plan, the two usage windows, and how old that reading is.
pub(crate) fn summary(row: &Value) -> String {
    if row["dead"] == true { return "Login expired. Click Sign in to log in again".into(); }
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

fn restart_wanted(cfg: &Config) -> bool { cfg.get("claude.restartSessions").as_bool() == Some(true) }

/// Restarts open Claude sessions so they run as account `n`; one line saying what happened.
fn restart_sessions(cfg: &Config, root: &Root, n: u64) -> String {
    let Some(how) = super::agent_hook::resume_of("claude", cfg.get("customAgents")) else { return String::new() };
    let dir = prof::config_dir_value(root, n).map(|p| p.to_string_lossy().into_owned());
    let r = super::agent_restart::restart(&super::agent_hook::sessions_dir("claude"), how, vec![("CLAUDE_CONFIG_DIR".into(), dir)]);
    super::agent_restart::summary(&r)
}

pub fn call(cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    let root = Root::live();
    let n = || args.first().and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
    Some(match name {
        "claude-accounts" => list(&root),
        "claude-switch" => {
            let n = n();
            let r = switch(&root, n);
            // the bar and Gojo show the active login's usage: read it now instead of at the next 3 minute tick
            if r.is_ok() { if let Some(u) = crate::claude_usage::fetch() { crate::app::post(Box::new(move |cx| cx.claude = Some(u))); } }
            match r {
                Ok(()) if restart_wanted(cfg) => json!({ "ok": true, "message": format!("New terminals will use this account. {}", restart_sessions(cfg, &root, n)) }),
                r => done(r),
            }
        }
        "claude-restart" => json!({ "ok": true, "message": restart_sessions(cfg, &root, prof::active(&root)) }),
        "claude-add" => done(add(&root)),
        "claude-relogin" => done(relogin(&root, n())),
        "claude-remove" => done(remove(&root, n())),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::web::b64_encode;

    const FOREVER: u64 = u64::MAX / 2;

    /// A fresh temp home and profiles folder. Nothing outside it is read or written.
    fn temp_root(tag: &str) -> Root {
        let base = std::env::temp_dir().join(format!("zorui-acct-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = Root { home: base.join("home"), dir: base.join("data").join("claude-profiles") };
        std::fs::create_dir_all(root.home.join(".claude")).unwrap();
        std::fs::create_dir_all(&root.dir).unwrap();
        root
    }

    fn oauth(token: &str, expires_at: u64) -> Value {
        json!({ "accessToken": token, "refreshToken": "r", "expiresAt": expires_at, "subscriptionType": "max" })
    }

    fn account(email: &str, org: &str) -> Value { json!({ "emailAddress": email, "organizationUuid": org, "organizationName": "Org" }) }

    /// A profile folder signed in as `email`, with its marker, as a finished sign-in leaves it.
    fn make_profile(root: &Root, n: u64, email: &str, org: &str) {
        let home = prof::create(root, n).unwrap();
        prof::write_login(&Files::of_profile(&home), &oauth("tok", FOREVER), &account(email, org)).unwrap();
        prof::write_marker(root, n, &json!({ "n": n, "email": email, "org": org, "orgName": "Org", "plan": "max" })).unwrap();
    }

    /// A slot of the old one-file store, sealed the way it was (DPAPI over the two values, then base64).
    fn sealed(email: &str, org: &str) -> Value {
        let body = json!({ "claudeAiOauth": oauth(&format!("tok-{email}"), FOREVER), "oauthAccount": account(email, org) });
        let blob = dpapi(body.to_string().as_bytes(), true).map(|b| b64_encode(&b)).unwrap();
        json!({ "email": email, "org": org, "blob": blob })
    }

    fn legacy(root: &Root) -> PathBuf { legacy_file(root) }

    #[test]
    fn identity_needs_an_email() {
        assert_eq!(ident(&json!({ "emailAddress": "a@x.com", "organizationUuid": "o" })), Some(Ident { email: "a@x.com".into(), org: "o".into() }));
        assert_eq!(ident(&json!({ "emailAddress": "" })), None);
        assert_eq!(ident(&json!({})), None);
    }

    #[test]
    fn same_login_is_matched_by_email_and_org() {
        let id = |e: &str, o: &str| Ident { email: e.into(), org: o.into() };
        let rows = [(0, id("a@x.com", "org-a")), (2, id("b@x.com", "org-b"))];
        assert_eq!(taken_by(&rows, &id("a@x.com", "org-a"), 1), Some(0), "the Default login is taken");
        assert_eq!(taken_by(&rows, &id("a@x.com", "org-b"), 1), None, "the same email in another organization is another login");
        assert_eq!(taken_by(&rows, &id("b@x.com", "org-b"), 2), None, "a row never matches itself");
        assert_eq!(taken_by(&rows, &id("c@x.com", "org-a"), 1), None);
    }

    #[test]
    fn state_keeps_usage_and_dead_flags() {
        let root = temp_root("state");
        let mut st = json!({});
        apply_outcome(&mut st, &Outcome { n: 1, dead: false, usage: Some(crate::claude_usage::Usage { session: 5, week: 6 }) }, 1000);
        assert_eq!(st["1"], json!({ "dead": false, "usage": { "session": 5, "week": 6, "at": 1000 } }));
        apply_outcome(&mut st, &Outcome { n: 1, dead: true, usage: None }, 2000);
        assert_eq!((st["1"]["dead"].clone(), st["1"]["usage"]["at"].clone()), (json!(true), json!(1000)), "dead is set and the last reading stays");
        write_state(&root, &st).unwrap();
        assert_eq!(read_state(&root), st, "state.json round-trips");
        apply_outcome(&mut st, &Outcome { n: 1, dead: false, usage: None }, 3000);
        assert_eq!(st["1"]["dead"], json!(false), "a valid login clears dead");
    }

    #[test]
    fn switch_refuses_unknown_expired_and_unsigned_accounts() {
        let root = temp_root("switch");
        assert_eq!(switch_error(&root, 9, false).as_deref(), Some("No such account"));
        assert!(switch_error(&root, 0, true).unwrap().contains("Use Sign in again"));
        prof::create(&root, 1).unwrap();
        prof::write_marker(&root, 1, &json!({ "n": 1, "email": "b@x.com", "org": "org-b" })).unwrap();
        assert!(switch_error(&root, 1, false).unwrap().contains("isn't signed in"), "a folder with no login can't be switched to");
        make_profile(&root, 2, "c@x.com", "org-c");
        assert_eq!(switch_error(&root, 2, false), None);
        assert_eq!(switch_error(&root, 0, false), None, "Default needs no profile login");
    }

    #[test]
    fn migration_skips_the_default_login_and_moves_the_rest() {
        let root = temp_root("migrate");
        prof::write_login(&Files::of(&root, 0), &oauth("default", FOREVER), &account("a@x.com", "org-a")).unwrap();
        let file = legacy(&root);
        std::fs::write(&file, json!({ "slots": [sealed("a@x.com", "org-a"), sealed("b@x.com", "org-b")] }).to_string()).unwrap();
        migrate(&root, &file);
        assert_eq!(root.numbers(), vec![1], "the Default login gets no profile; the other one becomes profile 1");
        assert_eq!(prof::read_marker(&root, 1).unwrap()["email"], "b@x.com");
        assert_eq!(prof::login_of(&Files::of(&root, 1)).unwrap().1["emailAddress"], "b@x.com");
        assert!(!file.exists() && file.with_file_name("claude-accounts.json.migrated").exists(), "a finished migration renames the file");
    }

    #[test]
    fn a_failed_slot_keeps_the_old_file_and_leaves_no_folder() {
        let root = temp_root("migrate-fail");
        let file = legacy(&root);
        let bad = json!({ "email": "c@x.com", "org": "org-c", "blob": "AAAA" });
        std::fs::write(&file, json!({ "slots": [sealed("b@x.com", "org-b"), bad] }).to_string()).unwrap();
        migrate(&root, &file);
        assert!(file.exists(), "a failed slot keeps the file for the next try");
        assert_eq!(root.numbers(), vec![1], "the good slot moved and the bad one left no folder");
        migrate(&root, &file);
        assert_eq!(root.numbers(), vec![1], "a retry doesn't move the good slot twice");
    }

    #[test]
    fn list_has_the_default_row_then_each_profile() {
        let root = temp_root("list");
        prof::write_login(&Files::of(&root, 0), &oauth("tok-a", FOREVER), &account("a@x.com", "org-a")).unwrap();
        make_profile(&root, 1, "b@x.com", "org-b");
        let v = list_with(&root, 0);
        let rows = v["accounts"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0]["n"].clone(), rows[0]["default"].clone(), rows[0]["email"].clone(), rows[0]["active"].clone()),
            (json!(0), json!(true), json!("a@x.com"), json!(true))
        );
        assert_eq!(
            (rows[1]["n"].clone(), rows[1]["default"].clone(), rows[1]["email"].clone(), rows[1]["plan"].clone(), rows[1]["active"].clone(), rows[1]["dead"].clone()),
            (json!(1), json!(false), json!("b@x.com"), json!("max"), json!(false), json!(false))
        );
        assert!(rows[1]["session"].is_null(), "no reading comes back for a fake token, so usage is null");
    }

    #[test]
    fn remove_refuses_default_and_unknown_accounts() {
        let root = temp_root("remove");
        assert!(remove(&root, 0).is_err());
        assert_eq!(remove(&root, 5), Err("No such account".into()));
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
    fn lock_guard_removes_its_directory() {
        let dir = std::env::temp_dir().join(format!("zorui-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let l = dir.join("x.lock");
        take(&l, Duration::from_secs(60), Duration::ZERO).unwrap();
        { let _g = Lock(vec![l.clone()]); assert!(l.is_dir()); }
        assert!(!l.exists(), "released on drop");
        let _ = std::fs::remove_dir_all(&dir);
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
    fn cooldown_is_ten_minutes() {
        assert!(cooled(5, 0));
        assert!(!cooled(1000, 500));
        assert!(cooled(1100, 500));
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
    fn summary_line() {
        assert_eq!(summary(&json!({ "plan": "pro", "dead": true, "session": 1, "week": 1 })), "Login expired. Click Sign in to log in again");
        assert_eq!(summary(&json!({ "plan": "pro", "session": 23, "week": 4, "ageMin": 0 })), "Pro · 5h 23% · 7d 4% · just now");
        assert_eq!(summary(&json!({ "plan": "max", "session": 1, "week": 2, "ageMin": 12 })), "Max · 5h 1% · 7d 2% · 12 min ago");
        assert_eq!(summary(&json!({ "session": 1, "week": 2, "ageMin": 190 })), "5h 1% · 7d 2% · 3 h ago");
        assert_eq!(summary(&json!({ "plan": "pro", "session": null, "week": null })), "Pro · usage unavailable");
    }
}
