//! The PC's CPU, memory and disk use right now, for "what's my CPU usage?". It reads the same counters the sidebar uses, so a question
//! like that doesn't open an app or read the screen. Read-only.

use crate::util::{self, Cpu};
use std::time::Duration;

/// How long the CPU is sampled. Its reading covers this window, so it is the use right now, not since an earlier question.
const CPU_WINDOW: Duration = Duration::from_millis(300);

/// The CPU busy fraction over `CPU_WINDOW`.
fn cpu_now() -> f32 {
    let mut cpu = Cpu::new();
    std::thread::sleep(CPU_WINDOW);
    cpu.usage()
}

/// The measure a plain question asks for: "what's my CPU usage?" gives "cpu", "how is my PC doing?" gives "all". Only questions made
/// of these words count, so a request with any other word (an action, a name, a longer sentence) returns None and goes to the model.
pub fn asked_for(said: &str) -> Option<&'static str> {
    const MEASURES: [(&str, &str); 6] = [("cpu", "cpu"), ("processor", "cpu"), ("memory", "memory"), ("ram", "memory"), ("disk", "disk"), ("storage", "disk")];
    const ALLOWED: [&str; 26] = ["what's", "whats", "what", "is", "are", "my", "the", "current", "currently", "right", "now", "how", "much", "how's",
        "hows", "of", "in", "on", "this", "pc", "computer", "a", "am", "i", "doing", "and"];
    const USE: [&str; 9] = ["usage", "use", "using", "load", "busy", "full", "percentage", "percent", "used"];
    let words: Vec<String> = said.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(String::from).collect();
    if words.is_empty() || words.len() > 8 { return None; }
    let known = |w: &str| ALLOWED.contains(&w) || USE.contains(&w) || MEASURES.iter().any(|(m, _)| *m == w);
    if !words.iter().all(|w| known(w)) { return None; }
    if !words.iter().any(|w| USE.contains(&w.as_str()) || w == "doing") { return None; }
    let mut picked: Vec<&'static str> = vec![];
    for (word, what) in MEASURES {
        if words.iter().any(|w| w == word) && !picked.contains(&what) { picked.push(what); }
    }
    match picked.as_slice() {
        [] if words.iter().any(|w| w == "pc" || w == "computer") => Some("all"),
        [] => None,
        [one] => Some(one),
        _ => Some("all"),
    }
}

/// The reading the AI asked for: "cpu", "memory" (or "ram"), "disk", or empty or "all" for all three.
pub fn read(what: &str) -> Result<String, String> {
    reading(what, cpu_now, util::memory_usage, util::disk_usage)
}

/// The reply for `what`. Each reader runs only when its number is needed.
fn reading(what: &str, cpu: impl FnOnce() -> f32, memory: impl FnOnce() -> f32, disk: impl FnOnce() -> f32) -> Result<String, String> {
    let pct = |f: f32| (f * 100.0).round() as u32;
    match what.trim().to_lowercase().as_str() {
        "" | "all" => Ok(format!("CPU {}%, memory {}%, disk {}% in use.", pct(cpu()), pct(memory()), pct(disk()))),
        "cpu" => Ok(format!("CPU {}% in use.", pct(cpu()))),
        "memory" | "ram" => Ok(format!("Memory {}% in use.", pct(memory()))),
        "disk" => Ok(format!("Disk {}% in use.", pct(disk()))),
        _ => Err(format!("I can read cpu, memory or disk, not \"{}\".", what.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never() -> f32 { panic!("this reading was not asked for") }

    #[test]
    fn one_measure_reads_only_its_own_counter() {
        assert_eq!(reading("cpu", || 0.34, never, never), Ok("CPU 34% in use.".to_string()));
        assert_eq!(reading("RAM", never, || 0.79, never), Ok("Memory 79% in use.".to_string()), "names are not case sensitive");
        assert_eq!(reading(" disk ", never, never, || 0.12), Ok("Disk 12% in use.".to_string()));
    }

    #[test]
    fn no_measure_or_all_reads_all_three() {
        let all = "CPU 34%, memory 79%, disk 12% in use.";
        assert_eq!(reading("", || 0.34, || 0.79, || 0.12), Ok(all.to_string()));
        assert_eq!(reading("all", || 0.34, || 0.79, || 0.12), Ok(all.to_string()));
    }

    #[test]
    fn a_fraction_is_rounded_to_the_nearest_percent() {
        assert_eq!(reading("cpu", || 0.345, never, never), Ok("CPU 35% in use.".to_string()));
        assert_eq!(reading("cpu", || 1.0, never, never), Ok("CPU 100% in use.".to_string()));
    }

    #[test]
    fn a_plain_question_about_the_pc_is_answered_without_the_model() {
        assert_eq!(asked_for("what's my CPU usage?"), Some("cpu"));
        assert_eq!(asked_for("What is the CPU usage right now"), Some("cpu"));
        assert_eq!(asked_for("how much memory am I using"), Some("memory"));
        assert_eq!(asked_for("how full is my disk?"), Some("disk"));
        assert_eq!(asked_for("how is my PC doing"), Some("all"));
        assert_eq!(asked_for("cpu and memory usage"), Some("all"), "two measures read both");
    }

    #[test]
    fn anything_else_goes_to_the_model() {
        assert_eq!(asked_for("open Task Manager and check the CPU usage"), None, "an action is not a plain question");
        assert_eq!(asked_for("turn the volume up"), None);
        assert_eq!(asked_for("what is the weather"), None);
        assert_eq!(asked_for("cpu"), None, "a bare word has no question in it");
        assert_eq!(asked_for("what's my CPU usage in the last hour and why is it high"), None, "a longer request");
        assert_eq!(asked_for("load the report"), None, "load is only a usage word when the rest is about the PC");
    }

    #[test]
    fn an_unknown_measure_is_refused_without_reading_anything() {
        assert!(reading("network", never, never, never).is_err());
    }
}
