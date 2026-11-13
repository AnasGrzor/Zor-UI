//! Antigravity plan usage for the island. agy keeps its quota on Google's side and writes no file, so this runs
//! `agy -p /usage --output-format json` (the same call Orca makes) and stores the windows as
//! agents/antigravity/usage.json, the format agent_hook writes for Claude and Codex.

use super::agent_hook::write;
use super::common::hidden;
use super::football::parse_iso;
use super::sub_cli::find;
use serde_json::{json, Map, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// agy older than this sends `/usage` to the model as a prompt and spends quota on it.
const MIN_VERSION: (u64, u64, u64) = (1, 1, 11);

static BUSY: AtomicBool = AtomicBool::new(false);
/// Set once agy answered `/usage` as a prompt. Orca's guard: never ask again this run.
static PROMPTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, PartialEq)]
pub(super) enum Parsed {
    Usage(Map<String, Value>),
    Prompted,
    Nothing,
}

/// Refreshes agents/antigravity/usage.json under `dir`. Returns at once when a refresh is already running.
pub(super) fn refresh(dir: &Path) {
    if PROMPTED.load(Ordering::SeqCst) || BUSY.swap(true, Ordering::SeqCst) { return; }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let Some(exe) = find("agy") else { BUSY.store(false, Ordering::SeqCst); return };
    let version = run(&exe, &["--version"], Duration::from_secs(5)).unwrap_or_default();
    if version_of(&version).is_none_or(|v| v < MIN_VERSION) { BUSY.store(false, Ordering::SeqCst); return; }
    let out = run(&exe, &["-p", "/usage", "--output-format", "json", "--print-timeout", "20s"], Duration::from_secs(30));
    match parse(&out.unwrap_or_default(), now) {
        Parsed::Usage(w) => { let _ = write(&dir.join("antigravity").join("usage.json"), &json!({ "windows": w, "model": "", "at": now })); }
        Parsed::Prompted => PROMPTED.store(true, Ordering::SeqCst),
        Parsed::Nothing => {}
    }
    BUSY.store(false, Ordering::SeqCst);
}

/// Runs agy with a time limit and returns its stdout, or None when it fails or runs past `wait`.
fn run(exe: &PathBuf, args: &[&str], wait: Duration) -> Option<String> {
    let mut child = hidden(exe).args(args).current_dir(std::env::temp_dir()).env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.by_ref().take(512 * 1024).read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    match rx.recv_timeout(wait) {
        Ok(s) => child.wait().ok().filter(|st| st.success()).map(|_| s),
        Err(_) => { let _ = child.kill(); let _ = child.wait(); None }
    }
}

/// "1.5.6" -> (1, 5, 6). None when the text has no version in it.
fn version_of(text: &str) -> Option<(u64, u64, u64)> {
    let nums: Vec<u64> = text.trim().split('.').take(3).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    (nums.len() == 3).then(|| (nums[0], nums[1], nums[2]))
}

/// Reads agy's JSON answer. Output is one JSON object per line; the first line that parses and is a usage
/// answer or a quota error wins.
pub(super) fn parse(stdout: &str, now: f64) -> Parsed {
    let Some(v) = stdout.lines().filter(|l| l.trim_start().starts_with('{')).find_map(|l| serde_json::from_str::<Value>(l).ok()) else {
        return Parsed::Nothing;
    };
    if v["status"] == "SUCCESS" && v["command"]["name"] == "usage" {
        return usage_windows(&v["command"]["data"]["groups"]).map_or(Parsed::Nothing, Parsed::Usage);
    }
    if v["status"] == "ERROR" {
        // "Individual quota reached. ... Resets in 160h1m17s." means the account is at 100%.
        let err = v["error"].as_str().unwrap_or("");
        if let (true, Some(secs)) = (err.contains("quota reached"), err.split_once("Resets in ").and_then(|(_, rest)| duration_secs(rest))) {
            let key = if secs > 5.0 * 3600.0 { "seven_day" } else { "five_hour" };
            return Parsed::Usage(Map::from_iter([(key.to_string(), json!({ "used_percentage": 100.0, "resets_at": now + secs }))]));
        }
        return Parsed::Nothing;
    }
    // A SUCCESS answer with no usage command: agy took `/usage` for a prompt.
    if v["status"] == "SUCCESS" && (v["num_turns"].as_f64().unwrap_or(0.0) > 0.0 || v["conversation_id"].is_string()) {
        return Parsed::Prompted;
    }
    Parsed::Nothing
}

/// The worst bucket per window: the highest used percentage, with its reset time.
fn usage_windows(groups: &Value) -> Option<Map<String, Value>> {
    let mut best: Map<String, Value> = Map::new();
    let mut best_used: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for g in groups.as_array()? {
        for b in g["buckets"].as_array().into_iter().flatten() {
            if b["disabled"] == true { continue; }
            let key = match b["window"].as_str() { Some("5h") => "five_hour", Some("weekly") => "seven_day", _ => continue };
            let Some(rf) = b["remaining_fraction"].as_f64().filter(|r| r.is_finite()) else { continue };
            let used = ((1.0 - rf) * 100.0).round().clamp(0.0, 100.0);
            if best_used.get(key).is_some_and(|&u| u >= used) { continue; }
            let resets_at = b["reset_time"].as_str().and_then(parse_iso).unwrap_or(0) as f64;
            best_used.insert(key.to_string(), used);
            best.insert(key.to_string(), json!({ "used_percentage": used, "resets_at": resets_at }));
        }
    }
    (!best.is_empty()).then_some(best)
}

/// Seconds in a duration like "160h1m17s" (the text stops at the first character that is not part of it).
fn duration_secs(text: &str) -> Option<f64> {
    let (mut total, mut n, mut seen) = (0.0, 0.0, false);
    for c in text.chars() {
        match c {
            '0'..='9' => { n = n * 10.0 + (c as u8 - b'0') as f64; seen = true; }
            'h' | 'm' | 's' if seen => {
                total += n * match c { 'h' => 3600.0, 'm' => 60.0, _ => 1.0 };
                n = 0.0;
                seen = false;
            }
            _ => break,
        }
    }
    (total > 0.0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_700_000_000.0;

    #[test]
    fn success_keeps_the_worst_bucket_per_window_and_skips_disabled() {
        let s = r#"{"status":"SUCCESS","command":{"name":"usage","data":{"groups":[
            {"name":"A","buckets":[{"id":"1","window":"5h","remaining_fraction":0.6,"reset_time":"2026-11-12T10:00:00Z"},
                                  {"id":"2","window":"weekly","remaining_fraction":0.9,"reset_time":"2026-11-15T00:00:00Z","disabled":true}]},
            {"name":"B","buckets":[{"id":"3","window":"5h","remaining_fraction":0.25,"reset_time":"2026-11-12T11:30:00+01:00"},
                                  {"id":"4","window":"weekly","remaining_fraction":0.5,"reset_time":"2026-11-15T00:00:00Z"}]}]}}}"#;
        // agy prints one JSON object per line, so the sample is joined onto one line.
        let Parsed::Usage(w) = parse(&s.replace('\n', ""), NOW) else { panic!("expected usage") };
        assert_eq!(w["five_hour"]["used_percentage"], 75.0);
        assert_eq!(w["five_hour"]["resets_at"], parse_iso("2026-11-12T11:30:00+01:00").unwrap() as f64);
        assert_eq!(w["seven_day"]["used_percentage"], 50.0, "the disabled bucket is skipped");
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn quota_error_reads_as_full_with_the_reset_time() {
        let s = r#"{"conversation_id":"x","status":"ERROR","response":"","error":"Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 160h1m17s.","num_turns":1}"#;
        let Parsed::Usage(w) = parse(s, NOW) else { panic!("expected usage") };
        assert_eq!(w.len(), 1);
        assert_eq!(w["seven_day"]["used_percentage"], 100.0);
        assert_eq!(w["seven_day"]["resets_at"], NOW + 160.0 * 3600.0 + 60.0 + 17.0);
    }

    #[test]
    fn short_quota_reset_goes_to_the_five_hour_window() {
        let s = r#"{"status":"ERROR","error":"Individual quota reached. Resets in 2h5m.","num_turns":1}"#;
        let Parsed::Usage(w) = parse(s, NOW) else { panic!("expected usage") };
        assert_eq!(w["five_hour"]["resets_at"], NOW + 2.0 * 3600.0 + 5.0 * 60.0);
    }

    #[test]
    fn a_prompt_style_answer_is_prompted() {
        let s = r#"{"conversation_id":"c","status":"SUCCESS","response":"Sure","num_turns":1}"#;
        assert_eq!(parse(s, NOW), Parsed::Prompted);
    }

    #[test]
    fn junk_and_other_errors_are_nothing() {
        assert_eq!(parse("", NOW), Parsed::Nothing);
        assert_eq!(parse("not json\n", NOW), Parsed::Nothing);
        assert_eq!(parse(r#"{"status":"ERROR","error":"Not logged in","num_turns":0}"#, NOW), Parsed::Nothing);
    }

    #[test]
    fn version_must_be_1_1_11_or_newer() {
        assert!(version_of("1.1.10").unwrap() < MIN_VERSION);
        assert!(version_of("1.1.11").unwrap() >= MIN_VERSION);
        assert!(version_of("1.5.6\n").unwrap() >= MIN_VERSION);
        assert_eq!(version_of("agy"), None);
    }
}
