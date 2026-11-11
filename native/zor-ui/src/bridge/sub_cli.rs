//! Chat through the Gemini CLI (`gemini`, Google account) and the Codex CLI (`codex`, ChatGPT account), so
//! the sidebar and the buddies can use those subscriptions instead of API keys. Models are "gemini-cli:default"
//! and "codex-cli:default"; each CLI picks the model its account allows. The prompt goes in on stdin and the
//! reply streams back on stdout. Claude has its own module (claude_cli.rs) because its output is stream-json.

use super::{common::hidden, Sink};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::Stdio,
    sync::mpsc,
    time::Duration,
};

/// (provider id, command name, label, arguments). Codex reads the prompt from stdin with "-" and
/// runs read-only. Antigravity (`agy`) ignores stdin in -p mode, so its prompt goes in as the last argument.
const CLIS: [(&str, &str, &str, &[&str]); 2] = [
    ("gemini-cli", "agy", "Gemini subscription", &["-p"]),
    ("codex-cli", "codex", "ChatGPT subscription", &["exec", "--skip-git-repo-check", "-s", "read-only", "-"]),
];

fn spec(provider: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static [&'static str])> {
    CLIS.iter().find(|c| c.0 == provider)
}

/// Path of the command: on PATH, else npm's global folder; None when it isn't installed.
fn find(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let npm = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("npm"));
    std::env::split_paths(&path).chain(npm).find_map(|d| ["exe", "cmd"].iter().map(|e| d.join(format!("{cmd}.{e}"))).find(|p| p.is_file()))
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

/// Antigravity's prompt: the last 12 turns (keeps it under the Windows command-line limit), no tools.
fn agy_prompt(msgs: &[Value], system: &str) -> String {
    let recent = &msgs[msgs.len().saturating_sub(12)..];
    format!("Reply as the assistant to this conversation. Do not use tools.\n\n{}", prompt(recent, system))
}

/// Streams the reply as {"delta"} lines, then {"done"}.
pub fn chat(provider: &str, msgs: &[Value], system: &str, out: &Sink) {
    let Some(&(_, cmd, label, args)) = spec(provider) else { out.emit(&json!({ "error": format!("unknown provider {provider}") })); return };
    let Some(exe) = find(cmd) else {
        let (pkg, how) = if cmd == "agy" { ("Antigravity CLI", "https://antigravity.google/docs/getting-started?tab=cli") } else { ("Codex CLI", "npm i -g @openai/codex") };
        out.emit(&json!({ "error": format!("{cmd} isn't installed, so the {label} can't be used. Install the {pkg} ({how}), run `{cmd}` once in a terminal to sign in, then use it here.") }));
        return;
    };
    let agy = cmd == "agy";
    let mut command = hidden(exe);
    command.args(args).current_dir(std::env::temp_dir()).stdout(Stdio::piped()).stderr(Stdio::null());
    if agy { command.arg(agy_prompt(msgs, system)).stdin(Stdio::null()); } else { command.stdin(Stdio::piped()); }
    let mut child = match command.spawn() {
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
            Ok(l) => { got = got || !l.trim().is_empty(); out.emit(&json!({ "delta": l })); }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if got || ok { out.emit(&json!({ "done": true })); }
    else { out.emit(&json!({ "error": format!("{cmd} stopped without an answer. Run `{cmd}` once in a terminal to sign in.") })); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_prompt() {
        let p = prompt(&[json!({ "role": "user", "content": "a" }), json!({ "role": "assistant", "content": "b" }), json!({ "role": "user", "content": "c" })], "sys");
        assert_eq!(p, "sys\n\nUser: a\n\nAssistant: b\n\nUser: c\n\nAssistant:");
    }

    #[test]
    fn agy_gets_the_last_twelve_turns() {
        let msgs: Vec<Value> = (0..20).map(|i| json!({ "role": "user", "content": format!("m{i}") })).collect();
        let p = agy_prompt(&msgs, "sys");
        assert!(p.starts_with("Reply as the assistant"));
        assert!(!p.contains("m7\n") && p.contains("User: m8") && p.contains("User: m19"));
    }

    #[test]
    fn knows_both_providers() {
        assert_eq!(spec("gemini-cli").unwrap().1, "agy");
        assert_eq!(spec("codex-cli").unwrap().1, "codex");
        assert!(spec("claude-cli").is_none());
        assert!(find("no-such-command-xyz").is_none());
    }
}
