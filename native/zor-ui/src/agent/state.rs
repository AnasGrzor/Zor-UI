//! The state of one buddy request. The loop decides how a request ended from its result, not from anything the AI says
//! about being finished. A task moves through these states in order, and a move that skips one is refused and logged.

use crate::browse;
use std::{cell::Cell, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Planning,
    Running,
    WaitingApproval,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    /// The app closed while the task was still running. Found and marked on the next start.
    Interrupted,
}

impl TaskStatus {
    pub fn is_final(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Interrupted)
    }

    /// Whether a task may move from this state to `next`.
    pub fn can_go_to(self, next: TaskStatus) -> bool {
        use TaskStatus::*;
        if self.is_final() { return false; }
        matches!((self, next),
            (Planning, Running)
            | (Running, WaitingApproval) | (WaitingApproval, Running)
            | (Running, Verifying)
            | (Verifying, Completed) | (Verifying, Failed)
            // a request can end without a verify step: no tool ran, it was stopped, or it failed
            | (Running, Completed)
            | (Planning | Running | WaitingApproval, Failed | Cancelled | Interrupted))
    }
}

/// How a request ended, read from the loop's result. The user stopping the browser is a cancel, not a failure.
pub fn status_of(result: &Result<String, String>) -> TaskStatus {
    match result {
        Ok(_) => TaskStatus::Completed,
        Err(e) if e == browse::STOPPED || e == crate::agent::CANCELLED => TaskStatus::Cancelled,
        Err(_) => TaskStatus::Failed,
    }
}

/// A request's state, moved through `to`, which refuses a skipped step and keeps the state it had.
pub struct Task { pub status: TaskStatus }

impl Task {
    pub fn new() -> Task { Task { status: TaskStatus::Planning } }

    /// Moves to `next` if the state machine allows it. Returns false (and logs) when it does not.
    pub fn to(&mut self, next: TaskStatus) -> bool {
        if self.status.can_go_to(next) {
            self.status = next;
            true
        } else {
            crate::log::line(&format!("buddy task: refused {:?} -> {:?}", self.status, next));
            false
        }
    }
}

#[cfg(test)]
mod task_tests {
    use super::*;

    #[test]
    fn a_task_walks_its_states_in_order() {
        let mut t = Task::new();
        assert!(t.to(TaskStatus::Running) && t.to(TaskStatus::WaitingApproval) && t.to(TaskStatus::Running));
        assert!(t.to(TaskStatus::Verifying) && t.to(TaskStatus::Completed));
        assert_eq!(t.status, TaskStatus::Completed);
    }

    #[test]
    fn a_skipped_or_backwards_step_is_refused_and_the_state_stays() {
        let mut t = Task::new();
        assert!(!t.to(TaskStatus::Verifying), "planning can't jump to verifying");
        assert!(!t.to(TaskStatus::Completed), "nothing completes without running");
        assert_eq!(t.status, TaskStatus::Planning);
        t.to(TaskStatus::Running);
        t.to(TaskStatus::Verifying);
        assert!(!t.to(TaskStatus::Running), "no going back from verifying");
    }

    #[test]
    fn a_final_state_stays_final() {
        let mut t = Task::new();
        t.to(TaskStatus::Running);
        assert!(t.to(TaskStatus::Cancelled));
        assert!(!t.to(TaskStatus::Running) && !t.to(TaskStatus::Failed));
        assert!(TaskStatus::Interrupted.is_final() && !TaskStatus::WaitingApproval.is_final());
    }

    #[test]
    fn a_cancel_is_a_cancelled_task_not_a_failed_one() {
        assert_eq!(status_of(&Err(crate::agent::CANCELLED.into())), TaskStatus::Cancelled);
        assert_eq!(status_of(&Err("claude returned 503".into())), TaskStatus::Failed);
    }

    #[test]
    fn a_stop_or_an_interruption_can_come_from_any_running_state() {
        for s in [TaskStatus::Planning, TaskStatus::Running, TaskStatus::WaitingApproval] {
            assert!(s.can_go_to(TaskStatus::Interrupted) && s.can_go_to(TaskStatus::Cancelled), "{s:?}");
        }
        assert!(!TaskStatus::Verifying.can_go_to(TaskStatus::Cancelled), "a task being verified finishes, it is not cancelled");
    }
}

/// Where one request's time went: the whole request, the model calls, and the tool calls (confirmations included).
pub struct Timing {
    started: Instant,
    model: Cell<Duration>,
    tools: Cell<Duration>,
    tool_calls: Cell<u32>,
}

impl Timing {
    pub fn start() -> Timing {
        Timing { started: Instant::now(), model: Cell::new(Duration::ZERO), tools: Cell::new(Duration::ZERO), tool_calls: Cell::new(0) }
    }

    /// Runs one model call and adds its time.
    pub fn model<T>(&self, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let r = f();
        self.model.set(self.model.get() + t.elapsed());
        r
    }

    /// Runs one tool call and adds its time.
    pub fn tool<T>(&self, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let r = f();
        self.tools.set(self.tools.get() + t.elapsed());
        self.tool_calls.set(self.tool_calls.get() + 1);
        r
    }

    /// The log line for the request.
    pub fn line(&self) -> String {
        format!("buddy timing: {} ms total, {} ms in the model, {} ms in {} tool call(s)",
            self.started.elapsed().as_millis(), self.model.get().as_millis(), self.tools.get().as_millis(), self.tool_calls.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_adds_up_each_part_and_counts_tool_calls() {
        let t = Timing::start();
        let v = t.model(|| 7);
        t.tool(|| ());
        t.tool(|| ());
        assert_eq!(v, 7, "the wrapped value comes back unchanged");
        assert_eq!(t.tool_calls.get(), 2);
        assert!(t.line().contains("2 tool call(s)"));
    }

    #[test]
    fn an_answer_completes_a_stop_cancels_and_anything_else_fails() {
        assert_eq!(status_of(&Ok("Done.".into())), TaskStatus::Completed);
        assert_eq!(status_of(&Err(browse::STOPPED.into())), TaskStatus::Cancelled);
        assert_eq!(status_of(&Err("I couldn't finish that in a few steps.".into())), TaskStatus::Failed);
        assert_eq!(status_of(&Err("That took too long, so I stopped.".into())), TaskStatus::Failed);
    }
}
