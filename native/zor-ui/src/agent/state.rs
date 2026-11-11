//! The state of one buddy request. The loop decides how a request ended from its result and the verifier's verdict, not from
//! anything the AI says about being finished. A task moves through these states in order, and a move that skips one is
//! refused and logged.

use crate::agent::verifier::Verdict;
use crate::browse;
use std::{cell::Cell, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Planning,
    Running,
    WaitingApproval,
    Verifying,
    /// Finished and every step was confirmed by the tools' reports.
    Completed,
    /// Finished, but some of the evidence could not be checked (volume, opening an app).
    Unverified,
    /// Some steps worked and the request as a whole did not.
    Partial,
    Failed,
    /// The user said no to what the request needed. Nothing was done by choice.
    Declined,
    Cancelled,
    /// The app closed while the task was still running. Found and marked on the next start.
    Interrupted,
}

impl TaskStatus {
    pub fn is_final(self) -> bool {
        use TaskStatus::*;
        matches!(self, Completed | Unverified | Partial | Failed | Declined | Cancelled | Interrupted)
    }

    /// Whether a task may move from this state to `next`.
    pub fn can_go_to(self, next: TaskStatus) -> bool {
        use TaskStatus::*;
        if self.is_final() { return false; }
        matches!((self, next),
            (Planning, Running)
            | (Running, WaitingApproval) | (WaitingApproval, Running)
            | (Running, Verifying)
            | (Verifying, Completed | Unverified | Partial | Failed | Declined)
            // a request can end without a verify step: no tool ran, or the user said no
            | (Running, Completed | Declined)
            | (Planning | Running | WaitingApproval, Failed | Cancelled | Interrupted))
    }
}

/// How a request ended. An answer is only as good as the verifier's verdict on its tools: an answer whose tools failed is
/// Failed, not Completed. A failure after a tool already acted is Partial, because the action happened. The user stopping
/// the browser or the request is a cancel, not a failure.
pub fn status_of(result: &Result<String, String>, verdict: Verdict, any_effect: bool) -> TaskStatus {
    match result {
        Err(e) if e == browse::STOPPED || e == crate::agent::CANCELLED => TaskStatus::Cancelled,
        Err(_) if verdict == Verdict::Declined => TaskStatus::Declined,
        Err(_) if any_effect => TaskStatus::Partial,
        Err(_) => TaskStatus::Failed,
        Ok(_) => match verdict {
            Verdict::Completed => TaskStatus::Completed,
            Verdict::Unverified => TaskStatus::Unverified,
            Verdict::Partial => TaskStatus::Partial,
            Verdict::Failed => TaskStatus::Failed,
            Verdict::Declined => TaskStatus::Declined,
        },
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
    fn a_verified_task_ends_in_the_verdict_it_got() {
        for end in [TaskStatus::Unverified, TaskStatus::Partial, TaskStatus::Failed, TaskStatus::Declined] {
            let mut t = Task::new();
            t.to(TaskStatus::Running);
            assert!(t.to(TaskStatus::Verifying) && t.to(end), "{end:?}");
            assert_eq!(t.status, end);
        }
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
    fn a_task_being_verified_cannot_be_cancelled() {
        let mut t = Task::new();
        t.to(TaskStatus::Running);
        t.to(TaskStatus::Verifying);
        assert!(!t.to(TaskStatus::Cancelled), "the request ends with its verdict, not as a cancel");
    }

    #[test]
    fn a_final_state_stays_final() {
        let mut t = Task::new();
        t.to(TaskStatus::Running);
        assert!(t.to(TaskStatus::Cancelled));
        assert!(!t.to(TaskStatus::Running) && !t.to(TaskStatus::Failed));
        assert!(TaskStatus::Interrupted.is_final() && TaskStatus::Partial.is_final() && !TaskStatus::WaitingApproval.is_final());
    }

    #[test]
    fn a_cancel_is_a_cancelled_task_not_a_failed_one() {
        assert_eq!(status_of(&Err(crate::agent::CANCELLED.into()), Verdict::Failed, false), TaskStatus::Cancelled);
        assert_eq!(status_of(&Err("claude returned 503".into()), Verdict::Completed, false), TaskStatus::Failed);
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
    fn an_answer_takes_its_verdict_a_stop_cancels_and_a_failure_fails() {
        assert_eq!(status_of(&Ok("Done.".into()), Verdict::Completed, true), TaskStatus::Completed);
        assert_eq!(status_of(&Err(browse::STOPPED.into()), Verdict::Completed, false), TaskStatus::Cancelled);
        assert_eq!(status_of(&Err("I couldn't finish that in a few steps.".into()), Verdict::Completed, false), TaskStatus::Failed);
        assert_eq!(status_of(&Err("That took too long, so I stopped.".into()), Verdict::Completed, false), TaskStatus::Failed);
    }

    #[test]
    fn a_fine_sounding_answer_is_not_a_completed_task_when_its_tools_failed() {
        assert_eq!(status_of(&Ok("Done.".into()), Verdict::Failed, false), TaskStatus::Failed, "the model said done; the tools said no");
        assert_eq!(status_of(&Ok("Done.".into()), Verdict::Unverified, true), TaskStatus::Unverified);
        assert_eq!(status_of(&Ok("Done.".into()), Verdict::Partial, true), TaskStatus::Partial);
        assert_eq!(status_of(&Ok("Okay.".into()), Verdict::Declined, false), TaskStatus::Declined);
    }

    #[test]
    fn a_failure_after_an_action_is_partial_and_a_declined_one_is_declined() {
        assert_eq!(status_of(&Err("That took too long, so I stopped.".into()), Verdict::Partial, true), TaskStatus::Partial, "the action already happened");
        assert_eq!(status_of(&Err("That took too long, so I stopped.".into()), Verdict::Failed, false), TaskStatus::Failed);
        assert_eq!(status_of(&Err("You didn't allow it, so I did nothing.".into()), Verdict::Declined, false), TaskStatus::Declined);
    }
}
