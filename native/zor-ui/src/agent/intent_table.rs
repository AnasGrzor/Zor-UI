//! The intent table: phrases with the intent each one must get, and the context each comes with. The rules are keyword rules
//! (intent.rs), so the table is what they are measured against. Rows marked by the context they come with: `Nothing` (no
//! window, project or earlier turn), `Window` (a window in front), `Project` (an active project in front), `Previous` (an
//! earlier turn). Run with `cargo test intent_table`.

use super::intent::{self, Context, Decision, Kind};
use super::{approve, Call, DECLINED};
use crate::conf::Config;
use serde_json::json;

#[derive(Clone, Copy, Debug)]
enum Around { Nothing, Window, Project, Previous }

fn context(around: Around) -> Context {
    match around {
        Around::Nothing => Context::default(),
        Around::Window => Context { window: "notes.txt - Notepad".into(), ..Context::default() },
        Around::Project => Context { project: true, window: "main.rs - zor-ui - Visual Studio Code".into(), ..Context::default() },
        Around::Previous => Context { previous_request: true, conversation: true, ..Context::default() },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Want {
    /// Goes ahead. The kind is checked when given.
    Proceed(Option<Kind>),
    /// Goes ahead with an assumption, and the assumption names this objective.
    Assume(&'static str),
    /// Asks one question and does not act.
    Ask,
}

const LAST: &str = "the last thing we talked about";
const WINDOW: &str = "the window in front";
const PROJECT: &str = "the active project";

/// (request, context, what it must get). The brief's six vague requests come first, then plain requests that must not be vague,
/// then follow-ups that must resolve to the previous turn.
const TABLE: &[(&str, Around, Want)] = &[
    // the brief's six vague requests, each with and without something to point at
    ("Fix this.", Around::Nothing, Want::Ask),
    ("Fix this.", Around::Window, Want::Assume(WINDOW)),
    ("Fix this.", Around::Project, Want::Assume(PROJECT)),
    ("Fix this.", Around::Previous, Want::Assume(LAST)),
    ("Make it better.", Around::Nothing, Want::Ask),
    ("Make it better.", Around::Window, Want::Assume(WINDOW)),
    ("Continue where we left off.", Around::Nothing, Want::Ask),
    ("Continue where we left off.", Around::Previous, Want::Assume(LAST)),
    ("Get this ready for tomorrow.", Around::Nothing, Want::Ask),
    ("Get this ready for tomorrow.", Around::Previous, Want::Assume(LAST)),
    ("Build something like that.", Around::Nothing, Want::Ask),
    ("Build something like that.", Around::Previous, Want::Assume(LAST)),
    ("Clean up my project.", Around::Nothing, Want::Ask),
    ("Clean up my project.", Around::Project, Want::Assume(PROJECT)),
    ("Clean up my project.", Around::Window, Want::Ask),
    // more vague requests
    ("Improve it.", Around::Previous, Want::Assume(LAST)),
    ("Tidy this up.", Around::Nothing, Want::Ask),
    ("Sort this out.", Around::Nothing, Want::Ask),
    ("Polish this.", Around::Window, Want::Assume(WINDOW)),
    ("How do I fix this?", Around::Nothing, Want::Ask),
    ("Continue.", Around::Nothing, Want::Ask),
    // follow-ups: the previous turn is the object when there is one, a question when there is not
    ("Do that again.", Around::Previous, Want::Assume(LAST)),
    ("Do that again.", Around::Nothing, Want::Ask),
    ("Same thing for the other one.", Around::Previous, Want::Assume(LAST)),
    ("Same thing for the other one.", Around::Nothing, Want::Ask),
    ("Close it.", Around::Previous, Want::Proceed(None)),
    // plain requests: clear, so they proceed and are not vague
    ("Fix the bug in main.rs.", Around::Nothing, Want::Proceed(None)),
    ("fix the code", Around::Nothing, Want::Proceed(None)),
    ("clean the screen", Around::Nothing, Want::Proceed(None)),
    ("delete the old logs", Around::Nothing, Want::Proceed(None)),
    ("what does this function do", Around::Nothing, Want::Proceed(Some(Kind::Answer))),
    ("what time is it", Around::Nothing, Want::Proceed(Some(Kind::Answer))),
    ("what time is it now", Around::Previous, Want::Proceed(Some(Kind::Answer))),
    ("what is the time in Tokyo", Around::Nothing, Want::Proceed(Some(Kind::Answer))),
    ("is it raining", Around::Nothing, Want::Proceed(Some(Kind::Answer))),
    ("is it going to rain tomorrow", Around::Nothing, Want::Proceed(Some(Kind::Answer))),
    ("it's cold", Around::Nothing, Want::Proceed(Some(Kind::Answer))),
    ("open notepad", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("open Spotify again", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("open the project in VS Code", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("turn the volume to 30", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("send Sam a message", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("play some lofi", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("remember my favourite colour is blue", Around::Nothing, Want::Proceed(Some(Kind::Act))),
    ("turn wifi off", Around::Nothing, Want::Proceed(Some(Kind::Setting))),
    ("turn it off", Around::Nothing, Want::Proceed(Some(Kind::Setting))),
    ("search for news about the election", Around::Nothing, Want::Proceed(Some(Kind::Lookup))),
    ("what's the weather in Oslo", Around::Nothing, Want::Proceed(Some(Kind::Lookup))),
    ("Thanks, that's all.", Around::Nothing, Want::Proceed(None)),
];

/// (request, whether "it" or another reference word resolves to the previous turn). The previous turn is fixed in the check.
const REFERENCES: &[(&str, bool)] = &[
    ("close it", true),
    ("fix this", true),
    ("make it better", true),
    ("turn it off", true),
    ("do that again", true),
    ("same thing for the other one", true),
    // "it" in these is not a reference: a question about the time, the weather or a state
    ("what time is it", false),
    ("what time is it now", false),
    ("is it raining", false),
    ("it's cold", false),
    ("it is cold", false),
    ("is it going to rain tomorrow", false),
    ("what is it", false),
    ("open notepad", false),
    ("what is the time in Tokyo", false),
    ("send Sam a message", false),
];

const PREVIOUS: (&str, &str) = ("play lofi", "Playing lofi.");

/// Each row's outcome: Ok, or the reason it failed (what was expected, what the rules gave).
fn check_row(said: &str, around: Around, want: Want) -> Result<(), String> {
    let ctx = context(around);
    let a = intent::classify(said, &ctx);
    let d = intent::decide(a, &ctx);
    let got = format!("{:?} {:?} -> {:?}", a.kind, a.confidence, d);
    let ok = match (want, &d) {
        (Want::Proceed(kind), Decision::Proceed) => kind.map_or(true, |k| k == a.kind),
        (Want::Assume(objective), Decision::Assume(note)) => note.contains(objective),
        (Want::Ask, Decision::Ask(_)) => true,
        _ => false,
    };
    if ok { Ok(()) } else { Err(format!("\"{said}\" with {around:?}: want {want:?}, got {got}")) }
}

/// Runs the intent table: (passed, total, the failing rows).
fn run_table() -> (usize, usize, Vec<String>) {
    let failures: Vec<String> = TABLE.iter().filter_map(|&(said, around, want)| check_row(said, around, want).err()).collect();
    (TABLE.len() - failures.len(), TABLE.len(), failures)
}

fn run_references() -> (usize, usize, Vec<String>) {
    let failures: Vec<String> = REFERENCES.iter().filter_map(|&(said, want)| {
        let refers = intent::resolve_reference(said, Some(PREVIOUS)).is_some();
        (refers != want).then(|| format!("\"{said}\": want refers={want}, got refers={refers}"))
    }).collect();
    (REFERENCES.len() - failures.len(), REFERENCES.len(), failures)
}

#[test]
fn intent_table_passes_every_row() {
    let (pass, total, failures) = run_table();
    assert!(failures.is_empty(), "intent table: {pass}/{total} passed. Failing:\n{}", failures.join("\n"));
}

#[test]
fn intent_reference_table_passes_every_row() {
    let (pass, total, failures) = run_references();
    assert!(failures.is_empty(), "reference table: {pass}/{total} passed. Failing:\n{}", failures.join("\n"));
}

#[test]
fn vague_request_with_no_context_asks_one_question_and_does_not_act() {
    for said in ["Fix this.", "Make it better.", "Continue where we left off.", "Clean up my project."] {
        let ctx = context(Around::Nothing);
        let d = intent::decide(intent::classify(said, &ctx), &ctx);
        match d {
            Decision::Ask(q) => assert_eq!(q.matches('?').count(), 1, "one question, not a list: {q}"),
            other => panic!("{said}: expected one question, got {other:?}"),
        }
    }
}

#[test]
fn clean_up_my_project_with_an_active_project_resolves_to_the_project_and_is_not_destructive() {
    let ctx = context(Around::Project);
    match intent::decide(intent::classify("Clean up my project.", &ctx), &ctx) {
        Decision::Assume(s) => assert!(s.contains(PROJECT), "{s}"),
        other => panic!("expected the project as the object, got {other:?}"),
    }
    // the assumption is made, so the request is vague: a destructive or consequential call is refused without a question
    let cfg = Config(json!({}));
    let _assumed = intent::Assumed::request(true);
    let refused = [Call::Power("shutdown".into()), Call::Power("restart".into()), Call::RunCheck("test".into()), Call::TakeOver("delete the build folder".into())]
        .iter().all(|c| approve(c, &cfg, &mut |_| panic!("a vague request asks nothing for a risky step")).is_err());
    assert!(refused, "every destructive or consequential step is refused for a vague request");
}

#[test]
fn a_vague_request_never_selects_a_consequential_or_destructive_action() {
    let cfg = Config(json!({}));
    let risky = [
        Call::Power("shutdown".into()),
        Call::Power("sign out".into()),
        Call::TakeOver("send the form".into()),
        Call::RunCheck("build".into()),
        Call::Browser { tool: "browser_click".into(), args: json!({ "element": "Send" }) },
    ];
    {
        // a vague request that was resolved by assumption: nothing risky runs, and no question is asked for it
        let _assumed = intent::Assumed::request(true);
        for c in &risky {
            assert_eq!(approve(c, &cfg, &mut |_| panic!("no question for a vague request")), Err(intent::ASSUMED_REFUSAL.to_string()), "{c:?}");
        }
        // a reversible call is not blocked by the rule
        assert!(approve(&Call::Open("notepad".into()), &cfg, &mut |_| panic!("opens ask nothing")).is_ok());
    }
    // the guard is gone: a clear request is not vague, so the same risky call goes through the usual question
    assert!(!intent::is_assumed());
    assert_eq!(approve(&Call::TakeOver("fill the form".into()), &cfg, &mut |_| false), Err(DECLINED.into()));
}
