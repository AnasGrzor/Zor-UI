//! The risk level of each tool, and the question the user must answer before a risky call runs. The table is a `match` on
//! `Call`, so a new tool doesn't compile until it has a level and a question. `agent::approve` asks every question here, in
//! one place, and a "no" is recorded by the verifier as Declined. A call that is not in the table can't be built, so
//! unknown tools fail closed (`agent::parse_step` refuses them before a `Call` exists).

use super::Call;
use crate::{browse, drive, webread};
use serde_json::Value;

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
        Search(_) | WebSearch(_) | ReadUrl(_) | FindApp(_) | FindFile(_) | SeeScreen(_) | ListTree(_) | ReadFile(_) | SearchCode(_) | GitStatus | Metrics(_) => Risk::ReadOnly,
        Open(_) | Url(_) | Play { .. } | Media(_) | Volume(_) | Brightness(_) | Radio { .. } | Clip(_) | OpenFile(_)
        | Wallpaper(_) | Message { .. } | Remember(_) | Forget(_) => Risk::Reversible,
        // lock can be undone by signing in; every other power action ends the session
        Power(what) => if what.trim().eq_ignore_ascii_case("lock") { Risk::Reversible } else { Risk::Destructive },
        // a check builds and runs the project's code, so it asks once per workspace (agent::approve)
        TakeOver(_) | RunCheck(_) => Risk::Consequential,
        // each browser tool has its own level (`BROWSER_RISK`); an unknown name is Consequential here and refused by `check_browser`
        Browser { tool, .. } => browser_risk(tool).unwrap_or(Risk::Consequential),
    }
}

/// The level of each browser tool, the same names as `browse::TOOLS` (a test checks both ways). A name not in this table is
/// refused by `check_browser` before anything runs.
pub const BROWSER_RISK: [(&str, Risk); 11] = [
    ("browser_snapshot", Risk::ReadOnly),
    ("browser_find", Risk::ReadOnly),
    ("browser_wait_for", Risk::ReadOnly),
    ("browser_navigate", Risk::Reversible),
    ("browser_navigate_back", Risk::Reversible),
    ("browser_press_key", Risk::Reversible),
    ("browser_close", Risk::Reversible),
    // a click, typing, a choice or a dialog answer can send, buy or delete on the site
    ("browser_click", Risk::Consequential),
    ("browser_type", Risk::Consequential),
    ("browser_select_option", Risk::Consequential),
    ("browser_handle_dialog", Risk::Consequential),
];

pub fn browser_risk(tool: &str) -> Option<Risk> { BROWSER_RISK.iter().find(|(name, _)| *name == tool).map(|(_, risk)| *risk) }

pub fn no_such_browser_tool(tool: &str) -> String { format!("There is no browser tool called \"{tool}\". Use one from the list.") }

/// What to do with one browser step.
#[derive(Debug, PartialEq)]
pub enum Gate {
    Run,
    /// Ask the user this first.
    Ask(String),
    /// Never done; the AI is told why.
    Refuse(String),
}

/// Proof that the policy let one browser step run. Its fields are private and only `check_browser` makes one, so the executor
/// in `browse` cannot run a step that did not pass the check.
#[derive(Debug)]
pub struct Permit { tool: String, args: Value }

impl Permit {
    pub fn tool(&self) -> &str { &self.tool }
    pub fn args(&self) -> &Value { &self.args }
}

/// Decides one browser step (`browse::run` calls `check_browser`, which calls this). `page` is the newest page list: the element's
/// own line there is judged together with the AI's description of it, because that description can be wrong, or steered by the page.
pub fn browser_gate(tool: &str, args: &Value, page: &str) -> Gate {
    if browser_risk(tool).is_none() { return Gate::Refuse(no_such_browser_tool(tool)); }
    let s = |k: &str| args[k].as_str().unwrap_or("");
    let label = format!("{} {}", s("element"), browse::ref_line(page, s("target")).unwrap_or(""));
    let shown = crate::assist::short(if s("element").is_empty() { label.trim() } else { s("element") }, 80);
    match tool {
        "browser_navigate" => match webread::public_url(s("url")) { Ok(()) => Gate::Run, Err(e) => Gate::Refuse(e) },
        "browser_type" | "browser_select_option" if browse::secret(&label) => Gate::Refuse("That field is for a password or a code, so fill that one in yourself in the browser window.".into()),
        "browser_click" | "browser_type" | "browser_select_option" if drive::risky(&label) => Gate::Ask(format!("The buddy wants to use \"{shown}\" in the browser. Allow it?")),
        // Enter after typing sends a chat message or a form as surely as a Send button does
        "browser_type" if args["submit"].as_bool().unwrap_or(false) && !search_box(&label) => Gate::Ask(format!("The buddy wants to type into \"{shown}\" and press Enter, which can send it. Allow it?")),
        "browser_press_key" if s("key").eq_ignore_ascii_case("enter") => Gate::Ask("The buddy wants to press Enter in the browser, which can send or submit. Allow it?".into()),
        "browser_handle_dialog" if args["accept"].as_bool().unwrap_or(false) => Gate::Ask("The buddy wants to say yes to a pop-up question in the browser. Allow it?".into()),
        _ => Gate::Run,
    }
}

/// A box you search or browse with: Enter in it sends nothing to anyone.
fn search_box(label: &str) -> bool {
    let l = label.to_lowercase();
    ["search", "find", "query", "address", "url", "filter"].iter().any(|w| l.contains(w))
}

/// The one way to run a browser step. `approved`: the user said yes for this request (`agent::approve`). `confirm` asks the island
/// about a risky step (`browser_gate`'s Ask). Returns a `Permit` only when the step may run.
pub fn check_browser(tool: &str, args: &Value, page: &str, approved: bool, confirm: &mut dyn FnMut(&str) -> bool) -> Result<Permit, String> {
    let ask = match browser_gate(tool, args, page) { Gate::Refuse(why) => return Err(why), Gate::Ask(q) => Some(q), Gate::Run => None };
    // the request's yes is asked before this call (`agent::approve`); a step that gets here without it does not run
    if !approved { return Err("The browser isn't allowed for this request.".into()); }
    if let Some(q) = ask {
        crate::log::line(&format!("browser confirm asked: {q}"));
        if !confirm(&q) { crate::log::line(&format!("browser confirm refused: {tool}")); return Err("You said no, so I didn't do that.".into()); }
        crate::log::line(&format!("browser confirm allowed: {tool}"));
    }
    Ok(Permit { tool: tool.to_string(), args: args.clone() })
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

    const PAGE: &str = "- button \"Delete account\" [ref=e7]\n- button \"Next\" [ref=e8]";

    #[test]
    fn every_browser_action_has_a_risk_level_and_the_table_is_the_browser_list() {
        for (name, _) in crate::browse::TOOLS {
            let level = browser_risk(name).unwrap_or_else(|| panic!("{name} has no risk level"));
            assert_eq!(risk_of(&Call::Browser { tool: name.into(), args: json!({}) }), level, "{name}");
        }
        for (name, _) in BROWSER_RISK {
            assert!(crate::browse::TOOLS.iter().any(|t| t.0 == name), "{name} is in the risk table but not offered");
        }
    }

    #[test]
    fn an_unknown_browser_action_is_refused_and_asks_nothing() {
        let mut asked = 0;
        for bad in ["browser_evaluate", "browser_run_code_unsafe", ""] {
            assert_eq!(browser_risk(bad), None, "{bad}");
            let r = check_browser(bad, &json!({}), PAGE, true, &mut |_| { asked += 1; true });
            assert!(matches!(r, Err(m) if m.contains("no browser tool")), "{bad}");
        }
        assert_eq!(asked, 0, "a refused name asks nothing");
    }

    #[test]
    fn a_consequential_browser_action_asks_and_does_not_run_on_no() {
        let mut asked = 0;
        let r = check_browser("browser_click", &json!({ "element": "Delete account", "target": "e7" }), PAGE, true, &mut |_| { asked += 1; false });
        assert_eq!(r.unwrap_err(), "You said no, so I didn't do that.");
        assert_eq!(asked, 1, "the click asked once");
        let yes = check_browser("browser_click", &json!({ "element": "Delete account", "target": "e7" }), PAGE, true, &mut |_| true);
        assert_eq!(yes.map(|p| p.tool().to_string()), Ok("browser_click".to_string()));
    }

    #[test]
    fn a_read_only_browser_action_runs_without_asking() {
        let mut asked = 0;
        for (tool, args) in [("browser_snapshot", json!({})), ("browser_find", json!({ "text": "x" })), ("browser_navigate", json!({ "url": "https://example.com" }))] {
            let p = check_browser(tool, &args, PAGE, true, &mut |_| { asked += 1; false }).expect(tool);
            assert_eq!(p.tool(), tool);
            assert_eq!(p.args(), &args);
        }
        assert_eq!(asked, 0);
    }

    #[test]
    fn a_browser_step_without_the_request_yes_does_not_pass() {
        let mut asked = 0;
        let r = check_browser("browser_snapshot", &json!({}), PAGE, false, &mut |_| { asked += 1; true });
        assert_eq!(r.unwrap_err(), "The browser isn't allowed for this request.");
        assert_eq!(asked, 0);
    }
}
