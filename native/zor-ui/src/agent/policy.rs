//! The risk level of each tool, from the plan's permission table. The table is a `match` on `Call`, so a new tool doesn't
//! compile until it has a level. The existing confirmations (island prompts for power and take-over) still do the asking;
//! this module says which calls need one, so the rule lives in one place.

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
        Search(_) | WebSearch(_) | ReadUrl(_) | FindApp(_) | FindFile(_) | SeeScreen(_) => Risk::ReadOnly,
        Open(_) | Url(_) | Play { .. } | Media(_) | Volume(_) | Brightness(_) | Radio { .. } | Clip(_) | OpenFile(_)
        | Wallpaper(_) | Message { .. } | Remember(_) | Forget(_) => Risk::Reversible,
        // lock can be undone by signing in; every other power action ends the session
        Power(what) => if what.trim().eq_ignore_ascii_case("lock") { Risk::Reversible } else { Risk::Destructive },
        TakeOver(_) | Browser { .. } => Risk::Consequential,
    }
}

/// Whether a failed call may be tried once more. Only read-only calls: a retry of anything that acts (open, play, save,
/// power) could do it twice.
pub fn may_retry(call: &Call) -> bool { risk_of(call) == Risk::ReadOnly }

/// Whether a call of this level must be confirmed by the user first. The confirmations still live in `exec` for now;
/// this is what they will be checked against once they move here.
#[allow(dead_code)]
pub fn needs_approval(risk: Risk) -> bool { matches!(risk, Risk::Consequential | Risk::Destructive) }

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
}
