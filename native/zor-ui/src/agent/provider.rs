//! Normalized facts about the models the buddy uses (M7): what a model can do, whether its provider can be used now, and the tokens a
//! reply used. The adapters in `bridge/web.rs` turn each provider's stream into the same events; this file reads the facts out of
//! them, so the agent loop never looks at a provider's wire format.

use serde_json::{json, Value};

/// What a model can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Can be sent pictures (the bridge's `can_see` rule, passed in).
    pub vision: bool,
    /// Can take native tool calls. Wired for Anthropic only (`agent::native`); the other providers use the JSON protocol.
    pub native_tools: bool,
}

pub fn capabilities(provider: &str, vision: bool) -> Capabilities {
    Capabilities { vision, native_tools: provider == "anthropic" }
}

/// Providers whose key is saved in Settings. Any other provider (ollama, a subscription command line, an unknown name) needs no
/// saved key here; its own tool reports a failure when it cannot run.
const KEYED: [&str; 5] = ["gemini", "openai", "anthropic", "mistral", "openrouter"];

/// Whether a provider can be used now. A keyed provider is usable when `has_key` says its key is saved.
pub fn usable(provider: &str, has_key: impl Fn(&str) -> bool) -> bool {
    if KEYED.contains(&provider) { has_key(provider) } else { true }
}

/// Tokens a reply used, as the provider reported them. Zero means the provider did not say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage { pub input: u64, pub output: u64 }

/// The tokens used so far by the request being answered: each model call's usage added together. `reset` starts a request.
static REQUEST: std::sync::Mutex<Usage> = std::sync::Mutex::new(Usage { input: 0, output: 0 });

pub fn reset() { *REQUEST.lock().unwrap_or_else(|e| e.into_inner()) = Usage::default(); }

/// Adds one model call's usage to the request's total.
pub fn add(u: Usage) {
    let mut t = REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    t.input += u.input;
    t.output += u.output;
}

/// The request's total so far.
pub fn request_total() -> Usage { *REQUEST.lock().unwrap_or_else(|e| e.into_inner()) }

/// Every prompt token an Anthropic usage object counts: the uncached input plus cache writes and cache reads. None when the
/// object has no input count.
fn anthropic_input(u: &Value) -> Option<u64> {
    let base = u["input_tokens"].as_u64()?;
    Some(base + u["cache_creation_input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0))
}

/// The usage event for one parsed stream event of `provider`, or None when the event carries no counts. Providers report running
/// counts, so the last event that carries one wins (`assist::fold_turn`).
pub fn usage_event(provider: &str, ev: &Value) -> Option<Value> {
    let (input, output) = match provider {
        // Anthropic reports cached prompt tokens apart from input_tokens; input here is every prompt token, cached or not
        "anthropic" => (anthropic_input(&ev["message"]["usage"]), ev["usage"]["output_tokens"].as_u64()),
        // the Claude subscription's final result line has the same Anthropic counts
        "claude-cli" if ev["type"] == "result" => (anthropic_input(&ev["usage"]), ev["usage"]["output_tokens"].as_u64()),
        "claude-cli" => (None, None),
        "gemini" => (ev["usageMetadata"]["promptTokenCount"].as_u64(), ev["usageMetadata"]["candidatesTokenCount"].as_u64()),
        _ => (ev["usage"]["prompt_tokens"].as_u64(), ev["usage"]["completion_tokens"].as_u64()),
    };
    (input.is_some() || output.is_some()).then(|| json!({ "usage": { "input": input, "output": output } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_anthropic_takes_native_tools_and_vision_is_passed_through() {
        assert!(capabilities("anthropic", true).native_tools);
        assert!(!capabilities("openai", true).native_tools);
        assert!(!capabilities("gemini", false).native_tools);
        assert!(capabilities("gemini", true).vision);
    }

    #[test]
    fn a_keyed_provider_is_usable_only_with_its_key_and_others_always() {
        let saved = |p: &str| p == "anthropic";
        assert!(usable("anthropic", saved));
        assert!(!usable("openai", saved), "no key saved");
        assert!(usable("ollama", |_| false), "no key needed");
        assert!(usable("claude-cli", |_| false), "a subscription command line needs no saved key");
    }

    #[test]
    fn each_provider_reports_its_counts_in_the_same_shape() {
        let a = usage_event("anthropic", &json!({ "type": "message_start", "message": { "usage": { "input_tokens": 12, "output_tokens": 1 } } }));
        assert_eq!(a, Some(json!({ "usage": { "input": 12, "output": null } })), "the start event has the input count");
        let d = usage_event("anthropic", &json!({ "type": "message_delta", "usage": { "output_tokens": 40 } }));
        assert_eq!(d, Some(json!({ "usage": { "input": null, "output": 40 } })), "the end event has the output count");
        let o = usage_event("openai", &json!({ "choices": [], "usage": { "prompt_tokens": 7, "completion_tokens": 9 } }));
        assert_eq!(o, Some(json!({ "usage": { "input": 7, "output": 9 } })));
        let g = usage_event("gemini", &json!({ "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 3 } }));
        assert_eq!(g, Some(json!({ "usage": { "input": 5, "output": 3 } })));
    }

    /// Switching the model changes the model field of each request and nothing else: the same reply, the same tool steps and
    /// the same number of model calls, whichever provider serves the model.
    #[test]
    fn switching_the_model_changes_only_the_model_field_of_the_request() {
        let run = |model: &str| {
            let sent = std::cell::RefCell::new(vec![]);
            let r = crate::agent::run_loop_with(model, "system", 0.3, &[], "what time is it", 3, std::time::Duration::from_secs(5),
                |convo| { sent.borrow_mut().push(convo.to_string()); Ok("{\"say\":\"It is noon.\"}".to_string()) },
                |_| Ok(String::new()));
            (r, sent.into_inner())
        };
        let (a, sent_a) = run("claude-haiku-5-5");
        let (b, sent_b) = run("gpt-4o-mini");
        assert_eq!(a, b, "the same answer");
        assert_eq!(sent_a.len(), sent_b.len(), "the same number of model calls");
        let strip = |c: &String| { let mut v: Value = serde_json::from_str(c).unwrap(); v.as_object_mut().unwrap().remove("model"); v };
        assert_eq!(strip(&sent_a[0]), strip(&sent_b[0]), "only the model field differs");
        assert_eq!(serde_json::from_str::<Value>(&sent_a[0]).unwrap()["model"], "claude-haiku-5-5");
    }

    #[test]
    fn a_request_adds_up_its_model_calls_and_a_new_request_starts_at_zero() {
        reset();
        add(Usage { input: 10, output: 2 });
        add(Usage { input: 30, output: 0 });
        assert_eq!(request_total(), Usage { input: 40, output: 2 }, "two calls, one total");
        reset();
        assert_eq!(request_total(), Usage::default());
    }

    #[test]
    fn the_claude_subscription_result_counts_its_cached_tokens_too() {
        // the shape of a real `claude -p --output-format stream-json` result line (usage trimmed)
        let result = json!({ "type": "result", "is_error": false, "result": "ok",
            "usage": { "input_tokens": 2, "cache_creation_input_tokens": 10199, "cache_read_input_tokens": 3396, "output_tokens": 5 } });
        assert_eq!(usage_event("claude-cli", &result), Some(json!({ "usage": { "input": 13597, "output": 5 } })));
        assert_eq!(usage_event("claude-cli", &json!({ "type": "stream_event", "event": {} })), None, "only the result line has counts");
        let start = json!({ "type": "message_start", "message": { "usage": { "input_tokens": 3, "cache_read_input_tokens": 100 } } });
        assert_eq!(usage_event("anthropic", &start), Some(json!({ "usage": { "input": 103, "output": null } })), "the API counts cached reads as input too");
    }

    #[test]
    fn an_event_without_counts_is_not_usage() {
        assert_eq!(usage_event("anthropic", &json!({ "type": "content_block_delta", "delta": { "text": "hi" } })), None);
        assert_eq!(usage_event("openai", &json!({ "choices": [{ "delta": { "content": "hi" } }] })), None);
    }
}
