//! Custom commands from Settings (`buddies.commands`), one entry each: "game time=play gaming playlist on spotify; open discord;
//! open last game". When a request holds a command's words, its steps run in order, each as if the user had said it.

/// One "words=step; step" entry as (words, steps). None when either side is empty.
pub fn parse_command(entry: &str) -> Option<(String, Vec<String>)> {
    let (words, steps) = entry.split_once('=')?;
    let steps: Vec<String> = steps.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let words = words.trim();
    (!words_of(words).trim().is_empty() && !steps.is_empty()).then(|| (words.to_string(), steps))
}

/// The entry the Settings form saves, or None when the words or steps are empty or the words hold "=".
pub fn format_command(words: &str, steps: &str) -> Option<String> {
    let entry = format!("{}={}", words.trim(), steps.trim());
    (!words.contains('=')).then_some(entry).filter(|e| parse_command(e).is_some())
}

/// `list` with `entry` saved: in the place of `original` when editing, else at the end. Another command with the same words is
/// dropped, so a phrase never has two commands.
pub fn with_command(list: &[String], original: Option<&str>, entry: String) -> Vec<String> {
    let key = |e: &str| parse_command(e).map(|c| words_of(&c.0));
    let new_key = key(&entry);
    let at = original.and_then(|o| list.iter().position(|e| e == o));
    let (mut out, mut slot) = (Vec::with_capacity(list.len() + 1), None);
    for (i, e) in list.iter().enumerate() {
        if Some(i) == at { slot = Some(out.len()); continue; }
        if new_key.is_some() && key(e) == new_key { continue; }
        out.push(e.clone());
    }
    out.insert(slot.unwrap_or(out.len()), entry);
    out
}

/// Lowercase words between single spaces, padded: "Hey, it's game time!" -> " hey it s game time ".
fn words_of(s: &str) -> String {
    format!(" {} ", s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
}

/// The steps of the command whose words are in `said` as whole words. When several match, the longest words win.
pub fn matching(entries: &[String], said: &str) -> Option<Vec<String>> {
    let s = words_of(said);
    entries.iter().filter_map(|e| parse_command(e)).filter(|(w, _)| s.contains(&words_of(w))).max_by_key(|(w, _)| words_of(w).len()).map(|(_, steps)| steps)
}

/// Runs each step with `run`, in order. A failed step doesn't stop the rest; its error is said with the step. Ok when one worked.
pub fn run_steps(steps: &[String], mut run: impl FnMut(&str) -> Result<String, String>) -> Result<String, String> {
    let (mut said, mut any) = (vec![], false);
    for s in steps {
        match run(s) {
            Ok(r) => { any = true; said.push(r) }
            Err(e) => said.push(format!("\"{s}\" failed: {e}")),
        }
    }
    if any { Ok(said.join(" ")) } else { Err(said.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "game time=play gaming playlist on spotify; open discord; open last game";

    #[test]
    fn an_entry_splits_into_words_and_steps() {
        assert_eq!(parse_command(GAME), Some(("game time".into(), vec!["play gaming playlist on spotify".into(), "open discord".into(), "open last game".into()])));
        assert_eq!(parse_command("game time= ; "), None, "no steps");
        assert_eq!(parse_command(" =open discord"), None, "no words");
        assert_eq!(parse_command("open discord"), None, "no =");
        assert_eq!(format_command(" game time ", " open discord "), Some("game time=open discord".into()));
        assert_eq!(format_command("a=b", "open discord"), None, "= in the words would split wrong");
        assert_eq!(format_command("game time", ""), None);
    }

    #[test]
    fn a_command_matches_on_whole_words_anywhere_in_the_request() {
        let all = vec![GAME.to_string(), "time=open clock".to_string()];
        let steps = |s| matching(&all, s).map(|v| v[0].clone());
        assert_eq!(steps("Hey buddy, it's game time!").as_deref(), Some("play gaming playlist on spotify"), "the longest match wins");
        assert_eq!(steps("what time is it").as_deref(), Some("open clock"));
        assert_eq!(steps("endgame timer"), None, "parts of words don't count");
        assert_eq!(matching(&[], "game time"), None);
    }

    #[test]
    fn saving_replaces_the_edited_command_in_place_and_drops_a_duplicate_phrase() {
        let list: Vec<String> = ["a=open x", "game time=open y", "b=open z"].map(String::from).to_vec();
        assert_eq!(with_command(&list, Some("game time=open y"), "Game Time=open w".into()), ["a=open x", "Game Time=open w", "b=open z"]);
        assert_eq!(with_command(&list, None, "c=open v".into()), ["a=open x", "game time=open y", "b=open z", "c=open v"]);
        assert_eq!(with_command(&list, None, "GAME time=open v".into()), ["a=open x", "b=open z", "GAME time=open v"], "same words, new command");
    }

    #[test]
    fn every_step_runs_and_a_failure_is_reported_with_its_step() {
        let steps: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let mut ran = vec![];
        let r = run_steps(&steps, |s| { ran.push(s.to_string()); if s == "b" { Err("no".into()) } else { Ok(format!("{s} done.")) } });
        assert_eq!(ran, ["a", "b", "c"], "a failed step doesn't stop the rest");
        assert_eq!(r, Ok("a done. \"b\" failed: no c done.".into()));
        assert!(run_steps(&steps, |_| Err("x".into())).is_err(), "nothing worked");
    }
}
