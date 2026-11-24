//! The verifier: checks whether what the tools reported is evidence that the request worked. A tool that returns Ok has not
//! proven anything by itself. Each result gets a check from what the tool reports, the checks give one verdict for the
//! request, and a partial or failed verdict adds a short note to the answer so the user hears what couldn't be confirmed.

use super::Call;

/// What kind of evidence a tool gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A track starts in the media session (`playsong` checks this before it says "Playing").
    Play,
    /// A fact is written to the memory database.
    Memory,
    /// A web search, page read, app list or screen answer returns text.
    Lookup,
    /// A file search lists the files it found.
    FileSearch,
    /// The tool reads the real state back after it changed it (radio, brightness), so its report is the evidence.
    ReadBack,
    /// Something happens on the device or in an app, and nothing reports back what it did (volume, opening an app).
    Action,
    /// A project check (`run_check`): its exit code is the evidence.
    Check,
    /// A file change (`edit_file`): not evidence of anything until a check passes after it.
    Edit,
    /// The output level or mute is read back from the device after a volume change (`agent::readback`).
    Volume,
    /// An app launch is checked against the running processes for a while (`agent::readback`).
    AppOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The tool's report shows the effect.
    Verified,
    /// The tool ran, but its report doesn't show the effect.
    Unverified,
    /// The tool failed, or found nothing.
    Failed,
    /// The user said no to the tool. Not a failure of the tool: nothing was done, by choice.
    Declined,
    /// A file was changed. The task is not Completed until a check passed after the last edit.
    Edited,
    /// A project check ran and exited 0.
    Passed,
    /// A project check ran and did not pass (exit code, or time limit). It stays failed for the whole request.
    CheckFailed,
    /// The tool ran, and its read-back could not confirm the effect (no read API, or the app never showed up). Judged as Unverified;
    /// the answer then says it could not be confirmed (`unverified_claim_note`).
    NotConfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No tools ran, or every tool was verified.
    Completed,
    /// Some tools ran and nothing could be checked.
    Unverified,
    /// Some worked and the last attempt failed, or something was declined and something else ran.
    Partial,
    /// Every tool failed.
    Failed,
    /// The user declined every tool the request needed.
    Declined,
}

/// The evidence kind of each call. No wildcard arm: a new `Call` variant does not compile until it is decided here, either as a kind
/// that reads the real outcome back or as `Kind::Action` with a reason in `unverified_reason`.
pub fn kind_of(call: &Call) -> Kind {
    use Call::*;
    match call {
        Play { .. } => Kind::Play,
        Remember(_) | Forget(_) => Kind::Memory,
        WebSearch(_) | ReadUrl(_) | FindApp(_) | SeeScreen(_) | ListTree(_) | ReadFile(_) | SearchCode(_) | GitStatus | Metrics(_) | SessionSearch(_) | SearchMemory(_) => Kind::Lookup,
        FindFile(_) => Kind::FileSearch,
        // brightness, radio, clipboard and wallpaper read the state back (`brightness_reply`, `readback::radio_reply`, `clip_reply`, `wallpaper_reply`)
        Radio { .. } | Brightness(_) | Clip(_) | Wallpaper(_) => Kind::ReadBack,
        Volume(_) => Kind::Volume,
        Open(_) => Kind::AppOpen,
        // a command's exit code is its evidence, as a check's is
        RunCheck(_) | RunCommand(_) => Kind::Check,
        // each reads its result back: the written size, the moved path, the deleted path gone (`pc.rs`)
        WriteFile { .. } | DeleteFile(_) | MoveFile { .. } => Kind::ReadBack,
        EditFile { .. } => Kind::Edit,
        // a page that loads or is read is evidence; the other browser tools report nothing back
        Browser { tool, .. } if matches!(tool.as_str(), "browser_navigate" | "browser_snapshot" | "browser_find") => Kind::Lookup,
        Browser { .. } | Search(_) | Url(_) | Media(_) | OpenFile(_) | Message { .. } | Power(_) | TakeOver(_) | StartAgent { .. } => Kind::Action,
    }
}

/// Why a `Kind::Action` call cannot be read back. None for every call that has evidence.
pub fn unverified_reason(call: &Call) -> Option<&'static str> {
    use Call::*;
    match call {
        Browser { .. } if kind_of(call) == Kind::Action => Some("a click, key press, typed text or dialog answer on a page has no read-back"),
        Search(_) | Url(_) => Some("the page is handed to the default browser; Windows reports only that the handoff worked"),
        Media(_) => Some("the player command is sent to the media session without waiting; there is no synchronous playback-state read"),
        OpenFile(_) => Some("the file is handed to its default app; Windows reports only that the handoff worked"),
        Message { .. } => Some("the chat opens in another app with the text ready; nothing on this PC reports whether it did"),
        Power(_) => Some("sleep, restart, shutdown and sign-out end this session, and lock has no cheap read of the lock state"),
        StartAgent { .. } => Some("the agent runs in its own window; its hooks report its progress to the island, not to this request"),
        TakeOver(_) => Some("the take-over runs on its own thread; its end state is saved on the task when it finishes (`tasks::take_over_ended`)"),
        _ => None,
    }
}

/// The check for one tool result, from what the tool reported.
pub fn check(kind: Kind, result: &Result<String, String>) -> Check {
    let text = match result {
        Ok(t) => t,
        // a check that ran and did not pass stays failed; one that never started (refused, not installed) is a plain failure
        Err(e) if kind == Kind::Check && e.starts_with("The check ") => return Check::CheckFailed,
        Err(_) => return Check::Failed,
    };
    let t = text.trim();
    match kind {
        Kind::Check => Check::Passed,
        Kind::Edit => Check::Edited,
        Kind::Play => if t.starts_with("Playing") { Check::Verified } else { Check::Unverified },
        // the saved or deleted row was checked in the database first (`readback::remember_reply`, `forget_reply`)
        Kind::Memory => if t.to_lowercase().contains("not confirmed") { Check::NotConfirmed } else if t.starts_with("Remembered") || t.starts_with("Updated what I remember") || t.starts_with("Forgot:") { Check::Verified } else { Check::Unverified },
        Kind::Lookup => if !t.is_empty() && !t.starts_with("No ") { Check::Verified } else { Check::Failed },
        Kind::FileSearch => if t.starts_with("Found:") { Check::Verified } else { Check::Failed },
        // an Ok from these tools is only returned after the state was read back; "not confirmed" is a read-back that could not answer
        Kind::ReadBack => if t.to_lowercase().contains("not confirmed") { Check::NotConfirmed } else { Check::Verified },
        Kind::Action => Check::Unverified,
        // the read-back texts say "not confirmed" when the device or the process list could not confirm the change
        Kind::Volume | Kind::AppOpen => if t.to_lowercase().contains("not confirmed") { Check::NotConfirmed } else { Check::Verified },
    }
}

/// The verdict for the request from its tools' checks, in the order the tools ran.
///
/// A failed attempt that a later attempt recovers from is not counted: only the last outcome decides whether the request
/// failed. A declined tool is never a failure, but a request where something was declined is only partial at best.
pub fn judge(checks: &[Check]) -> Verdict {
    let declined = checks.contains(&Check::Declined);
    // the latest check decides: a failure that a later passing check followed is dropped, so it no longer counts
    let last_pass = checks.iter().rposition(|c| *c == Check::Passed);
    let ran: Vec<Check> = checks.iter().enumerate()
        // an edit is covered by a check that passed after it; a failure also stops counting once a later check passed
        .filter(|(i, c)| **c != Check::Declined && !(matches!(**c, Check::CheckFailed | Check::Edited) && last_pass.is_some_and(|p| p > *i)))
        .map(|(_, c)| *c).collect();
    if ran.is_empty() { return if declined { Verdict::Declined } else { Verdict::Completed }; }
    if declined { return Verdict::Partial; }
    // a check that failed last is not undone by a later lookup, so the request is never Completed after one
    if ran.contains(&Check::CheckFailed) {
        return if ran.iter().any(|c| !matches!(c, Check::Failed | Check::CheckFailed)) { Verdict::Partial } else { Verdict::Failed };
    }
    match ran.last() {
        Some(Check::Failed) => if ran.iter().any(|c| *c != Check::Failed) { Verdict::Partial } else { Verdict::Failed },
        _ => if ran.iter().filter(|c| **c != Check::Failed).all(|c| matches!(c, Check::Verified | Check::Passed)) { Verdict::Completed } else { Verdict::Unverified },
    }
}

/// Whether an answer says the code works: tests pass, it builds, clippy is clean. A Completed answer that says so with no
/// project check run is Unverified (`agent::outcome`). Keywords only, so it can miss a claim, never invent one.
pub fn claims_code_result(answer: &str) -> bool {
    let a = answer.to_lowercase();
    ["clippy", "compiles", "compiled", "tests pass", "tests passed", "test passed", "builds", "build passed",
     "build succeeded", "no errors", "no warnings", "checks pass", "check passed"].iter().any(|k| a.contains(k))
}

/// The sentence to add to the answer for this verdict, or None when nothing needs saying. An unverified action (volume,
/// opening an app) is only logged: adding "I can't check" to every such answer would be noise, so only a partial or failed
/// request is said out loud. A declined request needs no note: the answer already says nothing was done.
pub fn note(verdict: Verdict) -> Option<&'static str> {
    match verdict {
        Verdict::Completed | Verdict::Unverified | Verdict::Declined => None,
        Verdict::Partial => Some("Only part of that was confirmed."),
        Verdict::Failed => Some("I couldn't confirm that it worked."),
    }
}

/// The sentence for a task that claims a code result with no passing check: its verdict is Unverified (`agent::outcome`).
pub fn unverified_claim_note(verdict: Verdict, answer: &Result<String, String>, checks: &[Check]) -> Option<&'static str> {
    if verdict != Verdict::Unverified { return None; }
    // a volume change or an app open that the read-back could not confirm
    if checks.contains(&Check::NotConfirmed) { return Some("I couldn't confirm that on this PC."); }
    // a file was changed and no check passed after the last change
    if let Some(e) = checks.iter().rposition(|c| *c == Check::Edited) {
        if !checks.iter().rposition(|c| *c == Check::Passed).is_some_and(|p| p > e) { return Some("I changed a file and no check passed after that, so this is unverified."); }
    }
    let claimed = answer.as_ref().is_ok_and(|a| claims_code_result(a));
    (claimed && !checks.contains(&Check::Passed)).then_some("I did not run a check, so this is unverified.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Result<String, String> { Ok(s.into()) }
    fn err() -> Result<String, String> { Err("no".into()) }

    #[test]
    fn each_tool_kind_is_checked_from_what_it_reports() {
        assert_eq!(check(Kind::Play, &ok("Playing lofi by X.")), Check::Verified);
        assert_eq!(check(Kind::Play, &ok("Opened YouTube for lofi. Press play if it doesn't start.")), Check::Unverified);
        assert_eq!(check(Kind::Memory, &ok("Remembered: My sister is called Lina")), Check::Verified);
        assert_eq!(check(Kind::Memory, &ok("Forgot: green tea")), Check::Verified);
        assert_eq!(check(Kind::Memory, &ok("I don't remember anything like that.")), Check::Unverified, "nothing was deleted");
        assert_eq!(check(Kind::Lookup, &ok("Web results: Oslo is 4 degrees")), Check::Verified);
        assert_eq!(check(Kind::Lookup, &ok("No results.")), Check::Failed);
        assert_eq!(check(Kind::FileSearch, &ok("Found: C:\\a\\cv.pdf.")), Check::Verified);
        assert_eq!(check(Kind::FileSearch, &ok("No file or folder matches \"cv\".")), Check::Failed);
        assert_eq!(check(Kind::Action, &ok("Done.")), Check::Unverified, "nothing reports what the device did");
    }

    /// One sample of every `Call` variant (and each browser tool). A variant added to `Call` fails to compile in `kind_of` until it is
    /// decided there; add it here too so the table below proves the decision.
    fn every_call() -> Vec<Call> {
        use Call::*;
        let mut v = vec![
            Search("x".into()), WebSearch("x".into()), ReadUrl("https://a.com".into()), FindApp("x".into()), FindFile("x".into()),
            SeeScreen("x".into()), ListTree("".into()), ReadFile("a".into()), SearchCode("x".into()), GitStatus, Metrics("".into()),
            SessionSearch("x".into()), SearchMemory("x".into()), Open("x".into()), Url("https://a.com".into()), Play { query: "x".into(), service: "".into() },
            Media("pause".into()), Volume("up".into()), Brightness("50".into()), Radio { which: "wifi".into(), state: "on".into() },
            Clip("x".into()), OpenFile("a".into()), Wallpaper("random".into()), Message { to: "a".into(), text: "b".into(), via: "".into() },
            Remember("x".into()), Forget("x".into()), Power("lock".into()), Power("sleep".into()), TakeOver("x".into()), RunCheck("cargo test".into()),
            EditFile { path: "a.rs".into(), old: "a".into(), new: "b".into() },
        ];
        v.extend(crate::browse::TOOLS.iter().map(|(t, _)| Browser { tool: t.to_string(), args: serde_json::json!({}) }));
        v
    }

    #[test]
    fn no_side_effecting_call_completes_from_its_own_ok_message() {
        use crate::agent::{outcome, policy::{risk_of, Risk}, state::TaskStatus};
        for c in every_call() {
            let kind = kind_of(&c);
            // a call is either unverified with a reason in code, or has a kind that reads evidence
            assert_eq!(kind == Kind::Action, unverified_reason(&c).is_some(), "{c:?}: Action and its reason go together");
            if risk_of(&c) == Risk::ReadOnly && kind != Kind::Action { continue; }
            let ok: Result<String, String> = Ok("Done.".into());
            let nc: Result<String, String> = Ok("Done. Not confirmed: no read.".into());
            match kind {
                // no read-back: any Ok, however it is worded, ends Unverified
                Kind::Action => {
                    assert_eq!(check(kind, &ok), Check::Unverified, "{c:?}");
                    assert_eq!(outcome(&ok, &[check(kind, &ok)]).1, TaskStatus::Unverified, "{c:?} must not complete from its own Ok");
                }
                // read-back kinds: the read-back's own "not confirmed" is never Verified, and a failure is never Completed
                Kind::ReadBack | Kind::Volume | Kind::AppOpen | Kind::Memory => {
                    assert_eq!(check(kind, &nc), Check::NotConfirmed, "{c:?}");
                    assert_eq!(outcome(&nc, &[check(kind, &nc)]).1, TaskStatus::Unverified, "{c:?}");
                    let bad: Result<String, String> = Err("no".into());
                    assert_ne!(outcome(&bad, &[check(kind, &bad)]).1, TaskStatus::Completed, "{c:?}");
                }
                // a play that does not say "Playing" is unverified; a check needs its own exit code
                Kind::Play => assert_eq!(check(kind, &ok), Check::Unverified, "{c:?}"),
                Kind::Check => assert_eq!(check(kind, &Err("The check failed: x".into())), Check::CheckFailed, "{c:?}"),
                // a change is never Completed from its own Ok: only a check that passed after it completes the task
                Kind::Edit => assert_eq!(outcome(&ok, &[check(kind, &ok)]).1, TaskStatus::Unverified, "{c:?}"),
                Kind::Lookup | Kind::FileSearch => {}
            }
        }
    }

    #[test]
    fn a_read_back_or_a_lookup_that_came_back_is_its_own_evidence() {
        assert_eq!(check(Kind::ReadBack, &ok("Wi-Fi is on.")), Check::Verified);
        assert_eq!(check(Kind::ReadBack, &err()), Check::Failed, "a radio or brightness that failed is not verified");
        assert_eq!(check(Kind::Lookup, &ok("Installed: Spotify.")), Check::Verified);
        assert_eq!(check(Kind::Lookup, &ok("No installed app or game matches \"x\".")), Check::Failed, "an empty app list is a failure, not a success");
        assert_eq!(check(Kind::Lookup, &ok("The screen shows a bright window.")), Check::Verified, "a screen answer is a lookup");
    }

    #[test]
    fn an_error_is_always_a_failed_check() {
        assert_eq!(check(Kind::Action, &err()), Check::Failed);
        assert_eq!(check(Kind::Play, &err()), Check::Failed);
    }

    #[test]
    fn the_request_verdict_follows_its_checks() {
        assert_eq!(judge(&[]), Verdict::Completed, "a plain answer needs no check");
        assert_eq!(judge(&[Check::Verified, Check::Verified]), Verdict::Completed);
        assert_eq!(judge(&[Check::Verified, Check::Unverified]), Verdict::Unverified);
        assert_eq!(judge(&[Check::Unverified]), Verdict::Unverified);
        assert_eq!(judge(&[Check::Verified, Check::Failed]), Verdict::Partial, "the last attempt failed after one worked");
        assert_eq!(judge(&[Check::Unverified, Check::Failed]), Verdict::Partial);
        assert_eq!(judge(&[Check::Failed, Check::Failed]), Verdict::Failed);
        assert_eq!(judge(&[Check::Failed]), Verdict::Failed);
    }

    #[test]
    fn a_failure_that_a_later_step_recovers_from_is_not_counted() {
        assert_eq!(judge(&[Check::Failed, Check::Unverified]), Verdict::Unverified, "open Spotify failed, the browser play worked");
        assert_eq!(judge(&[Check::Failed, Check::Verified]), Verdict::Completed);
        assert_eq!(judge(&[Check::Verified, Check::Failed, Check::Verified]), Verdict::Completed);
    }

    #[test]
    fn a_declined_tool_is_not_a_failure() {
        assert_eq!(judge(&[Check::Declined]), Verdict::Declined, "the user said no to the only tool");
        assert_eq!(judge(&[Check::Declined, Check::Declined]), Verdict::Declined);
        assert_eq!(judge(&[Check::Declined, Check::Verified]), Verdict::Partial, "something was declined and something else ran");
        assert_eq!(judge(&[Check::Declined, Check::Failed]), Verdict::Partial);
    }

    #[test]
    fn only_a_partial_or_failed_request_is_said_out_loud() {
        assert_eq!(note(Verdict::Completed), None);
        assert_eq!(note(Verdict::Unverified), None, "a volume change is not followed by a complaint about checking it");
        assert_eq!(note(Verdict::Declined), None, "the answer already says nothing was done");
        assert!(note(Verdict::Partial).is_some() && note(Verdict::Failed).is_some());
    }

    #[test]
    fn every_kind_of_call_has_a_kind() {
        assert_eq!(kind_of(&Call::Play { query: "lofi".into(), service: "spotify".into() }), Kind::Play);
        assert_eq!(kind_of(&Call::Remember("x".into())), Kind::Memory);
        assert_eq!(kind_of(&Call::WebSearch("x".into())), Kind::Lookup);
        assert_eq!(kind_of(&Call::FindApp("x".into())), Kind::Lookup);
        assert_eq!(kind_of(&Call::SeeScreen("x".into())), Kind::Lookup);
        assert_eq!(kind_of(&Call::FindFile("x".into())), Kind::FileSearch);
        assert_eq!(kind_of(&Call::Radio { which: "wifi".into(), state: "on".into() }), Kind::ReadBack);
        assert_eq!(kind_of(&Call::Brightness("50".into())), Kind::ReadBack);
        assert_eq!(kind_of(&Call::Volume("up".into())), Kind::Volume);
        assert_eq!(kind_of(&Call::Open("notepad".into())), Kind::AppOpen);
        let browser = |tool: &str| Call::Browser { tool: tool.into(), args: serde_json::json!({}) };
        assert_eq!(kind_of(&browser("browser_navigate")), Kind::Lookup, "a page that loads is evidence");
        assert_eq!(kind_of(&browser("browser_snapshot")), Kind::Lookup);
        assert_eq!(kind_of(&browser("browser_click")), Kind::Action, "a click reports nothing back");
        assert_eq!(kind_of(&browser("browser_type")), Kind::Action);
    }

    #[test]
    fn a_browser_read_that_came_back_verifies_and_a_click_does_not() {
        let nav = Call::Browser { tool: "browser_navigate".into(), args: serde_json::json!({}) };
        assert_eq!(check(kind_of(&nav), &ok("Browser (page text): Page Title: Example Domain")), Check::Verified);
        let click = Call::Browser { tool: "browser_click".into(), args: serde_json::json!({}) };
        assert_eq!(check(kind_of(&click), &ok("Clicked.")), Check::Unverified);
        assert_eq!(judge(&[Check::Verified, Check::Unverified]), Verdict::Unverified, "a click after a loaded page is still unverified");
    }

    #[test]
    fn a_passed_check_is_evidence_and_a_failed_one_is_not_completed() {
        let run = |s: &str| check(Kind::Check, &ok(s));
        assert_eq!(kind_of(&Call::RunCheck("cargo test".into())), Kind::Check);
        assert_eq!(run("The check passed: \"cargo test\" exited with code 0."), Check::Passed, "exit 0 is evidence");
        assert_eq!(judge(&[Check::Passed]), Verdict::Completed);
        assert_eq!(check(Kind::Check, &Err("The check failed: \"cargo test\" exited with code 101.".into())), Check::CheckFailed);
        assert_eq!(check(Kind::Check, &Err("The check ran out of time: \"cargo test\" was stopped.".into())), Check::CheckFailed);
        assert_eq!(check(Kind::Check, &Err("cargo isn't installed, so \"cargo test\" did not run.".into())), Check::Failed, "a check that never started is a plain failure");
        assert_eq!(judge(&[Check::CheckFailed]), Verdict::Failed);
        assert_eq!(judge(&[Check::Passed, Check::CheckFailed]), Verdict::Partial, "a failed check is never Completed");
        assert_eq!(judge(&[Check::CheckFailed, Check::Verified]), Verdict::Partial, "a later lookup does not clear it");
        assert_eq!(judge(&[Check::CheckFailed, Check::Declined]), Verdict::Partial);
    }

    #[test]
    fn the_latest_check_decides_for_the_workspace() {
        assert_eq!(judge(&[Check::CheckFailed, Check::Passed]), Verdict::Completed, "failed then passed: the later pass counts");
        assert_eq!(judge(&[Check::CheckFailed, Check::Passed, Check::Verified]), Verdict::Completed);
        assert_eq!(judge(&[Check::Passed, Check::CheckFailed]), Verdict::Partial, "passed then failed: not Completed");
        assert_eq!(judge(&[Check::Passed, Check::CheckFailed, Check::Passed]), Verdict::Completed);
        assert_eq!(judge(&[Check::CheckFailed, Check::Passed, Check::CheckFailed]), Verdict::Partial);
    }

    #[test]
    fn an_edit_is_completed_only_by_a_check_that_passed_after_it() {
        let call = Call::EditFile { path: "a.rs".into(), old: "a".into(), new: "b".into() };
        assert_eq!(kind_of(&call), Kind::Edit);
        assert_eq!(check(Kind::Edit, &ok("Changed a.rs at line 1.")), Check::Edited);
        assert_eq!(check(Kind::Edit, &err()), Check::Failed, "a refused edit changed nothing");
        use Check::{CheckFailed as F, Edited as E, Passed as P};
        assert_eq!(judge(&[E, P]), Verdict::Completed, "edit then pass");
        assert_eq!(judge(&[E]), Verdict::Unverified, "edit with no check");
        assert_eq!(judge(&[P, E]), Verdict::Unverified, "pass then edit: the pass is stale");
        assert_ne!(judge(&[E, F]), Verdict::Completed, "edit then fail");
        assert_eq!(judge(&[E, F, P]), Verdict::Completed, "the latest check decides");
        assert_eq!(judge(&[E, P, E]), Verdict::Unverified);
        assert_eq!(judge(&[E, P, E, P]), Verdict::Completed);
        let said = Ok("Fixed it.".to_string());
        assert!(unverified_claim_note(Verdict::Unverified, &said, &[E]).unwrap().contains("unverified"), "the answer says so");
        assert!(unverified_claim_note(Verdict::Unverified, &said, &[P, E]).is_some());
        assert_eq!(unverified_claim_note(Verdict::Completed, &said, &[E, P]), None);
    }

    #[test]
    fn a_code_claim_needs_a_check_and_a_plain_answer_does_not() {
        assert!(claims_code_result("All tests pass now."));
        assert!(claims_code_result("cargo clippy is clean."));
        assert!(!claims_code_result("Playing lofi by X."));
        assert!(!claims_code_result("Wi-Fi is on."));
    }

    #[test]
    fn an_unverified_code_claim_says_so_in_the_answer() {
        let claim = Ok("All tests pass.".to_string());
        let note = unverified_claim_note(Verdict::Unverified, &claim, &[Check::Verified]);
        assert_eq!(note, Some("I did not run a check, so this is unverified."));
        assert_eq!(unverified_claim_note(Verdict::Unverified, &claim, &[Check::Passed, Check::Unverified]), None, "a check ran");
        assert_eq!(unverified_claim_note(Verdict::Completed, &claim, &[Check::Passed]), None);
        assert_eq!(unverified_claim_note(Verdict::Unverified, &Ok("Wi-Fi is on.".into()), &[Check::Unverified]), None, "no code claim");
    }
}
