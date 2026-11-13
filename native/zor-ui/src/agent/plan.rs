//! Each tool step of a task, saved on the task's row (`tasks.steps`) as it runs. A step that changes something outside the buddy
//! (a message sent, a key pressed, a power action) is refused when an interrupted task already tried it, so an app restart
//! never repeats it. Reads may repeat. The AI picks the route after a failure; this file only records the steps and guards them.

use super::{routing, tasks, Call};
use crate::assist;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// The most steps a plan may have; a longer reply is cut to this.
pub const MAX_PLAN_STEPS: usize = 8;
/// Note keys in the step record start with NOTE, so they are never counted as tool steps.
const NOTE: &str = "note:";
const REPLANNED: &str = "note:replanned";
pub const WAITING: &str = "waiting: the next step changes something; ask the user";
pub const GAVE_UP: &str = "I couldn't finish that as planned, so I stopped.";
const PLAN_SYSTEM: &str = "You plan one request for a desktop buddy. Reply with only a JSON object: {\"objective\": string, \"done_when\": string, \"steps\": [{\"does\": string, \"expect\": string, \"side_effect\": bool}]}. At most 8 steps, in order. side_effect is true for a step that changes something outside the buddy (sends, opens, plays, sets, power); false for a lookup or a read.";

#[derive(Clone, Debug, PartialEq)]
pub struct PlanStep { pub does: String, pub expect: String, pub side: bool }

/// A plan declared before a multi-step request runs. Stored on the task row (`tasks.plan`) as `to_json`, the shape the model sends.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan { pub objective: String, pub done_when: String, pub steps: Vec<PlanStep> }

impl Plan {
    pub fn to_json(&self) -> String {
        json!({ "objective": self.objective, "done_when": self.done_when,
            "steps": self.steps.iter().map(|s| json!({ "does": s.does, "expect": s.expect, "side_effect": s.side })).collect::<Vec<_>>() }).to_string()
    }
}

/// Reads a plan from a model reply or a stored row. Text around the JSON (code fences included) is skipped. Unreadable text, no
/// steps, or only blank steps give None, and the task runs without a plan. A step with no `side_effect` flag counts as a change.
pub fn parse(text: &str) -> Option<Plan> {
    let t = text.trim();
    let (a, b) = (t.find('{')?, t.rfind('}')?);
    let v: Value = serde_json::from_str(t.get(a..=b)?).ok()?;
    let steps: Vec<PlanStep> = v["steps"].as_array()?.iter().filter_map(|s| {
        let does = s["does"].as_str()?.trim();
        (!does.is_empty()).then(|| PlanStep { does: does.to_string(), expect: s["expect"].as_str().unwrap_or("").trim().to_string(), side: s["side_effect"].as_bool().unwrap_or(true) })
    }).take(MAX_PLAN_STEPS).collect();
    if steps.is_empty() { return None; }
    Some(Plan { objective: v["objective"].as_str().unwrap_or("").trim().to_string(), done_when: v["done_when"].as_str().unwrap_or("").trim().to_string(), steps })
}

/// Whether a request gets a plan first: the same multi-step check that picks the strong model. Any other request makes no call.
pub fn wanted(said: &str) -> bool { routing::looks_multi_step(said) }

fn convo(model: &str, prompt: &str) -> String {
    json!({ "model": model, "system": PLAN_SYSTEM, "temperature": 0.2, "messages": [{ "role": "user", "content": prompt }] }).to_string()
}

/// One planning call: `ask` takes the conversation JSON `assist::chat` takes. A failed call or an unreadable reply is no plan.
fn call(model: &str, prompt: &str, ask: impl FnOnce(&str) -> Result<String, String>) -> Option<Plan> {
    parse(&ask(&convo(model, prompt)).ok()?)
}

/// The plan for a request that `wanted` flags. Any other request returns None without calling `ask`.
pub fn plan_if_wanted(said: &str, model: &str, ask: impl FnOnce(&str) -> Result<String, String>) -> Option<Plan> {
    if !wanted(said) { return None; }
    call(model, &format!("Request: {said}"), ask)
}

/// The new plan after a failure: the request, the plan so far, the steps already done, and what failed.
pub fn replan(model: &str, said: &str, old: &Plan, done: usize, failure: &str, ask: impl FnOnce(&str) -> Result<String, String>) -> Option<Plan> {
    let completed: Vec<&str> = old.steps.iter().take(done).map(|s| s.does.as_str()).collect();
    let completed = if completed.is_empty() { "none".to_string() } else { completed.join("; ") };
    call(model, &format!("Request: {said}\nPlan so far: {}\nCompleted steps: {completed}\nFailure: {failure}\nMake a new plan for the steps still needed.", old.to_json()), ask)
}

/// What the plan says about one tool call, or about a failure.
#[derive(Debug, PartialEq)]
pub enum Check { Allow, Refuse(String), Replan, GiveUp, Wait }

/// The plan of a running task: the next planned step, the refusals, and whether the task was re-planned. A task is re-planned at
/// most once; a failure after that ends it. A resumed task never runs a change.
pub struct Progress { plan: Option<Plan>, next: usize, refusals: u32, replanned: bool, resumed: bool, give_up: bool, waiting: bool }

impl Progress {
    pub fn new(plan: Option<Plan>, next: usize, resumed: bool) -> Progress {
        Progress { plan, next, refusals: 0, replanned: false, resumed, give_up: false, waiting: false }
    }
    pub fn plan(&self) -> Option<&Plan> { self.plan.as_ref() }
    pub fn next(&self) -> usize { self.next }
    pub fn waiting(&self) -> bool { self.waiting }

    /// Why the model's next turn must not run: the task is waiting for the user, or gave up. None while it can go on.
    pub fn halt(&self) -> Option<String> {
        if self.waiting { Some(WAITING.into()) } else if self.give_up { Some(GAVE_UP.into()) } else { None }
    }

    /// Checks one tool call against the next planned step, and counts it against that step when it is allowed.
    pub fn gate(&mut self, call: &Call) -> Check {
        let side = side_effect(call);
        let Some(plan) = &self.plan else { return Check::Allow };
        if self.resumed && side { self.waiting = true; return Check::Wait; }
        let Some(step) = plan.steps.get(self.next) else { return Check::Allow };
        if side && !step.side {
            self.refusals += 1;
            if self.refusals < 2 {
                return Check::Refuse(format!("The plan says step {} only reads ({}), so this change is refused. Do the planned step, or answer without the change.", self.next + 1, step.does));
            }
            return self.failure();
        }
        self.next += 1;
        Check::Allow
    }

    /// A failure: a failed tool step, or a planned step refused twice. The first re-plans (the caller asks for the new plan);
    /// any later one ends the task.
    pub fn failure(&mut self) -> Check {
        if self.plan.is_none() { return Check::Allow; }
        if self.replanned { self.give_up = true; return Check::GiveUp; }
        self.replanned = true;
        Check::Replan
    }

    /// Takes the new plan after a re-plan. None (the re-plan failed) ends the task.
    pub fn set_plan(&mut self, plan: Option<Plan>) {
        match plan {
            Some(p) => { self.plan = Some(p); self.next = 0; self.refusals = 0; }
            None => self.give_up = true,
        }
    }
}

/// What a restart does with an interrupted task that has a plan.
#[derive(Debug, PartialEq)]
pub enum Resume { Run(usize), Wait, Skip }

/// The index of the next unfinished planned step: the tool steps recorded since the last re-plan.
pub fn next_index(entries: &[Entry]) -> usize {
    entries.iter().rev().take_while(|e| e.key != REPLANNED).filter(|e| !e.key.starts_with(NOTE)).count()
}

/// Resumes from the next step when it only reads. A next step that changes something waits for the user. A finished plan is skipped.
pub fn resume_action(plan: &Plan, entries: &[Entry]) -> Resume {
    let next = next_index(entries);
    match plan.steps.get(next) {
        None => Resume::Skip,
        Some(s) if s.side => Resume::Wait,
        Some(_) => Resume::Run(next),
    }
}

/// How many recent interrupted tasks count when the buddy decides what not to repeat.
pub const RECENT: i64 = 5;

/// One tool step: its identity (the call as Debug text), whether it changes something, its outcome and a short detail.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub key: String, pub side: bool, pub status: Outcome, pub detail: String }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Running, Done, Failed, Declined, Blocked }

impl Outcome {
    fn name(self) -> &'static str {
        match self { Outcome::Running => "running", Outcome::Done => "done", Outcome::Failed => "failed", Outcome::Declined => "declined", Outcome::Blocked => "blocked" }
    }
    fn from_name(s: &str) -> Option<Outcome> {
        Some(match s { "running" => Outcome::Running, "done" => Outcome::Done, "failed" => Outcome::Failed, "declined" => Outcome::Declined, "blocked" => Outcome::Blocked, _ => return None })
    }
}

/// Reads and memory edits may repeat. Every other tool changes something outside the buddy, and an unknown one counts as a change.
pub fn side_effect(call: &Call) -> bool {
    !matches!(call, Call::WebSearch(_) | Call::ReadUrl(_) | Call::FindApp(_) | Call::FindFile(_) | Call::SeeScreen(_) | Call::Remember(_) | Call::Forget(_)
        | Call::ListTree(_) | Call::ReadFile(_) | Call::SearchCode(_) | Call::GitStatus)
}

/// The identity of a step: the call as the buddy holds it.
pub fn key(call: &Call) -> String { format!("{call:?}") }

pub fn encode(steps: &[Entry]) -> String { encode_at(steps, now()) }

/// Each step carries the time the record was saved (`at`), so an old interrupted task stops blocking repeats.
fn encode_at(steps: &[Entry], at: i64) -> String {
    json!(steps.iter().map(|e| json!({ "key": e.key, "side": e.side, "status": e.status.name(), "detail": e.detail, "at": at })).collect::<Vec<_>>()).to_string()
}

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

/// Steps read back from the database. A step that can't be read is dropped. A missing `side` counts as a change, so it is never repeated.
pub fn decode(text: &str) -> Vec<Entry> {
    serde_json::from_str::<Vec<Value>>(text).unwrap_or_default().iter().filter_map(|v| Some(Entry {
        key: v["key"].as_str()?.to_string(),
        side: v["side"].as_bool().unwrap_or(true),
        status: Outcome::from_name(v["status"].as_str()?)?,
        detail: v["detail"].as_str().unwrap_or("").to_string(),
    })).collect()
}

/// How long an interrupted task keeps blocking its changing steps. Past this, the task is old and a repeat is allowed again;
/// without the window the same step stayed blocked in every later request.
pub const BLOCK_WINDOW_SECS: i64 = 10 * 60;

/// The keys a repeat must not run: changing steps that an interrupted task started (no outcome yet) or finished, if the task was saved within `BLOCK_WINDOW_SECS`.
pub fn blocked_keys(records: &[String]) -> Vec<String> { blocked_keys_at(records, now()) }

fn blocked_keys_at(records: &[String], now: i64) -> Vec<String> {
    records.iter().filter(|text| saved_at(text).is_some_and(|at| now - at <= BLOCK_WINDOW_SECS))
        .flat_map(|text| decode(text))
        .filter(|e| e.side && matches!(e.status, Outcome::Running | Outcome::Done))
        .map(|e| e.key)
        .collect()
}

/// When a record was last saved. A record from before the stamp (no `at`) counts as old.
fn saved_at(text: &str) -> Option<i64> {
    serde_json::from_str::<Vec<Value>>(text).ok()?.iter().filter_map(|v| v["at"].as_i64()).max()
}

/// The steps of one request as it runs. With no task id (the database can't be used) nothing is saved and the request goes on.
pub struct Steps { id: Option<i64>, entries: Vec<Entry>, blocked: Vec<String> }

impl Steps {
    /// `interrupted`: the step records of the earlier interrupted tasks (`tasks::interrupted_steps`).
    pub fn new(id: Option<i64>, interrupted: &[String]) -> Steps {
        Steps { id, entries: vec![], blocked: blocked_keys(interrupted) }
    }

    /// A resumed task: its record so far (`prior`) is kept and new steps are added to it.
    pub fn resumed(id: Option<i64>, prior: Vec<Entry>, interrupted: &[String]) -> Steps {
        Steps { id, entries: prior, blocked: blocked_keys(interrupted) }
    }

    /// Adds a note to the record (a plan that could not be made, a re-plan, a wait). Notes are not tool steps.
    pub fn note(&mut self, key: &str, detail: &str) {
        self.entries.push(Entry { key: format!("{NOTE}{key}"), side: false, status: Outcome::Done, detail: detail.to_string() });
        self.save();
    }

    /// Records a step about to run. Returns why it must not run: a changing step an interrupted task already tried.
    pub fn begin(&mut self, call: &Call) -> Option<String> {
        let k = key(call);
        let side = side_effect(call);
        let refuse = side && self.blocked.contains(&k);
        let (status, detail) = if refuse { (Outcome::Blocked, "already tried in an interrupted task".to_string()) } else { (Outcome::Running, String::new()) };
        self.entries.push(Entry { key: k, side, status, detail });
        self.save();
        refuse.then(|| "I already tried that in a task the app cut off, so I won't repeat it. Ask me again if you want it done.".to_string())
    }

    /// Records the outcome of the step begun last. `declined`: the user said no to it.
    pub fn end(&mut self, res: &Result<String, String>, declined: bool) {
        if let Some(last) = self.entries.last_mut().filter(|s| s.status == Outcome::Running) {
            last.status = match (res.is_ok(), declined) { (_, true) => Outcome::Declined, (true, _) => Outcome::Done, _ => Outcome::Failed };
            last.detail = assist::short(match res { Ok(r) => r, Err(e) => e }, 200);
        }
        self.save();
    }

    fn save(&self) {
        if self.id.is_some() { tasks::save_steps(self.id, &encode(&self.entries)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(call: &Call, status: Outcome) -> Entry { Entry { key: key(call), side: side_effect(call), status, detail: String::new() } }

    #[test]
    fn reads_may_repeat_and_anything_that_changes_something_or_is_unknown_may_not() {
        assert!(!side_effect(&Call::WebSearch("news".into())));
        assert!(!side_effect(&Call::ReadUrl("https://a.b".into())));
        assert!(side_effect(&Call::Message { to: "Sam".into(), text: "hi".into(), via: String::new() }));
        assert!(side_effect(&Call::Power("restart".into())));
        assert!(side_effect(&Call::Browser { tool: "click".into(), args: json!({}) }), "an unknown or new browser tool fails closed");
    }

    #[test]
    fn steps_read_back_as_they_were_written() {
        let steps = vec![entry(&Call::Clip("x".into()), Outcome::Done), Entry { key: "k".into(), side: false, status: Outcome::Declined, detail: "no".into() }];
        assert_eq!(decode(&encode(&steps)), steps);
        assert!(decode("not json").is_empty(), "a damaged record is dropped, not a crash");
    }

    #[test]
    fn an_interrupted_tasks_changing_step_blocks_a_repeat_only_inside_the_window() {
        let msg = Call::Message { to: "Sam".into(), text: "hi".into(), via: String::new() };
        let saved = 10_000;
        let record = encode_at(&[entry(&msg, Outcome::Done)], saved);
        let recent = saved + BLOCK_WINDOW_SECS;
        assert_eq!(blocked_keys_at(std::slice::from_ref(&record), recent), [key(&msg)], "inside the window: a repeat is refused");
        assert!(blocked_keys_at(std::slice::from_ref(&record), recent + 1).is_empty(), "after the window: the step may run again");
        let unstamped = r#"[{"key":"x","side":true,"status":"done"}]"#.to_string();
        assert!(blocked_keys_at(&[unstamped], saved).is_empty(), "a record with no stamp counts as old");
        // within one request: a Steps that starts with a recent interrupted record refuses the repeat, and an old one doesn't
        let mut steps = Steps { id: None, entries: vec![], blocked: blocked_keys_at(&[record], saved) };
        assert!(steps.begin(&msg).is_some(), "the repeat is refused in the same request");
        let mut fresh = Steps { id: None, entries: vec![], blocked: blocked_keys_at(&[encode_at(&[entry(&msg, Outcome::Done)], saved)], saved + BLOCK_WINDOW_SECS + 1) };
        assert!(fresh.begin(&msg).is_none());
    }

    #[test]
    fn only_changing_steps_an_interrupted_task_started_or_finished_are_blocked() {
        let msg = Call::Message { to: "Sam".into(), text: "hi".into(), via: String::new() };
        let search = Call::WebSearch("news".into());
        let play = Call::Play { query: "x".into(), service: String::new() };
        let power = Call::Power("restart".into());
        let declined = Call::Volume("up".into());
        let record = encode(&[entry(&msg, Outcome::Done), entry(&search, Outcome::Done), entry(&play, Outcome::Failed),
            entry(&power, Outcome::Running), entry(&declined, Outcome::Declined)]);
        let keys = blocked_keys(&[record]);
        assert!(keys.contains(&key(&msg)), "a message that went out is not sent again");
        assert!(keys.contains(&key(&power)), "a power step with no outcome is not run again");
        assert!(!keys.contains(&key(&search)), "reads may repeat");
        assert!(!keys.contains(&key(&play)), "a failed step may be tried another way");
        assert!(!keys.contains(&key(&declined)), "a step the user said no to was never done");
    }

    fn planned(sides: &[bool]) -> Plan {
        Plan { objective: "o".into(), done_when: "d".into(), steps: sides.iter().enumerate()
            .map(|(i, &s)| PlanStep { does: format!("step {}", i + 1), expect: String::new(), side: s }).collect() }
    }
    fn done_entry(side: bool) -> Entry { Entry { key: "k".into(), side, status: Outcome::Done, detail: String::new() } }

    #[test]
    fn plan_parse_accepts_valid_json_and_reads_back_what_it_stored() {
        let p = parse(r#"{"objective":"send hi","done_when":"sent","steps":[{"does":"search","expect":"results","side_effect":false},{"does":"send","expect":"sent","side_effect":true}]}"#).unwrap();
        assert_eq!((p.objective.as_str(), p.done_when.as_str()), ("send hi", "sent"));
        assert_eq!(p.steps.len(), 2);
        assert!(!p.steps[0].side && p.steps[1].side);
        assert_eq!(parse(&p.to_json()), Some(p.clone()), "the stored plan reads back as it was");
        let many = format!(r#"{{"steps":[{}]}}"#, (0..10).map(|i| format!(r#"{{"does":"s{i}","side_effect":false}}"#)).collect::<Vec<_>>().join(","));
        assert_eq!(parse(&many).unwrap().steps.len(), MAX_PLAN_STEPS, "a long plan is cut to 8 steps");
    }

    #[test]
    fn plan_parse_strips_code_fences() {
        let p = parse("Here is the plan:\n```json\n{\"objective\":\"x\",\"steps\":[{\"does\":\"look\",\"side_effect\":false}]}\n```").unwrap();
        assert_eq!(p.steps[0].does, "look");
        assert!(!p.steps[0].side);
    }

    #[test]
    fn plan_parse_returns_none_on_garbage_or_an_empty_plan() {
        assert_eq!(parse("sorry, I can't plan that"), None);
        assert_eq!(parse(r#"{"objective":"x","steps":[]}"#), None);
        assert_eq!(parse(r#"{"objective":"x","steps":[{"does":"  "}]}"#), None, "a blank step is not a step");
        assert_eq!(parse("}{"), None);
    }

    #[test]
    fn a_request_without_multi_step_wording_makes_no_planning_call() {
        let mut calls = 0;
        let fast = plan_if_wanted("turn the volume up", "m", |_| { calls += 1; Ok(String::new()) });
        assert_eq!((fast, calls), (None, 0));
        assert!(wanted("open spotify, then play lofi"));
        let planned = plan_if_wanted("compare two laptops and then book one", "m", |_| Ok(r#"{"steps":[{"does":"look"}]}"#.into()));
        assert_eq!(planned.unwrap().steps.len(), 1);
    }

    #[test]
    fn a_side_effect_call_against_a_read_only_step_is_refused() {
        let mut p = Progress::new(Some(planned(&[false, true])), 0, false);
        let send = Call::Clip("hi".into());
        assert!(matches!(p.gate(&send), Check::Refuse(ref why) if why.contains("step 1")));
        assert_eq!(p.gate(&Call::WebSearch("news".into())), Check::Allow, "a read matches the read-only step");
        assert_eq!(p.gate(&send), Check::Allow, "the change is the next step, which allows it");
        assert_eq!(p.next(), 2);
    }

    #[test]
    fn a_second_refusal_triggers_exactly_one_re_plan() {
        let mut p = Progress::new(Some(planned(&[false])), 0, false);
        let send = Call::Clip("hi".into());
        assert!(matches!(p.gate(&send), Check::Refuse(_)), "the first refusal only tells the model");
        assert_eq!(p.gate(&send), Check::Replan, "the second refusal triggers the one re-plan");
        p.set_plan(Some(planned(&[true])));
        assert_eq!(p.gate(&send), Check::Allow, "the new plan has a change step");
        assert_eq!(p.failure(), Check::GiveUp, "a failure after the re-plan ends the task: no second re-plan");
    }

    #[test]
    fn a_third_failure_ends_the_task_and_never_re_plans_again() {
        let mut p = Progress::new(Some(planned(&[false, false])), 0, false);
        assert_eq!(p.failure(), Check::Replan, "the first failure re-plans");
        p.set_plan(Some(planned(&[false])));
        assert_eq!(p.failure(), Check::GiveUp, "the second failure ends the task");
        assert!(p.halt().is_some());
        assert_eq!(p.failure(), Check::GiveUp, "a third failure ends it again, with no re-plan");
        let mut none = Progress::new(None, 0, false);
        assert_eq!(none.failure(), Check::Allow, "a task with no plan is never re-planned or held");
        assert!(none.halt().is_none());
    }

    #[test]
    fn an_interrupted_task_whose_next_step_reads_resumes_from_it() {
        assert_eq!(resume_action(&planned(&[false, true]), &[]), Resume::Run(0));
        assert_eq!(resume_action(&planned(&[false, false, true]), &[done_entry(false)]), Resume::Run(1));
    }

    #[test]
    fn an_interrupted_task_whose_next_step_changes_something_waits_and_a_finished_plan_is_skipped() {
        assert_eq!(resume_action(&planned(&[false, true]), &[done_entry(false)]), Resume::Wait);
        assert_eq!(resume_action(&planned(&[false]), &[done_entry(false)]), Resume::Skip);
    }

    #[test]
    fn the_next_step_counts_only_the_steps_since_the_last_re_plan() {
        let replanned = Entry { key: REPLANNED.into(), side: false, status: Outcome::Done, detail: "new".into() };
        let entries = [done_entry(true), replanned, done_entry(false)];
        assert_eq!(next_index(&entries), 1, "the step before the re-plan is not counted against the new plan");
        assert_eq!(next_index(&[done_entry(true), Entry { key: "note:plan".into(), side: false, status: Outcome::Done, detail: String::new() }]), 1, "a note is not a step");
    }

    #[test]
    fn a_repeat_of_an_interrupted_step_is_refused_and_the_steps_are_recorded() {
        let old = Call::Clip("hello".into());
        let mut steps = Steps::new(None, &[encode(&[entry(&old, Outcome::Done)])]);
        assert!(steps.begin(&old).is_some(), "the copy already ran in the interrupted task");
        assert!(steps.begin(&Call::WebSearch("news".into())).is_none());
        steps.end(&Ok("results".into()), false);
        assert!(steps.begin(&Call::Volume("up".into())).is_none());
        steps.end(&Err("Volume takes up, down or 0 to 100.".into()), false);
        assert!(steps.begin(&Call::Volume("down".into())).is_none());
        steps.end(&Err("no".into()), true);
        let got: Vec<Outcome> = steps.entries.iter().map(|e| e.status).collect();
        assert_eq!(got, vec![Outcome::Blocked, Outcome::Done, Outcome::Failed, Outcome::Declined]);
    }
}
