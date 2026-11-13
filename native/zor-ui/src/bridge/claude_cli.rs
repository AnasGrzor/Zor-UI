//! Chat through the `claude` command (Claude Code), so the sidebar and the buddies can use your Claude
//! subscription instead of an API key (scripts/claude_cli.py is the same thing for the Python bridge).
//! Models are "claude-cli:haiku", "claude-cli:sonnet", "claude-cli:opus". The CLI runs with no tools, no
//! hooks, no MCP servers and no saved session, from the temp folder, so a chat message can't touch your
//! files or pick up your Claude Code setup.

use super::{claude_profile::{active, config_dir_value, Root}, common::hidden, Sink};
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
        let name = format!("{}{}", a[..1].to_uppercase(), &a[1..]);
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

/// True when any message carries pictures: the CLI then reads its input as stream-json, the only way to hand it an image
/// without giving it a tool to open files.
fn has_images(msgs: &[Value]) -> bool { msgs.iter().any(|m| !super::web::images_of(m).is_empty()) }

/// The stdin line for a conversation with pictures: one user message holding every picture (after its label), then the
/// conversation as text.
fn image_input(msgs: &[Value]) -> String {
    let mut blocks = vec![];
    for m in msgs {
        for (mime, data, label) in super::web::images_of(m) {
            if !label.is_empty() { blocks.push(json!({ "type": "text", "text": label })); }
            blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
        }
    }
    let text = prompt(msgs);
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

/// The system prompt in a temp file, removed when the chat ends. It goes by file because a .cmd shim (npm install)
/// refuses command-line arguments with newlines, and the prompt has them.
struct SystemFile(PathBuf);

impl SystemFile {
    fn write(text: &str) -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("zorui-system-{}-{nanos}.txt", std::process::id()));
        std::fs::write(&path, text)?;
        Ok(Self(path))
    }
}

impl Drop for SystemFile {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

/// Streams the reply as {"delta"} lines, then {"done"}.
pub fn chat(model_id: &str, msgs: &[Value], system: &str, out: &Sink) {
    let Some(exe) = find() else {
        out.emit(&json!({ "error": "Claude Code isn't installed, so the subscription can't be used. Install it and run `claude` once to log in." }));
        return;
    };
    let pictures = has_images(msgs);
    let sys = match SystemFile::write(system) {
        Ok(f) => f,
        Err(e) => { out.emit(&json!({ "error": format!("claude: {e}") })); return; }
    };
    let mut cmd = hidden(exe);
    cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--model", alias(model_id), "--tools", "",
              "--system-prompt-file"]).arg(&sys.0)
        .args(["--setting-sources", "", "--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config"]);
    if pictures { cmd.args(["--input-format", "stream-json"]); }
    // zor-ui's own environment goes stale after an account switch, so take the active account from the registry
    let root = Root::live();
    match config_dir_value(&root, active(&root)) { Some(d) => cmd.env("CLAUDE_CONFIG_DIR", d), None => cmd.env_remove("CLAUDE_CONFIG_DIR") };
    let spawned = cmd.current_dir(std::env::temp_dir()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => { out.emit(&json!({ "error": format!("claude: {e}") })); return; }
    };
    // dropping stdin after the write ends the input, which is what tells the CLI to answer
    if let Some(mut si) = child.stdin.take() { let _ = si.write_all(if pictures { image_input(msgs) } else { prompt(msgs) }.as_bytes()); }
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
        // the final result line carries the token counts (agent::provider)
        if let Some(u) = serde_json::from_str::<Value>(&line).ok().and_then(|v| crate::agent::provider::usage_event("claude-cli", &v)) { out.emit(&u); }
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
    fn the_system_prompt_file_keeps_its_newlines_and_goes_away_with_the_chat() {
        let f = SystemFile::write("line one\nline two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f.0).unwrap(), "line one\nline two\n");
        let path = f.0.clone();
        drop(f);
        assert!(!path.exists());
    }

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
        assert!(has_images(&msgs));
        let line = image_input(&msgs);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "one line, so the CLI reads one message");
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["content"], json!([{ "type": "text", "text": "screen 1" }, { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "QUJD" } }, { "type": "text", "text": "where is save?" }]));
    }

    #[test]
    fn earlier_turns_ride_along_as_text() {
        let msgs = [json!({ "role": "user", "content": "hi" }), json!({ "role": "assistant", "content": "hello" }),
                    json!({ "role": "user", "content": "and now?", "images": [{ "data": "QQ==" }] })];
        let v: Value = serde_json::from_str(image_input(&msgs).trim()).unwrap();
        let c = v["message"]["content"].as_array().unwrap();
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[1]["text"], "User: hi\n\nAssistant: hello\n\nUser: and now?\n\nAssistant:");
    }

    #[test]
    fn text_only_chats_keep_the_plain_prompt() {
        let msgs = [json!({ "role": "user", "content": "hi" })];
        assert!(!has_images(&msgs));
        assert!(!has_images(&[json!({ "role": "user", "content": "hi", "images": [{ "data": "" }] })]), "an empty picture is no picture");
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
