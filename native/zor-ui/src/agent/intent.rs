//! Intent: what a request is for, and how sure the buddy is, from the words and what the context holds. No model call. A clear
//! request goes ahead. One that is clear only with an assumption goes ahead with the assumption stated. One that is unclear and
//! has nothing to point at gets one question instead, and the next request is read as the answer.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A question or a chat: no tool needed.
    Answer,
    /// Look something up on the web.
    Lookup,
    /// Do something on the PC or for the user.
    Act,
    /// Change a setting.
    Setting,
    /// A request whose object is not clear ("fix this", "make it better").
    Unclear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub kind: Kind,
    pub confidence: Confidence,
}

/// What the context holds, so an unclear request can point at something.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// A request was saved before (the task table has one).
    pub previous_request: bool,
    /// The conversation has turns in memory.
    pub conversation: bool,
    /// The window in front is a project folder.
    pub project: bool,
    /// The title of the window in front, read without acting. Empty when there is none. Something the user can see is
    /// the likeliest object of "this", so an unclear request with a window in front is assumed, not asked about.
    pub window: String,
    /// This request answers the question the buddy asked last time.
    pub answering: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead.
    Proceed,
    /// Go ahead, and say the assumption to the AI (and so to the user, in its answer).
    Assume(String),
    /// Don't act: ask this one question.
    Ask(String),
}

/// Words that say "do the thing" without saying which thing.
const VAGUE: [&str; 7] = ["fix", "improve", "better", "clean", "ready", "sort", "tidy"];
/// Words that point at something already on the table.
const REFERENCE: [&str; 5] = ["this", "that", "it", "them", "those"];
const LOOKUP: [&str; 9] = ["search", "look", "news", "weather", "price", "prices", "score", "scores", "research"];
const ACT: [&str; 14] = ["open", "play", "volume", "brightness", "click", "message", "remember", "forget", "wallpaper", "lock", "copy", "pause", "skip", "take"];
const SETTING: [&str; 4] = ["turn", "enable", "disable", "switch"];

fn words(said: &str) -> Vec<String> {
    said.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect()
}

/// What the request is and how sure the buddy is of it.
pub fn classify(said: &str, ctx: &Context) -> Assessment {
    if ctx.answering { return Assessment { kind: Kind::Answer, confidence: Confidence::High }; }
    let w = words(said);
    let has = |list: &[&str]| w.iter().any(|x| list.contains(&x.as_str()));
    if has(&VAGUE) && has(&REFERENCE) {
        // the object is unclear: a project or window in front, or something said or done before, gives it one
        let confidence = if ctx.project || !ctx.window.is_empty() || ctx.previous_request || ctx.conversation { Confidence::Medium } else { Confidence::Low };
        return Assessment { kind: Kind::Unclear, confidence };
    }
    let kind = if has(&LOOKUP) { Kind::Lookup } else if has(&ACT) { Kind::Act } else if has(&SETTING) { Kind::Setting } else { Kind::Answer };
    Assessment { kind, confidence: Confidence::High }
}

/// What to do with an assessment. A high-confidence request proceeds. A medium one proceeds with the assumption stated. A low
/// one asks one question. Approval for risky actions is not decided here: the policy still asks before those run.
pub fn decide(a: Assessment, ctx: &Context) -> Decision {
    match a.confidence {
        Confidence::High => Decision::Proceed,
        Confidence::Medium => {
            let what = if ctx.project { "the active project".to_string() }
                else if !ctx.window.is_empty() { format!("the window in front, \"{}\"", ctx.window) }
                else { "the last thing we talked about".to_string() };
            Decision::Assume(format!("Taking \"this\" to mean {what}."))
        }
        Confidence::Low => Decision::Ask("Which one do you mean? Tell me what should change, and I'll take it from there.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Context { Context::default() }

    #[test]
    fn a_clear_request_is_high_confidence_and_its_kind_is_read_from_the_words() {
        let c = |s: &str| classify(s, &none());
        assert_eq!(c("what time is it"), Assessment { kind: Kind::Answer, confidence: Confidence::High });
        assert_eq!(c("search for the weather in Oslo").kind, Kind::Lookup);
        assert_eq!(c("open Spotify").kind, Kind::Act);
        assert_eq!(c("turn the volume up").kind, Kind::Act, "the volume word wins over the turn word");
        assert_eq!(c("turn wifi on").kind, Kind::Setting);
        assert_eq!(c("turn wifi on").confidence, Confidence::High);
    }

    #[test]
    fn an_unclear_request_with_nothing_to_point_at_is_low_confidence() {
        assert_eq!(classify("fix this", &none()), Assessment { kind: Kind::Unclear, confidence: Confidence::Low });
        assert_eq!(classify("make it better", &none()), Assessment { kind: Kind::Unclear, confidence: Confidence::Low });
    }

    #[test]
    fn an_unclear_request_with_something_to_point_at_is_medium_confidence() {
        let before = Context { previous_request: true, ..Context::default() };
        assert_eq!(classify("fix this", &before).confidence, Confidence::Medium, "something was said or done before");
        let project = Context { project: true, ..Context::default() };
        assert_eq!(classify("clean up this", &project).confidence, Confidence::Medium, "the project in front is the object");
    }

    #[test]
    fn an_answer_to_a_question_the_buddy_asked_is_taken_as_the_answer() {
        let answering = Context { answering: true, ..Context::default() };
        assert_eq!(classify("fix it", &answering), Assessment { kind: Kind::Answer, confidence: Confidence::High }, "the reply resolves the question");
    }

    /// Investigate first: an unclear request with a window in front takes that window as the object, and asks nothing.
    #[test]
    fn an_unclear_request_with_a_window_in_front_assumes_the_window() {
        let ctx = Context { window: "notes.txt - Notepad".into(), ..Context::default() };
        let a = classify("fix this", &ctx);
        assert_eq!(a.confidence, Confidence::Medium);
        match decide(a, &ctx) {
            Decision::Assume(s) => assert!(s.contains("notes.txt - Notepad"), "{s}"),
            other => panic!("{other:?}"),
        }
    }

    /// Requests that must not ask a question, each with the context it comes with.
    #[test]
    fn requests_that_are_clear_or_have_an_object_do_not_ask() {
        let none = Context::default();
        let window = Context { window: "main.rs - zor-ui".into(), ..Context::default() };
        for (said, ctx) in [
            ("what time is it", &none),
            ("open Spotify", &none),
            ("search for news about the election", &none),
            ("turn wifi off", &none),
            ("fix the bug in main.rs", &none),
            ("make it better", &window),
            ("fix this", &window),
        ] {
            assert!(!matches!(decide(classify(said, ctx), ctx), Decision::Ask(_)), "should not ask: {said}");
        }
    }

    /// The one case that still asks: an unclear request with nothing on the table.
    #[test]
    fn an_unclear_request_with_nothing_to_point_at_still_asks() {
        assert!(matches!(decide(classify("make it better", &Context::default()), &Context::default()), Decision::Ask(_)));
    }

    #[test]
    fn high_proceeds_medium_assumes_and_low_asks_one_question() {
        assert_eq!(decide(Assessment { kind: Kind::Act, confidence: Confidence::High }, &none()), Decision::Proceed);
        let project = Context { project: true, ..Context::default() };
        match decide(Assessment { kind: Kind::Unclear, confidence: Confidence::Medium }, &project) {
            Decision::Assume(s) => assert!(s.contains("the active project"), "{s}"),
            other => panic!("{other:?}"),
        }
        match decide(Assessment { kind: Kind::Unclear, confidence: Confidence::Low }, &none()) {
            Decision::Ask(q) => assert!(q.ends_with('?') || q.contains("Tell me"), "{q}"),
            other => panic!("{other:?}"),
        }
    }
}
