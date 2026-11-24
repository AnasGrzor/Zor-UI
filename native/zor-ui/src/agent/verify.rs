//! The check-it pass: when a task ends with a step nothing could confirm (a click in the browser, a link or file handed to
//! another app, a message, a code edit with no check after it), the buddy checks its own work before it answers. It gets the
//! request, its answer and its steps, may use only read-only tools (look at the screen, read a file, list a folder, run the
//! project's check, read the page), and ends with VERIFIED or PROBLEM. A task whose steps all read their result back costs
//! nothing extra.

use super::{policy, verifier::Check, Call};

/// Most tool steps and seconds the check may take.
pub const STEPS: usize = 4;
pub const SECS: u64 = 45;

/// Whether the check-it pass runs (`ai.verify`, on by default).
pub fn on(cfg: &crate::conf::Config) -> bool { cfg.get("ai.verify").as_bool().unwrap_or(true) }

/// Whether a finished request needs checking: it answered, and a step could not confirm itself (Unverified, NotConfirmed), or a
/// file was edited and no check passed after the last edit.
pub fn needed(r: &Result<String, String>, checks: &[Check]) -> bool {
    if r.is_err() { return false; }
    let last_pass = checks.iter().rposition(|c| *c == Check::Passed);
    checks.iter().enumerate().any(|(i, c)| match c {
        Check::Unverified | Check::NotConfirmed => true,
        Check::Edited => last_pass.is_none_or(|p| p < i),
        _ => false,
    })
}

/// The tools the check may use: lookups that change nothing (not a web search, which opens the browser), the project's check,
/// and reading the browser's page.
pub fn allowed(c: &Call) -> bool {
    match c {
        Call::Search(_) => false,
        Call::RunCheck(_) => true,
        Call::Browser { tool, .. } => matches!(tool.as_str(), "browser_snapshot" | "browser_find"),
        _ => policy::risk_of(c) == policy::Risk::ReadOnly,
    }
}

/// What a tool the check may not use gets back.
pub const ONLY_CHECKING: &str = "You are only checking now: use a read-only tool (see_screen, read_file, list_tree, find_file, search_code, git_status, run_check), or give your final line.";

/// The request the check starts from.
pub fn prompt(said: &str, answer: &str, did: &str) -> String {
    format!("Check your work before the user hears it. Their request: \"{said}\". Your answer: \"{answer}\". What your tools did: {did}. \
Some of these steps could not report whether they worked. Check that the request really worked, with read-only tools only: look at the screen \
for something on the screen, read the file for a file, run the project's check (run_check) after a code change. Keep \"more\" true while you check. \
Then give one short final line that starts with VERIFIED: and what you saw, or PROBLEM: and what is wrong. Never repeat the task itself.",
        said = crate::assist::short(said, 300), answer = crate::assist::short(answer, 600), did = if did.is_empty() { "nothing reported" } else { did })
}

/// What the check found.
#[derive(Debug, PartialEq)]
pub enum Found { Verified(String), Problem(String), Unclear }

/// Reads the check's final line: VERIFIED: or PROBLEM: (any case, quotes and marks around it allowed), with what follows.
pub fn read(text: &str) -> Found {
    let t = text.trim().trim_start_matches(['"', '*', '`', ' ']);
    let rest = |n: usize| t[n..].trim_start_matches([':', ' ', '-', '*']).trim().to_string();
    let up = t.to_uppercase();
    if up.starts_with("VERIFIED") { Found::Verified(rest("VERIFIED".len())) }
    else if up.starts_with("PROBLEM") { Found::Problem(rest("PROBLEM".len())) }
    else { Found::Unclear }
}

/// The checks after a VERIFIED: the steps that could not confirm themselves count as confirmed now.
pub fn confirm(checks: &mut [Check]) {
    for c in checks.iter_mut() { if matches!(c, Check::Unverified | Check::NotConfirmed) { *c = Check::Verified; } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unconfirmed_steps_and_unchecked_edits_need_a_check() {
        assert!(needed(&Ok("a".into()), &[Check::Verified, Check::Unverified]));
        assert!(needed(&Ok("a".into()), &[Check::NotConfirmed]));
        assert!(needed(&Ok("a".into()), &[Check::Edited]), "an edit with no check after it");
        assert!(!needed(&Ok("a".into()), &[Check::Edited, Check::Passed]), "the check after the edit passed");
        assert!(needed(&Ok("a".into()), &[Check::Passed, Check::Edited]), "edited again after the check");
        assert!(!needed(&Ok("a".into()), &[Check::Verified, Check::Failed]));
        assert!(!needed(&Ok("a".into()), &[]), "an answer with no tools");
        assert!(!needed(&Err("x".into()), &[Check::Unverified]), "a failed request is not checked");
    }

    #[test]
    fn the_check_may_only_look() {
        assert!(allowed(&Call::SeeScreen("is steam open".into())));
        assert!(allowed(&Call::ReadFile("a.txt".into())));
        assert!(allowed(&Call::RunCheck("cargo test".into())));
        assert!(allowed(&Call::Browser { tool: "browser_snapshot".into(), args: serde_json::Value::Null }));
        assert!(!allowed(&Call::Browser { tool: "browser_click".into(), args: serde_json::Value::Null }));
        assert!(!allowed(&Call::Search("x".into())), "a web search opens the browser");
        assert!(!allowed(&Call::Open("steam".into())));
        assert!(!allowed(&Call::DeleteFile("a.txt".into())));
    }

    #[test]
    fn the_final_line_is_read() {
        assert_eq!(read("VERIFIED: Steam is open on the screen."), Found::Verified("Steam is open on the screen.".into()));
        assert_eq!(read("**Verified** - the file has the new line"), Found::Verified("the file has the new line".into()));
        assert_eq!(read("PROBLEM: the page still shows the old title"), Found::Problem("the page still shows the old title".into()));
        assert_eq!(read("Steam is open."), Found::Unclear);
        assert_eq!(read(""), Found::Unclear);
    }

    #[test]
    fn a_verified_check_confirms_the_unconfirmed_steps_only() {
        let mut c = vec![Check::Unverified, Check::Failed, Check::NotConfirmed, Check::Edited];
        confirm(&mut c);
        assert_eq!(c, vec![Check::Verified, Check::Failed, Check::Verified, Check::Edited], "an edit still needs its check to pass");
    }
}
