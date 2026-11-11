//! The risk level of each tool, and the question the user must answer before a risky call runs. The table is a `match` on
//! `Call`, so a new tool doesn't compile until it has a level and a question. `agent::approve` asks every question here, in
//! one place, and a "no" is recorded by the verifier as Declined. A call that is not in the table can't be built, so
//! unknown tools fail closed (`agent::parse_step` refuses them before a `Call` exists).

use super::Call;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    /// Looks things up and changes nothing.
    ReadOnly,
    /// Changes something the user can undo (opens, volume, a saved fact).
    Reversible,
    /// Acts for the user on things outside the machine's settings (moves the mouse, drives a browser).
    Consequential,
    /// Can lose work or end the session (sleep, restart, shut down, sign out).
    Destructive,
}

pub fn risk_of(call: &Call) -> Risk {
    use Call::*;
    match call {
        Search(_) | WebSearch(_) | ReadUrl(_) | FindApp(_) | FindFile(_) | SeeScreen(_) | ListTree(_) | ReadFile(_) | SearchCode(_) | GitStatus => Risk::ReadOnly,
        Open(_) | Url(_) | Play { .. } | Media(_) | Volume(_) | Brightness(_) | Radio { .. } | Clip(_) | OpenFile(_)
        | Wallpaper(_) | Message { .. } | Remember(_) | Forget(_) => Risk::Reversible,
        // lock can be undone by signing in; every other power action ends the session
        Power(what) => if what.trim().eq_ignore_ascii_case("lock") { Risk::Reversible } else { Risk::Destructive },
        // a check builds and runs the project's code, so it asks once per workspace (agent::approve)
        TakeOver(_) | Browser { .. } | RunCheck(_) => Risk::Consequential,
    }
}

/// Whether a failed call may be tried once more. Only read-only calls: a retry of anything that acts (open, play, save,
/// power) could do it twice.
pub fn may_retry(call: &Call) -> bool { risk_of(call) == Risk::ReadOnly }

/// Whether a call of this level must be confirmed by the user first.
pub fn needs_approval(risk: Risk) -> bool { matches!(risk, Risk::Consequential | Risk::Destructive) }

/// The question the user answers in the island before this call runs, or None when it runs without asking. Power asks for
/// everything but lock, and an unknown power word asks nothing here because `power` refuses it before it does anything.
/// The browser asks once per request, and only when Settings doesn't skip the question (`agent::approve` does that part).
pub fn approval_for(call: &Call) -> Option<String> {
    match call {
        Call::Power(w) => match w.trim().to_lowercase().as_str() {
            "sleep" => Some("Put the PC to sleep?".into()),
            "restart" => Some("Restart the PC? Unsaved work is lost.".into()),
            "shutdown" => Some("Shut down the PC? Unsaved work is lost.".into()),
            "logout" => Some("Sign out? Unsaved work is lost.".into()),
            _ => None,
        },
        Call::TakeOver(goal) => Some(crate::drive::question(goal)),
        Call::Browser { .. } => Some("The buddy wants to use its browser for this. Allow it?".into()),
        Call::RunCheck(name) => Some(format!("Run \"{}\" in the developer workspace? It builds or tests the project's code.", name.trim())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reading_is_read_only_and_opening_or_saving_is_reversible() {
        assert_eq!(risk_of(&Call::WebSearch("weather".into())), Risk::ReadOnly);
        assert_eq!(risk_of(&Call::FindFile("cv".into())), Risk::ReadOnly);
        assert_eq!(risk_of(&Call::Open("Spotify".into())), Risk::Reversible);
        assert_eq!(risk_of(&Call::Volume("up".into())), Risk::Reversible);
        assert_eq!(risk_of(&Call::Remember("My sister is called Lina".into())), Risk::Reversible);
    }

    #[test]
    fn power_asks_for_everything_but_lock() {
        assert_eq!(risk_of(&Call::Power("lock".into())), Risk::Reversible);
        assert_eq!(risk_of(&Call::Power("  Lock ".into())), Risk::Reversible);
        for w in ["sleep", "restart", "shutdown", "logout"] {
            assert_eq!(risk_of(&Call::Power(w.into())), Risk::Destructive, "{w}");
        }
    }

    #[test]
    fn taking_over_the_mouse_and_browsing_are_consequential() {
        assert_eq!(risk_of(&Call::TakeOver("fill the form".into())), Risk::Consequential);
        assert_eq!(risk_of(&Call::RunCheck("cargo test".into())), Risk::Consequential, "a check asks once per workspace");
        assert_eq!(risk_of(&Call::Browser { tool: "browser_click".into(), args: json!({}) }), Risk::Consequential);
    }

    #[test]
    fn only_a_read_only_call_is_tried_again() {
        assert!(may_retry(&Call::WebSearch("weather".into())));
        assert!(may_retry(&Call::ReadUrl("https://example.com".into())));
        assert!(!may_retry(&Call::Open("Spotify".into())), "an open could happen twice");
        assert!(!may_retry(&Call::Play { query: "lofi".into(), service: String::new() }));
        assert!(!may_retry(&Call::Power("shutdown".into())));
    }

    #[test]
    fn only_the_risky_levels_need_approval() {
        assert!(!needs_approval(Risk::ReadOnly));
        assert!(!needs_approval(Risk::Reversible));
        assert!(needs_approval(Risk::Consequential));
        assert!(needs_approval(Risk::Destructive));
        assert!(needs_approval(risk_of(&Call::Power("shutdown".into()))));
        assert!(!needs_approval(risk_of(&Call::Power("lock".into()))));
    }

    #[test]
    fn every_risky_call_has_a_question_and_no_safe_call_does() {
        let calls = [
            Call::Power("sleep".into()), Call::Power("restart".into()), Call::Power("shutdown".into()), Call::Power("logout".into()),
            Call::Power("lock".into()), Call::TakeOver("fill the form".into()), Call::Browser { tool: "browser_click".into(), args: json!({}) },
            Call::Open("Spotify".into()), Call::Volume("up".into()), Call::WebSearch("x".into()), Call::FindFile("cv".into()),
            Call::RunCheck("cargo test".into()), Call::GitStatus,
        ];
        for c in &calls {
            assert_eq!(approval_for(c).is_some(), needs_approval(risk_of(c)), "{c:?}: the question and the level must agree");
        }
    }

    #[test]
    fn the_power_questions_name_what_is_lost() {
        assert!(approval_for(&Call::Power("shutdown".into())).unwrap().contains("Unsaved work is lost"));
        assert!(approval_for(&Call::Power("sleep".into())).is_some());
        assert_eq!(approval_for(&Call::Power("format c:".into())), None, "an unknown word asks nothing; `power` refuses it");
    }
}
