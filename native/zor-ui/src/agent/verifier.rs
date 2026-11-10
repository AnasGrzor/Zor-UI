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
    /// A web search or page read returns text.
    Lookup,
    /// A file search lists the files it found.
    FileSearch,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No tools ran, or every tool was verified.
    Completed,
    /// Some tools ran and nothing could be checked.
    Unverified,
    /// Some were verified or unverified, and some failed.
    Partial,
    /// Every tool failed.
    Failed,
}

pub fn kind_of(call: &Call) -> Kind {
    use Call::*;
    match call {
        Play { .. } => Kind::Play,
        Remember(_) | Forget(_) => Kind::Memory,
        WebSearch(_) | ReadUrl(_) => Kind::Lookup,
        FindFile(_) => Kind::FileSearch,
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
        Kind::Action => Check::Unverified,
    }
}

/// The verdict for the request from its tools' checks.
pub fn judge(checks: &[Check]) -> Verdict {
    let verified = checks.iter().any(|c| *c == Check::Verified);
    let unverified = checks.iter().any(|c| *c == Check::Unverified);
    let failed = checks.iter().any(|c| *c == Check::Failed);
    match (checks.is_empty(), failed, verified || unverified) {
        (true, _, _) => Verdict::Completed,
        (false, true, false) => Verdict::Failed,
        (false, true, true) => Verdict::Partial,
        (false, false, true) if unverified && !verified => Verdict::Unverified,
        (false, false, true) => if checks.iter().all(|c| *c == Check::Verified) { Verdict::Completed } else { Verdict::Unverified },
        _ => Verdict::Completed,
    }
}

/// The sentence to add to the answer for this verdict, or None when nothing needs saying. An unverified action (volume,
/// opening an app) is only logged: adding "I can't check" to every such answer would be noise, so only a partial or failed
/// request is said out loud.
pub fn note(verdict: Verdict) -> Option<&'static str> {
    match verdict {
        Verdict::Completed | Verdict::Unverified => None,
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
        assert_eq!(judge(&[Check::Verified, Check::Failed]), Verdict::Partial);
        assert_eq!(judge(&[Check::Unverified, Check::Failed]), Verdict::Partial);
        assert_eq!(judge(&[Check::Failed, Check::Failed]), Verdict::Failed);
    }

    #[test]
    fn only_a_partial_or_failed_request_is_said_out_loud() {
        assert_eq!(note(Verdict::Completed), None);
        assert_eq!(note(Verdict::Unverified), None, "a volume change is not followed by a complaint about checking it");
        assert!(note(Verdict::Partial).is_some() && note(Verdict::Failed).is_some());
    }

    #[test]
    fn every_kind_of_call_has_a_kind() {
        assert_eq!(kind_of(&Call::Play { query: "lofi".into(), service: "spotify".into() }), Kind::Play);
        assert_eq!(kind_of(&Call::Remember("x".into())), Kind::Memory);
        assert_eq!(kind_of(&Call::WebSearch("x".into())), Kind::Lookup);
        assert_eq!(kind_of(&Call::FindFile("x".into())), Kind::FileSearch);
        assert_eq!(kind_of(&Call::Volume("up".into())), Kind::Action);
    }
}
