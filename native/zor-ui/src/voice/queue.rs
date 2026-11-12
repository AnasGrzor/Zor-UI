//! The speech queue's rules, kept apart from the thread that runs them so they can be tested directly.
//! A new line cuts the one playing when it is at least as important; a less important line waits its turn.

use super::contract::{Cancel, SynthesisRequest};
use std::collections::VecDeque;

/// Ambient remarks yield to replies, and replies yield to urgent speech.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority { Ambient, Reply, Urgent }

#[derive(Debug)]
pub struct Job {
    pub priority: Priority,
    pub req: SynthesisRequest,
}

#[derive(Debug, PartialEq)]
pub enum Push {
    /// The line now playing was cancelled to make room.
    Cut,
    Queued,
    /// The queue was full of more important lines.
    Dropped,
}

pub struct Queue {
    pending: VecDeque<Job>,
    running: Option<(Priority, Cancel)>,
    max_pending: usize,
}

impl Queue {
    pub fn new(max_pending: usize) -> Queue {
        Queue { pending: VecDeque::new(), running: None, max_pending }
    }

    pub fn push(&mut self, job: Job) -> Push {
        if let Some((running, _)) = &self.running {
            if *running > job.priority {
                if self.pending.len() >= self.max_pending { return Push::Dropped; }
                self.pending.push_back(job);
                return Push::Queued;
            }
        }
        // Newest wins: anything waiting at this level or below is stale now.
        self.pending.retain(|j| j.priority > job.priority);
        let cut = match &self.running {
            Some((_, cancel)) => { cancel.cancel(); true }
            None => false,
        };
        self.pending.push_back(job);
        if cut { Push::Cut } else { Push::Queued }
    }

    /// The most important waiting line (the oldest of equal rank) starts running.
    pub fn take(&mut self) -> Option<(Job, Cancel)> {
        let mut best: Option<usize> = None;
        for (i, j) in self.pending.iter().enumerate() {
            if best.is_none_or(|b| j.priority > self.pending[b].priority) { best = Some(i); }
        }
        let job = self.pending.remove(best?)?;
        let cancel = Cancel::new();
        self.running = Some((job.priority, cancel.clone()));
        Some((job, cancel))
    }

    pub fn finish(&mut self) { self.running = None; }

    /// Silences everything: cancels the running line and drops the waiting ones.
    pub fn stop(&mut self) {
        if let Some((_, cancel)) = &self.running { cancel.cancel(); }
        self.pending.clear();
    }

    pub fn is_idle(&self) -> bool { self.running.is_none() && self.pending.is_empty() }
    pub fn pending_len(&self) -> usize { self.pending.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::contract::SpeechSettings;

    fn job(priority: Priority, text: &str) -> Job {
        Job { priority, req: SynthesisRequest { voice_id: "v".into(), model_id: None, text: text.into(), settings: SpeechSettings::default() } }
    }

    #[test]
    fn a_line_waits_when_nothing_is_playing_and_plays_in_order() {
        let mut q = Queue::new(4);
        assert_eq!(q.push(job(Priority::Reply, "a")), Push::Queued);
        let (j, _) = q.take().unwrap();
        assert_eq!(j.req.text, "a");
        q.finish();
        assert!(q.is_idle());
    }

    #[test]
    fn a_newer_reply_cuts_the_playing_one() {
        let mut q = Queue::new(4);
        q.push(job(Priority::Reply, "old"));
        let (_, cancel) = q.take().unwrap();
        assert_eq!(q.push(job(Priority::Reply, "new")), Push::Cut);
        assert!(cancel.is_cancelled(), "the playing line is told to stop");
    }

    #[test]
    fn a_less_important_line_waits_behind_the_playing_one() {
        let mut q = Queue::new(4);
        q.push(job(Priority::Reply, "reply"));
        let (_, cancel) = q.take().unwrap();
        assert_eq!(q.push(job(Priority::Ambient, "idle chatter")), Push::Queued);
        assert!(!cancel.is_cancelled(), "a remark never interrupts a reply");
    }

    #[test]
    fn a_waiting_reply_starts_before_an_older_ambient_remark() {
        let mut q = Queue::new(4);
        q.push(job(Priority::Urgent, "playing"));
        let _ = q.take();
        assert_eq!(q.push(job(Priority::Ambient, "older remark")), Push::Queued);
        assert_eq!(q.push(job(Priority::Reply, "newer reply")), Push::Queued);
        q.finish();
        assert_eq!(q.take().unwrap().0.req.text, "newer reply", "rank beats arrival order");
        q.finish();
        assert_eq!(q.take().unwrap().0.req.text, "older remark");
    }

    #[test]
    fn full_queue_drops_new_ambient_remarks() {
        let mut q = Queue::new(1);
        q.push(job(Priority::Reply, "playing"));
        let _ = q.take();
        assert_eq!(q.push(job(Priority::Ambient, "one")), Push::Queued);
        assert_eq!(q.push(job(Priority::Ambient, "two")), Push::Dropped);
        assert_eq!(q.pending_len(), 1);
    }

    #[test]
    fn stop_cancels_the_playing_line_and_clears_the_rest() {
        let mut q = Queue::new(4);
        q.push(job(Priority::Reply, "a"));
        let (_, cancel) = q.take().unwrap();
        q.push(job(Priority::Ambient, "b"));
        q.stop();
        assert!(cancel.is_cancelled());
        assert_eq!(q.pending_len(), 0);
    }
}
