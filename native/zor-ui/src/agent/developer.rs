//! The developer tools (M9a): read-only access to one workspace folder set in Settings (`buddies.developerWorkspace`). Every path is
//! resolved to its real location before it is used, so a junction or symlink inside the workspace cannot reach outside it. Secret
//! files are never read, and secret-looking text is redacted from every result. Nothing here changes a file or runs a project.

use crate::bridge::common::hidden;
use crate::conf::Config;
use std::path::{Component, Path, PathBuf};

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
}
