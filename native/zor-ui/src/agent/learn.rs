//! Facts the buddy keeps from what the user says, without being asked to remember: "my sister is called Lina", "I prefer short
//! answers". Fixed keyword rules, no model call. Only the PC's own conversation is read (never a remote chat, where someone else may
//! be talking), and only when Settings allows it (`ai.learn`). A kept fact is marked heard (`memory::SOURCE_HEARD`), goes through
//! the same secret check and duplicate merge as a remembered one, and is listed in Settings, where it can be deleted.
//! shortcut: statements only, picked by how they start; a fact said any other way is missed. Upgrade to a model pass if too few are kept.

use super::memory;

/// How a statement must start to count as a fact about the user, after any lead-in ("btw", "also").
const STARTS: &[&str] = &[
    "my name is ", "call me ", "my favourite ", "my favorite ", "my birthday is ",
    "i prefer ", "i like ", "i love ", "i hate ", "i don't like ", "i dont like ", "i always ", "i never ", "i usually ",
    "i live in ", "i work at ", "i work as ", "i work on ", "i'm allergic to ", "i am allergic to ",
];
/// A "my ..." statement that names something: "my sister is called Lina", "my dog's name is Rex".
const NAMES: &[&str] = &[" is called ", " are called ", " is named ", " name is "];
/// Words said before a fact that are not part of it.
const LEAD_INS: &[&str] = &["btw ", "by the way ", "fyi ", "also ", "just so you know ", "for the record ", "oh and ", "and "];
/// Words that make a statement about the conversation, not a fact: "I never said that", "I always forget".
const NOT_FACT: &[&str] = &["remember", "forget", "said", "say", "meant", "mean", "asked", "it", "this", "that", "these", "those", "here"];
/// Facts shorter than this say too little ("I like cats" is kept, "I like it" is not); longer ones are a story, not a fact.
const MIN_WORDS: usize = 3;
const MAX_WORDS: usize = 25;

/// The facts in what the user said, each a sentence as they said it, without its end mark. Empty for questions, requests and chat.
pub fn facts(said: &str) -> Vec<String> {
    said.split(['.', '!', ';', '\n']).filter_map(|s| fact(s.trim())).collect()
}

fn fact(sentence: &str) -> Option<String> {
    if sentence.contains('?') { return None; }
    let mut rest = sentence.trim();
    while let Some(l) = LEAD_INS.iter().find(|l| rest.get(..l.len()).is_some_and(|p| p.eq_ignore_ascii_case(l))) { rest = rest[l.len()..].trim_start(); }
    let s = rest.to_lowercase();
    let words: Vec<&str> = s.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    if words.len() < MIN_WORDS || words.len() > MAX_WORDS || words.iter().any(|w| NOT_FACT.contains(w)) { return None; }
    let named = s.starts_with("my ") && NAMES.iter().any(|n| s.contains(n));
    if !named && !STARTS.iter().any(|p| s.starts_with(p)) { return None; }
    Some(rest.to_string())
}

/// Keeps the facts in `said`, each as a preference when it says how to do things, else a plain fact.
pub fn keep(said: &str) {
    for f in facts(said) { memory::heard(&f, memory::kind_to_save(&f, None)); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_about_the_user_are_kept_as_said() {
        assert_eq!(facts("my sister is called Lina"), ["my sister is called Lina"]);
        assert_eq!(facts("btw I prefer short answers."), ["I prefer short answers"]);
        assert_eq!(facts("I live in Oslo. What's the weather?"), ["I live in Oslo"]);
        assert_eq!(facts("My dog's name is Rex"), ["My dog's name is Rex"]);
        assert_eq!(facts("I never want emojis in replies"), ["I never want emojis in replies"]);
        assert_eq!(facts("call me Sam please"), ["call me Sam please"]);
    }

    #[test]
    fn requests_questions_and_chat_are_not_facts() {
        for said in [
            "open spotify", "what is my name?", "I like it", "I never said that", "remember my sister is Lina",
            "my screen is broken", "I'd like to open chrome", "is my sister called Lina", "Hi", "I meant on my machine", "Yes",
            "I like cats and dogs and birds and fish and horses and rabbits and mice and rats and snakes and lizards and frogs and ducks too",
        ] {
            assert!(facts(said).is_empty(), "{said:?} gave {:?}", facts(said));
        }
    }

    #[test]
    fn a_secret_said_as_a_fact_is_refused_by_the_memory() {
        assert_eq!(facts("my password is called hunter2"), ["my password is called hunter2"], "the rules keep it");
        assert!(memory::is_sensitive("my password is called hunter2"), "the memory refuses it");
    }

    #[test]
    fn the_logged_real_requests_give_no_facts() {
        // the distinct requests in buddy.db on 2026-11-19: none states a fact about the user
        for said in ["But I", "Let's what if I", "Open Discord and", "What's the current state of zor ui", "I meant on my machine", "Take over", "Tell me"] {
            assert!(facts(said).is_empty(), "{said:?}");
        }
    }
}
