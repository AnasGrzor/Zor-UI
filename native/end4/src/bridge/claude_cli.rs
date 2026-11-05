//! Chat through the `claude` command (Claude Code), so the sidebar and the buddies can use your Claude
//! subscription instead of an API key (scripts/claude_cli.py is the same thing for the Python bridge).
//! Models are "claude-cli:haiku", "claude-cli:sonnet", "claude-cli:opus". The CLI runs with no tools, no
//! hooks, no MCP servers and no saved session, from the temp folder, so a chat message can't touch your
//! files or pick up your Claude Code setup.

use super::{common::hidden, Sink};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    time::Duration,
};

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

/// Streams the reply as {"delta"} lines, then {"done"}.
pub fn chat(model_id: &str, msgs: &[Value], system: &str, out: &Sink) {
    let Some(exe) = find() else {
        out.emit(&json!({ "error": "Claude Code isn't installed, so the subscription can't be used. Install it and run `claude` once to log in." }));
        return;
    };
    let spawned = hidden(exe)
        .args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--model", alias(model_id), "--tools", "",
               "--system-prompt", system, "--setting-sources", "", "--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config"])
        .current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => { out.emit(&json!({ "error": format!("claude: {e}") })); return; }
    };
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(prompt(msgs).as_bytes()); }
    let Some(stdout) = child.stdout.take() else { let _ = child.kill(); return };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || { for l in BufReader::new(stdout).lines().map_while(Result::ok) { if tx.send(l).is_err() { break; } } });
    let (mut got, mut failed) = (false, false);
    loop {
        if out.stopped() { let _ = child.kill(); let _ = child.wait(); return; }
        let line = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(l) => l,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match read_line(&line) {
            Some(Ev::Delta(t)) => { got = true; out.emit(&json!({ "delta": t })); }
            Some(Ev::Error(e)) => { out.emit(&json!({ "error": e })); failed = true; break; }
            Some(Ev::Done(t)) if !got && !t.is_empty() => { got = true; out.emit(&json!({ "delta": t })); }
            _ => {}
        }
    }
    let _ = child.kill();
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if failed { return; }
    if !got && !ok { out.emit(&json!({ "error": "Claude Code stopped without an answer. Run `claude` once to log in." })); }
    else { out.emit(&json!({ "done": true })); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(t: &str) -> String { json!({ "type": "stream_event", "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": t } } }).to_string() }

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
