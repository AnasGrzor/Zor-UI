//! Failure diagnosis (M6): a tool's error becomes one hint for the model, saying what kind of failure it was and what to try next.
//! The hint is added to the error the model reads, and the user never sees it. It matches the wording the tools already use,
//! so a new tool error with unfamiliar wording gets no hint and goes back as it was.

/// The hint for an error, or None when its wording is not one of the known kinds.
pub fn hint(err: &str) -> Option<&'static str> {
    let e = err.to_lowercase();
    if e.contains("turned off in settings") {
        return Some("The user turned this off in Settings. Tell them; do not retry it or use another tool for it.");
    }
    if e.contains("can't run that safely") || e.contains("won't keep") {
        return Some("Do not retry. Tell the user what to do themselves.");
    }
    if e.contains("isn't there") || e.contains("can't find") || e.starts_with("no ") || e.contains("no file") || e.contains("not installed") {
        return Some("Nothing matched. Try other words, or find_app or find_file, before you answer.");
    }
    if e.contains("took too long") || e.contains("didn't answer") || e.contains("didn't confirm") {
        return Some("The PC did not answer in time. One more try is reasonable; then say it did not work.");
    }
    if e.contains("takes ") || e.contains("unknown ") || e.contains("only ") || e.contains("must be") {
        return Some("The argument was not one the tool accepts. Use one of the values the tool lists.");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_known_kind_of_failure_gets_its_hint() {
        assert!(hint("Opening apps is turned off in Settings.").unwrap().contains("Tell them"));
        assert!(hint("I can't run that safely.").unwrap().starts_with("Do not retry"));
        assert!(hint("That file isn't there.").unwrap().contains("find_file"));
        assert!(hint("No app called Nope.").unwrap().contains("find_app"));
        assert!(hint("Windows didn't answer.").unwrap().contains("did not answer"));
        assert!(hint("Volume takes up, down, mute, unmute or 0 to 100, not \"loud\".").unwrap().contains("values"));
    }

    #[test]
    fn an_unfamiliar_error_gets_no_hint_and_goes_back_as_it_was() {
        assert_eq!(hint("Windows couldn't open the browser."), None);
        assert_eq!(hint("claude exited with exit status: 1"), None);
    }
}
