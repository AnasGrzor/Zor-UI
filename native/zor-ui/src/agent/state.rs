//! The state of one buddy request. The loop decides how a request ended from its result, not from anything the AI says
//! about being finished. Only the states the loop reaches today are here; the rest (waiting for approval, verifying) come
//! with the planner and verifier.

use crate::browse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Completed,
    Failed,
    Cancelled,
}

/// How a request ended, read from the loop's result. The user stopping the browser is a cancel, not a failure.
pub fn status_of(result: &Result<String, String>) -> TaskStatus {
    match result {
        Ok(_) => TaskStatus::Completed,
        Err(e) if e == browse::STOPPED => TaskStatus::Cancelled,
        Err(_) => TaskStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_completes_a_stop_cancels_and_anything_else_fails() {
        assert_eq!(status_of(&Ok("Done.".into())), TaskStatus::Completed);
        assert_eq!(status_of(&Err(browse::STOPPED.into())), TaskStatus::Cancelled);
        assert_eq!(status_of(&Err("I couldn't finish that in a few steps.".into())), TaskStatus::Failed);
        assert_eq!(status_of(&Err("That took too long, so I stopped.".into())), TaskStatus::Failed);
    }
}
