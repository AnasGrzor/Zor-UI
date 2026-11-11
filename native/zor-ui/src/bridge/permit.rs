//! Allow or Deny for a Claude Code permission prompt, from the island or by voice, the way the buddy's questions work
//! (drive.rs). The PermissionRequest hook (agent_hook.rs) waits here for the answer and prints it for Claude Code.
//! Claude Code also shows its own prompt, so whichever answers first wins, and no answer leaves that prompt alone.

use super::agent_hook::{safe, write};
use super::common::{load_json, root};
use crate::drive::{spoken_answer, Answer};
use crate::hub;
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, time::{Duration, Instant}};

/// Agents whose PermissionRequest hook waits for Allow or Deny. Gemini CLI has no such event, and custom agents'
/// reply format is unknown, so they keep only their own prompt.
pub const PERMIT_AGENTS: &[&str] = &["claude", "codex"];
/// How long the hook waits for an answer. The hook's own timeout (PERMIT_TIMEOUT in agent_hook.rs) is a little longer.
const WAIT: Duration = Duration::from_secs(110);

fn permits() -> PathBuf { root().join("permits") }

/// The permission the island shows: (agent label, session, what the agent wants to do), from the agents list.
pub fn pending() -> Option<(String, String, String)> {
    hub::get_json("zor-ui.agents").as_array()?.iter().find_map(|a| {
        let p = &a["permit"];
        let session = p["session"].as_str().filter(|s| !s.is_empty())?;
        let msg = p["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("needs permission");
        let label = a["label"].as_str().filter(|l| !l.is_empty()).unwrap_or("Agent");
        Some((label.to_string(), session.to_string(), msg.to_string()))
    })
}

/// A permission is waiting for you.
pub fn waiting() -> bool { pending().is_some() }

/// Your answer for the waiting permission. Allow, or Deny with `message` (why, sent back to Claude).
pub fn decide(session: &str, allow: bool, message: &str) { decide_in(&permits(), session, allow, message) }

/// What you said, as an answer to the waiting permission: yes allows, no denies, anything else denies with your words.
/// False when nothing is waiting or nothing was heard.
pub fn answer_with(said: &str) -> bool {
    let (Some((_, session, _)), Some((allow, message))) = (pending(), decision_for(said)) else { return false };
    decide(&session, allow, &message);
    true
}

/// (allow, message) for what you said, or None for nothing heard.
pub fn decision_for(said: &str) -> Option<(bool, String)> {
    match spoken_answer(said)? {
        Answer::Yes => Some((true, String::new())),
        Answer::No => Some((false, String::new())),
        Answer::Said(t) => Some((false, t)),
    }
}

/// Drops a decision left from an earlier prompt of this session. Call before the session shows as waiting.
pub fn clear(session: &str) { clear_in(&permits(), session) }

/// Waits for your answer to this prompt of `agent`: the decision, or None when the session stops waiting (answered in
/// the agent's terminal, dismissed) or the time runs out.
pub fn await_answer(agents: &Path, agent: &str, session: &str) -> Option<Value> { await_in(agents, &permits(), agent, session, WAIT) }

/// What the PermissionRequest hook prints: the agent's decision.
pub fn hook_reply(d: &Value) -> Value {
    let decision = if d["behavior"] == "allow" { json!({ "behavior": "allow" }) } else {
        let msg = d["message"].as_str().filter(|m| !m.is_empty()).unwrap_or("The user denied this from zor-ui.");
        json!({ "behavior": "deny", "message": msg })
    };
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
}

fn decision_file(dir: &Path, session: &str) -> PathBuf { dir.join(format!("{}.json", safe(session))) }

fn session_file(agents: &Path, agent: &str, session: &str) -> PathBuf { agents.join(safe(agent)).join(format!("{}.json", safe(session))) }

fn decide_in(dir: &Path, session: &str, allow: bool, message: &str) {
    let _ = write(&decision_file(dir, session), &json!({ "behavior": if allow { "allow" } else { "deny" }, "message": message }));
}

fn clear_in(dir: &Path, session: &str) { let _ = std::fs::remove_file(decision_file(dir, session)); }

fn await_in(agents: &Path, dir: &Path, agent: &str, session: &str, wait: Duration) -> Option<Value> {
    let (start, df, sf) = (Instant::now(), decision_file(dir, session), session_file(agents, agent, session));
    loop {
        let d = load_json(&df, Value::Null);
        if d.is_object() {
            let _ = std::fs::remove_file(&df);
            let mut s = load_json(&sf, json!({}));
            if s.is_object() { s["state"] = json!("working"); let _ = write(&sf, &s); } // the row leaves "Needs you" at once
            return Some(d);
        }
        if load_json(&sf, json!({}))["state"] != "waiting" || start.elapsed() >= wait { return None; }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zorui-permit-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("claude")).unwrap();
        d
    }

    #[test]
    fn an_answer_reaches_the_waiting_hook_and_clears_the_row() {
        let d = tmp("answer");
        let (agents, permits) = (d.join("agents"), d.join("permits"));
        std::fs::create_dir_all(agents.join("claude")).unwrap();
        write(&session_file(&agents, "claude", "s1"), &json!({ "state": "waiting", "message": "Allow Bash: npm test" })).unwrap();
        decide_in(&permits, "s1", false, "use the test script");
        let got = await_in(&agents, &permits, "claude", "s1", Duration::from_secs(5)).unwrap();
        assert_eq!(hook_reply(&got)["hookSpecificOutput"]["decision"], json!({ "behavior": "deny", "message": "use the test script" }));
        assert_eq!(load_json(&session_file(&agents, "claude", "s1"), Value::Null)["state"], "working");
        assert!(!decision_file(&permits, "s1").exists(), "a decision is used once");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_hook_gives_up_when_the_prompt_is_answered_elsewhere() {
        let d = tmp("elsewhere");
        let (agents, permits) = (d.join("agents"), d.join("permits"));
        write(&session_file(&agents, "codex", "s2"), &json!({ "state": "working" })).unwrap();
        assert!(await_in(&agents, &permits, "codex", "s2", Duration::from_secs(5)).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn each_agent_waits_on_its_own_session_file() {
        let d = tmp("per-agent");
        let (agents, permits) = (d.join("agents"), d.join("permits"));
        write(&session_file(&agents, "codex", "s3"), &json!({ "state": "waiting" })).unwrap();
        assert!(await_in(&agents, &permits, "claude", "s3", Duration::from_millis(300)).is_none(), "claude has no session s3 waiting");
        decide_in(&permits, "s3", true, "");
        assert!(await_in(&agents, &permits, "codex", "s3", Duration::from_secs(5)).is_some(), "codex's file says waiting");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn allow_and_deny_replies_have_the_hook_shape() {
        assert_eq!(hook_reply(&json!({ "behavior": "allow" })), json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": { "behavior": "allow" } } }));
        assert_eq!(hook_reply(&json!({ "behavior": "deny" }))["hookSpecificOutput"]["decision"]["message"], "The user denied this from zor-ui.");
    }

    #[test]
    fn what_you_say_answers_like_the_buddy_question() {
        assert_eq!(decision_for("Yes."), Some((true, String::new())));
        assert_eq!(decision_for("no"), Some((false, String::new())));
        assert_eq!(decision_for("run it in the test folder"), Some((false, "run it in the test folder".into())));
        assert_eq!(decision_for(" ... "), None, "nothing heard");
    }
}
