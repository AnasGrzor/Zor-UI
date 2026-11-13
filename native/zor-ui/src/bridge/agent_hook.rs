//! Rust port of scripts/end4_agent.py: the hook that coding agents (Claude Code, Codex, ...) call so the
//! island knows when one needs you.
//!
//!   zorui.exe --agent <agent>                 reads one JSON object on stdin, records it, prints nothing
//!   zorui.exe --agent <agent> --then <cmd>    status line mode: records it, then runs <cmd> with the same
//!                                             stdin and prints its output (wraps an existing status line)
//!   zorui.exe --install-agents                adds the hooks (and Claude Code's status line) to each agent found
//!                                             on the PC, see known() (backs the files up first)
//!
//! Understood input: hook payloads with "hook_event_name" (Claude Code, Codex, Gemini CLI), Codex's notify
//! payload ({"type": "agent-turn-complete"}, also accepted as the last argument), Claude Code's status line
//! JSON ("rate_limits"), and zor-ui's own {"session", "state": waiting|working|done|ended, "message", "usage"}.
//! Writes agents\<agent>\<session>.json and usage.json under the data folder; notify.rs reads them.

use super::claude_profile::Root;
use super::common::{hidden, root, save_json};
use serde_json::{json, Map, Value};
use std::{
    io::{IsTerminal, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const WAITING: &[&str] = &["PermissionRequest", "Elicitation"];
const WORKING: &[&str] = &["UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "SubagentStart", "SubagentStop",
    "PreCompact", "PostCompact", "ElicitationResult", "BeforeAgent", "BeforeTool", "AfterTool", "BeforeModel", "AfterModel", "PreCompress",
    "beforeSubmitPrompt", "beforeShellExecution", "beforeMCPExecution", "afterFileEdit"];
const DONE: &[&str] = &["Stop", "StopFailure", "AfterAgent", "agent-turn-complete", "stop"];
/// Seconds the PermissionRequest hook may wait for Allow or Deny (permit.rs gives up a little sooner).
const PERMIT_TIMEOUT: u32 = 120;
const CLAUDE_EVENTS: &[&str] = &["UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "PermissionRequest",
    "Notification", "Elicitation", "ElicitationResult", "Stop", "StopFailure", "SessionEnd", "SubagentStart", "SubagentStop"];
const CODEX_EVENTS: &[&str] = &["UserPromptSubmit", "PreToolUse", "PermissionRequest", "PostToolUse", "Stop"];
const GEMINI_EVENTS: &[&str] = &["BeforeAgent", "BeforeTool", "AfterTool", "Notification", "AfterAgent", "SessionEnd"];
/// Cursor's own hook names (a flat hooks file, see add_flat_hooks).
const CURSOR_EVENTS: &[&str] = &["beforeSubmitPrompt", "beforeShellExecution", "beforeMCPExecution", "afterFileEdit", "stop"];
/// Keys that name a session, in the order they are tried.
const SESSION_KEYS: &[&str] = &["session_id", "session", "thread-id", "conversation_id"];

/// An agent whose hooks we can add. Installed means `dir` (under the home folder, or absolute) exists and holds one of
/// `marks` (empty: the folder is enough). `ms`: the agent reads hook timeouts in milliseconds (Gemini), not seconds.
/// `flat`: its hooks file is Cursor's flat layout ({"version": 1, "hooks": {event: [{"command"}]}}), not the nested one.
/// A new Claude-style agent is one more row, or an entry the user adds under "customAgents" in the settings.
/// `resume` resumes session {id} (agent_restart.rs); `model_arg` adds the model the session last used, {model}.
struct Known { id: String, name: String, dir: String, file: String, marks: Vec<String>, events: Vec<String>, status: bool, ms: bool, flat: bool, resume: String, model_arg: String }

fn strs(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

fn known(custom: &Value) -> Vec<Known> {
    let row = |id: &str, name: &str, dir: &str, file: &str, marks: &[&str], events: &[&str], status, ms, flat, resume: &str, model_arg: &str| Known {
        id: id.into(), name: name.into(), dir: dir.into(), file: file.into(), marks: strs(marks), events: strs(events), status, ms, flat, resume: resume.into(), model_arg: model_arg.into() };
    let mut all = vec![
        row("claude", "Claude Code", ".claude", "settings.json", &[], CLAUDE_EVENTS, true, false, false, "claude --resume {id}", "--model {model}"),
        row("codex", "Codex", ".codex", "hooks.json", &[], CODEX_EVENTS, false, false, false, "codex resume {id}", "-m {model}"),
        // ~/.gemini also holds other Google tools, so wait for a file only the Gemini CLI writes
        row("gemini", "Gemini CLI", ".gemini", "settings.json", &["settings.json", "oauth_creds.json"], GEMINI_EVENTS, false, true, false, "gemini --resume {id}", "-m {model}"),
        // Cursor also runs Claude Code's hooks; its payloads carry cursor_version, which cli() uses to file them under cursor
        row("cursor", "Cursor", ".cursor", "hooks.json", &[], CURSOR_EVENTS, false, false, true, "", ""),
        // same nested hook format as Claude Code, each in its own settings file (from CodeIsland's list of agents; not yet checked against each tool's docs)
        row("qwen", "Qwen Code", ".qwen", "settings.json", &["settings.json"], CODEX_EVENTS, false, false, false, "", ""),
        row("qoder", "Qoder", ".qoder", "settings.json", &["settings.json"], CODEX_EVENTS, false, false, false, "", ""),
        row("factory", "Factory Droid", ".factory", "settings.json", &["settings.json"], CODEX_EVENTS, false, false, false, "", ""),
        row("codebuddy", "CodeBuddy", ".codebuddy", "settings.json", &["settings.json"], CODEX_EVENTS, false, false, false, "", ""),
    ];
    for c in custom.as_array().into_iter().flatten() {
        let (name, dir, file) = (c["name"].as_str().unwrap_or("").trim(), c["dir"].as_str().unwrap_or("").trim(), c["file"].as_str().unwrap_or("").trim());
        let id = safe(&name.to_lowercase());
        if name.is_empty() || dir.is_empty() || file.is_empty() || all.iter().any(|k| k.id == id) { continue; } // a custom agent can't replace a built-in one
        let (resume, model_arg) = (c["resume"].as_str().unwrap_or("").trim(), c["modelArg"].as_str().unwrap_or("").trim());
        all.push(row(&id, name, dir, file, &[], CODEX_EVENTS, false, false, false, resume, model_arg));
    }
    all
}

/// How to resume a session of `agent`: (command template with {id}, model argument with {model}). None for an agent
/// without a resume command.
pub(super) fn resume_of(agent: &str, custom: &Value) -> Option<(String, String)> {
    known(custom).into_iter().find(|k| k.id == agent && !k.resume.is_empty()).map(|k| (k.resume, k.model_arg))
}

/// The agents' session folder, `agents\<agent>` under the data folder.
pub(super) fn sessions_dir(agent: &str) -> PathBuf { root().join("agents").join(safe(agent)) }

/// The model a payload names: Claude Code's status line has {"model": {"id"}}, other agents a plain string.
fn model_of(p: &Value) -> Option<Value> {
    p["model"]["id"].as_str().or(p["model"].as_str()).filter(|m| !m.is_empty()).map(|m| json!(m))
}

fn now_s() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

/// A file-name-safe version of `name` (80 chars at most).
pub(super) fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '_' }).take(80).collect();
    if s.is_empty() { "default".into() } else { s }
}

/// Collapses whitespace and cuts to `n` characters with an ellipsis.
fn short(v: &str, n: usize) -> String {
    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
    if v.chars().count() <= n { v } else { format!("{}…", v.chars().take(n - 1).collect::<String>()) }
}

/// The first non-empty string among `keys` of `p`.
fn first_str<'a>(p: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().filter_map(|k| p[*k].as_str()).find(|s| !s.is_empty()).unwrap_or("")
}

/// The tool a payload names and its one detail (the command or the file): ("Bash", "npm test").
fn tool_of(p: &Value) -> (&str, &str) {
    let inp = &p["tool_input"];
    let detail = if inp.is_object() { first_str(inp, &["command", "file_path", "path", "url", "description"]) } else { "" };
    (p["tool_name"].as_str().unwrap_or(""), detail)
}

/// Longest permission question kept. The island wraps and scrolls it, so the user can read what they allow.
/// Cut at 90 it hid the end of the command or the reason, which is what the user needs to decide.
const ASK_TEXT_MAX: usize = 400;

/// What the agent is asking for. Kept whole up to ASK_TEXT_MAX, so the island can show all of it.
fn ask_text(p: &Value) -> String {
    if let Some(m) = p["message"].as_str().filter(|m| !m.is_empty()) { return short(m, ASK_TEXT_MAX); }
    let (tool, detail) = tool_of(p);
    if tool == "AskUserQuestion" {
        if let Some(q) = p["tool_input"]["questions"].as_array().and_then(|q| q.first()) { return short(q["question"].as_str().unwrap_or("Has a question"), ASK_TEXT_MAX); }
    }
    if tool == "ExitPlanMode" { return "Plan ready for review".into(); } // the plan itself is in choice_of
    short(&if !detail.is_empty() { format!("Allow {tool}: {detail}") } else if !tool.is_empty() { format!("Allow {tool}?") } else { "Needs your input".into() }, ASK_TEXT_MAX)
}

/// What Claude Code asks beyond a yes or no: the plan (ExitPlanMode) or the questions (AskUserQuestion), for the island
/// to show and take answers to. Null for a tool permission.
fn choice_of(p: &Value) -> Value {
    let inp = &p["tool_input"];
    match p["tool_name"].as_str() {
        Some("ExitPlanMode") => json!({ "kind": "plan", "plan": inp["plan"].as_str().unwrap_or("") }),
        Some("AskUserQuestion") => inp["questions"].as_array().filter(|q| !q.is_empty()).map_or(Value::Null, |q| json!({ "kind": "question", "questions": q })),
        _ => Value::Null,
    }
}

/// The tool a session runs now, in one line: "Bash: npm test". Cursor's shell hook has no tool name, only the command.
fn tool_text(p: &Value) -> String {
    let (tool, detail) = tool_of(p);
    let (tool, detail) = if tool.is_empty() && !first_str(p, &["command"]).is_empty() { ("Shell", first_str(p, &["command"])) } else { (tool, detail) };
    short(&match (tool.is_empty(), detail.is_empty()) {
        (false, false) => format!("{tool}: {detail}"),
        (false, true) => tool.to_string(),
        (true, false) => detail.to_string(),
        _ => "working".into(),
    }, 90)
}

/// Claude Code's status line: the context fill, tokens so far and session cost. Only the fields it sends.
fn spend_of(p: &Value) -> Option<Value> {
    let cw = &p["context_window"];
    let tokens = match (cw["total_input_tokens"].as_u64(), cw["total_output_tokens"].as_u64()) {
        (Some(i), Some(o)) => Some(i + o),
        (i, o) => i.or(o),
    };
    let mut s = Map::new();
    if let Some(c) = cw["used_percentage"].as_f64() { s.insert("ctx".into(), json!(c)); }
    if let Some(t) = tokens { s.insert("tok".into(), json!(t)); }
    if let Some(c) = p["cost"]["total_cost_usd"].as_f64() { s.insert("cost".into(), json!(c)); }
    (!s.is_empty()).then(|| Value::Object(s))
}

/// Codex's tokens for the session: the running total in its rollout's newest token event.
fn codex_spend(text: &str) -> Option<Value> {
    let tok = text.lines().rev().filter(|l| l.contains("\"total_token_usage\"")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        v.pointer("/payload/info/total_token_usage/total_tokens")?.as_u64()
    })?;
    Some(json!({ "tok": tok }))
}

/// What the turn is doing: its prompt, when it started, the tool in use, the subagents and the spend.
/// A subagent's tool goes to that subagent, not to the session.
fn turn_fields(rec: &mut Value, old: &Value, p: &Value, state: &str) {
    let (ev, now) = (first_str(p, &["hook_event_name", "type"]), now_s());
    let active = state == "working" || state == "waiting";
    let was_active = matches!(old["state"].as_str(), Some("working" | "waiting"));
    let starts = matches!(ev, "UserPromptSubmit" | "BeforeAgent" | "beforeSubmitPrompt");
    let tool_ev = matches!(ev, "PreToolUse" | "BeforeTool" | "beforeShellExecution" | "beforeMCPExecution");
    let end_ev = matches!(ev, "PostToolUse" | "PostToolUseFailure" | "AfterTool" | "afterFileEdit");
    let sub = first_str(p, &["agent_id"]);
    let mut subs = old["subs"].as_object().cloned().unwrap_or_default();
    match ev {
        "SubagentStart" => { subs.insert(sub.to_string(), json!({ "type": first_str(p, &["agent_type"]), "tool": "", "since": now })); }
        "SubagentStop" => { subs.remove(sub); }
        _ => {}
    }
    let mut tool = old["tool"].as_str().unwrap_or("").to_string();
    if !sub.is_empty() {
        if let (Some(s), true) = (subs.get_mut(sub), tool_ev || end_ev) { s["tool"] = json!(if tool_ev { tool_text(p) } else { String::new() }); }
    } else if tool_ev || end_ev || !active {
        tool = if tool_ev { tool_text(p) } else { String::new() };
    }
    if !active { subs.clear(); } // a finished turn has no subagent still running
    let since = if !active { Value::Null } else if starts || !was_active || old["since"].is_null() { json!(now) } else { old["since"].clone() };
    rec["prompt"] = json!(if starts { short(first_str(p, &["prompt"]), 90) } else { old["prompt"].as_str().unwrap_or("").to_string() });
    rec["tool"] = json!(tool);
    rec["since"] = since;
    rec["subs"] = Value::Object(subs);
    rec["spend"] = spend_of(p).or_else(|| Some(old["spend"].clone()).filter(|s| !s.is_null())).unwrap_or(Value::Null);
}

/// (state, message) from a payload, or None if it carries no state.
fn state_of(p: &Value) -> Option<(&'static str, String)> {
    if let Some(s) = ["waiting", "working", "done", "ended"].into_iter().find(|s| p["state"] == *s) {
        return Some((s, short(p["message"].as_str().unwrap_or(""), 90)));
    }
    let ev = first_str(p, &["hook_event_name", "type"]);
    if ev == "Notification" {
        // Claude Code: idle_prompt is the "waiting for your next prompt" reminder, not a question.
        return Some(if p["notification_type"] == "idle_prompt" { ("done", String::new()) } else { ("waiting", ask_text(p)) });
    }
    if WAITING.contains(&ev) { Some(("waiting", ask_text(p))) }
    else if WORKING.contains(&ev) { Some(("working", String::new())) }
    else if DONE.contains(&ev) { Some(("done", String::new())) }
    else if ev == "SessionEnd" { Some(("ended", String::new())) }
    else { None }
}

fn usage_of(p: &Value) -> Option<Map<String, Value>> {
    let lim = if p["usage"].is_object() { &p["usage"] } else { &p["rate_limits"] }.as_object()?;
    let w: Map<String, Value> = lim.iter().filter_map(|(k, v)| {
        let used = v.get("used_percentage")?.as_f64()?;
        Some((k.clone(), json!({ "used_percentage": used, "resets_at": v.get("resets_at").cloned().unwrap_or(Value::Null) })))
    }).collect();
    (!w.is_empty()).then_some(w)
}

/// Codex keeps its plan limits out of the hook payload. They sit in the session's rollout file, in the newest
/// `rate_limits` of its token events. Read from the file's tail: the file grows large over a long session.
fn codex_text(p: &Value) -> Option<String> {
    let path = p["transcript_path"].as_str().map(PathBuf::from).filter(|f| f.is_file())
        .or_else(|| rollout_of(&home().join(".codex").join("sessions"), first_str(p, SESSION_KEYS)))?;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The session's rollout file: `rollout-<date>-<session id>.jsonl` somewhere under the sessions folder.
fn rollout_of(dir: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() { return None; }
    let end = format!("{id}.jsonl");
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() { if let Some(f) = rollout_of(&p, id) { return Some(f); } }
        else if p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&end)) { return Some(p); }
    }
    None
}

/// The newest rate limits in a rollout's text, named like Claude's windows (300 minutes is five_hour, a week seven_day).
fn codex_windows(text: &str) -> Option<Map<String, Value>> {
    text.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        let rl = v.pointer("/payload/rate_limits").or(v.get("rate_limits"))?;
        let w: Map<String, Value> = ["primary", "secondary"].into_iter().filter_map(|k| {
            let win = rl.get(k)?;
            let name = match win["window_minutes"].as_u64()? { 300 => "five_hour".to_string(), 10080 => "seven_day".to_string(), _ => k.to_string() };
            Some((name, json!({ "used_percentage": win["used_percent"].as_f64()?, "resets_at": win["resets_at"].clone() })))
        }).collect();
        (!w.is_empty()).then_some(w)
    })
}

pub(super) fn write(path: &Path, data: &Value) -> std::io::Result<()> {
    if let Some(d) = path.parent() { std::fs::create_dir_all(d)?; }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, data.to_string())?;
    std::fs::rename(&tmp, path)
}

/// Stores one payload under `agents` (one file per session, so parallel hooks never race).
fn record(agents: &Path, agent: &str, p: &Value) -> std::io::Result<()> {
    let d = agents.join(safe(agent));
    let text = (agent == "codex").then(|| codex_text(p)).flatten();
    if let Some(w) = usage_of(p).or_else(|| text.as_deref().and_then(codex_windows)) {
        let model = p["model"]["display_name"].as_str().unwrap_or("");
        write(&d.join("usage.json"), &json!({ "windows": w, "model": model, "at": now_s() }))?;
    }
    let id = first_str(p, SESSION_KEYS);
    let f = d.join(format!("{}.json", safe(id)));
    let old: Value = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let Some((state, mut message)) = state_of(p) else {
        // the status line carries no state, but it names the model and the spend the session has now
        let (m, sp) = (model_of(p), spend_of(p));
        if old.is_object() && (m.is_some() || sp.is_some()) {
            let mut rec = old;
            if let Some(m) = m { rec["model"] = m; }
            if let Some(s) = sp { rec["spend"] = s; }
            write(&f, &rec)?;
        }
        return Ok(());
    };
    // Claude Code's permission_prompt Notification arrives ~6 s after PermissionRequest with a generic text: keep the tool detail.
    let kept = old["message"].as_str().unwrap_or("");
    if old["state"] == "waiting" && p["notification_type"] == "permission_prompt" && !kept.is_empty() { message = kept.to_string(); }
    if state == "ended" { let _ = std::fs::remove_file(f); return Ok(()); }
    let mut rec = session_record(&old, p, state, &message, id, super::agent_restart::alive, super::agent_restart::capture);
    // Codex's tokens come from its rollout, not from the hook payload
    if let Some(s) = text.as_deref().and_then(codex_spend) { rec["spend"] = s; }
    // a permission prompt: the island offers Allow and Deny (permit.rs). Its permission_prompt Notification keeps the mark.
    let permission = p["hook_event_name"] == "PermissionRequest" || (p["notification_type"] == "permission_prompt" && old["ask"] == "permission");
    if state == "waiting" && permission {
        rec["ask"] = json!("permission");
        // the plan or questions come with the PermissionRequest; its permission_prompt Notification carries them on
        let choice = if p["hook_event_name"] == "PermissionRequest" { choice_of(p) } else { old["choice"].clone() };
        if !choice.is_null() { rec["choice"] = choice; }
    }
    write(&f, &rec)
}

/// One session's record: the new state, plus what earlier events learned (process, environment, model, folder, the turn).
/// `capture` runs only when the recorded process isn't the one running now, so most events skip it.
fn session_record(old: &Value, p: &Value, state: &str, message: &str, id: &str, alive: impl Fn(&Value) -> bool, capture: impl Fn() -> Option<Value>) -> Value {
    let cwd = p["cwd"].as_str().filter(|c| !c.is_empty()).or(old["cwd"].as_str()).unwrap_or("");
    let mut rec = json!({ "state": state, "message": message, "cwd": cwd, "at": now_s(), "id": id });
    if let Some(m) = model_of(p).or_else(|| Some(old["model"].clone()).filter(|m| !m.is_null())) { rec["model"] = m; }
    let proc = if old["pid"].is_u64() && alive(old) { Some(old.clone()) } else { capture() };
    if let Some(c) = proc {
        for k in ["pid", "start", "exe", "orca", "env"] { if !c[k].is_null() { rec[k] = c[k].clone(); } }
    }
    turn_fields(&mut rec, old, p, state);
    rec
}

/// `zorui.exe --agent <agent> [json] [--then cmd...]` and `--install-agents`; the exit code.
pub fn cli(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("--install") { return install(); }
    let Some(agent) = args.first() else { eprintln!("usage: zorui.exe --agent <agent> [--then <cmd>]"); return 2 };
    let then = args.iter().position(|a| a == "--then").map(|i| &args[i + 1..]).filter(|t| !t.is_empty());
    let mut raw = Vec::new();
    if !std::io::stdin().is_terminal() { let _ = std::io::stdin().read_to_end(&mut raw); }
    let mut text = String::from_utf8_lossy(&raw).trim_start_matches('\u{feff}').trim().to_string(); // some shells' pipes add a BOM
    if text.is_empty() { if let Some(l) = args.get(1..).and_then(|a| a.last()).filter(|l| l.starts_with('{')) { text = l.clone(); } } // Codex notify passes its JSON as an argument
    // never break the agent over a status update
    let agents = root().join("agents");
    if let Ok(p) = serde_json::from_str::<Value>(if text.is_empty() { "{}" } else { &text }) {
        // Cursor runs Claude Code's hooks too, and its payloads say so: file those under cursor, not claude
        let who = if agent == "claude" && !p["cursor_version"].is_null() { "cursor" } else { agent.as_str() };
        let asks = super::permit::PERMIT_AGENTS.contains(&who) && p["hook_event_name"] == "PermissionRequest";
        let id = first_str(&p, SESSION_KEYS).to_string();
        if asks { super::permit::clear(&id); } // a decision left from an earlier prompt must not answer this one
        let _ = record(&agents, who, &p);
        // Claude Code shows its own prompt too: whichever answers first wins, and no answer leaves that prompt alone
        if asks { if let Some(d) = super::permit::await_answer(&agents, who, &id) { println!("{}", super::permit::hook_reply(&d)); } }
    }
    if let Some(cmd) = then {
        use std::{os::windows::process::CommandExt, process::Stdio};
        // through cmd.exe, so npx, .cmd shims and && work; a bare spawn finds none of them on Windows
        if let Ok(mut c) = hidden("cmd").raw_arg(shell_args(cmd)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
            if let Some(mut i) = c.stdin.take() { let _ = i.write_all(&raw); }
            if let Ok(o) = c.wait_with_output() { let _ = std::io::stdout().write_all(&o.stdout); }
        }
    }
    0
}

/// The arguments for `cmd`: the user's status line as one cmd.exe line. Parts with spaces are quoted again, since the
/// shell that ran the status line already took its quotes off.
fn shell_args(parts: &[String]) -> String {
    let line: Vec<String> = parts.iter().map(|p| if p.contains(' ') && !p.starts_with('"') { format!("\"{p}\"") } else { p.clone() }).collect();
    format!("/S /C \"{}\"", line.join(" "))
}

// ------------------------------------------------------------------ install

fn is_ours(cmd: &str) -> bool { cmd.contains("end4_agent.py") || (cmd.contains("zorui") && cmd.contains(" --agent ")) }

/// `cmd`, keeping whatever status line `existing` already wrapped (" --then ...").
fn rewrap(cmd: &str, existing: &str) -> String {
    existing.find(" --then ").map_or(cmd.to_string(), |i| format!("{cmd}{}", &existing[i..]))
}

/// Adds one hook per event to a Claude-style hooks file and points ours at `cmd`; `status` also wraps the
/// status line (where Claude Code's usage comes from). Returns whether the file changed.
fn add_hooks(path: &Path, cmd: &str, events: &[&str], status: bool, timeout: u32) -> Result<bool, String> {
    let mut data: Value = match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(t.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => json!({}),
    };
    let mut changed = false;
    let hooks = data.as_object_mut().ok_or("settings is not an object")?.entry("hooks").or_insert_with(|| json!({}));
    for ev in events {
        // a permission hook waits for the island's answer (permit.rs), so it needs more time than the others
        let t = if *ev == "PermissionRequest" { PERMIT_TIMEOUT } else { timeout };
        let groups = hooks.as_object_mut().ok_or("hooks is not an object")?.entry(*ev).or_insert_with(|| json!([])).as_array_mut().ok_or("hook list is not an array")?;
        let mut found = false;
        for h in groups.iter_mut().filter_map(|g| g["hooks"].as_array_mut()).flatten() {
            let Some(old) = h["command"].as_str().filter(|c| is_ours(c)).map(String::from) else { continue };
            found = true;
            let new = rewrap(cmd, &old);
            if new != old { h["command"] = json!(new); changed = true; }
            if *ev == "PermissionRequest" && h["timeout"] != json!(t) { h["timeout"] = json!(t); changed = true; }
        }
        if !found {
            let mut h = json!({ "type": "command", "command": cmd, "timeout": t });
            if status { h["async"] = json!(true); }
            groups.push(json!({ "hooks": [h] }));
            changed = true;
        }
    }
    if status {
        let sl = data["statusLine"].clone();
        let old = sl["command"].as_str().unwrap_or("");
        let new = if is_ours(old) { rewrap(cmd, old) } else if old.is_empty() { cmd.to_string() } else { format!("{cmd} --then {old}") };
        if new != old {
            let mut s = if sl.is_object() { sl } else { json!({}) };
            s["type"] = json!("command");
            s["command"] = json!(new);
            data["statusLine"] = s;
            changed = true;
        }
    }
    if changed { save(path, &data)?; }
    Ok(changed)
}

/// Backs the file up, then writes `data` to it.
fn save(path: &Path, data: &Value) -> Result<(), String> {
    if path.is_file() {
        let bak = path.with_file_name(format!("{}.bak-{}", path.file_name().unwrap_or_default().to_string_lossy(), now_s() as u64));
        std::fs::copy(path, bak).map_err(|e| e.to_string())?;
    }
    if let Some(d) = path.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    std::fs::write(path, serde_json::to_string_pretty(data).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Adds one hook per event to a flat hooks file, Cursor's layout: {"version": 1, "hooks": {event: [{"command": ...}]}}.
/// Returns whether the file changed.
fn add_flat_hooks(path: &Path, cmd: &str, events: &[&str], timeout: u32) -> Result<bool, String> {
    let mut data: Value = match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(t.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => json!({}),
    };
    let mut changed = false;
    let obj = data.as_object_mut().ok_or("hooks file is not an object")?;
    if obj.get("version").is_none() { obj.insert("version".into(), json!(1)); changed = true; }
    let hooks = obj.entry("hooks").or_insert_with(|| json!({})).as_object_mut().ok_or("hooks is not an object")?;
    for ev in events {
        let list = hooks.entry(*ev).or_insert_with(|| json!([])).as_array_mut().ok_or("hook list is not an array")?;
        match list.iter_mut().find(|h| h["command"].as_str().is_some_and(is_ours)) {
            Some(h) if h["command"] != json!(cmd) => { h["command"] = json!(cmd); changed = true; }
            Some(_) => {}
            None => { list.push(json!({ "command": cmd, "timeout": timeout })); changed = true; }
        }
    }
    if changed { save(path, &data)?; }
    Ok(changed)
}

fn command(agent: &str) -> String {
    let exe: PathBuf = std::env::current_exe().unwrap_or_else(|_| root().join("zorui.exe"));
    format!("\"{}\" --agent {agent}", exe.to_string_lossy().replace('\\', "/"))
}

/// Config folders to look in for `k`: its own under `home`, plus CLAUDE_CONFIG_DIR for Claude Code.
fn dirs_of(k: &Known, home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(&k.dir)];
    // not in tests: they pass a temp home and must never touch the real Claude config
    if k.id == "claude" && !cfg!(test) { if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") { dirs.push(PathBuf::from(d)); } }
    // profile folders get zor-ui's hooks too, since each one is a Claude Code config folder
    if k.id == "claude" && !cfg!(test) { let r = Root::live(); for n in r.numbers() { dirs.push(r.profile_home(n)); } }
    dirs.dedup();
    dirs.into_iter().filter(|d| d.is_dir() && (k.marks.is_empty() || k.marks.iter().any(|m| d.join(m).is_file()))).collect()
}

/// Adds our hooks to every installed agent in `known(custom)`: (agent name, folder, result).
fn install_all(home: &Path, custom: &Value) -> Vec<(String, PathBuf, Result<bool, String>)> {
    known(custom).into_iter().flat_map(|k| dirs_of(&k, home).into_iter().map(|d| {
        let ev: Vec<&str> = k.events.iter().map(String::as_str).collect();
        let (file, cmd) = (d.join(&k.file), command(&k.id));
        let r = if k.flat { add_flat_hooks(&file, &cmd, &ev, 10) } else { add_hooks(&file, &cmd, &ev, k.status, if k.ms { 10_000 } else { 10 }) };
        (k.name.clone(), d, r)
    }).collect::<Vec<_>>()).collect()
}

fn home() -> PathBuf { PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()) }

fn install() -> i32 {
    for (name, d, r) in install_all(&home(), &crate::conf::Config::load().get("customAgents").clone()) {
        let say = match r { Ok(true) => "added".to_string(), Ok(false) => "already set up".into(), Err(e) => format!("failed ({e})") };
        println!("{name} {}: {say}", d.display());
    }
    0
}

/// Hooks up every agent found on this PC that isn't yet; the names of the ones newly added. Safe to call often:
/// it writes nothing when everything is already set up.
pub fn auto_install(custom: &Value) -> Vec<String> {
    install_all(&home(), custom).into_iter().filter(|(_, _, r)| matches!(r, Ok(true))).map(|(n, _, _)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zorui-agent-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn summary(dir: &Path) -> std::collections::HashMap<String, Value> {
        super::super::notify::agents_summary(dir, now_s() + 1.0).into_iter().map(|a| (a["id"].as_str().unwrap().to_string(), a)).collect()
    }

    #[test]
    fn hook_events_reach_the_island() {
        let d = tmp("events");
        let send = |a: &str, p: Value| record(&d, a, &p).unwrap();
        send("claude", json!({ "hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash" }));
        assert_eq!(summary(&d)["claude"]["state"], "working");
        send("claude", json!({ "hook_event_name": "PermissionRequest", "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": "npm test" } }));
        assert_eq!(summary(&d)["claude"]["message"], "Allow Bash: npm test");
        send("claude", json!({ "hook_event_name": "Notification", "session_id": "s2", "notification_type": "idle_prompt", "message": "waiting" }));
        assert_eq!(summary(&d)["claude"]["waiting"], 1);
        send("claude", json!({ "hook_event_name": "Stop", "session_id": "s1" }));
        assert_eq!(summary(&d)["claude"]["state"], "idle");
        send("my-bot", json!({ "session": "x", "state": "waiting", "message": "Approve deploy?" }));
        assert_eq!(summary(&d)["my-bot"]["label"], "My-Bot");
        send("codex", json!({ "type": "agent-turn-complete", "thread-id": "t" }));
        assert_eq!(summary(&d)["codex"]["state"], "idle");
        send("claude", json!({ "hook_event_name": "SessionEnd", "session_id": "s1" }));
        send("claude", json!({ "hook_event_name": "SessionEnd", "session_id": "s2" }));
        assert!(!summary(&d).contains_key("claude"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn permission_prompt_keeps_the_tool_detail() {
        let d = tmp("perm");
        let send = |p: Value| record(&d, "claude", &p).unwrap();
        send(json!({ "hook_event_name": "PermissionRequest", "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": "npm test" } }));
        send(json!({ "hook_event_name": "Notification", "session_id": "s1", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash" }));
        assert_eq!(summary(&d)["claude"]["message"], "Allow Bash: npm test");
        assert_eq!(summary(&d)["claude"]["state"], "waiting");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_plan_or_questions_reach_the_island_and_a_tool_does_not() {
        let d = tmp("choice");
        let send = |p: Value| record(&d, "claude", &p).unwrap();
        send(json!({ "hook_event_name": "PermissionRequest", "session_id": "s1", "tool_name": "ExitPlanMode", "tool_input": { "plan": "1. Do it" } }));
        assert_eq!(session(&d, "claude", "s1")["choice"], json!({ "kind": "plan", "plan": "1. Do it" }));
        assert_eq!(session(&d, "claude", "s1")["message"], "Plan ready for review");
        send(json!({ "hook_event_name": "Notification", "session_id": "s1", "notification_type": "permission_prompt", "message": "Claude needs your permission" }));
        assert_eq!(session(&d, "claude", "s1")["choice"]["kind"], "plan", "the follow-up Notification keeps the plan");
        send(json!({ "hook_event_name": "PermissionRequest", "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": "ls" } }));
        assert!(session(&d, "claude", "s1")["choice"].is_null(), "a tool permission has no choice");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_status_line_is_wrapped_for_cmd() {
        let parts = ["npx", "-y", "my tool", "&&", "echo"].map(String::from);
        assert_eq!(shell_args(&parts), r#"/S /C "npx -y "my tool" && echo""#);
        assert_eq!(shell_args(&["npx".to_string(), "line".to_string()]), r#"/S /C "npx line""#);
    }

    #[test]
    fn status_line_usage_drops_past_resets() {
        let d = tmp("usage");
        let later = now_s() + 3600.0;
        record(&d, "claude", &json!({ "model": { "display_name": "Opus" }, "rate_limits": {
            "five_hour": { "used_percentage": 23.5, "resets_at": later }, "seven_day": { "used_percentage": 41, "resets_at": now_s() - 5.0 } } })).unwrap();
        let a = &summary(&d)["claude"];
        assert_eq!(a["model"], "Opus");
        assert_eq!(a["usage"].as_object().unwrap().keys().collect::<Vec<_>>(), ["five_hour"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn codex_rate_limits_come_from_the_rollout() {
        let text = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":4.0,"window_minutes":300,"resets_at":2000000000},"secondary":{"used_percent":1.0,"window_minutes":10080,"resets_at":2000600000}}}}"#;
        let w = codex_windows(&format!("{text}\n{{\"type\":\"other\"}}\n")).unwrap();
        assert_eq!(w["five_hour"]["used_percentage"], 4.0);
        assert_eq!(w["seven_day"]["resets_at"], 2000600000);
        assert!(codex_windows(r#"{"payload":{"rate_limits":null}}"#).is_none());
    }

    #[test]
    fn codex_hook_reads_its_rollout_into_usage() {
        let d = tmp("codex-usage");
        let roll = d.join("rollout-x.jsonl");
        std::fs::write(&roll, r#"{"payload":{"rate_limits":{"primary":{"used_percent":7.0,"window_minutes":300,"resets_at":4102444800}}}}"#).unwrap();
        record(&d, "codex", &json!({ "session_id": "s1", "hook_event_name": "Stop", "transcript_path": roll })).unwrap();
        let a = &summary(&d)["codex"];
        assert_eq!(a["usage"]["five_hour"]["used_percentage"], 7.0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn text_is_cut_and_ask_text_has_fallbacks() {
        assert_eq!(short("a   b\n c", 90), "a b c");
        assert_eq!(short("abcdef", 4), "abc…");
        // a permission keeps the whole command up to ASK_TEXT_MAX, so the end of it can be read before allowing
        let cmd = format!("{}{}", "npm test ".repeat(9), "--watch=false");
        let asked = ask_text(&json!({ "tool_name": "Bash", "tool_input": { "command": cmd.clone() } }));
        assert_eq!(asked, format!("Allow Bash: {cmd}"));
        assert!(asked.ends_with("--watch=false"), "the end of the command is shown");
        let long = "x".repeat(ASK_TEXT_MAX + 50);
        let cut = ask_text(&json!({ "tool_name": "Bash", "tool_input": { "command": long } }));
        assert_eq!(cut.chars().count(), ASK_TEXT_MAX, "longer than the limit is cut to it, with the ellipsis inside the limit");
        assert!(cut.ends_with('…'));
        assert_eq!(ask_text(&json!({})), "Needs your input");
        assert_eq!(ask_text(&json!({ "tool_name": "Read" })), "Allow Read?");
        assert_eq!(ask_text(&json!({ "tool_name": "AskUserQuestion", "tool_input": { "questions": [{ "question": "Which one?" }] } })), "Which one?");
        assert_eq!(safe("a/b c"), "a_b_c");
        assert_eq!(safe(""), "default");
    }

    #[test]
    fn install_adds_migrates_and_wraps() {
        let d = tmp("install");
        let f = d.join("settings.json");
        std::fs::write(&f, json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "\"python\" -I -S \"x/end4_agent.py\" claude" }] }] },
            "statusLine": { "type": "command", "command": "npx line" } }).to_string()).unwrap();
        let cmd = "\"C:/z/zorui.exe\" --agent claude";
        assert!(add_hooks(&f, cmd, CLAUDE_EVENTS, true, 10).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], cmd); // the old python hook was repointed, not duplicated
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["async"], true);
        assert_eq!(v["statusLine"]["command"], format!("{cmd} --then npx line"));
        assert!(!add_hooks(&f, cmd, CLAUDE_EVENTS, true, 10).unwrap()); // second run changes nothing
        assert!(std::fs::read_dir(&d).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".bak-")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_installed_agents_only() {
        let h = tmp("found");
        std::fs::create_dir_all(h.join(".codex")).unwrap();
        std::fs::create_dir_all(h.join(".gemini")).unwrap(); // no Gemini CLI file in it: not Gemini
        let found = |h: &Path| known(&json!([])).into_iter().filter(|k| !dirs_of(k, h).is_empty()).map(|k| k.id).collect::<Vec<_>>();
        assert!(found(&h).contains(&"codex".to_string()) && !found(&h).contains(&"gemini".to_string()));
        std::fs::write(h.join(".gemini").join("oauth_creds.json"), "{}").unwrap();
        assert!(found(&h).contains(&"gemini".to_string()));
        let f = h.join(".gemini").join("settings.json");
        assert!(add_hooks(&f, "zorui --agent gemini", GEMINI_EVENTS, false, 10_000).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["hooks"]["BeforeTool"][0]["hooks"][0]["timeout"], 10_000);
    }

    #[test]
    fn session_record_keeps_what_earlier_events_learned() {
        let old = json!({ "state": "working", "cwd": "C:/proj", "model": "opus", "pid": 7, "start": 9, "exe": "node.exe", "orca": false, "env": "sealed" });
        let caught = |_: &Value| true;
        let never = || -> Option<Value> { panic!("a live recorded process isn't captured again") };
        let r = session_record(&old, &json!({ "hook_event_name": "Stop" }), "done", "", "s1", caught, never);
        assert_eq!((r["state"].clone(), r["cwd"].clone(), r["model"].clone(), r["pid"].clone(), r["env"].clone(), r["id"].clone()),
            (json!("done"), json!("C:/proj"), json!("opus"), json!(7), json!("sealed"), json!("s1")), "folder, model and process carry over");
        let fresh = || Some(json!({ "pid": 8, "start": 10, "exe": "claude.exe", "orca": true, "env": "new" }));
        let r = session_record(&old, &json!({ "cwd": "C:/other", "model": { "id": "sonnet" } }), "working", "", "s1", |_: &Value| false, fresh);
        assert_eq!((r["pid"].clone(), r["orca"].clone(), r["cwd"].clone(), r["model"].clone()), (json!(8), json!(true), json!("C:/other"), json!("sonnet")),
            "a process that's gone is captured again, and new event data wins");
        let r = session_record(&Value::Null, &json!({}), "working", "", "s2", |_: &Value| false, || None);
        assert!(r["pid"].is_null() && r["model"].is_null(), "nothing known, nothing made up");
    }

    #[test]
    fn status_line_updates_the_model_of_a_known_session_only() {
        let d = tmp("model");
        record(&d, "claude", &json!({ "model": { "id": "opus" }, "session_id": "s1" })).unwrap();
        assert!(!d.join("claude").join("s1.json").exists(), "no session file is made from a status line alone");
        record(&d, "claude", &json!({ "hook_event_name": "Stop", "session_id": "s1" })).unwrap();
        record(&d, "claude", &json!({ "model": { "id": "opus" }, "session_id": "s1" })).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("claude").join("s1.json")).unwrap()).unwrap();
        assert_eq!((v["model"].clone(), v["state"].clone()), (json!("opus"), json!("done")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resume_commands_for_built_in_and_custom_agents() {
        let cfg = json!([{ "name": "Bot", "dir": ".bot", "file": "h.json", "resume": "bot --continue {id}", "modelArg": "--llm {model}" }, { "name": "Plain", "dir": ".p", "file": "h.json" }]);
        assert_eq!(resume_of("claude", &cfg), Some(("claude --resume {id}".into(), "--model {model}".into())));
        assert_eq!(resume_of("codex", &cfg).unwrap().0, "codex resume {id}");
        assert_eq!(resume_of("bot", &cfg), Some(("bot --continue {id}".into(), "--llm {model}".into())));
        assert_eq!(resume_of("plain", &cfg), None, "a custom agent without a resume command can't be restarted");
    }

    #[test]
    fn custom_agents_get_hooks_but_cannot_replace_built_ins() {
        let h = tmp("custom");
        std::fs::create_dir_all(h.join(".myagent")).unwrap();
        let cfg = json!([{ "name": "My Agent", "dir": ".myagent", "file": "hooks.json" }, { "name": "Claude", "dir": ".x", "file": "a.json" }, { "name": "", "dir": ".y", "file": "b" }]);
        assert_eq!(known(&cfg).len(), 9); // 8 built in + My Agent; "Claude" would take the built-in id and the empty name has no id
        let r = install_all(&h, &cfg);
        assert!(r.iter().any(|(n, _, r)| n == "My Agent" && matches!(r, Ok(true))));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(h.join(".myagent").join("hooks.json")).unwrap()).unwrap();
        assert!(v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().ends_with("--agent my_agent"));
        assert!(install_all(&h, &cfg).iter().all(|(_, _, r)| matches!(r, Ok(false)))); // a rescan changes nothing
    }

    fn session(d: &Path, agent: &str, id: &str) -> Value {
        serde_json::from_str(&std::fs::read_to_string(d.join(agent).join(format!("{id}.json"))).unwrap()).unwrap()
    }

    #[test]
    fn turn_tracks_prompt_tool_and_start() {
        let d = tmp("turn");
        let send = |p: Value| record(&d, "claude", &p).unwrap();
        send(json!({ "hook_event_name": "UserPromptSubmit", "session_id": "s1", "prompt": "run the tests\n please", "cwd": "C:/p" }));
        let v = session(&d, "claude", "s1");
        assert_eq!((v["state"].clone(), v["prompt"].clone()), (json!("working"), json!("run the tests please")));
        let since = v["since"].as_f64().unwrap();
        assert!(since > 0.0);
        send(json!({ "hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash", "tool_input": { "command": "npm test" } }));
        let v = session(&d, "claude", "s1");
        assert_eq!(v["tool"], "Bash: npm test");
        assert_eq!(v["since"].as_f64().unwrap(), since, "a tool call doesn't restart the turn");
        send(json!({ "hook_event_name": "PostToolUse", "session_id": "s1" }));
        assert_eq!(session(&d, "claude", "s1")["tool"], "");
        send(json!({ "hook_event_name": "Stop", "session_id": "s1" }));
        let v = session(&d, "claude", "s1");
        assert!(v["since"].is_null() && v["tool"] == "" && v["subs"] == json!({}), "a finished turn clears what it was doing");
        assert_eq!(v["prompt"], "run the tests please", "the last prompt stays for the idle row");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn subagents_hang_under_their_session() {
        let d = tmp("subs");
        let send = |p: Value| record(&d, "claude", &p).unwrap();
        send(json!({ "hook_event_name": "UserPromptSubmit", "session_id": "s1", "prompt": "review" }));
        send(json!({ "hook_event_name": "SubagentStart", "session_id": "s1", "agent_id": "a1", "agent_type": "Explore" }));
        send(json!({ "hook_event_name": "SubagentStart", "session_id": "s1", "agent_id": "a2", "agent_type": "Plan" }));
        send(json!({ "hook_event_name": "PreToolUse", "session_id": "s1", "agent_id": "a1", "tool_name": "Grep", "tool_input": { "path": "src" } }));
        let v = session(&d, "claude", "s1");
        assert_eq!(v["subs"]["a1"]["tool"], "Grep: src");
        assert_eq!(v["subs"]["a1"]["type"], "Explore");
        assert_eq!(v["tool"], "", "a subagent's tool is not the session's");
        send(json!({ "hook_event_name": "SubagentStop", "session_id": "s1", "agent_id": "a1" }));
        let v = session(&d, "claude", "s1");
        assert_eq!(v["subs"].as_object().unwrap().keys().collect::<Vec<_>>(), ["a2"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cursor_hooks_use_its_flat_layout() {
        let h = tmp("flat");
        let f = h.join("hooks.json");
        std::fs::write(&f, r#"{"version":1,"hooks":{"beforeShellExecution":[{"command":"./keep.sh"}]}}"#).unwrap();
        let cmd = "\"C:/z/zorui.exe\" --agent cursor";
        assert!(add_flat_hooks(&f, cmd, CURSOR_EVENTS, 10).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["hooks"]["beforeShellExecution"][0]["command"], "./keep.sh", "the user's own hook stays");
        assert_eq!(v["hooks"]["beforeShellExecution"][1]["command"], cmd);
        assert_eq!(v["hooks"]["stop"][0]["command"], cmd);
        assert!(!add_flat_hooks(&f, cmd, CURSOR_EVENTS, 10).unwrap(), "a second run changes nothing");
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn spend_comes_from_the_status_line_and_codex_rollouts() {
        let d = tmp("spend");
        record(&d, "claude", &json!({ "hook_event_name": "Stop", "session_id": "s1" })).unwrap();
        record(&d, "claude", &json!({ "session_id": "s1", "context_window": { "used_percentage": 62, "total_input_tokens": 180000, "total_output_tokens": 4000 }, "cost": { "total_cost_usd": 1.2 } })).unwrap();
        assert_eq!(session(&d, "claude", "s1")["spend"], json!({ "ctx": 62.0, "tok": 184000, "cost": 1.2 }));
        record(&d, "claude", &json!({ "session_id": "nobody", "cost": { "total_cost_usd": 9.0 } })).unwrap();
        assert!(!d.join("claude").join("nobody.json").exists(), "a status line alone makes no session");
        let roll = d.join("rollout-x.jsonl");
        std::fs::write(&roll, r#"{"payload":{"info":{"total_token_usage":{"total_tokens":5000}}}}"#).unwrap();
        record(&d, "codex", &json!({ "session_id": "c1", "hook_event_name": "Stop", "transcript_path": roll })).unwrap();
        assert_eq!(session(&d, "codex", "c1")["spend"], json!({ "tok": 5000 }));
        let _ = std::fs::remove_dir_all(&d);
    }
}
