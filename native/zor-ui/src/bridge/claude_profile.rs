//! One Claude Code config folder per saved login (`claude-profiles\<n>\home`, used as CLAUDE_CONFIG_DIR), the shared
//! state each folder links back to the user's own `~\.claude`, and the user environment variable that picks the
//! folder new terminals get. Nothing here edits the user's own login files; they are only read, and copied from.

use super::common::hidden;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    os::windows::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::Command,
};
use windows::{
    core::w,
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, LPARAM, WPARAM},
        System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ},
        UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE},
    },
};

/// Shared folders inside `~\.claude`, each linked into every account folder.
const SHARED_DIRS: [&str; 13] = [
    "projects", "todos", "plans", "file-history", "shell-snapshots", "session-env", "tasks", "paste-cache", "transcripts", "commands", "agents", "skills", "plugins",
];
/// The user's own CLAUDE_CONFIG_DIR, saved the first time zor-ui replaces it.
const OWN_FILE: &str = "user-config-dir.txt";
const NESTED: &str = "The account folder and your own Claude config folder can't contain each other";
const CREATE_NEW_CONSOLE: u32 = 0x10;

/// Where everything lives. Tests build one over temp folders.
pub struct Root {
    pub home: PathBuf,
    pub dir: PathBuf,
}

impl Root {
    pub fn live() -> Root { Root { home: crate::conf::home(), dir: super::common::root().join("claude-profiles") } }
    pub fn profile_home(&self, n: u64) -> PathBuf { self.dir.join(n.to_string()).join("home") }
    pub fn marker(&self, n: u64) -> PathBuf { self.dir.join(n.to_string()).join("profile.json") }
    pub fn numbers(&self) -> Vec<u64> {
        let mut v: Vec<u64> = folders(&self.dir).into_iter().filter(|n| self.marker(*n).is_file()).collect();
        v.sort();
        v
    }
    pub fn next(&self) -> u64 { folders(&self.dir).into_iter().max().unwrap_or(0) + 1 }
    pub fn default_dir(&self) -> PathBuf { saved_own(&self.dir).unwrap_or_else(|| self.home.join(".claude")) }
}

/// The numbers of the `<n>` folders directly under `dir` ("01" is not a folder zor-ui makes).
fn folders(dir: &Path) -> Vec<u64> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n: u64 = name.parse().ok()?;
            (n.to_string() == name).then_some(n)
        })
        .collect()
}

/// The two files of a login, plus Claude Code's refresh lock directory for it.
pub struct Files {
    pub creds: PathBuf,
    pub config: PathBuf,
    pub lock: PathBuf,
}

impl Files {
    pub fn of_profile(home: &Path) -> Files {
        Files { creds: home.join(".credentials.json"), config: home.join(".claude.json"), lock: home.join(".oauth_refresh.lock") }
    }
    pub fn of_default(root: &Root) -> Files {
        let d = root.default_dir();
        // with no custom folder, Claude Code keeps the identity next to the home folder, not inside .claude
        let config = if key_of(&d) == key_of(&root.home.join(".claude")) { root.home.join(".claude.json") } else { d.join(".claude.json") };
        Files { creds: d.join(".credentials.json"), config, lock: d.join(".oauth_refresh.lock") }
    }
    pub fn of(root: &Root, n: u64) -> Files {
        if n == 0 { Files::of_default(root) } else { Files::of_profile(&root.profile_home(n)) }
    }
}

/// (claudeAiOauth, oauthAccount) when the login has a non-empty access token and an email. None on any problem.
pub fn login_of(f: &Files) -> Option<(Value, Value)> {
    let oauth = read_obj(&f.creds).ok()?["claudeAiOauth"].clone();
    let account = read_obj(&f.config).ok()?["oauthAccount"].clone();
    let token = oauth["accessToken"].as_str().is_some_and(|t| !t.is_empty());
    let email = account["emailAddress"].as_str().is_some_and(|e| !e.is_empty());
    (token && email).then_some((oauth, account))
}

/// Writes claudeAiOauth and oauthAccount, keeping every other key. Both files are read before either is written,
/// so a damaged file leaves both as they were.
pub fn write_login(f: &Files, oauth: &Value, account: &Value) -> Result<(), String> {
    let mut creds = read_or_empty(&f.creds)?;
    let mut config = read_or_empty(&f.config)?;
    creds["claudeAiOauth"] = oauth.clone();
    config["oauthAccount"] = account.clone();
    write_json(&f.creds, &creds)?;
    write_json(&f.config, &config)
}

/// Creates dir\<n>\home, links the shared state, seeds .claude.json and CLAUDE.md. Returns the home. No marker.
pub fn create(root: &Root, n: u64) -> Result<PathBuf, String> {
    let dir = profile_dir(root, n)?;
    let home = root.profile_home(n);
    safe_under(&root.dir, &home)?;
    if nested(&home, &root.default_dir()) { return Err(NESTED.into()); }
    fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    fs::create_dir_all(&home).map_err(|e| format!("can't create {}: {e}", home.display()))?;
    for w in link_shared(root, &home) { eprintln!("claude profile {n}: {w}"); }
    seed(root, &home)?;
    Ok(home)
}

/// Copies .claude.json (without the account) and CLAUDE.md into a new home; existing files are kept.
fn seed(root: &Root, home: &Path) -> Result<(), String> {
    let cfg = home.join(".claude.json");
    if !cfg.exists() {
        let mut v = read_obj(&Files::of_default(root).config).unwrap_or_else(|_| json!({}));
        if let Some(o) = v.as_object_mut() { o.remove("oauthAccount"); }
        write_json(&cfg, &v)?;
    }
    let md = home.join("CLAUDE.md");
    if !md.exists() { write_atomic(&md, "@~/.claude/CLAUDE.md\n")?; }
    Ok(())
}

/// Links shared state from the default folder into a profile home. Returns warnings; never deletes user data.
pub fn link_shared(root: &Root, home: &Path) -> Vec<String> {
    let def = root.default_dir();
    if nested(home, &def) { return vec![NESTED.into()]; }
    if let Err(e) = fs::create_dir_all(home) { return vec![format!("can't create {}: {e}", home.display())]; }
    let mut warn: Vec<String> = SHARED_DIRS.iter().filter_map(|name| link_dir(&def.join(name), &home.join(name)).err()).collect();
    warn.extend(copy_settings(&def, home).err());
    warn.extend(share_history(&def, home).err());
    warn
}

/// A junction from `link` to `target`, made when `link` is missing or an empty folder; anything else is left alone.
fn link_dir(target: &Path, link: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| format!("can't create {}: {e}", target.display()))?;
    match fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            if points_at(link, target) { Ok(()) } else { Err(format!("{} links somewhere else, left alone", link.display())) }
        }
        Ok(m) if m.is_dir() && is_empty_dir(link) => {
            fs::remove_dir(link).map_err(|e| format!("can't replace the empty {}: {e}", link.display()))?;
            mklink(link, target)
        }
        Ok(m) if m.is_dir() => Err(format!("{} has files of its own, left alone", link.display())),
        Ok(_) => Err(format!("{} is a file, left alone", link.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => mklink(link, target),
        Err(e) => Err(format!("can't look at {}: {e}", link.display())),
    }
}

/// A directory junction. mklink is used because it needs no admin rights.
fn mklink(link: &Path, target: &Path) -> Result<(), String> {
    let out = hidden("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).output().map_err(|e| format!("can't run mklink: {e}"))?;
    if out.status.success() { return Ok(()); }
    Err(format!("can't link {} to {}: {}", link.display(), target.display(), String::from_utf8_lossy(&out.stdout).trim()))
}

fn points_at(link: &Path, target: &Path) -> bool {
    matches!((fs::canonicalize(link), fs::canonicalize(target)), (Ok(a), Ok(b)) if a == b)
}

fn is_empty_dir(p: &Path) -> bool { fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(false) }

/// settings.json is copied, not linked: Claude Code saves it by writing a new file, which would break a hard link.
fn copy_settings(def: &Path, home: &Path) -> Result<(), String> {
    let (src, dst) = (def.join("settings.json"), home.join("settings.json"));
    if !src.is_file() { return Ok(()); }
    let text = fs::read(&src).map_err(|e| format!("can't read {}: {e}", src.display()))?;
    if fs::read(&dst).ok().as_deref() == Some(text.as_slice()) { return Ok(()); }
    fs::copy(&src, &dst).map(|_| ()).map_err(|e| format!("can't copy settings to {}: {e}", dst.display()))
}

/// history.jsonl is a hard link to the shared file. If Claude Code replaced the link with its own file, that file's
/// new lines are merged into the shared one first, so nothing is lost.
fn share_history(def: &Path, home: &Path) -> Result<(), String> {
    let (shared, prof) = (def.join("history.jsonl"), home.join("history.jsonl"));
    if !shared.exists() { fs::File::create(&shared).map_err(|e| format!("can't create {}: {e}", shared.display()))?; }
    if !prof.exists() { return hard(&shared, &prof); }
    let sb = fs::read(&shared).map_err(|e| format!("can't read {}: {e}", shared.display()))?;
    let pb = fs::read(&prof).map_err(|e| format!("can't read {}: {e}", prof.display()))?;
    if sb == pb { return Ok(()); }
    let add = merge_history(&String::from_utf8_lossy(&sb), &String::from_utf8_lossy(&pb));
    if !add.is_empty() {
        fs::OpenOptions::new()
            .append(true)
            .open(&shared)
            .and_then(|mut f| f.write_all(add.as_bytes()))
            .map_err(|e| format!("can't add to {}: {e}", shared.display()))?;
    }
    fs::remove_file(&prof).map_err(|e| format!("can't replace {}: {e}", prof.display()))?;
    hard(&shared, &prof)
}

fn hard(shared: &Path, prof: &Path) -> Result<(), String> {
    fs::hard_link(shared, prof).map_err(|e| format!("can't link {}: {e}", prof.display()))
}

/// The lines of `profile` that `shared` doesn't have yet, each newline-ended, ready to append to `shared`.
fn merge_history(shared: &str, profile: &str) -> String {
    let mut seen: HashSet<&str> = shared.lines().collect();
    let mut out = String::new();
    for line in profile.lines() {
        if line.is_empty() || !seen.insert(line) { continue; }
        out.push_str(line);
        out.push('\n');
    }
    if !out.is_empty() && !shared.is_empty() && !shared.ends_with('\n') { out.insert(0, '\n'); }
    out
}

/// Deletes dir\<n>. Links inside the home are removed as links first, so the shared folders behind them survive.
pub fn remove(root: &Root, n: u64) -> Result<(), String> {
    let dir = profile_dir(root, n)?;
    if !dir.exists() { return Ok(()); }
    if let Ok(rd) = fs::read_dir(dir.join("home")) {
        for e in rd.flatten() {
            let p = e.path();
            if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_dir(&p).or_else(|_| fs::remove_file(&p)).map_err(|e| format!("can't unlink {}: {e}", p.display()))?;
            }
        }
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("can't delete {}: {e}", dir.display()))
}

pub fn write_marker(root: &Root, n: u64, v: &Value) -> Result<(), String> {
    profile_dir(root, n)?;
    safe_under(&root.dir, &root.marker(n))?;
    write_json(&root.marker(n), v)
}

pub fn read_marker(root: &Root, n: u64) -> Option<Value> {
    serde_json::from_str::<Value>(fs::read_to_string(root.marker(n)).ok()?.trim_start_matches('\u{feff}')).ok()
}

/// Which account new terminals get: n when the user variable is the home of an existing profile, else 0.
pub fn active(root: &Root) -> u64 {
    let Some(cur) = env_get() else { return 0 };
    root.numbers().into_iter().find(|n| key(&cur) == key_of(&root.profile_home(*n))).unwrap_or(0)
}

/// The CLAUDE_CONFIG_DIR value for account n: its home for n > 0; for 0 the user's own saved value, or None.
pub fn config_dir_value(root: &Root, n: u64) -> Option<PathBuf> {
    if n > 0 { Some(root.profile_home(n)) } else { saved_own(&root.dir) }
}

/// Makes account n the one new terminals use. The user's own value is saved the first time it is replaced.
pub fn select(root: &Root, n: u64) -> Result<(), String> {
    if n > 0 {
        if read_marker(root, n).is_none() { return Err("No such account".into()); }
        let home = root.profile_home(n);
        safe_under(&root.dir, &home)?;
        if nested(&home, &root.default_dir()) { return Err(NESTED.into()); }
        for warn in link_shared(root, &home) { eprintln!("claude profile {n}: {warn}"); }
    }
    let target = config_dir_value(root, n).map(|p| p.to_string_lossy().into_owned());
    let current = env_get();
    let plan = env_plan(current.as_deref(), &root.dir, root.dir.join(OWN_FILE).is_file(), target.as_deref());
    if let Some(own) = plan.save_own {
        fs::create_dir_all(&root.dir).map_err(|e| format!("can't create {}: {e}", root.dir.display()))?;
        write_atomic(&root.dir.join(OWN_FILE), &own)?;
    }
    match &plan.set {
        Some(v) => env_set(v)?,
        None => env_delete()?,
    }
    broadcast_env();
    Ok(())
}

/// Opens a new terminal that runs `claude /login` in account n's folder.
pub fn open_login(root: &Root, n: u64) -> Result<(), String> {
    let cmd = login_command(config_dir_value(root, n).as_deref());
    // Windows Terminal first; plain cmd in its own console when it isn't installed
    if Command::new("wt.exe").args(["-w", "new", "cmd", "/k"]).raw_arg(&cmd).spawn().is_ok() { return Ok(()); }
    Command::new("cmd.exe")
        .raw_arg(format!("/k {cmd}"))
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("can't open a terminal: {e}"))
}

/// The command line a login terminal runs: sets CLAUDE_CONFIG_DIR (cleared when None), then `claude /login`.
fn login_command(dir: Option<&Path>) -> String {
    format!(r#"set "CLAUDE_CONFIG_DIR={}" && claude /login"#, dir.map(|d| d.display().to_string()).unwrap_or_default())
}

// ------------------------------------------------------------------ the user environment variable

/// What a switch does to HKCU CLAUDE_CONFIG_DIR: `save_own` is the user's own value to keep first (None when there is
/// nothing to keep), `set` the value to write (None deletes it).
#[derive(Debug, PartialEq)]
struct EnvPlan {
    save_own: Option<String>,
    set: Option<String>,
}

/// `current` is the variable now, `target` the one for the new account; `own_saved` says the user's value was kept.
fn env_plan(current: Option<&str>, dir: &Path, own_saved: bool, target: Option<&str>) -> EnvPlan {
    let save_own = current.filter(|c| !own_saved && !inside(c, dir)).map(str::to_string);
    EnvPlan { save_own, set: target.map(str::to_string) }
}

fn env_get() -> Option<String> {
    let mut buf = vec![0u16; 32_768];
    let mut bytes = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Environment"), w!("CLAUDE_CONFIG_DIR"), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)).ok().ok()?;
    }
    let n = (bytes as usize / 2).min(buf.len());
    let v = String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string();
    (!v.is_empty()).then_some(v)
}

fn env_set(v: &str) -> Result<(), String> {
    let d: Vec<u16> = v.encode_utf16().chain(Some(0)).collect();
    let r = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, w!("Environment"), w!("CLAUDE_CONFIG_DIR"), REG_SZ.0, Some(d.as_ptr().cast()), (d.len() * 2) as u32) };
    r.ok().map_err(|e| format!("can't set CLAUDE_CONFIG_DIR: {e}"))
}

fn env_delete() -> Result<(), String> {
    let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, w!("Environment"), w!("CLAUDE_CONFIG_DIR")) };
    if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(format!("can't clear CLAUDE_CONFIG_DIR: error {}", r.0)) }
}

fn broadcast_env() {
    unsafe {
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("Environment").as_ptr() as isize), SMTO_ABORTIFHUNG, 1000, None);
    }
}

// ------------------------------------------------------------------ paths and checks

/// dir\<n> for a real account (never 0), refusing links on the way.
fn profile_dir(root: &Root, n: u64) -> Result<PathBuf, String> {
    if n == 0 { return Err("Default has no account folder".into()); }
    let d = root.dir.join(n.to_string());
    safe_under(&root.dir, &d)?;
    Ok(d)
}

/// Refuses a target that isn't under `dir`, or has a link between `dir` and itself (a junction would lead outside).
fn safe_under(dir: &Path, target: &Path) -> Result<(), String> {
    let rel = target.strip_prefix(dir).map_err(|_| format!("{} is outside {}", target.display(), dir.display()))?;
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) { return Err(format!("{} isn't a plain path under {}", target.display(), dir.display())); }
    let mut p = dir.to_path_buf();
    for c in rel.components() {
        p.push(c);
        if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{} is a link, so it isn't used", p.display()));
        }
    }
    Ok(())
}

/// A path as text for comparing: case-insensitive, no trailing separator.
fn key(p: &str) -> String { p.trim_end_matches(['\\', '/']).to_lowercase() }
fn key_of(p: &Path) -> String { key(&p.to_string_lossy()) }

/// True when `a` is `b` or sits inside it.
fn inside(a: &str, b: &Path) -> bool {
    let (a, b) = (key(a), key_of(b));
    a == b || a.starts_with(&format!("{b}\\"))
}

fn nested(a: &Path, b: &Path) -> bool { inside(&a.to_string_lossy(), b) || inside(&b.to_string_lossy(), a) }

fn saved_own(dir: &Path) -> Option<PathBuf> {
    let t = fs::read_to_string(dir.join(OWN_FILE)).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

// ------------------------------------------------------------------ JSON files

/// A JSON object file read strictly: a missing, unreadable or non-object file is an error, never an empty default.
fn read_obj(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{} isn't valid JSON: {e}", path.display()))?;
    if !v.is_object() { return Err(format!("{} isn't a JSON object", path.display())); }
    Ok(v)
}

fn read_or_empty(path: &Path) -> Result<Value, String> {
    if !path.exists() { return Ok(json!({})); }
    read_obj(path)
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    if let Some(d) = path.parent() { fs::create_dir_all(d).map_err(|e| format!("can't create {}: {e}", d.display()))?; }
    write_atomic(path, &serde_json::to_string_pretty(v).unwrap_or_default())
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("zor-tmp");
    fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, path)).map_err(|e| format!("can't write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh user home and account root under the temp folder. Never the real home, registry or LOCALAPPDATA.
    fn fresh(tag: &str) -> Root {
        let base = std::env::temp_dir().join(format!("zorui-prof-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = Root { home: base.join("home"), dir: base.join("dir") };
        fs::create_dir_all(&root.home).unwrap();
        root
    }

    #[test]
    fn numbers_and_next() {
        let r = fresh("nums");
        assert_eq!(r.next(), 1);
        write_marker(&r, 1, &json!({ "n": 1 })).unwrap();
        fs::create_dir_all(r.dir.join("5")).unwrap(); // no marker: not a login
        fs::create_dir_all(r.dir.join("x")).unwrap();
        assert_eq!(r.numbers(), vec![1]);
        assert_eq!(r.next(), 6);
        assert_eq!(read_marker(&r, 1).unwrap()["n"], 1);
        assert!(read_marker(&r, 2).is_none());
    }

    #[test]
    fn default_files_follow_the_saved_config_dir() {
        let r = fresh("files");
        let f = Files::of_default(&r);
        assert_eq!(f.creds, r.home.join(".claude").join(".credentials.json"));
        assert_eq!(f.config, r.home.join(".claude.json"), "no custom folder: the identity sits in the home");
        assert_eq!(f.lock, r.home.join(".claude").join(".oauth_refresh.lock"));
        let custom = r.home.join("custom");
        fs::create_dir_all(&r.dir).unwrap();
        fs::write(r.dir.join(OWN_FILE), custom.to_string_lossy().as_bytes()).unwrap();
        let f = Files::of_default(&r);
        assert_eq!(f.creds, custom.join(".credentials.json"));
        assert_eq!(f.config, custom.join(".claude.json"), "a custom folder keeps its own identity file");
        assert_eq!(Files::of(&r, 0).config, custom.join(".claude.json"));
        assert_eq!(Files::of(&r, 3).creds, r.profile_home(3).join(".credentials.json"));
    }

    #[test]
    fn login_round_trip_keeps_other_keys() {
        let r = fresh("login");
        let f = Files::of_profile(&r.profile_home(2));
        assert_eq!(login_of(&f), None, "no files, no login");
        write_login(&f, &json!({ "accessToken": "t", "refreshToken": "r" }), &json!({ "emailAddress": "a@x.com" })).unwrap();
        assert_eq!(login_of(&f), Some((json!({ "accessToken": "t", "refreshToken": "r" }), json!({ "emailAddress": "a@x.com" }))));

        fs::write(&f.config, r#"{ "numStartups": 7, "oauthAccount": { "emailAddress": "a@x.com" } }"#).unwrap();
        fs::write(&f.creds, r#"{ "mcpOAuth": { "srv": 1 }, "claudeAiOauth": { "accessToken": "old", "refreshToken": "r" } }"#).unwrap();
        write_login(&f, &json!({ "accessToken": "new", "refreshToken": "r2" }), &json!({ "emailAddress": "b@x.com" })).unwrap();
        let cfg: Value = serde_json::from_str(&fs::read_to_string(&f.config).unwrap()).unwrap();
        let cred: Value = serde_json::from_str(&fs::read_to_string(&f.creds).unwrap()).unwrap();
        assert_eq!(cfg["numStartups"], 7);
        assert_eq!(cfg["oauthAccount"]["emailAddress"], "b@x.com");
        assert_eq!(cred["mcpOAuth"]["srv"], 1);
        assert_eq!(login_of(&f).unwrap().0["accessToken"], "new");

        fs::write(&f.config, "{ nope").unwrap();
        assert!(write_login(&f, &json!({ "accessToken": "x" }), &json!({ "emailAddress": "x" })).is_err());
        assert_eq!(fs::read_to_string(&f.config).unwrap(), "{ nope", "a damaged file is left as it is");
        assert_eq!(read_obj(&f.creds).unwrap()["claudeAiOauth"]["accessToken"], "new", "the credentials were not written either");
        assert_eq!(login_of(&f), None, "a damaged identity file is not a login");
    }

    #[test]
    fn create_seeds_config_and_links_shared_folders() {
        let r = fresh("create");
        let def = r.home.join(".claude");
        fs::create_dir_all(def.join("projects")).unwrap();
        fs::write(def.join("projects").join("p.txt"), "proj").unwrap();
        fs::write(def.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        fs::write(def.join("history.jsonl"), "one\n").unwrap();
        fs::write(r.home.join(".claude.json"), r#"{ "numStartups": 3, "oauthAccount": { "emailAddress": "a@x.com" } }"#).unwrap();

        let home = create(&r, 1).unwrap();
        assert_eq!(home, r.profile_home(1));
        let cfg: Value = serde_json::from_str(&fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(cfg["numStartups"], 3);
        assert!(cfg.get("oauthAccount").is_none(), "the account is not copied");
        assert_eq!(fs::read_to_string(home.join("CLAUDE.md")).unwrap(), "@~/.claude/CLAUDE.md\n");
        assert!(fs::symlink_metadata(home.join("projects")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(home.join("projects").join("p.txt")).unwrap(), "proj", "reads go through the junction");
        assert!(fs::symlink_metadata(home.join("todos")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(home.join("settings.json")).unwrap(), r#"{"theme":"dark"}"#);
        assert_eq!(fs::read_to_string(home.join("history.jsonl")).unwrap(), "one\n");

        // a second create keeps what is there
        fs::write(home.join(".claude.json"), r#"{"numStartups": 9}"#).unwrap();
        create(&r, 1).unwrap();
        assert_eq!(fs::read_to_string(home.join(".claude.json")).unwrap(), r#"{"numStartups": 9}"#);
    }

    #[test]
    fn create_without_a_default_identity_seeds_an_empty_object() {
        let r = fresh("noid");
        let home = create(&r, 1).unwrap();
        assert_eq!(fs::read_to_string(home.join(".claude.json")).unwrap(), "{}");
    }

    #[test]
    fn link_shared_leaves_a_real_folder_with_files_alone() {
        let r = fresh("keep");
        let home = r.profile_home(1);
        fs::create_dir_all(home.join("todos")).unwrap();
        fs::write(home.join("todos").join("mine.json"), "x").unwrap();
        fs::create_dir_all(home.join("tasks")).unwrap(); // empty: gets the junction
        let warnings = link_shared(&r, &home);
        assert!(warnings.iter().any(|w| w.contains("todos")), "{warnings:?}");
        assert!(warnings.iter().all(|w| !w.contains("tasks")), "{warnings:?}");
        assert_eq!(fs::read_to_string(home.join("todos").join("mine.json")).unwrap(), "x");
        assert!(!fs::symlink_metadata(home.join("todos")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(home.join("tasks")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn merge_adds_only_the_new_lines() {
        assert_eq!(merge_history("a\n", "a\nb\n\nb\nc"), "b\nc\n");
        assert_eq!(merge_history("a", "b\n"), "\nb\n", "a shared file without a final newline gets one first");
        assert_eq!(merge_history("", "x\n"), "x\n");
        assert_eq!(merge_history("a\n", "a\n"), "");
    }

    #[test]
    fn replaced_history_link_is_merged_and_linked_again() {
        let r = fresh("hist");
        let home = r.profile_home(1);
        let shared = r.home.join(".claude").join("history.jsonl");
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::write(&shared, "old\n").unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("history.jsonl"), "old\nnew\n").unwrap(); // Claude Code replaced the link with its own file
        assert!(link_shared(&r, &home).is_empty());
        assert_eq!(fs::read_to_string(&shared).unwrap(), "old\nnew\n");
        assert_eq!(fs::read(home.join("history.jsonl")).unwrap(), fs::read(&shared).unwrap());

        let mut f = fs::OpenOptions::new().append(true).open(home.join("history.jsonl")).unwrap();
        f.write_all(b"more\n").unwrap();
        drop(f);
        assert_eq!(fs::read_to_string(&shared).unwrap(), "old\nnew\nmore\n", "the profile's file is the shared one again");
    }

    #[test]
    fn remove_deletes_the_profile_but_not_the_shared_folders() {
        let r = fresh("rm");
        let def = r.home.join(".claude");
        fs::create_dir_all(def.join("projects")).unwrap();
        fs::write(def.join("projects").join("keep.txt"), "k").unwrap();
        write_marker(&r, 1, &json!({ "n": 1 })).unwrap();
        create(&r, 1).unwrap();
        remove(&r, 1).unwrap();
        assert!(!r.dir.join("1").exists());
        assert_eq!(fs::read_to_string(def.join("projects").join("keep.txt")).unwrap(), "k");
        assert!(remove(&r, 0).is_err(), "Default has no folder to remove");
    }

    #[test]
    fn env_plan_saves_the_users_own_value_once() {
        let dir = Path::new(r"C:\Users\a\AppData\Local\zor-ui\claude-profiles");
        let own = r"D:\work\claude";
        let home1 = r"C:\Users\a\AppData\Local\zor-ui\claude-profiles\1\home";
        assert_eq!(env_plan(None, dir, false, Some(home1)), EnvPlan { save_own: None, set: Some(home1.into()) });
        assert_eq!(env_plan(None, dir, false, None), EnvPlan { save_own: None, set: None }, "back to Default with nothing set");
        assert_eq!(env_plan(Some(own), dir, false, Some(home1)).save_own, Some(own.into()), "the user's value is kept first");
        assert_eq!(env_plan(Some(own), dir, true, Some(home1)).save_own, None, "a later switch doesn't overwrite the kept copy");
        assert_eq!(env_plan(Some(home1), dir, false, None), EnvPlan { save_own: None, set: None }, "a zor-ui value isn't the user's");
        let upper = home1.to_uppercase();
        assert_eq!(env_plan(Some(&upper), dir, false, None).save_own, None, "case doesn't matter");
        let trailing = format!("{home1}\\");
        assert_eq!(env_plan(Some(&trailing), dir, false, None).save_own, None, "a trailing separator doesn't matter");
    }

    #[test]
    fn login_command_line() {
        assert_eq!(login_command(Some(Path::new(r"C:\Users\a\home"))), r#"set "CLAUDE_CONFIG_DIR=C:\Users\a\home" && claude /login"#);
        assert_eq!(login_command(None), r#"set "CLAUDE_CONFIG_DIR=" && claude /login"#);
    }

    #[test]
    fn path_checks_refuse_nesting_links_and_account_zero() {
        let r = fresh("paths");
        assert!(create(&r, 0).is_err() && remove(&r, 0).is_err() && write_marker(&r, 0, &json!({})).is_err());

        // the user's own config folder inside an account folder
        fs::create_dir_all(r.dir.join("1")).unwrap();
        fs::write(r.dir.join(OWN_FILE), r.profile_home(2).to_string_lossy().as_bytes()).unwrap();
        assert!(create(&r, 2).unwrap_err().contains("contain"));

        // an account folder that contains the user's own folder
        fs::write(r.dir.join(OWN_FILE), r.dir.to_string_lossy().as_bytes()).unwrap();
        assert!(create(&r, 3).is_err());
        fs::remove_file(r.dir.join(OWN_FILE)).unwrap();

        // a link where the account folder should be: nothing is followed or deleted through it
        let target = r.home.join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        let out = hidden("cmd").args(["/c", "mklink", "/J"]).arg(r.dir.join("4")).arg(&target).output().unwrap();
        assert!(out.status.success(), "the test needs a junction");
        assert!(create(&r, 4).is_err() && remove(&r, 4).is_err());
        assert!(target.is_dir(), "the folder behind the link is untouched");

        assert!(safe_under(&r.dir, &r.home).is_err(), "outside the account root");
        assert!(safe_under(&r.dir, &r.dir.join("..").join("x")).is_err(), "no parent steps");
    }
}
