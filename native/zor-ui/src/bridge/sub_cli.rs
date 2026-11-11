//! Chat through Antigravity's `agy` command (Google account; it replaced the Gemini CLI) and the Codex CLI (`codex`,
//! ChatGPT account), so the sidebar and the buddies can use those subscriptions instead of API keys. Models are
//! "antigravity-cli:default" and "codex-cli:default"; each CLI picks the model its account allows. Claude has its own
//! module (claude_cli.rs) because its output is stream-json.
//!
//! Codex prints one JSON event per line (`--json`): the agent's message as it completes, and the turn's token counts at
//! the end. Antigravity's print mode prints one JSON object when it finishes (`response`, `usage`, `error`), so its reply is
//! read whole and then split into the same events.

use super::{common::hidden, Sink};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    time::Duration,
};

/// (provider id, command name, label, arguments). Codex reads the prompt from stdin with "-" and runs read-only. Antigravity's
/// arguments are built in `chat_agy` (the prompt is its -p value), so its entry has none here.
const CLIS: [(&str, &str, &str, &[&str]); 2] = [
    ("antigravity-cli", "agy", "Antigravity subscription", &[]),
    ("codex-cli", "codex", "ChatGPT subscription", &["exec", "--skip-git-repo-check", "-s", "read-only", "--json", "-"]),
];

fn spec(provider: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static [&'static str])> {
    CLIS.iter().find(|c| c.0 == provider)
}

/// Path of the command: on PATH, else npm's global folder, else the folder Antigravity's installer uses (`~\.gemini\bin`,
/// which the app's own environment may not have on its PATH). None when it isn't installed.
fn find(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let npm = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("npm"));
    let agy = std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join(".gemini").join("bin"));
    std::env::split_paths(&path).chain(npm).chain(agy).find_map(|d| ["exe", "cmd"].iter().map(|e| d.join(format!("{cmd}.{e}"))).find(|p| p.is_file()))
}

/// True when the provider's command is installed.
pub fn installed(provider: &str) -> bool { spec(provider).is_some_and(|c| find(c.1).is_some()) }

/// Model entries for both CLIs, installed or not, so picking one tells you how to install it.
pub fn models() -> Vec<Value> {
    CLIS.iter()
        .map(|c| json!({ "id": format!("{}:default", c.0), "name": c.2, "provider": c.0 })).collect()
}

/// The whole conversation as one prompt, system text first.
fn prompt(msgs: &[Value], system: &str) -> String {
    let turns: Vec<String> = msgs.iter()
        .map(|m| format!("{}: {}", if m["role"] == "assistant" { "Assistant" } else { "User" }, m["content"].as_str().unwrap_or(""))).collect();
    format!("{system}\n\n{}\n\nAssistant:", turns.join("\n\n"))
}

/// Streams the reply as {"delta"} lines, then {"done"}.
pub fn chat(provider: &str, msgs: &[Value], system: &str, out: &Sink) {
    let Some(&(_, cmd, label, args)) = spec(provider) else { out.emit(&json!({ "error": format!("unknown provider {provider}") })); return };
    let Some(exe) = find(cmd) else {
        let how = if cmd == "agy" { "Install Antigravity, then sign in to it once.".to_string() }
            else { format!("Install it with `npm i -g @openai/codex`, run `{cmd}` once in a terminal to sign in, then use it here.") };
        out.emit(&json!({ "error": format!("{cmd} isn't installed, so the {label} can't be used. {how}") }));
        return;
    };
    if cmd == "agy" { return chat_agy(exe, msgs, system, out); }
    let spawned = hidden(exe).args(args).current_dir(std::env::temp_dir())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => { out.emit(&json!({ "error": format!("{cmd}: {e}") })); return; }
    };
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(prompt(msgs, system).as_bytes()); }
    let Some(stdout) = child.stdout.take() else { let _ = child.kill(); return };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        let mut buf = Vec::new();
        while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
            if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() { break; }
            buf.clear();
        }
    });
    let mut got = false;
    loop {
        if out.stopped() { let _ = child.kill(); let _ = child.wait(); return; }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(l) => for e in codex_events(&l) { got = got || e["delta"].is_string(); out.emit(&e); },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if got || ok { out.emit(&json!({ "done": true })); }
    else { out.emit(&json!({ "error": format!("{cmd} stopped without an answer. Run `{cmd}` once in a terminal to sign in.") })); }
}

/// What one line of `codex exec --json` means, as the events the chat sends: the agent's message text when it completes, and the
/// turn's token counts from `turn.completed` (`input_tokens` already includes cached tokens). Other events are not shown.
pub fn codex_events(line: &str) -> Vec<Value> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return vec![] };
    match v["type"].as_str() {
        Some("item.completed") if v["item"]["type"] == "agent_message" => {
            v["item"]["text"].as_str().filter(|t| !t.is_empty()).map(|t| vec![json!({ "delta": t })]).unwrap_or_default()
        }
        Some("turn.completed") => match (v["usage"]["input_tokens"].as_u64(), v["usage"]["output_tokens"].as_u64()) {
            (Some(i), Some(o)) => vec![json!({ "usage": { "input": i, "output": o } })],
            _ => vec![],
        },
        _ => vec![],
    }
}

/// Antigravity's print mode: the prompt is the -p value, and the answer is one JSON object printed when the turn ends. `--mode plan`
/// keeps it read-only. `--disable-slash-commands` is not passed, because with it `--mode plan` does nothing.
fn chat_agy(exe: PathBuf, msgs: &[Value], system: &str, out: &Sink) {
    let p = prompt(msgs, system);
    let spawned = hidden(exe).args(["-p", p.as_str(), "--output-format", "json", "--mode", "plan"])
        .current_dir(std::env::temp_dir()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => { out.emit(&json!({ "error": format!("agy: {e}") })); return; }
    };
    let Some(mut stdout) = child.stdout.take() else { let _ = child.kill(); return };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    // the reply is read whole, so it is waited for; Stop still ends it
    let text = loop {
        if out.stopped() { let _ = child.kill(); let _ = child.wait(); return; }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(t) => break t,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break String::new(),
        }
    };
    let _ = child.wait();
    match parse_agy(&text) {
        Ok((reply, usage)) => {
            out.emit(&json!({ "delta": reply }));
            if let Some((input, output)) = usage { out.emit(&json!({ "usage": { "input": input, "output": output } })); }
            out.emit(&json!({ "done": true }));
        }
        Err(e) => { out.emit(&json!({ "error": e })); }
    }
}

/// Reads Antigravity's JSON reply: its text, and the token counts when it gave them. An error reply, a reply that isn't JSON,
/// and one with no text are all errors the user can read.
pub fn parse_agy(stdout: &str) -> Result<(String, Option<(u64, u64)>), String> {
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| "Antigravity printed a reply I couldn't read.".to_string())?;
    if let Some(e) = v["error"].as_str().filter(|e| !e.trim().is_empty()) { return Err(format!("Antigravity: {e}")); }
    if v["status"] == "ERROR" { return Err("Antigravity could not answer.".to_string()); }
    let reply = v["response"].as_str().unwrap_or("").trim().to_string();
    if reply.is_empty() { return Err("Antigravity returned no answer.".to_string()); }
    let usage = match (v["usage"]["input_tokens"].as_u64(), v["usage"]["output_tokens"].as_u64()) {
        (Some(i), Some(o)) => Some((i, o)),
        _ => None,
    };
    Ok((reply, usage))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reply Antigravity printed when the account was over quota (captured from `agy -p ... --output-format json`).
    const QUOTA_REPLY: &str = r#"{"conversation_id":"f465549c","status":"ERROR","response":"","error":"Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 163h7m0s.","duration_seconds":0,"num_turns":1,"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0}}"#;

    #[test]
    fn the_quota_reply_is_an_error_the_user_can_read() {
        let e = parse_agy(QUOTA_REPLY).unwrap_err();
        assert!(e.starts_with("Antigravity: Individual quota reached"), "{e}");
    }

    /// A success reply. The field names come from the quota reply above; the status value and the text are not yet captured
    /// from a real answer, so this shape is assumed until the account has quota again.
    #[test]
    fn a_reply_gives_its_text_and_its_counts() {
        let ok = r#"{"conversation_id":"x","status":"OK","response":" ok ","error":"","usage":{"input_tokens":12,"output_tokens":1,"total_tokens":13}}"#;
        assert_eq!(parse_agy(ok), Ok(("ok".to_string(), Some((12, 1)))));
        let no_counts = r#"{"status":"OK","response":"hi","error":"","usage":{}}"#;
        assert_eq!(parse_agy(no_counts), Ok(("hi".to_string(), None)), "no counts given: none reported");
    }

    #[test]
    fn text_it_cannot_read_or_an_empty_answer_is_an_error() {
        assert!(parse_agy("not json").unwrap_err().contains("couldn't read"));
        assert!(parse_agy(r#"{"status":"OK","response":"","error":""}"#).unwrap_err().contains("no answer"));
    }

    /// Lines from a real `codex exec --json` run (one word answer), as Codex printed them.
    #[test]
    fn codex_lines_give_the_message_and_the_turn_counts() {
        assert!(codex_events(r#"{"type":"thread.started","thread_id":"01a1cc10"}"#).is_empty());
        assert!(codex_events(r#"{"type":"turn.started"}"#).is_empty());
        assert_eq!(codex_events(r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"ok"}}"#), vec![json!({ "delta": "ok" })]);
        assert_eq!(codex_events(r#"{"type":"turn.completed","usage":{"input_tokens":21287,"cached_input_tokens":12416,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}"#),
            vec![json!({ "usage": { "input": 21287, "output": 5 } })], "input_tokens already includes the cached ones");
        assert!(codex_events(r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"ls"}}"#).is_empty(), "tool steps are not shown");
        assert!(codex_events("warning: not json").is_empty());
    }

    #[test]
    fn builds_the_prompt() {
        let p = prompt(&[json!({ "role": "user", "content": "a" }), json!({ "role": "assistant", "content": "b" }), json!({ "role": "user", "content": "c" })], "sys");
        assert_eq!(p, "sys\n\nUser: a\n\nAssistant: b\n\nUser: c\n\nAssistant:");
    }

    #[test]
    fn knows_both_providers() {
        assert_eq!(spec("antigravity-cli").unwrap().1, "agy");
        assert!(spec("gemini-cli").is_none(), "the Gemini CLI is discontinued");
        assert_eq!(spec("codex-cli").unwrap().1, "codex");
        assert!(spec("claude-cli").is_none());
        assert!(find("no-such-command-xyz").is_none());
    }
}
