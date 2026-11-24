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

/// The most text `Queue::append` lets wait, for every caller: about a minute of speech.
pub const APPEND_CHARS: usize = 1000;

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
    /// Lines taken and not finished: the one playing and the one being made ahead of it.
    running: Vec<(Priority, Cancel)>,
    max_pending: usize,
}

impl Queue {
    pub fn new(max_pending: usize) -> Queue {
        Queue { pending: VecDeque::new(), running: Vec::new(), max_pending }
    }

    pub fn push(&mut self, job: Job) -> Push {
        if let Some(running) = self.running.iter().map(|(p, _)| *p).max() {
            if running > job.priority {
                if self.pending.len() >= self.max_pending { return Push::Dropped; }
                self.pending.push_back(job);
                return Push::Queued;
            }
        }
        // Newest wins: anything waiting at this level or below is stale now.
        self.pending.retain(|j| j.priority > job.priority);
        // every running line ranks at most this one here, so all of them stop
        let cut = !self.running.is_empty();
        for (_, cancel) in &self.running { cancel.cancel(); }
        self.pending.push_back(job);
        if cut { Push::Cut } else { Push::Queued }
    }

    /// Queues `job` after the line playing and the lines waiting, and cuts nothing, so the sentences of one
    /// streamed answer play in order. A line that would take the waiting text past `APPEND_CHARS` is dropped.
    pub fn append(&mut self, job: Job) -> Push {
        let waiting: usize = self.pending.iter().map(|j| j.req.text.chars().count()).sum();
        if waiting + job.req.text.chars().count() > APPEND_CHARS { return Push::Dropped; }
        self.pending.push_back(job);
        Push::Queued
    }

    /// The most important waiting line (the oldest of equal rank) starts running.
    pub fn take(&mut self) -> Option<(Job, Cancel)> {
        let mut best: Option<usize> = None;
        for (i, j) in self.pending.iter().enumerate() {
            if best.is_none_or(|b| j.priority > self.pending[b].priority) { best = Some(i); }
        }
        let job = self.pending.remove(best?)?;
        let cancel = Cancel::new();
        self.running.push((job.priority, cancel.clone()));
        Some((job, cancel))
    }

    /// The line taken with `cancel` is done (played, failed or cut).
    pub fn finish(&mut self, cancel: &Cancel) { self.running.retain(|(_, c)| !c.same(cancel)); }

    /// Silences everything: cancels the running lines and drops the waiting ones.
    pub fn stop(&mut self) {
        for (_, cancel) in &self.running { cancel.cancel(); }
        self.pending.clear();
    }

    pub fn is_idle(&self) -> bool { self.running.is_empty() && self.pending.is_empty() }
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
        let (j, c) = q.take().unwrap();
        assert_eq!(j.req.text, "a");
        q.finish(&c);
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
        let (_, c) = q.take().unwrap();
        assert_eq!(q.push(job(Priority::Ambient, "older remark")), Push::Queued);
        assert_eq!(q.push(job(Priority::Reply, "newer reply")), Push::Queued);
        q.finish(&c);
        let (j, c) = q.take().unwrap();
        assert_eq!(j.req.text, "newer reply", "rank beats arrival order");
        q.finish(&c);
        assert_eq!(q.take().unwrap().0.req.text, "older remark");
    }

    #[test]
    fn appended_lines_stop_at_the_central_cap() {
        let mut q = Queue::new(4);
        let line = "x".repeat(APPEND_CHARS / 2);
        assert_eq!(q.append(job(Priority::Reply, &line)), Push::Queued);
        assert_eq!(q.append(job(Priority::Reply, &line)), Push::Queued, "exactly at the cap still fits");
        assert_eq!(q.append(job(Priority::Reply, "one more")), Push::Dropped);
        assert_eq!(q.pending_len(), 2);
    }

    #[test]
    fn a_cut_stops_the_playing_line_and_the_one_made_ahead() {
        let mut q = Queue::new(4);
        q.append(job(Priority::Reply, "playing."));
        q.append(job(Priority::Reply, "made ahead."));
        let (_, playing) = q.take().unwrap();
        let (_, ahead) = q.take().unwrap();
        q.finish(&ahead);
        assert!(!q.is_idle(), "the playing line still runs after the other one finishes");
        let (_, ahead) = { q.append(job(Priority::Reply, "next.")); q.take().unwrap() };
        assert_eq!(q.push(job(Priority::Reply, "new question")), Push::Cut);
        assert!(playing.is_cancelled() && ahead.is_cancelled());
        q.finish(&playing);
        q.finish(&ahead);
        assert_eq!(q.take().unwrap().0.req.text, "new question");
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
