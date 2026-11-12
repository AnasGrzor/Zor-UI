//! A small client for the Model Context Protocol (MCP) over stdio, so the app can start a tool server (Playwright's) as a child process and call its tools.
//! The protocol is JSON-RPC 2.0 with one JSON object per line. Lines that are not JSON, notifications and replies to other ids are skipped while waiting.

use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

/// Hides the console window of the child process.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The reply when the tool server does not answer in time.
const TOO_LONG: &str = "The tool server took too long.";
/// The reply when the pipe to the tool server is closed or cannot be written.
const CLOSED: &str = "The tool server closed.";
/// The reply when a tool gives no text at all.
const NO_ANSWER: &str = "The tool gave no answer.";
/// The reply when the server reports an error without a message.
const NO_MESSAGE: &str = "The tool server reported an error.";

/// One JSON-RPC request line (no trailing newline): {"jsonrpc":"2.0","id":id,"method":method,"params":params}.
pub fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// One JSON-RPC notification line (no id): {"jsonrpc":"2.0","method":method}.
pub fn notification(method: &str) -> String {
    json!({"jsonrpc": "2.0", "method": method}).to_string()
}

/// What a line from the server means for the request `id`: None when the line is not JSON, is a notification, or answers another id;
/// Some(Ok(result)) for its result; Some(Err(message)) for its error (the "error"."message" text, or "The tool server reported an error." when there is none).
pub fn reply_for(line: &str, id: u64) -> Option<Result<Value, String>> {
    let v: Value = serde_json::from_str(line).ok()?;
    // A line with a method is a notification or a request from the server, never a reply to us.
    if v.get("method").is_some() || v["id"].as_u64() != Some(id) {
        return None;
    }
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        let message = err["message"].as_str().unwrap_or(NO_MESSAGE).to_string();
        return Some(Err(message));
    }
    Some(Ok(v.get("result").cloned().unwrap_or(Value::Null)))
}

/// The text of a tools/call result: the "text" of every content item of type "text", joined with newlines.
/// Err(that text) when the result has "isError": true; Err("The tool gave no answer.") when there is no text at all.
pub fn tool_text(result: &Value) -> Result<String, String> {
    let texts: Vec<&str> = result["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["type"].as_str() == Some("text"))
                .filter_map(|item| item["text"].as_str())
                .collect()
        })
        .unwrap_or_default();
    if texts.is_empty() {
        return Err(NO_ANSWER.into());
    }
    let text = texts.join("\n");
    if result["isError"].as_bool() == Some(true) {
        Err(text)
    } else {
        Ok(text)
    }
}

/// A running tool server.
pub struct Server {
    /// The launched process, when there is one. `over` has none.
    child: Option<Child>,
    /// Where requests are written (the child's stdin).
    input: Box<dyn Write + Send>,
    /// Lines read from the server's stdout, in order.
    lines: Receiver<String>,
    /// The id the next request will use.
    next_id: u64,
}

impl Server {
    /// A server over a given pipe pair, without a process; `start` and the tests use it. Does NOT send anything.
    #[cfg(test)]
    pub fn over(input: Box<dyn Write + Send>, lines: Receiver<String>) -> Server {
        Server { child: None, input, lines, next_id: 1 }
    }

    /// Starts `program` with `args` in folder `dir` (hidden window, stdin and stdout piped, stderr discarded), then does the MCP handshake:
    /// request "initialize" with params {"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"zor-ui","version":"1"}} (wait up to `wait`),
    /// then the notification "notifications/initialized". A thread reads stdout line by line into the channel and ends when the pipe closes.
    /// `program` is run through `cmd.exe /c` so that .cmd launchers (pnpm, npx, corepack) are found: Command::new("cmd").arg("/c").arg(program).args(args).
    /// Each `(key, value)` in `env` is set on the child.
    pub fn start(program: &str, args: &[&str], dir: &Path, env: &[(&str, &str)], wait: Duration) -> Result<Server, String> {
        let mut child = Command::new("cmd")
            .arg("/c")
            .arg(program)
            .args(args)
            .current_dir(dir)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|_| "I couldn't start the tool server.".to_string())?;
        let input = child.stdin.take().ok_or("I couldn't start the tool server.")?;
        let stdout = child.stdout.take().ok_or("I couldn't start the tool server.")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        // If the handshake fails, dropping this server kills the process.
        let mut server = Server { child: Some(child), input: Box::new(input), lines: rx, next_id: 1 };
        server.ask(
            "initialize",
            json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "zor-ui", "version": "1"}}),
            wait,
        )?;
        server.send(&notification("notifications/initialized"))?;
        Ok(server)
    }

    /// Writes one line to the server and flushes it.
    fn send(&mut self, line: &str) -> Result<(), String> {
        let bytes = format!("{line}\n");
        self.input.write_all(bytes.as_bytes()).and_then(|_| self.input.flush()).map_err(|_| CLOSED.to_string())
    }

    /// Sends one request and waits up to `wait` for its reply, skipping every other line.
    /// Err("The tool server closed.") when the pipe is closed or the write fails; Err("The tool server took too long.") on timeout.
    pub fn ask(&mut self, method: &str, params: Value, wait: Duration) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&request(id, method, params))?;
        let start = Instant::now();
        loop {
            // Time left is counted from the start, so many skipped lines cannot extend the wait.
            let remaining = wait.saturating_sub(start.elapsed());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    if let Some(reply) = reply_for(&line, id) {
                        return reply;
                    }
                }
                Err(RecvTimeoutError::Timeout) => return Err(TOO_LONG.into()),
                Err(RecvTimeoutError::Disconnected) => return Err(CLOSED.into()),
            }
        }
    }

    /// The server's tools: the "tools" array of "tools/list".
    #[cfg(test)]
    pub fn tools(&mut self, wait: Duration) -> Result<Vec<Value>, String> {
        let result = self.ask("tools/list", json!({}), wait)?;
        Ok(result["tools"].as_array().cloned().unwrap_or_default())
    }

    /// Runs one tool: "tools/call" with {"name": name, "arguments": args}, returned through `tool_text`.
    pub fn call(&mut self, name: &str, args: &Value, wait: Duration) -> Result<String, String> {
        let result = self.ask("tools/call", json!({"name": name, "arguments": args}), wait)?;
        tool_text(&result)
    }
}

impl Drop for Server {
    /// Ends the server and everything it started (the launcher starts node, which starts the browser): `taskkill /PID <pid> /T /F`, hidden, when there is a child.
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = Command::new("taskkill")
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .status();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{Arc, Mutex};

    /// A writer that keeps every byte it is given, so a test can read back what was sent.
    #[derive(Clone)]
    struct Recorder(Arc<Mutex<Vec<u8>>>);

    impl Write for Recorder {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn request_and_notification_lines() {
        let line = request(7, "tools/list", json!({}));
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "tools/list");

        let note: Value = serde_json::from_str(&notification("notifications/initialized")).unwrap();
        assert_eq!(note["method"], "notifications/initialized");
        assert!(note.get("id").is_none());
    }

    #[test]
    fn reply_for_picks_the_right_line() {
        let ok = reply_for(r#"{"jsonrpc":"2.0","id":5,"result":{"x":1}}"#, 5);
        assert_eq!(ok, Some(Ok(json!({"x": 1}))));

        let err = reply_for(r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32000,"message":"boom"}}"#, 5);
        assert_eq!(err, Some(Err("boom".to_string())));

        let no_message = reply_for(r#"{"jsonrpc":"2.0","id":5,"error":{"code":-1}}"#, 5);
        assert_eq!(no_message, Some(Err(NO_MESSAGE.to_string())));

        assert_eq!(reply_for(r#"{"jsonrpc":"2.0","id":6,"result":{}}"#, 5), None);
        assert_eq!(reply_for(r#"{"jsonrpc":"2.0","method":"notifications/progress"}"#, 5), None);
        assert_eq!(reply_for(r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#, 5), None);
        assert_eq!(reply_for("not json", 5), None);
        assert_eq!(reply_for("", 5), None);
    }

    #[test]
    fn tool_text_joins_and_reports() {
        let two = json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]});
        assert_eq!(tool_text(&two), Ok("a\nb".to_string()));

        let with_image = json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "image", "data": "xx", "mimeType": "image/png"},
            {"type": "text", "text": "b"}
        ]});
        assert_eq!(tool_text(&with_image), Ok("a\nb".to_string()));

        let failed = json!({"isError": true, "content": [{"type": "text", "text": "bad"}]});
        assert_eq!(tool_text(&failed), Err("bad".to_string()));

        let no_text = json!({"content": [{"type": "image", "data": "x"}]});
        assert_eq!(tool_text(&no_text), Err(NO_ANSWER.to_string()));
    }

    #[test]
    fn round_trip_over_a_pipe_pair() {
        let (tx, rx) = mpsc::channel::<String>();
        tx.send("npm notice".into()).unwrap();
        tx.send(notification("notifications/message")).unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":99,"result":{}}"#.into()).unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"hi"}]}}"#.into()).unwrap();

        let sent = Recorder(Arc::new(Mutex::new(Vec::new())));
        let mut server = Server::over(Box::new(sent.clone()), rx);
        assert_eq!(server.call("browser_snapshot", &json!({}), Duration::from_secs(1)), Ok("hi".to_string()));

        let bytes = sent.0.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.ends_with('\n'));
        let v: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(v["method"], "tools/call");
        assert_eq!(v["params"]["name"], "browser_snapshot");

        // Nothing queued and the sender alive: timeout.
        assert_eq!(server.call("browser_snapshot", &json!({}), Duration::from_millis(50)), Err(TOO_LONG.to_string()));

        // Sender gone: closed.
        drop(tx);
        assert_eq!(server.call("browser_snapshot", &json!({}), Duration::from_millis(50)), Err(CLOSED.to_string()));
    }

    #[test]
    fn tools_reads_the_list() {
        let (tx, rx) = mpsc::channel::<String>();
        tx.send(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"a"},{"name":"b"}]}}"#.into()).unwrap();
        let mut server = Server::over(Box::new(Recorder(Arc::new(Mutex::new(Vec::new())))), rx);
        let tools = server.tools(Duration::from_secs(1)).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1]["name"], "b");
    }
}
