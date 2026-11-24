//! Live mode: a hands-free voice loop in one chat. It listens (stopping on silence), the words go to the buddy, the answer is
//! spoken sentence by sentence while it is still being written (`on_text`), and once it is done and the voice is quiet it
//! listens again. The mic is off while the buddy speaks, so it never hears itself; a tap
//! on the orb (or the buddy hotkey) cuts the voice and listens at once. It ends on "stop", "bye" or "that's all", on two
//! listens in a row that heard nothing, or when the window closes.
//!
//! This file is only the state machine: `on_heard`, `on_text`, `on_answer`, `tick` and `tap` say what to do next (`Act`), and the window
//! does it (`buddy_chat.rs`), so the loop can be tested without a microphone or a voice.

use std::time::{Duration, Instant};

/// Where the loop is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase { Listening, Thinking, Speaking }

/// What the window must do next.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Start recording into the chat.
    Listen,
    /// Speak this answer (the window skips it when the buddy's own voice already says it).
    Speak(String),
    /// Cut the voice, then start recording.
    Interrupt,
    /// Leave Live mode.
    End,
}

/// How long a spoken answer has before the loop checks that the voice went quiet: the line reaches the voice queue a moment
/// after it is handed over, and an idle check before that would start listening over it.
pub const SPEAK_GRACE: Duration = Duration::from_millis(900);
/// Listens in a row that may hear nothing before the loop ends.
pub const MAX_EMPTY: u8 = 2;
/// The most characters of an answer spoken aloud; the chat shows all of it.
pub const SPEAK_CHARS: usize = 600;

/// Where the text's last whole sentence ends (a byte index just past its `.`, `!`, `?` or line break, before the space after
/// it), so it can be spoken while the rest is still being written. None when no sentence is finished yet.
pub fn sentence_end(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut end = None;
    for (i, ch) in text.char_indices() {
        let next_is_space = b.get(i + ch.len_utf8()).is_some_and(|n| n.is_ascii_whitespace());
        if ch == '\n' || (matches!(ch, '.' | '!' | '?' | '\u{3002}') && next_is_space) { end = Some(i + ch.len_utf8()); }
    }
    end.filter(|&e| text[..e].trim().chars().count() > 1)
}

/// Markdown marks read aloud as words ("asterisk"): they are left out of what is spoken.
fn speakable(s: &str) -> String { s.chars().filter(|c| !matches!(c, '*' | '#' | '`' | '_' | '>')).collect::<String>().trim().to_string() }

/// Words that end Live mode when they are all that was said.
const END_WORDS: &[&str] = &["stop", "bye", "goodbye", "bye bye", "that's all", "thats all", "that is all", "end", "end live", "stop live",
    "exit", "quit", "we're done", "were done", "i'm done", "im done", "thanks bye", "thank you bye", "ok bye", "okay bye"];

/// True when `said` only asks to end the loop ("Bye.", "that's all"), not when it is a request that contains such a word
/// ("stop the music").
pub fn is_end(said: &str) -> bool {
    let s: String = said.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '\'').collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    END_WORDS.contains(&s.as_str())
}

#[derive(Debug)]
pub struct Live {
    pub phase: Phase,
    /// What was heard last, and the last answer: the captions.
    pub heard: String,
    pub answer: String,
    empties: u8,
    since: Instant,
    /// The streamed text whose sentences were already spoken, and how many characters of this answer were spoken in all.
    said: String,
    spoken: usize,
    /// The answer is complete: once the voice is quiet the loop listens again.
    answered: bool,
}

impl Live {
    /// A new loop, listening. The window starts the first recording (`Act::Listen`).
    pub fn start() -> (Live, Act) {
        (Live { phase: Phase::Listening, heard: String::new(), answer: String::new(), empties: 0, since: Instant::now(), said: String::new(), spoken: 0, answered: false }, Act::Listen)
    }

    fn to(&mut self, p: Phase, now: Instant) { self.phase = p; self.since = now; }

    /// The recorder heard `text` (empty: nothing). A real request goes to the buddy on its own (the recorder sends it), so the
    /// loop only waits for the answer.
    pub fn on_heard(&mut self, text: &str, now: Instant) -> Option<Act> {
        let text = text.trim();
        if text.is_empty() {
            self.empties += 1;
            if self.empties >= MAX_EMPTY { return Some(Act::End); }
            self.to(Phase::Listening, now);
            return Some(Act::Listen);
        }
        self.empties = 0;
        self.heard = text.to_string();
        if is_end(text) { return Some(Act::End); }
        self.to(Phase::Thinking, now);
        (self.said, self.spoken, self.answered) = (String::new(), 0, false);
        self.answer.clear();
        None
    }

    /// The answer so far, as it streams in: each newly finished sentence is spoken now. A text that no longer starts with what
    /// was spoken is the next model call's, so it is spoken from its start.
    pub fn on_text(&mut self, text: &str, now: Instant) -> Option<Act> {
        if self.answered || self.phase == Phase::Listening { return None; }
        if !text.starts_with(&self.said) { self.said.clear(); }
        self.answer = text.to_string();
        let end = sentence_end(text).filter(|&e| e > self.said.len())?;
        let chunk = speakable(&text[self.said.len()..end]);
        self.said = text[..end].to_string();
        if chunk.is_empty() || self.spoken >= SPEAK_CHARS { return None; }
        self.spoken += chunk.chars().count();
        if self.phase != Phase::Speaking { self.to(Phase::Speaking, now); }
        Some(Act::Speak(chunk))
    }

    /// The buddy answered (an error is spoken too, so the user hears why). What was not spoken while it streamed is spoken now.
    pub fn on_answer(&mut self, result: &Result<String, String>, now: Instant) -> Option<Act> {
        let text = match result { Ok(a) | Err(a) => a.trim().to_string() };
        self.answer = text.clone();
        self.answered = true;
        // the rest after the sentences already spoken; an answer that is not the streamed text (a tool's own result) is spoken whole
        let rest = if !self.said.is_empty() && text.starts_with(self.said.trim_end()) { &text[self.said.trim_end().len()..] } else if self.spoken == 0 { &text[..] } else { "" };
        let room = SPEAK_CHARS.saturating_sub(self.spoken);
        self.to(Phase::Speaking, now);
        let rest = speakable(rest);
        if rest.is_empty() || room == 0 { return None; }
        Some(Act::Speak(crate::assist::short(&rest, room)))
    }

    /// Called every frame with whether the voice is quiet: after an answer has been spoken, listen again.
    pub fn tick(&mut self, voice_idle: bool, now: Instant) -> Option<Act> {
        if self.phase == Phase::Speaking && self.answered && voice_idle && now.saturating_duration_since(self.since) >= SPEAK_GRACE {
            self.to(Phase::Listening, now);
            return Some(Act::Listen);
        }
        None
    }

    /// A tap on the orb: while the buddy speaks it is cut off and the loop listens. At other times a tap does nothing.
    pub fn tap(&mut self, now: Instant) -> Option<Act> {
        if self.phase != Phase::Speaking { return None; }
        self.to(Phase::Listening, now);
        // the rest of a still-streaming answer is not spoken after the cut
        self.answered = true;
        Some(Act::Interrupt)
    }

    /// The label under the orb.
    pub fn label(&self) -> &'static str {
        match self.phase { Phase::Listening => "Listening", Phase::Thinking => "Thinking", Phase::Speaking => "Speaking · tap to interrupt" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_bare_goodbye_ends_the_loop() {
        for s in ["stop", "Bye.", "that's all", "That is all!", "  okay bye ", "goodbye"] { assert!(is_end(s), "{s:?}"); }
        for s in ["stop the music", "bye bye birdie song", "what's the time", "end the meeting at five", ""] { assert!(!is_end(s), "{s:?}"); }
    }

    #[test]
    fn a_turn_goes_listen_think_speak_and_listens_again_once_quiet() {
        let t0 = Instant::now();
        let (mut l, first) = Live::start();
        assert_eq!(first, Act::Listen);
        assert_eq!(l.on_heard("what's the weather", t0), None);
        assert_eq!(l.phase, Phase::Thinking);
        assert_eq!(l.on_answer(&Ok("Sunny.".into()), t0), Some(Act::Speak("Sunny.".into())));
        assert_eq!(l.phase, Phase::Speaking);
        assert_eq!(l.tick(true, t0 + Duration::from_millis(100)), None, "too soon: the line may not be queued yet");
        assert_eq!(l.tick(false, t0 + SPEAK_GRACE * 3), None, "still speaking");
        assert_eq!(l.tick(true, t0 + SPEAK_GRACE * 3), Some(Act::Listen));
        assert_eq!(l.phase, Phase::Listening);
    }

    #[test]
    fn a_tap_interrupts_only_while_speaking() {
        let t0 = Instant::now();
        let (mut l, _) = Live::start();
        assert_eq!(l.tap(t0), None, "listening: nothing to cut");
        l.on_heard("hi", t0);
        assert_eq!(l.tap(t0), None, "thinking: nothing to cut");
        l.on_answer(&Ok("Hello.".into()), t0);
        assert_eq!(l.tap(t0), Some(Act::Interrupt));
        assert_eq!(l.phase, Phase::Listening);
    }

    #[test]
    fn silence_twice_or_a_goodbye_ends_it() {
        let t0 = Instant::now();
        let (mut l, _) = Live::start();
        assert_eq!(l.on_heard("", t0), Some(Act::Listen), "one empty listen tries again");
        assert_eq!(l.on_heard("  ", t0), Some(Act::End));
        let (mut l, _) = Live::start();
        assert_eq!(l.on_heard("", t0), Some(Act::Listen));
        assert_eq!(l.on_heard("hello", t0), None, "something heard resets the count");
        l.on_answer(&Ok("Hi.".into()), t0);
        assert_eq!(l.on_heard("", t0), Some(Act::Listen));
        let (mut l, _) = Live::start();
        assert_eq!(l.on_heard("bye", t0), Some(Act::End));
    }

    #[test]
    fn whole_sentences_are_found_and_half_ones_wait() {
        assert_eq!(sentence_end("Hello there. How are"), Some(12));
        assert_eq!(sentence_end("No end yet"), None);
        assert_eq!(sentence_end("Version 2.5 is out"), None, "a dot inside a number is not an end");
        assert_eq!(sentence_end("First line\nsecond"), Some(11));
        assert_eq!(sentence_end("One. Two! Three"), Some(9));
    }

    #[test]
    fn a_streaming_answer_is_spoken_a_sentence_at_a_time_and_waits_for_the_end() {
        let t0 = Instant::now();
        let (mut l, _) = Live::start();
        l.on_heard("what's the weather", t0);
        assert_eq!(l.on_text("It is sun", t0), None, "half a sentence waits");
        assert_eq!(l.on_text("It is **sunny**. Wind is", t0), Some(Act::Speak("It is sunny.".into())), "markdown marks are not read");
        assert_eq!(l.phase, Phase::Speaking);
        assert_eq!(l.tick(true, t0 + SPEAK_GRACE * 3), None, "the answer isn't done: no listening between sentences");
        assert_eq!(l.on_text("It is **sunny**. Wind is light. Take", t0), Some(Act::Speak("Wind is light.".into())));
        assert_eq!(l.on_answer(&Ok("It is **sunny**. Wind is light. Take a hat.".into()), t0), Some(Act::Speak("Take a hat.".into())), "only the rest");
        assert_eq!(l.tick(true, t0 + SPEAK_GRACE * 3), Some(Act::Listen));
    }

    #[test]
    fn an_answer_that_was_not_streamed_is_spoken_whole_and_a_new_call_starts_over() {
        let t0 = Instant::now();
        let (mut l, _) = Live::start();
        l.on_heard("open steam", t0);
        assert_eq!(l.on_text("Let me look. ", t0), Some(Act::Speak("Let me look.".into())));
        assert_eq!(l.on_text("Opened Steam. ", t0), Some(Act::Speak("Opened Steam.".into())), "the next model call's text is its own");
        assert_eq!(l.on_answer(&Ok("Opened Steam.".into()), t0), None, "nothing left to say");
        let (mut l, _) = Live::start();
        l.on_heard("open steam", t0);
        assert_eq!(l.on_answer(&Ok("Opened Steam.".into()), t0), Some(Act::Speak("Opened Steam.".into())));
    }

    #[test]
    fn an_error_is_spoken_and_a_long_answer_is_cut_for_the_voice() {
        let t0 = Instant::now();
        let (mut l, _) = Live::start();
        assert_eq!(l.on_answer(&Err("I couldn't reach the AI.".into()), t0), Some(Act::Speak("I couldn't reach the AI.".into())));
        let long = "a".repeat(SPEAK_CHARS + 50);
        let Some(Act::Speak(s)) = l.on_answer(&Ok(long.clone()), t0) else { panic!() };
        assert_eq!(s.chars().count(), SPEAK_CHARS + 1, "cut, plus the ellipsis");
        assert_eq!(l.answer, long, "the caption keeps all of it");
    }
}
