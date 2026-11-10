//! Restarting coding-agent sessions (Claude Code, Codex, Gemini CLI, custom agents) so they pick up a change such as a
//! new Claude account.
//!
//! The agent hook (agent_hook.rs) records, per session, which process runs it, whether Orca hosts it, and the
//! environment it runs with, sealed with DPAPI because it holds API keys. A restart stops that process and resumes the
//! same session in a new console, with that same environment plus the overrides. So the provider, base URL and model
//! settings carry over whatever the agent, model or IDE; the model the session last used is passed back too when the
//! agent reported it. Sessions inside Orca are left to Orca. A busy session (working, or waiting on a question) is
//! restarted once it goes idle, so no tool call is cut off.
//!
//! shortcut: other command-line flags of the original launch (permission modes, extra dirs) aren't carried over;
//! reading another process's command line needs NtQueryInformationProcess, add it if anyone misses them.

use super::web::{b64_decode, b64_encode, dpapi};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{atomic::{AtomicBool, Ordering}, Mutex},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{CloseHandle, FILETIME},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Threading::{GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE},
    },
};

/// Processes that sit between an agent and its hook: the shells agents run hooks through, and the hook itself.
const BETWEEN: [&str; 7] = ["cmd.exe", "bash.exe", "sh.exe", "pwsh.exe", "powershell.exe", "conhost.exe", "zorui.exe"];
/// Variables an agent sets for its own children. A resumed agent must not inherit them: Claude Code refuses to start
/// inside what looks like another Claude Code session.
const AGENT_SET: [&str; 4] = ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_PROJECT_DIR", "CLAUDE_ENV_FILE"];
const CREATE_NEW_CONSOLE: u32 = 0x10;
/// How long a busy session is waited for before it's left alone.
const LATER_FOR: Duration = Duration::from_secs(30 * 60);

// ------------------------------------------------------------------ processes

/// (pid, parent pid, exe name) of every process.
fn snapshot() -> Vec<(u32, u32, String)> {
    let mut v = vec![];
    unsafe {
        let Ok(s) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return v };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(s, &mut e).is_ok();
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            v.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
            ok = Process32NextW(s, &mut e).is_ok();
        }
        let _ = CloseHandle(s);
    }
    v
}

/// The agent's process above `me` (the first ancestor that isn't a shell or the hook), its exe name, and whether
/// Orca is further up. None when there is no such ancestor.
fn agent_of(list: &[(u32, u32, String)], me: u32) -> Option<(u32, String, bool)> {
    let entry = |pid: u32| list.iter().find(|e| e.0 == pid);
    let (mut cur, mut seen, mut agent, mut orca) = (me, HashSet::new(), None, false);
    // a parent pid can be reused by an unrelated process, so the walk is bounded and never revisits
    while seen.insert(cur) && seen.len() <= 64 {
        let Some(parent) = entry(cur).map(|e| e.1).filter(|p| *p != 0) else { break };
        let Some(pe) = entry(parent) else { break };
        let name = pe.2.to_lowercase();
        if agent.is_none() {
            if !BETWEEN.contains(&name.as_str()) { agent = Some((parent, name)); }
        } else if name == "orca.exe" {
            orca = true;
        }
        cur = parent;
    }
    agent.map(|(pid, name)| (pid, name, orca))
}

/// When process `pid` started (FILETIME ticks); a pid reused by another process has another start time.
fn started(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let r = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
        let _ = CloseHandle(h);
        r.ok()?;
        Some((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64)
    }
}

/// Whether the process recorded in session `s` still runs (same pid and same start time).
pub(super) fn alive(s: &Value) -> bool {
    match (s["pid"].as_u64(), s["start"].as_u64()) {
        (Some(pid), Some(start)) => started(pid as u32) == Some(start),
        _ => false,
    }
}

fn kill(pid: u32, start: u64) -> bool {
    if started(pid) != Some(start) { return false; }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let ok = TerminateProcess(h, 1).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

// ------------------------------------------------------------------ what the hook records

/// The environment worth replaying: everything but what the agent set for its own children.
fn replayable(vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    vars.filter(|(k, _)| !AGENT_SET.iter().any(|a| a.eq_ignore_ascii_case(k))).collect()
}

fn seal(env: &[(String, String)]) -> Option<String> { dpapi(&serde_json::to_vec(env).ok()?, true).map(|b| b64_encode(&b)) }

fn unseal(s: &str) -> Option<Vec<(String, String)>> { serde_json::from_slice(&dpapi(&b64_decode(s), false)?).ok() }

/// What the hook process knows about the agent running it: {pid, start, exe, orca, env}. Runs inside the hook, whose
/// environment is the agent's. None when no agent process is found (and always in tests).
pub(super) fn capture() -> Option<Value> {
    if cfg!(test) { return None; }
    let (pid, exe, orca) = agent_of(&snapshot(), std::process::id())?;
    let start = started(pid)?;
    let env = seal(&replayable(std::env::vars()))?;
    Some(json!({ "pid": pid, "start": start, "exe": exe, "orca": orca, "env": env }))
}

// ------------------------------------------------------------------ the resume command

/// Text that is safe inside a cmd.exe command line: session ids and model names only ever use these characters.
fn plain(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c) || extra.contains(c))
}

/// The command that resumes session `id`: `template` with {id}, plus `model_arg` with {model} when the model is known.
/// None when the agent has no template or the id isn't plain text.
fn resume_line(template: &str, model_arg: &str, id: &str, model: Option<&str>) -> Option<String> {
    if template.trim().is_empty() || !plain(id, "") { return None; }
    let mut line = template.replace("{id}", id);
    if let Some(m) = model.filter(|m| plain(m, ":/@[]")) {
        if !model_arg.trim().is_empty() { line = format!("{line} {}", model_arg.replace("{model}", m)); }
    }
    Some(line)
}

// ------------------------------------------------------------------ restarting

/// What to do with one recorded session.
#[derive(Debug, PartialEq)]
enum Step { Now, Later, Skip(&'static str) }

fn step(s: &Value, alive: bool) -> Step {
    if s["pid"].as_u64().is_none() { return Step::Skip("started before zor-ui tracked it"); }
    if !alive { return Step::Skip("no longer running"); }
    if s["orca"] == true { return Step::Skip("runs in Orca"); }
    match s["state"].as_str() {
        Some("done") => Step::Now,
        Some("working") | Some("waiting") => Step::Later,
        _ => Step::Skip("in an unknown state"),
    }
}

/// How to resume an agent's sessions: (template, model argument), from agent_hook's list.
type Resume = (String, String);

/// Stops one session's process and resumes it in a new console with its own environment plus `overrides`.
fn restart_one(s: &Value, how: &Resume, overrides: &[(String, Option<String>)]) -> Result<(), String> {
    let id = s["id"].as_str().unwrap_or("");
    let line = resume_line(&how.0, &how.1, id, s["model"].as_str()).ok_or("this agent has no resume command")?;
    let env = s["env"].as_str().and_then(unseal).ok_or("its environment can't be read (saved by another Windows user?)")?;
    let (pid, start) = (s["pid"].as_u64().unwrap_or(0) as u32, s["start"].as_u64().unwrap_or(0));
    let cwd = s["cwd"].as_str().map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(crate::conf::home);
    if !kill(pid, start) { return Err("it couldn't be stopped".into()); }
    std::thread::sleep(Duration::from_millis(400)); // let the old process let go of its transcript
    let mut cmd = Command::new("cmd.exe");
    cmd.raw_arg(format!("/k {line}")).current_dir(cwd).env_clear().envs(env);
    for (k, v) in overrides {
        match v { Some(v) => { cmd.env(k, v); } None => { cmd.env_remove(k); } }
    }
    // a new console is hosted by whatever terminal Windows is set to use, so this works from any IDE or terminal
    cmd.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ()).map_err(|e| format!("it was stopped but couldn't be resumed ({e}); run {line} yourself"))
}

/// The session files of one agent: (path, record).
fn sessions(dir: &Path) -> Vec<(PathBuf, Value)> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json") && p.file_name().is_some_and(|n| n != "usage.json"))
        .filter_map(|p| Some((p.clone(), serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?)))
        .collect()
}

/// Busy sessions waiting to go idle, each with what its restart should use.
static LATER: Mutex<Vec<(PathBuf, Resume, Vec<(String, Option<String>)>)>> = Mutex::new(Vec::new());
static WATCHING: AtomicBool = AtomicBool::new(false);

fn later_lock() -> std::sync::MutexGuard<'static, Vec<(PathBuf, Resume, Vec<(String, Option<String>)>)>> {
    LATER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Restarts every session of the agent whose session files are in `dir`. Idle ones go now, busy ones once they finish.
/// Returns {"restarted": n, "later": n, "skipped": [reason...], "failed": [reason...]}.
pub fn restart(dir: &Path, how: Resume, overrides: Vec<(String, Option<String>)>) -> Value {
    let (mut now, mut later, mut skipped, mut failed) = (0, 0, vec![], vec![]);
    for (path, s) in sessions(dir) {
        match step(&s, alive(&s)) {
            Step::Now => match restart_one(&s, &how, &overrides) { Ok(()) => now += 1, Err(e) => failed.push(e) },
            Step::Later => {
                let mut l = later_lock();
                l.retain(|(p, _, _)| *p != path); // the newest switch wins
                l.push((path, how.clone(), overrides.clone()));
                later += 1;
            }
            Step::Skip(why) => skipped.push(why.to_string()),
        }
    }
    if later > 0 && !WATCHING.swap(true, Ordering::SeqCst) { std::thread::spawn(watch_later); }
    json!({ "restarted": now, "later": later, "skipped": skipped, "failed": failed })
}

/// Restarts busy sessions as they go idle; gives up on each after LATER_FOR.
fn watch_later() {
    let start = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let due: Vec<_> = {
            let mut l = later_lock();
            let mut due = vec![];
            l.retain(|(path, how, over)| {
                let Some(s) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return false }; // ended
                match step(&s, alive(&s)) {
                    Step::Now => { due.push((s, how.clone(), over.clone())); false }
                    Step::Later => start.elapsed() < LATER_FOR,
                    Step::Skip(_) => false,
                }
            });
            due
        };
        for (s, how, over) in due {
            if let Err(e) = restart_one(&s, &how, &over) { super::claude_acct::banner(format!("Couldn't restart a session: {e}"), "", 8); }
        }
        if later_lock().is_empty() { WATCHING.store(false, Ordering::SeqCst); return; }
    }
}

/// One line for a toast or banner about what a restart did.
pub fn summary(r: &Value) -> String {
    let n = |k: &str| r[k].as_u64().unwrap_or(0);
    let s = |k: u64, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    let mut parts = vec![];
    if n("restarted") > 0 { parts.push(format!("Restarted {}", s(n("restarted"), "session", "sessions"))); }
    if n("later") > 0 { parts.push(format!("{} more after {}", n("later"), if n("later") == 1 { "it finishes" } else { "they finish" })); }
    let orca = r["skipped"].as_array().map_or(0, |a| a.iter().filter(|x| *x == "runs in Orca").count()) as u64;
    if orca > 0 { parts.push(format!("{} in Orca left to Orca", s(orca, "session", "sessions"))); }
    if let Some(f) = r["failed"].as_array().and_then(|a| a.first()).and_then(Value::as_str) { parts.push(format!("one failed: {f}")); }
    if parts.is_empty() { return "No open sessions to restart".into(); }
    let mut text = parts.join(". ");
    text.push('.');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Vec<(u32, u32, String)> {
        vec![
            (1, 0, "explorer.exe".into()),
            (10, 1, "Orca.exe".into()),
            (11, 10, "pwsh.exe".into()),
            (12, 11, "node.exe".into()),  // the agent
            (13, 12, "bash.exe".into()),  // the shell the agent runs hooks in
            (14, 13, "zorui.exe".into()), // the hook
            (20, 1, "WindowsTerminal.exe".into()),
            (21, 20, "cmd.exe".into()),
            (22, 21, "claude.exe".into()),
            (23, 22, "zorui.exe".into()),
        ]
    }

    #[test]
    fn finds_the_agent_above_the_hook_and_whether_orca_hosts_it() {
        assert_eq!(agent_of(&tree(), 14), Some((12, "node.exe".into(), true)), "shells between agent and hook are skipped");
        assert_eq!(agent_of(&tree(), 23), Some((22, "claude.exe".into(), false)), "the agent's own launch shell is above it, not between");
        assert_eq!(agent_of(&tree(), 99), None, "an unknown process has no agent");
        let looped = vec![(5, 6, "zorui.exe".into()), (6, 5, "cmd.exe".into())];
        assert_eq!(agent_of(&looped, 5), None, "a parent loop ends the walk");
    }

    #[test]
    fn the_replayed_environment_drops_what_the_agent_set_for_its_children() {
        let vars = vec![("CLAUDECODE".to_string(), "1".to_string()), ("ANTHROPIC_BASE_URL".into(), "https://x".into()), ("claude_project_dir".into(), "c".into()), ("PATH".into(), "p".into())];
        assert_eq!(replayable(vars.into_iter()), vec![("ANTHROPIC_BASE_URL".to_string(), "https://x".to_string()), ("PATH".into(), "p".into())]);
    }

    #[test]
    fn sealed_environment_round_trips() {
        let env = vec![("OPENAI_API_KEY".to_string(), "sk-1".to_string()), ("A".into(), "b c".into())];
        let s = seal(&env).unwrap();
        assert!(!s.contains("sk-1"), "the key isn't stored in the clear");
        assert_eq!(unseal(&s), Some(env));
        assert_eq!(unseal("not sealed"), None);
    }

    #[test]
    fn resume_line_fills_the_template_and_rejects_unsafe_text() {
        assert_eq!(resume_line("claude --resume {id}", "--model {model}", "0b1c-2d", Some("claude-opus-4-1[1m]")).as_deref(), Some("claude --resume 0b1c-2d --model claude-opus-4-1[1m]"));
        assert_eq!(resume_line("codex resume {id}", "-m {model}", "abc", None).as_deref(), Some("codex resume abc"), "no model known: no model argument");
        assert_eq!(resume_line("x {id}", "", "abc", Some("m")).as_deref(), Some("x abc"), "an agent without a model argument");
        assert_eq!(resume_line("claude --resume {id}", "--model {model}", "a b", None), None, "an id with a space");
        assert_eq!(resume_line("claude --resume {id}", "--model {model}", "a&calc", None), None, "an id that would chain a command");
        assert_eq!(resume_line("claude --resume {id}", "--model {model}", "a", Some("m&x")).as_deref(), Some("claude --resume a"), "an unsafe model is left out");
        assert_eq!(resume_line("", "", "a", None), None, "no template");
    }

    #[test]
    fn steps_follow_state_orca_and_liveness() {
        let s = |state: &str, orca: bool| json!({ "pid": 5, "start": 1, "state": state, "orca": orca });
        assert_eq!(step(&s("done", false), true), Step::Now);
        assert_eq!(step(&s("working", false), true), Step::Later);
        assert_eq!(step(&s("waiting", false), true), Step::Later, "a question waiting for you isn't cut off");
        assert_eq!(step(&s("done", true), true), Step::Skip("runs in Orca"));
        assert_eq!(step(&s("done", false), false), Step::Skip("no longer running"));
        assert_eq!(step(&json!({ "state": "done" }), true), Step::Skip("started before zor-ui tracked it"));
    }

    #[test]
    fn restart_with_nothing_to_do_touches_nothing() {
        let d = std::env::temp_dir().join(format!("zorui-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("usage.json"), "{}").unwrap();
        std::fs::write(d.join("old.json"), json!({ "state": "done", "id": "old" }).to_string()).unwrap();
        std::fs::write(d.join("gone.json"), json!({ "state": "done", "id": "g", "pid": 1, "start": 1 }).to_string()).unwrap();
        let r = restart(&d, ("claude --resume {id}".into(), String::new()), vec![]);
        assert_eq!((r["restarted"].clone(), r["later"].clone()), (json!(0), json!(0)));
        assert_eq!(r["skipped"].as_array().unwrap().len(), 2, "usage.json isn't a session");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn summary_lines() {
        assert_eq!(summary(&json!({ "restarted": 0, "later": 0, "skipped": [], "failed": [] })), "No open sessions to restart");
        assert_eq!(summary(&json!({ "restarted": 2, "later": 1, "skipped": ["runs in Orca"], "failed": [] })), "Restarted 2 sessions. 1 more after it finishes. 1 session in Orca left to Orca.");
        assert_eq!(summary(&json!({ "restarted": 1, "later": 0, "skipped": [], "failed": ["it couldn't be stopped"] })), "Restarted 1 session. one failed: it couldn't be stopped.");
    }
}
