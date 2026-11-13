//! Protocol eval: replies the model might send, each checked against the step it should become. It measures the reply
//! parser and the tool mapping only. It does not measure the model. The cases live in `eval/protocol-cases.json`.

use super::{parse_step, Step};
use serde_json::Value;

/// What a reply became, as text: the call's name and fields, the answer, or "Bad".
pub fn describe(step: &Step) -> String {
    match step {
        Step::Call { call, .. } => format!("{call:?}"),
        Step::Say(s) => format!("Say({s})"),
        Step::Bad(why) => format!("Bad({why})"),
    }
}

/// Every case in the file, as (reply, expected prefix of the described step). Panics on a malformed file.
pub fn cases() -> Vec<(String, String)> {
    let v: Value = serde_json::from_str(include_str!("../../eval/protocol-cases.json")).expect("protocol-cases.json is valid JSON");
    v.as_array().expect("a list of cases").iter()
        .map(|c| (c["reply"].as_str().expect("reply").to_string(), c["expect"].as_str().expect("expect").to_string()))
        .collect()
}

/// Share of cases whose reply became the expected step, as (passed, total).
pub fn score() -> (usize, usize) {
    let all = cases();
    let passed = all.iter().filter(|(reply, expect)| describe(&parse_step(reply)).starts_with(expect.as_str())).count();
    (passed, all.len())
}

/// One live request: what the user said, and the step prefixes any of which counts as right.
#[derive(Debug)]
pub struct LiveCase { pub request: String, pub expect: Vec<String>, pub contains: Option<String> }

pub fn live_cases() -> Vec<LiveCase> {
    let v: Value = serde_json::from_str(include_str!("../../eval/live-cases.json")).expect("live-cases.json is valid JSON");
    v.as_array().expect("a list of cases").iter().map(|c| LiveCase {
        request: c["request"].as_str().expect("request").to_string(),
        expect: c["expect"].as_array().expect("expect is a list").iter().map(|e| e.as_str().expect("prefix").to_string()).collect(),
        contains: c["contains"].as_str().map(str::to_string),
    }).collect()
}

/// Whether a described step is right for a case: one of the expected prefixes, and the case's word inside it when it names one.
pub fn right_for(case: &LiveCase, described: &str) -> bool {
    let prefix = case.expect.iter().any(|e| described.starts_with(e.as_str()));
    let word = case.contains.as_ref().is_none_or(|w| described.to_lowercase().contains(&w.to_lowercase()));
    prefix && word
}

/// One model reply from the subscription, with the token counts the provider reported.
#[derive(Debug, Default)]
pub struct Reply { pub text: String, pub input: u64, pub cache_create: u64, pub cache_read: u64, pub output: u64 }

/// Asks the model through the Claude subscription (the `claude` command, as the app's subscription chat does), with the
/// given system text and user text. No API key is used. The counts are the ones the provider reports, cache included.
pub fn ask_subscription(model: &str, system: &str, user: &str) -> Result<Reply, String> {
    use std::io::Write;
    let exe = crate::bridge::claude_cli::find().ok_or("Claude Code is not installed")?;
    let mut child = std::process::Command::new(exe)
        .args(["-p", "--output-format", "stream-json", "--verbose", "--model", model, "--tools", "", "--system-prompt", system,
               "--setting-sources", "", "--no-session-persistence", "--disable-slash-commands", "--strict-mcp-config"])
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
        .spawn().map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() { stdin.write_all(user.as_bytes()).map_err(|e| e.to_string())?; }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("claude exited with {}", out.status)); }
    parse_reply(&String::from_utf8_lossy(&out.stdout))
}

/// Reads the `result` line of a stream-json run: the reply text and the usage counts.
pub fn parse_reply(stream: &str) -> Result<Reply, String> {
    let result = stream.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["type"] == "result")
        .ok_or("no result line in the output")?;
    let u = &result["usage"];
    Ok(Reply {
        text: result["result"].as_str().unwrap_or("").trim().to_string(),
        input: u["input_tokens"].as_u64().unwrap_or(0),
        cache_create: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        cache_read: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
        output: u["output_tokens"].as_u64().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_case_needs_its_prefix_and_its_word() {
        let case = LiveCase { request: "x".into(), expect: vec!["Message {".into()], contains: Some("dinner".into()) };
        assert!(right_for(&case, "Message { to: \"Mum\", text: \"Dinner is at seven\", via: \"\" }"), "prefix and word, any case");
        assert!(!right_for(&case, "Message { to: \"Mum\", text: \"See you\", via: \"\" }"), "the right tool with other words is wrong");
        assert!(!right_for(&case, "Say(Dinner is at seven)"), "the word alone is not the right step");
        let open = LiveCase { request: "x".into(), expect: vec!["Open(".into()], contains: None };
        assert!(right_for(&open, "Open(\"Spotify\")"));
    }

    #[test]
    fn a_stream_result_line_gives_the_reply_and_its_cache_counts() {
        let s = "{\"type\":\"system\"}\n{\"type\":\"result\",\"result\":\"pong\",\"usage\":{\"input_tokens\":2,\"cache_creation_input_tokens\":560,\"cache_read_input_tokens\":1590,\"output_tokens\":4}}\n";
        let r = parse_reply(s).unwrap();
        assert_eq!((r.text.as_str(), r.input, r.cache_create, r.cache_read, r.output), ("pong", 2, 560, 1590, 4));
        assert!(parse_reply("{\"type\":\"system\"}").is_err(), "no result line is an error");
    }

    #[test]
    fn the_live_cases_are_well_formed() {
        let cases = live_cases();
        assert!(cases.len() >= 10, "enough live requests to mean something");
        for c in &cases {
            assert!(!c.request.trim().is_empty());
            assert!(!c.expect.is_empty(), "{:?} says what counts as right", c.request);
        }
    }

    /// Cache check: the same five requests in the old layout (clock in the system text, which changes every call) and in the
    /// new one (fixed system text, context only in the user turn). Prints the cached tokens for each call. Spends usage, so
    /// it is ignored by default. Run it with: `cargo test live_cache_layout_eval -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_cache_layout_eval() {
        assert!(crate::bridge::claude_cli::find().is_some(), "Claude Code must be installed for the cache check");
        let model = std::env::var("ZORUI_EVAL_MODEL").unwrap_or_else(|_| "haiku".into());
        let cfg = crate::conf::Config::parse(r#"{"ai":{"memory":false}}"#);
        let requests: Vec<String> = live_cases().into_iter().take(5).map(|c| c.request).collect();
        for layout in ["old", "new"] {
            let (mut read, mut create) = (0u64, 0u64);
            for (i, request) in requests.iter().enumerate() {
                let (system, user) = if layout == "old" {
                    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                    (format!("{}\n{}", crate::agent::SYSTEM, crate::agent::context_text("today", &ms.to_string(), "", None)), request.clone())
                } else {
                    (crate::agent::system_for(crate::agent::SYSTEM, &cfg), crate::agent::request_text_with(&cfg, request, ""))
                };
                let r = ask_subscription(&model, &system, &user).unwrap_or_default();
                read += r.cache_read;
                create += r.cache_create;
                eprintln!("{layout} layout, call {}: input {}, cache read {}, cache created {}, output {}", i + 1, r.input, r.cache_read, r.cache_create, r.output);
            }
            eprintln!("{layout} layout total: cache read {read}, cache created {create}");
        }
    }

    /// Spends subscription usage, so it is ignored by default. Run it with:
    /// `cargo test live_protocol_eval -- --ignored --nocapture`. Set `ZORUI_EVAL_MODEL` to pick the model (default haiku).
    #[test]
    #[ignore]
    fn live_protocol_eval() {
        assert!(crate::bridge::claude_cli::find().is_some(), "Claude Code must be installed for the live eval");
        let model = std::env::var("ZORUI_EVAL_MODEL").unwrap_or_else(|_| "haiku".into());
        let cfg = crate::conf::Config::parse(r#"{"ai":{"memory":false}}"#);
        let system = crate::agent::system_for(crate::agent::SYSTEM, &cfg);
        let cases = live_cases();
        let (mut passed, mut total_ms) = (0usize, 0u128);
        for c in &cases {
            let t = std::time::Instant::now();
            let reply = ask_subscription(&model, &system, &crate::agent::request_text_with(&cfg, &c.request, ""));
            let ms = t.elapsed().as_millis();
            total_ms += ms;
            let (ok, shown) = match &reply {
                Ok(r) => {
                    let d = describe(&parse_step(&r.text));
                    (right_for(c, &d), d)
                }
                Err(e) => (false, format!("error: {e}")),
            };
            if ok { passed += 1; }
            let shown: String = shown.chars().take(120).collect();
            eprintln!("{} {:>6} ms  {:?} -> {}", if ok { "PASS" } else { "FAIL" }, ms, c.request, shown);
        }
        eprintln!("live eval ({model}): {passed}/{} passed, {} ms average", cases.len(), total_ms / cases.len().max(1) as u128);
    }

    #[test]
    fn every_protocol_case_maps_to_its_expected_step() {
        let mut failures = vec![];
        for (reply, expect) in cases() {
            let got = describe(&parse_step(&reply));
            if !got.starts_with(&expect) { failures.push(format!("{reply} -> {got}, expected {expect}")); }
        }
        assert!(failures.is_empty(), "{} case(s) failed:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn the_cases_cover_the_tools_and_the_refusals() {
        let all = cases();
        assert!(all.len() >= 15, "the eval needs enough cases to mean something");
        assert!(all.iter().any(|(_, e)| e == "Bad("), "a refused reply is in the set");
        assert!(all.iter().any(|(_, e)| e == "Say("), "a plain answer is in the set");
    }

    #[test]
    fn the_score_counts_every_case() {
        let (passed, total) = score();
        assert_eq!(passed, total, "every protocol case passes");
    }
}
