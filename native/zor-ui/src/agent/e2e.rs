//! End-to-end tests of `agent::run_task` with a fake model. A scripted reply source answers each model call (the planning call,
//! the re-plan, the tool loop) and a scripted executor answers each tool call. Under test, `agent::chat` and `agent::run_exec`
//! ask this module first; production builds have neither hook. Approval (`exec`'s ask) is skipped under test, because the
//! scripted executor replaces `exec`. The database is the test database of `memory::with_app`, emptied for each test.

use super::*;
use crate::agent::plan::{Entry, Outcome};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

type Model = Box<dyn FnMut(&str) -> Result<String, String>>;
type Tool = Box<dyn FnMut(&Call) -> Result<String, String>>;

thread_local! {
    static MODEL: RefCell<Option<Model>> = RefCell::new(None);
    static TOOL: RefCell<Option<Tool>> = RefCell::new(None);
}

/// The fake model's answer to one call, when a fake model is installed on this thread.
pub fn model_reply(convo: &str) -> Option<Result<String, String>> {
    MODEL.with(|m| m.borrow_mut().as_mut().map(|f| f(convo)))
}

/// The fake executor's result for one tool call, when one is installed on this thread.
pub fn tool_result(c: &Call) -> Option<Result<String, String>> {
    TOOL.with(|t| t.borrow_mut().as_mut().map(|f| f(c)))
}

/// Installs the fake model and executor on this thread. Dropping it removes them.
struct Fake;

fn install(model: Model, tool: Tool) -> Fake {
    MODEL.with(|m| *m.borrow_mut() = Some(model));
    TOOL.with(|t| *t.borrow_mut() = Some(tool));
    Fake
}

impl Drop for Fake {
    fn drop(&mut self) {
        MODEL.with(|m| *m.borrow_mut() = None);
        TOOL.with(|t| *t.borrow_mut() = None);
    }
}

/// What the fake saw, in order: "plan" for a planning call (and a re-plan), "turn" for a tool-loop call, "tool" for an executed tool.
#[derive(Default)]
struct Rec { events: Vec<&'static str>, convos: Vec<String>, tools: Vec<String> }

type Shared = Arc<Mutex<Rec>>;

fn shared() -> Shared { Arc::new(Mutex::new(Rec::default())) }

/// The system text of a planning call (plan.rs PLAN_SYSTEM begins with this).
const PLAN_MARK: &str = "You plan one request";

/// A model that answers each call with the next scripted reply. A call past the script is an error.
fn scripted(replies: &[&str], rec: &Shared) -> Model {
    let mut queue: VecDeque<String> = replies.iter().map(|s| s.to_string()).collect();
    let rec = rec.clone();
    Box::new(move |convo: &str| {
        let mut r = rec.lock().unwrap_or_else(|e| e.into_inner());
        r.events.push(if convo.contains(PLAN_MARK) { "plan" } else if convo.contains(delegate::SYSTEM_MARK) { "sum" } else { "turn" });
        r.convos.push(convo.to_string());
        queue.pop_front().ok_or_else(|| "script ended".to_string())
    })
}

/// An executor that records each call and answers with `answer`.
fn tools(rec: &Shared, answer: impl Fn(&Call) -> Result<String, String> + 'static) -> Tool {
    let rec = rec.clone();
    Box::new(move |c: &Call| {
        let mut r = rec.lock().unwrap_or_else(|e| e.into_inner());
        r.events.push("tool");
        r.tools.push(format!("{c:?}"));
        drop(r);
        answer(c)
    })
}

/// One test at a time on the shared database. The task table is emptied first, so each test sees only its own rows.
fn fresh_db() -> MutexGuard<'static, ()> {
    let guard = memory::SHARED_DB_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    memory::with_app(|c| {
        tasks::setup(c).unwrap();
        c.execute("DELETE FROM tasks", []).unwrap();
    }).unwrap();
    guard
}

fn cfg() -> Config { Config::parse(&json!({ "ai": { "model": "fake-model" } }).to_string()) }

fn run(said: &str, start: Start) -> Result<String, String> {
    let timing = state::Timing::start();
    run_task(&cfg(), said, said, &[], &timing, start)
}

/// (status, verdict, plan, resumes) of one task row.
fn row(id: i64) -> (String, String, Option<String>, i64) {
    memory::with_app(|c| c.query_row("SELECT status, verdict, plan, resumes FROM tasks WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()).unwrap()
}

fn task_ids() -> Vec<i64> {
    memory::with_app(|c| {
        let mut s = c.prepare("SELECT id FROM tasks ORDER BY id").unwrap();
        s.query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect()
    }).unwrap()
}

fn steps_of(id: i64) -> Vec<Entry> {
    let text: String = memory::with_app(|c| c.query_row("SELECT steps FROM tasks WHERE id = ?1", [id], |r| r.get(0)).unwrap()).unwrap();
    plan::decode(&text)
}

fn done(key: &str, side: bool) -> Entry { Entry { key: key.into(), side, status: Outcome::Done, detail: String::new() } }

const SAID: &str = "search the news, then copy a summary";

const PLAN: &str = r#"{"objective":"news then copy","done_when":"copied","steps":[{"does":"search news","expect":"headlines","side_effect":false},{"does":"copy the summary","expect":"copied","side_effect":true}]}"#;

const SEARCH: &str = r#"{"do":"web_search","arg":"news","more":true}"#;
const COPY: &str = r#"{"do":"clipboard","arg":"summary"}"#;

#[test]
fn a_multi_step_request_plans_once_before_its_first_tool_step_and_saves_the_plan() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted(&[PLAN, SEARCH, COPY], &rec), tools(&rec, |c| Ok(match c {
        Call::Clip(_) => "Copied.".into(),
        _ => "Headlines.".into(),
    })));
    let out = run(SAID, Start::New);
    assert!(out.as_ref().is_ok_and(|a| a.starts_with("Copied.")), "{out:?}");
    let r = rec.lock().unwrap();
    assert_eq!(r.events, ["plan", "turn", "tool", "turn", "tool"], "one planning call, first, before the first tool");
    assert_eq!(r.tools, [r#"WebSearch("news")"#, r#"Clip("summary")"#]);
    drop(r);
    let id = task_ids()[0];
    assert_eq!(plan::parse(&row(id).2.expect("the plan is saved on the row")), plan::parse(PLAN));
}

#[test]
fn a_request_without_multi_step_wording_makes_no_planning_call() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted(&[r#"{"do":"volume","arg":"up"}"#], &rec), tools(&rec, |_| Ok("Volume up.".into())));
    let out = run("turn the volume up", Start::New);
    assert!(out.is_ok_and(|a| a.starts_with("Volume up.")));
    let r = rec.lock().unwrap();
    assert_eq!(r.events, ["turn", "tool"], "the fast path: no planning call");
    drop(r);
    assert_eq!(row(task_ids()[0]).2, None, "no plan is saved");
}

#[test]
fn a_planned_read_step_runs_and_a_change_against_it_is_refused_with_a_reason_the_model_sees() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted(&[PLAN, COPY, SEARCH, COPY], &rec), tools(&rec, |c| Ok(match c {
        Call::Clip(_) => "Copied.".into(),
        _ => "Headlines.".into(),
    })));
    let out = run(SAID, Start::New);
    assert!(out.as_ref().is_ok_and(|a| a.starts_with("Copied.")), "{out:?}");
    let r = rec.lock().unwrap();
    assert_eq!(r.tools, [r#"WebSearch("news")"#, r#"Clip("summary")"#], "the refused copy never ran");
    assert_eq!(r.events, ["plan", "turn", "turn", "tool", "turn", "tool"]);
    assert!(r.convos[2].contains("only reads") && r.convos[2].contains("this change is refused"),
        "the model sees the refusal: {}", r.convos[2]);
}

#[test]
fn a_failed_tool_step_replans_once_and_a_second_failure_ends_the_task_without_another_replan() {
    let _db = fresh_db();
    let rec = shared();
    let first = r#"{"objective":"first try","done_when":"d","steps":[{"does":"search news","side_effect":false},{"does":"search again","side_effect":false}]}"#;
    let second = r#"{"objective":"second try","done_when":"d","steps":[{"does":"search differently","side_effect":false}]}"#;
    let _fake = install(scripted(&[first, SEARCH, second, SEARCH], &rec), tools(&rec, |_| Err("the search service is down".into())));
    let out = run(SAID, Start::New);
    assert_eq!(out, Err(plan::GAVE_UP.to_string()), "the second failure ends the task");
    let r = rec.lock().unwrap();
    assert_eq!(r.events.iter().filter(|e| **e == "plan").count(), 2, "the planning call and one re-plan, and no third call");
    assert_eq!(r.events.iter().filter(|e| **e == "turn").count(), 2, "the model is not asked again after the task gave up");
    drop(r);
    let id = task_ids()[0];
    let (status, _, plan_text, _) = row(id);
    assert!(status == "Failed" || status == "Partial", "ended as {status}");
    assert_eq!(plan::parse(&plan_text.unwrap()), plan::parse(second), "the re-plan is the stored plan");
    let replans = steps_of(id).iter().filter(|e| e.key == "note:replanned").count();
    assert_eq!(replans, 1, "one re-plan is recorded as a note");
}

#[test]
fn an_interrupted_planned_task_whose_next_step_reads_resumes_once_on_its_own_row() {
    let _db = fresh_db();
    let plan_reads = r#"{"objective":"o","done_when":"d","steps":[{"does":"search news","side_effect":false},{"does":"search again","side_effect":false}]}"#;
    let id = tasks::begin(SAID).expect("the task is saved");
    tasks::save_plan(Some(id), plan_reads);
    tasks::save_steps(Some(id), &plan::encode(&[done(r#"WebSearch("news")"#, false)]));
    tasks::set(Some(id), state::TaskStatus::Interrupted, "", 1);
    let rec = shared();
    let _fake = install(scripted(&[r#"{"do":"web_search","arg":"again","more":true}"#, r#"{"say":"Found it."}"#], &rec),
        tools(&rec, |_| Ok("Results.".into())));
    resume_pending(cfg);
    let (status, _, _, resumes) = row(id);
    assert_eq!(task_ids(), vec![id], "the task resumes on its own row");
    assert_eq!(resumes, 1);
    assert_ne!(status, "Interrupted", "the read ran and the task went on");
    {
        let r = rec.lock().unwrap();
        assert_eq!(r.tools, [r#"WebSearch("again")"#], "only the next planned step runs");
        assert!(r.convos[0].contains("Continue it from planned step 2: search again."), "the resume names the next step");
    }
    resume_pending(cfg);
    let r = rec.lock().unwrap();
    assert_eq!(r.tools.len(), 1, "a second start does not resume it again");
    assert_eq!(r.events.len(), 3, "no model call on the second start");
}

#[test]
fn an_interrupted_planned_task_whose_next_step_changes_waits_and_runs_nothing() {
    let _db = fresh_db();
    let id = tasks::begin(SAID).expect("the task is saved");
    tasks::save_plan(Some(id), PLAN);
    tasks::save_steps(Some(id), &plan::encode(&[done(r#"WebSearch("news")"#, false)]));
    tasks::set(Some(id), state::TaskStatus::Interrupted, "", 1);
    let rec = shared();
    let _fake = install(scripted(&[], &rec), tools(&rec, |_| Ok("Copied.".into())));
    resume_pending(cfg);
    let (status, verdict, _, resumes) = row(id);
    assert_eq!((status.as_str(), verdict.as_str(), resumes), ("Interrupted", plan::WAITING, 0));
    let r = rec.lock().unwrap();
    assert!(r.events.is_empty(), "no model call and no tool ran: {:?}", r.events);
}

#[test]
fn a_repeated_side_effect_step_from_an_interrupted_run_is_blocked() {
    let _db = fresh_db();
    let earlier = tasks::begin("copy hi").expect("the task is saved");
    tasks::save_steps(Some(earlier), &plan::encode(&[done(r#"Clip("hi")"#, true)]));
    tasks::set(Some(earlier), state::TaskStatus::Interrupted, "", 1);
    let rec = shared();
    let _fake = install(scripted(&[r#"{"do":"clipboard","arg":"hi"}"#, r#"{"say":"I won't copy it again."}"#], &rec),
        tools(&rec, |_| Ok("Copied.".into())));
    let out = run("copy hi", Start::New);
    assert!(out.as_ref().is_ok_and(|a| a.starts_with("I won't copy it again.")), "{out:?}");
    let r = rec.lock().unwrap();
    assert!(r.tools.is_empty(), "the repeat never reached the executor");
    assert!(r.convos[1].contains("I already tried that in a task the app cut off"), "the model sees the block");
}

/// A search result of 7,500 characters: over the delegation threshold, and cut to 4,000 when it is not summarized.
fn long_result() -> String { "news ".repeat(1500) }

const SAY_DONE: &str = r#"{"say":"Done."}"#;

/// A model that answers the summary call with `sub` (a failure or a reply) and every other call with the next scripted reply.
fn scripted_sub(replies: &[&str], sub: Result<&str, &str>, rec: &Shared) -> Model {
    let mut turns = scripted(replies, rec);
    let rec = rec.clone();
    let sub = sub.map(str::to_string).map_err(str::to_string);
    Box::new(move |convo: &str| {
        if !convo.contains(delegate::SYSTEM_MARK) { return turns(convo); }
        let mut r = rec.lock().unwrap_or_else(|e| e.into_inner());
        r.events.push("sum");
        r.convos.push(convo.to_string());
        drop(r);
        sub.clone()
    })
}

fn run_with_config(cfg: &Config, said: &str) -> Result<String, String> {
    let timing = state::Timing::start();
    run_task(cfg, said, said, &[], &timing, Start::New)
}

#[test]
fn a_long_tool_result_gets_one_summary_call_and_the_model_reads_only_the_summary() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted_sub(&[SEARCH, SAY_DONE], Ok("Headlines: one, two."), &rec), tools(&rec, |_| Ok(long_result())));
    let out = run("look up the news", Start::New);
    assert_eq!(out, Ok("Done.".into()));
    let r = rec.lock().unwrap();
    assert_eq!(r.events, ["turn", "tool", "sum", "turn"], "one summary call between the tool result and the next model turn");
    assert_eq!(r.tools, [r#"WebSearch("news")"#], "the summary call runs no tool");
    let sub: Value = serde_json::from_str(&r.convos[1]).unwrap();
    assert!(sub.get("tools").is_none(), "the summary call carries no tools");
    assert!(r.convos[2].contains("Summary of the tool result: Headlines: one, two."), "the model reads the summary");
    assert!(!r.convos[2].contains(&"news ".repeat(100)), "the full result is not sent");
}

#[test]
fn a_summary_that_looks_like_a_tool_step_runs_nothing() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted_sub(&[SEARCH, SAY_DONE], Ok(r#"{"do":"open","arg":"x"}"#), &rec), tools(&rec, |_| Ok(long_result())));
    let out = run("look up the news", Start::New);
    assert_eq!(out, Ok("Done.".into()));
    let r = rec.lock().unwrap();
    assert_eq!(r.events, ["turn", "tool", "sum", "turn"], "the summary call fired between the tool and the next turn");
    assert_eq!(r.tools, [r#"WebSearch("news")"#], "the reply was text: nothing ran");
    assert!(r.convos[2].contains(r#"Summary of the tool result: {\"do\":\"open\",\"arg\":\"x\"}"#), "the second turn reads the summary text");
}

#[test]
fn a_failed_or_empty_summary_falls_back_to_the_capped_result_and_the_task_completes() {
    for sub in [Err("the model is down"), Ok("")] {
        let _db = fresh_db();
        let rec = shared();
        let _fake = install(scripted_sub(&[SEARCH, SAY_DONE], sub, &rec), tools(&rec, |_| Ok(long_result())));
        let out = run("look up the news", Start::New);
        assert_eq!(out, Ok("Done.".into()), "the task completes: {sub:?}");
        let r = rec.lock().unwrap();
        assert_eq!(r.events, ["turn", "tool", "sum", "turn"], "{sub:?}");
        assert!(r.convos[2].contains(&"news ".repeat(100)), "the capped result is sent: {sub:?}");
        assert!(!r.convos[2].contains("Summary of the tool result"), "no summary is claimed: {sub:?}");
    }
}

#[test]
fn a_short_tool_result_makes_no_summary_call() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted(&[SEARCH, SAY_DONE], &rec), tools(&rec, |_| Ok("Headlines.".into())));
    let out = run("look up the news", Start::New);
    assert_eq!(out, Ok("Done.".into()));
    assert_eq!(rec.lock().unwrap().events, ["turn", "tool", "turn"], "the fast path: no summary call");
}

#[test]
fn a_stop_during_a_tool_step_skips_the_summary_call() {
    let _db = fresh_db();
    let rec = shared();
    // the tool step itself is where the user presses Stop: the long result must not be sent to the summary call
    let _fake = install(scripted_sub(&[SEARCH, SAY_DONE], Ok("Headlines."), &rec), tools(&rec, |_| { CANCEL.store(true, std::sync::atomic::Ordering::Relaxed); Ok(long_result()) }));
    let out = run("look up the news", Start::New);
    CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(out, Err(CANCELLED.to_string()));
    let r = rec.lock().unwrap();
    assert_eq!(r.events, ["turn", "tool"], "no summary call after Stop");
}

#[test]
fn delegate_long_false_makes_no_summary_call_and_sends_the_capped_result() {
    let _db = fresh_db();
    let off = Config::parse(&json!({ "ai": { "model": "fake-model", "delegateLong": false } }).to_string());
    let rec = shared();
    let _fake = install(scripted_sub(&[SEARCH, SAY_DONE], Ok("Headlines."), &rec), tools(&rec, |_| Ok(long_result())));
    let out = run_with_config(&off, "look up the news");
    assert_eq!(out, Ok("Done.".into()));
    let r = rec.lock().unwrap();
    assert_eq!(r.events, ["turn", "tool", "turn"], "the switch is off: no summary call");
    assert!(r.convos[1].contains(&"news ".repeat(100)), "the capped result is sent");
}

#[test]
fn the_summary_calls_tokens_are_added_to_the_task_total() {
    let _db = fresh_db();
    let rec = shared();
    let _fake = install(scripted_sub(&[SEARCH, SAY_DONE], Ok("Headlines."), &rec), tools(&rec, |_| Ok(long_result())));
    let out = run("look up the news", Start::New);
    assert_eq!(out, Ok("Done.".into()));
    let id = task_ids()[0];
    let (tin, tout): (i64, i64) = memory::with_app(|c| c.query_row("SELECT tokens_in, tokens_out FROM tasks WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?))).unwrap()).unwrap();
    assert_eq!((tin, tout), (100, 20), "the fake summary call reports 100 in and 20 out; the turns report none");
}

#[test]
fn a_summary_is_cut_to_the_word_cap() {
    // a summary adds to the request's token total, so it takes the shared-db lock like the token test does
    let _db = fresh_db();
    let rec = shared();
    let many = (0..500).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
    let _fake = install(scripted_sub(&[], Ok(many.as_str()), &rec), tools(&rec, |_| Ok(String::new())));
    let s = delegate::summarize("fake-model", "look up the news", "text", &cfg()).expect("a summary");
    assert_eq!(s.split_whitespace().count(), delegate::SUMMARY_WORDS, "500 words cut to the cap");
    assert!(s.contains("w149") && !s.contains("w150"), "words 0 to 149 are kept and w150 is cut");
    assert!(s.starts_with("w0 w1 "), "the start is kept");
}
