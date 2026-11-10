//! How much of the Claude plan limit is used, from the endpoint Claude Code's own /usage reads.
//! The OAuth token comes from the credentials file of the account new terminals use (the active Claude
//! account); Claude Code refreshes it, so a stale token just makes `fetch` return None until the next
//! time Claude Code runs.

use crate::bridge::claude_profile::{active, Files, Root};
use serde_json::Value;
use std::{io::Write, os::windows::process::CommandExt, path::Path, process::{Command, Stdio}};

/// Percent used (0-100) of the 5-hour session window and of the 7-day window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Usage { pub session: u8, pub week: u8 }

impl Usage {
    /// The window closest to its limit.
    pub fn worst(&self) -> u8 { self.session.max(self.week) }
}

fn token() -> Option<String> {
    let root = Root::live();
    token_at(&Files::of(&root, active(&root)).creds)
}

/// The access token in the Claude Code credentials file at `path`.
fn token_at(path: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(v["claudeAiOauth"]["accessToken"].as_str()?.to_string())
}

/// The two percentages out of the endpoint's JSON.
fn parse(body: &str) -> Option<Usage> {
    let v: Value = serde_json::from_str(body).ok()?;
    let pct = |k: &str| v[k]["utilization"].as_f64().map(|p| p.clamp(0.0, 100.0).round() as u8);
    Some(Usage { session: pct("five_hour")?, week: pct("seven_day")? })
}

pub fn fetch() -> Option<Usage> { fetch_with(token()?) }

/// The same read for a given OAuth access token (Settings reads each saved Claude account this way).
pub fn fetch_with(token: String) -> Option<Usage> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // the header goes in through stdin so the token never shows in the process list
    let mut child = Command::new("curl.exe")
        .args(["-s", "-f", "--max-time", "20", "-H", "@-", "https://api.anthropic.com/api/oauth/usage"])
        .stdin(Stdio::piped()).stdout(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn().ok()?;
    child.stdin.take()?.write_all(format!("Authorization: Bearer {token}\nanthropic-beta: oauth-2025-04-20\n").as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    parse(&String::from_utf8_lossy(&out.stdout)).filter(|_| out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_comes_from_the_given_credentials_file() {
        let dir = std::env::temp_dir().join(format!("zor-ui-usage-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".credentials.json");
        std::fs::write(&file, r#"{"claudeAiOauth":{"accessToken":"tok-123"}}"#).unwrap();
        assert_eq!(token_at(&file), Some("tok-123".to_string()));
        assert_eq!(token_at(&dir.join("missing.json")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn reads_both_windows() {
        let u = parse(r#"{"five_hour":{"utilization":82.6,"resets_at":"x"},"seven_day":{"utilization":2.0}}"#).unwrap();
        assert_eq!(u, Usage { session: 83, week: 2 });
        assert_eq!(u.worst(), 83);
        assert_eq!(parse(r#"{"five_hour":null,"seven_day":null}"#), None);
        assert_eq!(parse("not json"), None);
    }
    /// Needs a logged-in Claude Code: cargo test live_usage -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_usage() { println!("{:?}", fetch()); assert!(fetch().is_some()); }
}
