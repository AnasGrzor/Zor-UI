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

pub fn kind_of(call: &Call) -> Kind {
    use Call::*;
    match call {
        Play { .. } => Kind::Play,
        Remember(_) | Forget(_) => Kind::Memory,
        WebSearch(_) | ReadUrl(_) | FindApp(_) | SeeScreen(_) | ListTree(_) | ReadFile(_) | SearchCode(_) | GitStatus => Kind::Lookup,
        FindFile(_) => Kind::FileSearch,
        Radio { .. } | Brightness(_) => Kind::ReadBack,
        // a page that loads or is read is evidence; a click or typing is an action that nothing reports back on
        Browser { tool, .. } if matches!(tool.as_str(), "browser_navigate" | "browser_snapshot" | "browser_find") => Kind::Lookup,
        _ => Kind::Action,
    }
}

/// The check for one tool result, from what the tool reported.
pub fn check(kind: Kind, result: &Result<String, String>) -> Check {
    let Ok(text) = result else { return Check::Failed };
    let t = text.trim();
    match kind {
        Kind::Play => if t.starts_with("Playing") { Check::Verified } else { Check::Unverified },
        Kind::Memory => if t.starts_with("Remembered") || t.starts_with("Updated what I remember") || t.starts_with("Forgot:") { Check::Verified } else { Check::Unverified },
        Kind::Lookup => if !t.is_empty() && !t.starts_with("No ") { Check::Verified } else { Check::Failed },
        Kind::FileSearch => if t.starts_with("Found:") { Check::Verified } else { Check::Failed },
        // an Ok from these tools is only returned after the state was read back (radio: `exec`; brightness: `brightness_reply`)
        Kind::ReadBack => Check::Verified,
        Kind::Action => Check::Unverified,
    }
}

/// The verdict for the request from its tools' checks, in the order the tools ran.
///
/// A failed attempt that a later attempt recovers from is not counted: only the last outcome decides whether the request
/// failed. A declined tool is never a failure, but a request where something was declined is only partial at best.
pub fn judge(checks: &[Check]) -> Verdict {
    let declined = checks.iter().any(|c| *c == Check::Declined);
    let ran: Vec<Check> = checks.iter().copied().filter(|c| *c != Check::Declined).collect();
    if ran.is_empty() { return if declined { Verdict::Declined } else { Verdict::Completed }; }
    if declined { return Verdict::Partial; }
    match ran.last() {
        Some(Check::Failed) => if ran.iter().any(|c| *c != Check::Failed) { Verdict::Partial } else { Verdict::Failed },
        _ => if ran.iter().filter(|c| **c != Check::Failed).all(|c| *c == Check::Verified) { Verdict::Completed } else { Verdict::Unverified },
    }
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
        assert_eq!(kind_of(&Call::Volume("up".into())), Kind::Action);
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
}
