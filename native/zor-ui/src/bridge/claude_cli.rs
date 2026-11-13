//! Chat through the `claude` command (Claude Code), so the sidebar and the buddies can use your Claude
//! subscription instead of an API key (scripts/claude_cli.py is the same thing for the Python bridge).
//! Models are "claude-cli:haiku", "claude-cli:sonnet", "claude-cli:opus". The CLI runs with no tools, no
//! hooks, no MCP servers and no saved session, from the temp folder, so a chat message can't touch your
//! files or pick up your Claude Code setup.

use super::{claude_profile::{active, config_dir_value, Root}, common::hidden, Sink};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Stdio},
    sync::{mpsc, Mutex, OnceLock},
    time::{Duration, Instant},
};

/// The system text every process starts with. The real instructions (persona, the time, the PC) travel in the message
/// they change with, so one process serves every task.
const BASE: &str = "You are the assistant inside a desktop app. A message may begin with an \"Instructions\" block: follow it for that message and the ones after, until a newer block replaces it.";

/// A process no request has used for this long is ended, so an idle `claude` does not stay in memory.
const IDLE: Duration = Duration::from_secs(300);

const ALIASES: [&str; 3] = ["haiku", "sonnet", "opus"];

/// The model entries for the picker.
pub fn models() -> Vec<Value> {
    ALIASES.iter().map(|a| {
        let name = format!("{}{}", &a[..1].to_uppercase(), &a[1..]);
        json!({ "id": format!("claude-cli:{a}"), "name": format!("Claude subscription: {name}"), "provider": "claude-cli" })
    }).collect()
}

/// Path of the claude command: on PATH, else the installer's own folder; None when Claude Code isn't installed.
pub fn find() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".local").join("bin"));
    std::env::split_paths(&path).chain(home).find_map(|d| ["claude.exe", "claude.cmd"].iter().map(|n| d.join(n)).find(|p| p.is_file()))
}

fn alias(model_id: &str) -> &str {
    let a = model_id.rsplit(':').next().unwrap_or("");
    ALIASES.iter().find(|x| **x == a).copied().unwrap_or("haiku")
}

/// The conversation as one prompt: just the text for a single message, else a labelled transcript.
fn prompt(msgs: &[Value]) -> String {
    let text = |m: &Value| m["content"].as_str().unwrap_or("").to_string();
    if let [only] = msgs { return text(only); }
    let turns: Vec<String> = msgs.iter().map(|m| format!("{}: {}", if m["role"] == "assistant" { "Assistant" } else { "User" }, text(m))).collect();
    format!("{}\n\nAssistant:", turns.join("\n\n"))
}

/// The stdin line for a message or a conversation: one user message holding every picture (after its label), then the
/// text. Input is always stream-json, the way to hand the CLI a picture without giving it a tool to open files.
/// `lead` (the instructions, when they changed) goes before the text.
fn image_input(msgs: &[Value], lead: &str) -> String {
    let mut blocks = vec![];
    for m in msgs {
        for (mime, data, label) in super::web::images_of(m) {
            if !label.is_empty() { blocks.push(json!({ "type": "text", "text": label })); }
            blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
        }
    }
    let text = match (lead.is_empty(), prompt(msgs)) {
        (true, text) => text,
        (false, text) if text.trim().is_empty() => lead.to_string(),
        (false, text) => format!("{lead}\n\n{text}"),
    };
    if !text.trim().is_empty() { blocks.push(json!({ "type": "text", "text": text })); }
    format!("{}\n", json!({ "type": "user", "message": { "role": "user", "content": blocks } }))
}

#[derive(Debug, PartialEq)]
enum Ev { Delta(String), Done(String), Error(String) }

/// One stream-json line from the CLI.
fn read_line(line: &str) -> Option<Ev> {
    let ev: Value = serde_json::from_str(line).ok()?;
    match ev["type"].as_str()? {
        "stream_event" => {
            let d = &ev["event"]["delta"];
            (d["type"] == "text_delta").then(|| d["text"].as_str().filter(|t| !t.is_empty()).map(|t| Ev::Delta(t.to_string()))).flatten()
        }
        "result" => {
            let text = ev["result"].as_str().unwrap_or("").to_string();
            Some(if ev["is_error"] == true {
                Ev::Error(if text.is_empty() { "Claude Code returned an error. Run `claude` once to log in.".into() } else { text })
            } else { Ev::Done(text) })
        }
        _ => None,
    }
}

/// One live `claude` process that a task talks through. The CLI keeps the conversation, so each step sends only the
/// message that is new; a new process starts only for a new task (its system text differs) or another model or account.
struct Session {
    key: String,
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    /// (role, content) of every message the CLI has, the ones sent and its own answers, to tell what is new.
    seen: Vec<(String, String)>,
    /// The system text the process was last given, in an instructions block; "" before the first turn.
    sent_system: String,
    /// When a request last used it (the idle timeout counts from here).
    last: Instant,
}

/// The instructions block for a message: sent with the first turn of a task and again when they change.
fn instructions(system: &str) -> String { format!("Instructions for this and the following messages:\n{system}") }

/// True once `last` is `limit` or more before `now`.
fn is_idle(last: Instant, now: Instant, limit: Duration) -> bool { now.duration_since(last) >= limit }

impl Drop for Session {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

/// The session in use. A request that finds it busy (another request holds it) gets its own process, ended with the request.
/// shortcut: the session lives until the next task or the app exits, no idle timeout; upgrade: end it after a few idle minutes.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn spawn(exe: &Path, key: String, alias: &str, dir: Option<PathBuf>) -> Result<Session, String> {
    let mut cmd = hidden(exe);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--model", alias, "--tools", "",
              "--system-prompt", BASE, "--setting-sources", "", "--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config"]);
    match dir { Some(d) => cmd.env("CLAUDE_CONFIG_DIR", d), None => cmd.env_remove("CLAUDE_CONFIG_DIR") };
    let mut child = cmd.current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("claude: {e}"))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else { let _ = child.kill(); return Err("claude: no pipes".into()) };
    let (tx, lines) = mpsc::channel::<String>();
    std::thread::spawn(move || { for l in BufReader::new(stdout).lines().map_while(Result::ok) { if tx.send(l).is_err() { break; } } });
    Ok(Session { key, child, stdin, lines, seen: vec![], sent_system: String::new(), last: Instant::now() })
}

enum Turn { Text(String), Failed(String), Dead, Stopped }

/// Sends one user line on the session and streams its answer to `out` as deltas, until the CLI's result for it.
fn run_turn(s: &mut Session, line: &str, out: &Sink) -> Turn {
    if s.stdin.write_all(line.as_bytes()).and_then(|_| s.stdin.flush()).is_err() { return Turn::Dead; }
    let mut text = String::new();
    loop {
        if out.stopped() { return Turn::Stopped; }
        match s.lines.recv_timeout(Duration::from_millis(200)) {
            Ok(l) => {
                // the final result line carries the token counts (agent::provider)
                if let Some(u) = serde_json::from_str::<Value>(&l).ok().and_then(|v| crate::agent::provider::usage_event("claude-cli", &v)) { out.emit(&u); }
                match read_line(&l) {
                Some(Ev::Delta(t)) => { out.emit(&json!({ "delta": t })); text.push_str(&t); }
                Some(Ev::Error(e)) => return Turn::Failed(e),
                Some(Ev::Done(t)) => {
                    if text.is_empty() && !t.is_empty() { out.emit(&json!({ "delta": t })); text = t; }
                    return Turn::Text(text);
                }
                None => {}
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Turn::Dead,
        }
    }
}

/// Starts the idle check once: every 15 s it ends the session when no request has used it for `IDLE`. A session a request
/// holds right now is skipped (try_lock), so a running turn is never cut.
fn reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(15));
            let Ok(mut g) = SESSION.try_lock() else { continue };
            if g.as_ref().is_some_and(|s| is_idle(s.last, Instant::now(), IDLE)) {
                *g = None;
                crate::log::line("claude session ended after 5 idle minutes");
            }
        });
    });
}

fn seen_of(msgs: &[Value]) -> Vec<(String, String)> {
    msgs.iter().map(|m| (m["role"].as_str().unwrap_or("").to_string(), m["content"].as_str().unwrap_or("").to_string())).collect()
}

/// True when `msgs` is what the session has seen plus one new user message (the step after the last answer).
fn extends(seen: &[(String, String)], msgs: &[Value]) -> bool {
    msgs.len() == seen.len() + 1 && msgs.last().is_some_and(|m| m["role"] == "user")
        && seen.iter().zip(msgs).all(|((role, text), m)| m["role"] == role.as_str() && m["content"].as_str().unwrap_or("") == text)
}

/// Streams the reply as {"delta"} lines, then {"done"}. Steps of one task reuse the same process (see `Session`).
pub fn chat(model_id: &str, msgs: &[Value], system: &str, out: &Sink) {
    let Some(exe) = find() else {
        out.emit(&json!({ "error": "Claude Code isn't installed, so the subscription can't be used. Install it and run `claude` once to log in." }));
        return;
    };
    // zor-ui's own environment goes stale after an account switch, so take the active account from the registry
    let root = Root::live();
    let dir = config_dir_value(&root, active(&root));
    let alias = alias(model_id);
    let key = format!("{alias}\n{dir:?}");
    reaper();
    let mut own = None;
    let mut guard = SESSION.try_lock().ok();
    let slot: &mut Option<Session> = match guard.as_deref_mut() { Some(s) => s, None => &mut own };
    for attempt in 0..2 {
        let reuse = attempt == 0 && slot.as_ref().is_some_and(|s| s.key == key && extends(&s.seen, msgs));
        let new = if reuse { &msgs[slot.as_ref().map_or(0, |s| s.seen.len())..] } else { msgs };
        if !reuse {
            *slot = None;
            match spawn(&exe, key.clone(), alias, dir.clone()) {
                Ok(s) => *slot = Some(s),
                Err(e) => { out.emit(&json!({ "error": e })); return; }
            }
        }
        let Some(s) = slot.as_mut() else { return };
        // the instructions go out with the first message of a task, and again whenever they change
        let lead = if s.sent_system == system { String::new() } else { instructions(system) };
        match run_turn(s, &image_input(new, &lead), out) {
            Turn::Text(text) => {
                s.seen = seen_of(msgs);
                s.seen.push(("assistant".into(), text));
                s.sent_system = system.to_string();
                s.last = Instant::now();
                out.emit(&json!({ "done": true }));
                return;
            }
            Turn::Failed(e) => { *slot = None; out.emit(&json!({ "error": e })); return; }
            Turn::Stopped => { *slot = None; return; }
            // the process ended while it sat idle: a fresh one answers this step
            Turn::Dead if reuse => { *slot = None; }
            Turn::Dead => { *slot = None; out.emit(&json!({ "error": "Claude Code stopped without an answer. Run `claude` once to log in." })); return; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(t: &str) -> String { json!({ "type": "stream_event", "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": t } } }).to_string() }

    /// What Rust's std does with a .cmd shim (npm installs `claude.cmd`): a newline argument is refused, a 7,000-char
    /// argument runs, and an 8,500-char one fails at cmd.exe's line limit (8,191). Pins why the prompt can't be an argument.
    #[test]
    fn batch_shims_refuse_newline_arguments() {
        let dir = std::env::temp_dir().join(format!("zorui-cmdshim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bat = dir.join("echo.cmd");
        std::fs::write(&bat, "@echo off\r\necho %~1\r\n").unwrap();
        let run = |arg: String| std::process::Command::new(&bat).arg(arg).output().map(|o| o.status.success()).ok();
        let nl = std::process::Command::new(&bat).arg("line one\nline two").output();
        let (short, long) = (run("x".repeat(7000)), run("x".repeat(8500)));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(nl.is_err(), "std accepted a newline argument: {nl:?}");
        assert_eq!(short, Some(true));
        assert_eq!(long, Some(false));
    }

    #[test]
    fn model_ids_map_to_aliases() {
        assert_eq!(alias("claude-cli:sonnet"), "sonnet");
        assert_eq!(alias("claude-cli:opus"), "opus");
        assert_eq!(alias("claude-cli:bogus"), "haiku");
        let ids: Vec<String> = models().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, ["claude-cli:haiku", "claude-cli:sonnet", "claude-cli:opus"]);
        assert_eq!(models()[1]["name"], "Claude subscription: Sonnet");
    }

    #[test]
    fn builds_the_prompt() {
        assert_eq!(prompt(&[json!({ "role": "user", "content": "hi" })]), "hi");
        let p = prompt(&[json!({ "role": "user", "content": "a" }), json!({ "role": "assistant", "content": "b" }), json!({ "role": "user", "content": "c" })]);
        assert_eq!(p, "User: a\n\nAssistant: b\n\nUser: c\n\nAssistant:");
    }

    #[test]
    fn pictures_go_in_as_one_stream_json_user_message() {
        let msgs = [json!({ "role": "user", "content": "where is save?", "images": [{ "mime": "image/jpeg", "data": "QUJD", "label": "screen 1" }] })];
        let line = image_input(&msgs, "");
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "one line, so the CLI reads one message");
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["content"], json!([{ "type": "text", "text": "screen 1" }, { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "QUJD" } }, { "type": "text", "text": "where is save?" }]));
    }

    #[test]
    fn earlier_turns_ride_along_as_text() {
        let msgs = [json!({ "role": "user", "content": "hi" }), json!({ "role": "assistant", "content": "hello" }),
                    json!({ "role": "user", "content": "and now?", "images": [{ "data": "QQ==" }] })];
        let v: Value = serde_json::from_str(image_input(&msgs, "").trim()).unwrap();
        let c = v["message"]["content"].as_array().unwrap();
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[1]["text"], "User: hi\n\nAssistant: hello\n\nUser: and now?\n\nAssistant:");
    }

    #[test]
    fn instructions_ride_in_front_of_the_text() {
        let v: Value = serde_json::from_str(image_input(&[json!({ "role": "user", "content": "hi" })], "Instructions:\nbe brief").trim()).unwrap();
        assert_eq!(v["message"]["content"], json!([{ "type": "text", "text": "Instructions:\nbe brief\n\nhi" }]));
    }

    #[test]
    fn a_session_idles_out_after_its_limit() {
        let t0 = Instant::now();
        assert!(!is_idle(t0, t0 + Duration::from_secs(299), IDLE));
        assert!(is_idle(t0, t0 + IDLE, IDLE));
    }

    #[test]
    fn a_step_sends_only_the_message_the_session_has_not_seen() {
        let said = json!({ "role": "user", "content": "open notepad" });
        let answer = json!({ "role": "assistant", "content": "{\"say\":\"ok\"}" });
        let fb = json!({ "role": "user", "content": "Result: opened" });
        let seen = seen_of(&[said.clone(), answer.clone()]);
        assert!(extends(&seen, &[said.clone(), answer.clone(), fb.clone()]), "one new message after the answer reuses the process");
        assert!(!extends(&seen, &[said.clone(), fb.clone()]), "a different history starts a new one");
        assert!(!extends(&seen, &[said.clone(), answer.clone(), json!({ "role": "user", "content": "x" }), fb.clone()]), "two new messages start a new one");
        assert!(!extends(&seen, &[json!({ "role": "user", "content": "other" }), answer.clone(), fb.clone()]), "an edited message starts a new one");
    }

    #[test]
    fn reads_stream_lines() {
        assert_eq!(read_line(&delta("Hel")), Some(Ev::Delta("Hel".into())));
        assert_eq!(read_line(&json!({ "type": "result", "is_error": false, "result": "Hello" }).to_string()), Some(Ev::Done("Hello".into())));
        assert_eq!(read_line(&json!({ "type": "result", "is_error": true, "result": "Not logged in" }).to_string()), Some(Ev::Error("Not logged in".into())));
        assert!(matches!(read_line(&json!({ "type": "result", "is_error": true }).to_string()), Some(Ev::Error(e)) if e.contains("log in")));
        assert_eq!(read_line(&json!({ "type": "system", "subtype": "init" }).to_string()), None);
        assert_eq!(read_line("not json"), None);
        assert_eq!(read_line(&delta("")), None);
    }
}
