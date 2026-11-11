//! Each tool step of a task, saved on the task's row (`tasks.steps`) as it runs. A step that changes something outside the buddy
//! (a message sent, a key pressed, a power action) is refused when an interrupted task already tried it, so an app restart
//! never repeats it. Reads may repeat. The AI picks the route after a failure; this file only records the steps and guards them.

use super::{tasks, Call};
use crate::assist;
use serde_json::{json, Value};

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
    !matches!(call, Call::WebSearch(_) | Call::ReadUrl(_) | Call::FindApp(_) | Call::FindFile(_) | Call::SeeScreen(_) | Call::Remember(_) | Call::Forget(_))
}

/// The identity of a step: the call as the buddy holds it.
pub fn key(call: &Call) -> String { format!("{call:?}") }

pub fn encode(steps: &[Entry]) -> String {
    json!(steps.iter().map(|e| json!({ "key": e.key, "side": e.side, "status": e.status.name(), "detail": e.detail })).collect::<Vec<_>>()).to_string()
}

/// Steps read back from the database. A step that can't be read is dropped. A missing `side` counts as a change, so it is never repeated.
pub fn decode(text: &str) -> Vec<Entry> {
    serde_json::from_str::<Vec<Value>>(text).unwrap_or_default().iter().filter_map(|v| Some(Entry {
        key: v["key"].as_str()?.to_string(),
        side: v["side"].as_bool().unwrap_or(true),
        status: Outcome::from_name(v["status"].as_str()?)?,
        detail: v["detail"].as_str().unwrap_or("").to_string(),
    })).collect()
}

/// The keys a repeat must not run: changing steps that an interrupted task started (no outcome yet) or finished.
pub fn blocked_keys(records: &[String]) -> Vec<String> {
    records.iter().flat_map(|text| decode(text))
        .filter(|e| e.side && matches!(e.status, Outcome::Running | Outcome::Done))
        .map(|e| e.key)
        .collect()
}

/// The steps of one request as it runs. With no task id (the database can't be used) nothing is saved and the request goes on.
pub struct Steps { id: Option<i64>, entries: Vec<Entry>, blocked: Vec<String> }

impl Steps {
    /// `interrupted`: the step records of the earlier interrupted tasks (`tasks::interrupted_steps`).
    pub fn new(id: Option<i64>, interrupted: &[String]) -> Steps {
        Steps { id, entries: vec![], blocked: blocked_keys(interrupted) }
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
