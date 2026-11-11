//! The developer tools (M9a): read-only access to one workspace folder set in Settings (`buddies.developerWorkspace`). Every path is
//! resolved to its real location before it is used, so a junction or symlink inside the workspace cannot reach outside it. Secret
//! files are never read, and secret-looking text is redacted from every result. Nothing here changes a file or runs a project.

use crate::bridge::common::hidden;
use crate::conf::Config;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

/// Config key for the workspace folder. Empty or missing turns the developer tools off.
const KEY: &str = "buddies.developerWorkspace";
const TREE_ENTRIES: usize = 200;
const TREE_DEPTH: usize = 3;
const READ_BYTES: usize = 20_000;
const SEARCH_HITS: usize = 50;
const SEARCH_DEPTH: usize = 6;
const SEARCH_FILE_BYTES: u64 = 1_000_000;
const LINE_CHARS: usize = 200;
const RESULT_CHARS: usize = 4_000;
/// Folders that are never listed or searched: version control, build output and dependencies. Folders starting with a dot are
/// skipped too.
const SKIP_DIRS: [&str; 3] = ["target", "node_modules", "dist"];

/// The workspace folder, resolved to its real location. An error the user can read when none is set or it is not a folder.
pub fn workspace(cfg: &Config) -> Result<PathBuf, String> {
    let w = cfg.str(KEY).trim();
    if w.is_empty() { return Err("No developer workspace is set in Settings.".into()); }
    if Path::new(w).components().any(|c| matches!(c, Component::ParentDir)) { return Err("The developer workspace in Settings has \"..\" in it. Give the full folder path.".into()); }
    std::fs::canonicalize(w).ok().filter(|p| p.is_dir()).ok_or_else(|| "The developer workspace in Settings is not a folder.".to_string())
}

/// The real path of `rel` inside `ws`. Absolute paths, drive or root prefixes, `..`, and anything that resolves outside the
/// workspace are refused.
pub fn resolve(ws: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    let p = Path::new(rel);
    if rel.is_empty() || p.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir)) {
        return Err(format!("\"{rel}\" is not a path inside the workspace."));
    }
    let real = std::fs::canonicalize(ws.join(p)).map_err(|_| format!("There is no \"{rel}\" in the workspace."))?;
    if real.starts_with(ws) { Ok(real) } else { Err(format!("\"{rel}\" leads outside the workspace, so I won't open it.")) }
}

/// Whether a file's contents are never read or searched: keys, certificates, credentials, and anything under `.git`.
pub fn is_secret(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let in_git = path.components().any(|c| c.as_os_str().eq_ignore_ascii_case(".git"));
    in_git || name.starts_with(".env") || name.starts_with("credentials") || name.starts_with("id_rsa")
        || [".pem", ".pfx", ".key", ".kdbx", ".p12"].iter().any(|e| name.ends_with(e))
}

/// Whether a folder entry is skipped by listing and searching.
fn skipped_dir(name: &str) -> bool { name.starts_with('.') || SKIP_DIRS.contains(&name) }

/// Replaces the value of a key-like line (`api_key = …`, `password: …`), a private key block, and token-shaped words with
/// `[redacted]`. Matching is by keyword, so an ordinary line such as "token count: 5" is also redacted.
pub fn redact(text: &str) -> String {
    const KEYS: [&str; 6] = ["api_key", "apikey", "api-key", "secret", "token", "password"];
    text.lines().map(|line| {
        if line.contains("-----BEGIN") { return "[redacted]".to_string(); }
        // lowercase keeps every byte where it was, so the positions found below are valid in `line`
        let low = line.to_ascii_lowercase();
        if let Some(at) = KEYS.iter().filter_map(|k| low.find(k)).min() {
            if let Some(sep) = line[at..].find(['=', ':']) {
                return format!("{} [redacted]", &line[..at + sep + 1]);
            }
        }
        line.split(' ').map(|w| if is_token(w) { "[redacted]" } else { w }).collect::<Vec<_>>().join(" ")
    }).collect::<Vec<_>>().join("\n")
}

fn is_token(word: &str) -> bool {
    let w = word.trim_matches(|c: char| c == '"' || c == '\'' || c == ',');
    w.len() >= 20 && ["sk-", "AKIA", "ghp_", "github_pat_"].iter().any(|p| w.starts_with(p))
}

/// Names of the files and folders under `rel` (`.` for the top), two levels down at most, sorted, capped. Links are listed by name
/// and not followed.
pub fn list_tree(ws: &Path, rel: &str) -> Result<String, String> {
    let root = resolve(ws, if rel.trim().is_empty() { "." } else { rel })?;
    if !root.is_dir() { return Err(format!("\"{rel}\" is a file, not a folder. Use read_file for it.")); }
    let mut out = vec![];
    walk_names(&root, &root, 1, &mut out);
    if out.is_empty() { return Ok("The folder is empty.".into()); }
    Ok(out.join("\n"))
}

fn walk_names(dir: &Path, base: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > TREE_DEPTH { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= TREE_ENTRIES { out.push("(more not shown)".into()); return; }
        let name = e.file_name().to_string_lossy().into_owned();
        // file_type does not follow links, so a link into another folder is listed and not entered
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && skipped_dir(&name) { continue; }
        let shown = e.path().strip_prefix(base).map(|p| p.display().to_string()).unwrap_or(name);
        out.push(if is_dir { format!("{shown}\\") } else { shown });
        if is_dir { walk_names(&e.path(), base, depth + 1, out); }
    }
}

/// The text of one file in the workspace: secret files are refused, binary files are refused, and long files are cut.
pub fn read_file(ws: &Path, rel: &str) -> Result<String, String> {
    let real = resolve(ws, rel)?;
    if is_secret(&real) || is_secret(Path::new(rel)) { return Err("That file holds secrets, so I won't read it.".into()); }
    if !real.is_file() { return Err(format!("\"{rel}\" is not a file.")); }
    let bytes = std::fs::read(&real).map_err(|e| format!("Could not read \"{rel}\": {e}"))?;
    if bytes.contains(&0) { return Err(format!("\"{rel}\" is not a text file.")); }
    let cut = bytes.len() > READ_BYTES;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(READ_BYTES)]);
    Ok(format!("{}{}", redact(&text), if cut { "\n(cut short)" } else { "" }))
}

/// Lines in the workspace's text files that contain `query` (case-insensitive), as `path:line: text`, capped.
pub fn search_code(ws: &Path, query: &str) -> Result<String, String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() { return Err("Search needs some words.".into()); }
    let mut hits = vec![];
    search_dir(ws, ws, &q, 1, &mut hits);
    if hits.is_empty() { return Ok(format!("Nothing in the workspace contains \"{}\".", query.trim())); }
    Ok(hits.join("\n"))
}

fn search_dir(dir: &Path, base: &Path, q: &str, depth: usize, hits: &mut Vec<String>) {
    if depth > SEARCH_DEPTH || hits.len() >= SEARCH_HITS { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        if hits.len() >= SEARCH_HITS { return; }
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let ft = e.file_type();
        if ft.as_ref().is_ok_and(|t| t.is_dir()) {
            if !skipped_dir(&name) { search_dir(&path, base, q, depth + 1, hits); }
            continue;
        }
        if !ft.is_ok_and(|t| t.is_file()) || is_secret(&path) { continue; }
        if e.metadata().map(|m| m.len() > SEARCH_FILE_BYTES).unwrap_or(true) { continue; }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let shown = path.strip_prefix(base).map(|p| p.display().to_string()).unwrap_or(name);
        for (i, line) in text.lines().enumerate() {
            if !line.to_lowercase().contains(q) { continue; }
            let clipped: String = line.chars().take(LINE_CHARS).collect();
            hits.push(format!("{shown}:{}: {}", i + 1, redact(&clipped)));
            if hits.len() >= SEARCH_HITS { return; }
        }
    }
}

/// The git status and a diff summary of the workspace. The argument lists are fixed; no AI text reaches git. External diff tools,
/// textconv filters and the pager are off, so a setting in the repository cannot run a program.
pub fn git_status(ws: &Path) -> Result<String, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let out = hidden("git").args(args).current_dir(ws).output().map_err(|_| "git isn't installed, so I can't read the repository.".to_string())?;
        if !out.status.success() { return Err("This folder is not a git repository.".into()); }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let status = run(&["--no-pager", "-c", "core.fsmonitor=false", "status", "--short"])?;
    let diff = run(&["--no-pager", "-c", "core.fsmonitor=false", "diff", "--stat", "--no-ext-diff", "--no-textconv"])?;
    let text = format!("Status:\n{}\nDiff summary:\n{}", status.trim_end(), diff.trim_end());
    Ok(redact(&text).chars().take(RESULT_CHARS).collect())
}

/// One check `run_check` may run. The argument list is fixed here, so no AI text ever becomes an argument.
#[derive(Debug)]
pub struct CheckSpec {
    pub name: &'static str,
    pub args: &'static [&'static str],
}

pub static CHECKS: [CheckSpec; 5] = [
    CheckSpec { name: "cargo check", args: &["check"] },
    CheckSpec { name: "cargo test", args: &["test"] },
    CheckSpec { name: "cargo build", args: &["build"] },
    CheckSpec { name: "cargo build --release", args: &["build", "--release"] },
    CheckSpec { name: "cargo clippy", args: &["clippy"] },
];
/// The wall-clock limit of one check. A check still running at the limit is killed.
const CHECK_LIMIT: Duration = Duration::from_secs(600);
/// The most output a check adds to the AI's result. The tail is kept, since a build or test run ends with its answer.
const OUTPUT_BYTES: usize = 16_384;
/// Workspaces the user said yes to running checks in. Memory only, for this app session: never Settings, the key store or buddy.db.
static CHECK_YES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The fixed entry for a check name the AI gave. Only an exact name from the list is accepted.
pub fn check_named(name: &str) -> Result<&'static CheckSpec, String> {
    let n = name.trim();
    CHECKS.iter().find(|c| c.name == n).ok_or_else(|| {
        let list: Vec<&str> = CHECKS.iter().map(|c| c.name).collect();
        format!("\"{n}\" is not a check I can run. Use one of: {}.", list.join(", "))
    })
}

/// Whether the user already said yes to checks in this workspace during this app session.
pub fn check_allowed_here(ws: &Path) -> bool { CHECK_YES.lock().is_ok_and(|v| v.iter().any(|p| p == ws)) }

pub fn remember_check_yes(ws: &Path) {
    if let Ok(mut v) = CHECK_YES.lock() { if !v.iter().any(|p| p == ws) { v.push(ws.to_path_buf()); } }
}

/// Runs one check in `ws` (a workspace from `workspace`). The caller checks the name and gets the user's yes first.
pub fn run_check(ws: &Path, name: &str) -> Result<String, String> {
    let spec = check_named(name)?;
    run_program(ws, spec.name, "cargo", spec.args, CHECK_LIMIT)
}

/// Runs `program args` in `ws` with no shell, stdin closed and both output streams captured. A run still going at `limit` is
/// killed. Ok when it exits 0; otherwise Err, and an Err that starts "The check " is a check that ran and did not pass. The
/// output is redacted and cut to its last `OUTPUT_BYTES`. `program` comes from the fixed table; tests pass a harmless one.
pub fn run_program(ws: &Path, label: &str, program: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut child = hidden(program).args(args).current_dir(ws).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|_| format!("{program} isn't installed or could not start, so \"{label}\" did not run."))?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut pipes = 0;
    if let Some(out) = child.stdout.take() { pipes += 1; drain(out, tx.clone()); }
    if let Some(err) = child.stderr.take() { pipes += 1; drain(err, tx.clone()); }
    drop(tx);
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => { let _ = child.kill(); let _ = child.wait(); break None; }
        }
    };
    // a grandchild (rustc under cargo) can hold a pipe open after the child is gone, so the wait for output is short
    let mut bytes = vec![];
    for _ in 0..pipes { if let Ok(b) = rx.recv_timeout(Duration::from_secs(2)) { bytes.extend(b); } }
    let out = cap_tail(&redact(&String::from_utf8_lossy(&bytes)), OUTPUT_BYTES);
    let head = match status {
        None => format!("The check ran out of time: \"{label}\" was stopped."),
        Some(s) if s.success() => format!("The check passed: \"{label}\" exited with code 0."),
        Some(s) => format!("The check failed: \"{label}\" exited with code {}.", s.code().unwrap_or(-1)),
    };
    let text = if out.is_empty() { head } else { format!("{head}\n{out}") };
    match status {
        Some(s) if s.success() => Ok(text),
        _ => Err(text),
    }
}

/// Reads a pipe to its end on a thread and sends the last `OUTPUT_BYTES` it kept (a longer read is cut as it goes).
fn drain(mut pipe: impl Read + Send + 'static, tx: mpsc::Sender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut kept = vec![];
        let mut chunk = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 { break; }
            kept.extend_from_slice(&chunk[..n]);
            if kept.len() > 4 * OUTPUT_BYTES { kept.drain(..kept.len() - OUTPUT_BYTES); }
        }
        let _ = tx.send(kept);
    });
}

/// The last `max` bytes of `text`, from a character boundary, with a note on top when anything was cut.
pub fn cap_tail(text: &str, max: usize) -> String {
    if text.len() <= max { return text.to_string(); }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) { start += 1; }
    format!("(cut short: the start of the output is not shown)\n{}", &text[start..])
}

/// The test folder for the developer tools: `%LOCALAPPDATA%\zor-ui\dev-workspace`, a small Cargo project written by hand (no
/// `cargo new`, no network). Files already there are kept. Made only when asked, never at startup. The user then sets
/// `buddies.developerWorkspace` to the folder.
#[allow(dead_code)] // no caller yet: the Settings button that asks for it comes after M9b
pub fn create_test_workspace() -> Result<PathBuf, String> { create_test_workspace_in(&crate::conf::data_dir()) }

fn create_test_workspace_in(root: &Path) -> Result<PathBuf, String> {
    const FILES: [(&str, &str); 3] = [
        ("Cargo.toml", "[package]\nname = \"dev-workspace\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"),
        (".gitignore", "/target\n"),
        ("src/main.rs", "fn main() {\n    println!(\"Hello, world!\");\n}\n"),
    ];
    let dir = root.join("dev-workspace");
    std::fs::create_dir_all(dir.join("src")).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    for (rel, text) in FILES {
        let path = dir.join(rel);
        if path.exists() { continue; }
        std::fs::write(&path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the temp dir, with a canonical path, so the checks compare like with like.
    fn ws(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zorui-dev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn paths_must_stay_inside_the_workspace() {
        let w = ws("paths");
        std::fs::write(w.join("src").join("lib.rs"), "fn main() {}").unwrap();
        assert!(resolve(&w, "src/lib.rs").is_ok());
        assert!(resolve(&w, "../etc/hosts").is_err(), "a .. is refused");
        assert!(resolve(&w, "C:\\Windows\\win.ini").is_err(), "a drive path is refused");
        assert!(resolve(&w, "\\Windows").is_err(), "a root path is refused");
        assert!(resolve(&w, "").is_err());
        assert!(resolve(&w, "missing.rs").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_inside_the_workspace_cannot_reach_outside_it() {
        let w = ws("junction");
        let outside = std::env::temp_dir().join(format!("zorui-dev-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret-note.txt"), "outside").unwrap();
        let link = w.join("link");
        // a junction needs no admin rights; if the tool can't create one here, the test has nothing to prove
        if std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&outside).status().is_ok_and(|s| s.success()) {
            assert!(resolve(&w, "link/secret-note.txt").is_err(), "the junction points outside");
            assert!(list_tree(&w, "link").is_err() || !list_tree(&w, ".").unwrap().contains("secret-note"));
        }
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn secret_files_are_never_read_and_their_names_say_so() {
        let w = ws("secrets");
        std::fs::write(w.join(".env"), "TOKEN=abc").unwrap();
        std::fs::write(w.join("server.pem"), "-----BEGIN PRIVATE KEY-----").unwrap();
        assert!(read_file(&w, ".env").unwrap_err().contains("holds secrets"));
        assert!(read_file(&w, "server.pem").unwrap_err().contains("holds secrets"));
        assert!(is_secret(Path::new(".git/config")) && is_secret(Path::new("app/.git/HEAD")));
        assert!(!is_secret(Path::new("src/main.rs")));
    }

    #[test]
    fn key_like_text_is_redacted_in_results() {
        assert_eq!(redact("api_key = abc123"), "api_key = [redacted]");
        assert_eq!(redact("let password: hunter2;"), "let password: [redacted]");
        assert_eq!(redact("-----BEGIN RSA PRIVATE KEY-----"), "[redacted]");
        assert_eq!(redact("key sk-abcdefghijklmnopqrstuvwx here"), "key [redacted] here");
        assert_eq!(redact("fn add(a: i32)"), "fn add(a: i32)", "an ordinary line is kept");
    }

    #[test]
    fn searches_and_listings_skip_build_output_and_git() {
        let w = ws("search");
        std::fs::create_dir_all(w.join("target")).unwrap();
        std::fs::create_dir_all(w.join(".git")).unwrap();
        std::fs::write(w.join("src").join("lib.rs"), "fn Needle() {}\nother").unwrap();
        std::fs::write(w.join("target").join("out.rs"), "needle in build output").unwrap();
        std::fs::write(w.join(".git").join("config"), "needle in git").unwrap();
        let found = search_code(&w, "needle").unwrap();
        assert!(found.contains("src") && found.contains(":1: fn Needle"), "{found}");
        assert!(!found.contains("target") && !found.contains(".git"), "{found}");
        let tree = list_tree(&w, "").unwrap();
        assert!(tree.contains("src\\") && tree.contains("lib.rs"), "{tree}");
        assert!(!tree.contains("target") && !tree.contains(".git"), "{tree}");
    }

    #[test]
    fn long_files_are_cut_and_binary_files_are_refused() {
        let w = ws("read");
        std::fs::write(w.join("big.txt"), "a".repeat(READ_BYTES + 500)).unwrap();
        std::fs::write(w.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        assert!(read_file(&w, "big.txt").unwrap().ends_with("(cut short)"));
        assert!(read_file(&w, "bin.dat").unwrap_err().contains("not a text file"));
        assert!(list_tree(&w, "big.txt").unwrap_err().contains("not a folder"));
    }

    #[test]
    fn the_model_reaches_the_read_only_tools_through_the_protocol() {
        use crate::agent::{parse_step, Call, Step};
        assert_eq!(parse_step(r#"{"do":"read_file","arg":"src/lib.rs"}"#), Step::Call { call: Call::ReadFile("src/lib.rs".into()), more: false });
        assert_eq!(parse_step(r#"{"do":"git_status","arg":""}"#), Step::Call { call: Call::GitStatus, more: false });
        assert_eq!(parse_step(r#"{"do":"list_tree","arg":""}"#), Step::Call { call: Call::ListTree(String::new()), more: false }, "empty means the top");
        assert!(matches!(parse_step(r#"{"do":"read_file"}"#), Step::Bad(_)), "a read needs a path");
    }

    #[test]
    fn no_workspace_means_the_tools_say_so() {
        let none = Config::parse("{}");
        assert!(workspace(&none).unwrap_err().contains("No developer workspace"));
        let missing = Config::parse(r#"{"buddies":{"developerWorkspace":"Z:\\no\\such\\folder"}}"#);
        assert!(workspace(&missing).unwrap_err().contains("not a folder"));
    }

    fn cfg_for(path: &Path) -> Config {
        Config::parse(&serde_json::json!({ "buddies": { "developerWorkspace": path.display().to_string() } }).to_string())
    }

    #[test]
    fn the_workspace_setting_is_canonical_and_never_has_dot_dot() {
        let w = ws("setting");
        assert_eq!(workspace(&cfg_for(&w)).unwrap(), w, "the setting resolves to its real folder");
        // a plain (not canonical) path: a \\?\ path does not treat .. as a parent step, so only this form shows the refusal
        let plain = std::env::temp_dir().join(format!("zorui-dev-setting-{}", std::process::id())).join("src").join("..");
        let err = workspace(&cfg_for(&plain)).unwrap_err();
        assert!(err.contains("\"..\""), "a .. in the setting is refused: {err}");
    }

    #[test]
    fn check_names_are_the_fixed_list_and_nothing_else() {
        assert_eq!(check_named("cargo test").unwrap().args, &["test"]);
        assert_eq!(check_named("  cargo build --release ").unwrap().args, &["build", "--release"]);
        for bad in ["", "cargo", "CARGO TEST", "cargo publish", "cargo test; del x", "cargo test && echo", "cargo test --", "cmd /C del x"] {
            assert!(check_named(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(check_named("cargo publish").unwrap_err().contains("cargo clippy"), "the refusal lists what is allowed");
    }

    #[test]
    fn output_is_capped_and_the_tail_is_kept() {
        let long = format!("{}END", "a".repeat(50_000));
        let capped = cap_tail(&long, OUTPUT_BYTES);
        assert!(capped.starts_with("(cut short"), "the cut is marked");
        assert!(capped.len() <= OUTPUT_BYTES + 60 && capped.ends_with("END"), "the end is kept, the start is not");
        assert_eq!(cap_tail("short", OUTPUT_BYTES), "short");
        let accents = "é".repeat(20_000);
        assert!(cap_tail(&accents, 101).ends_with('é'), "a cut never splits a character");
    }

    #[cfg(windows)]
    #[test]
    fn a_check_that_runs_past_its_limit_is_stopped() {
        let w = ws("timeout");
        let started = Instant::now();
        // a fast fake command (ping sleeps for 30 seconds) with a 300 ms limit stands in for a long cargo run
        let r = run_program(&w, "fake slow check", "ping", &["-n", "30", "127.0.0.1"], Duration::from_millis(300));
        assert!(r.unwrap_err().starts_with("The check ran out of time"));
        assert!(started.elapsed() < Duration::from_secs(20), "it was stopped, not waited for");
    }

    #[cfg(windows)]
    #[test]
    fn check_output_is_redacted_and_a_failed_exit_is_a_check_that_failed() {
        let w = ws("output");
        let ok = run_program(&w, "fake", "cmd", &["/C", "echo api_key = hunter2"], Duration::from_secs(30)).unwrap();
        assert!(ok.starts_with("The check passed") && ok.contains("[redacted]") && !ok.contains("hunter2"), "{ok}");
        let bad = run_program(&w, "fake", "cmd", &["/C", "exit 3"], Duration::from_secs(30)).unwrap_err();
        assert!(bad.starts_with("The check failed") && bad.contains("code 3"), "{bad}");
    }

    #[test]
    fn a_program_that_cannot_start_is_not_a_check_that_ran() {
        let w = ws("missing");
        let err = run_program(&w, "fake", "zorui-no-such-program-anywhere", &[], Duration::from_secs(5)).unwrap_err();
        assert!(!err.starts_with("The check "), "{err}");
    }

    #[test]
    fn the_test_workspace_is_a_cargo_project_and_is_never_overwritten() {
        let root = std::env::temp_dir().join(format!("zorui-dev-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = create_test_workspace_in(&root).unwrap();
        assert_eq!(dir, root.join("dev-workspace"));
        assert!(std::fs::read_to_string(dir.join("Cargo.toml")).unwrap().contains("name = \"dev-workspace\""));
        assert!(dir.join("src").join("main.rs").is_file() && dir.join(".gitignore").is_file());
        std::fs::write(dir.join("Cargo.toml"), "mine").unwrap();
        create_test_workspace_in(&root).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("Cargo.toml")).unwrap(), "mine", "an existing file is kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_check_is_a_fixed_tool_for_the_model() {
        use crate::agent::{parse_step, Call, Step};
        assert_eq!(parse_step(r#"{"do":"run_check","arg":"cargo test"}"#), Step::Call { call: Call::RunCheck("cargo test".into()), more: false });
        assert!(matches!(parse_step(r#"{"do":"run_check"}"#), Step::Bad(_)), "a check needs its name");
    }
}
