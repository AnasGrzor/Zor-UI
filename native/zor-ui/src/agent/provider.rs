//! Normalized facts about the models the buddy uses (M7): what a model can do, whether its provider can be used now, and the tokens a
//! reply used. The adapters in `bridge/web.rs` turn each provider's stream into the same events; this file reads the facts out of
//! them, so the agent loop never looks at a provider's wire format.

use crate::conf::Config;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

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

/// The window used when no window is known. A conservative floor, not a model fact: each default model has more, so a
/// smaller guess only drops older turns sooner.
pub const UNKNOWN_CONTEXT_TOKENS: u32 = 32_000;

/// Context windows (input tokens) of the default models in `bridge/web.rs` `default_models()`, as each provider publishes them.
/// Models with no confirmed published number are left out; a number is never guessed.
const PUBLISHED: [(&str, u32); 5] = [
    // https://platform.claude.com/docs/en/models/overview ("Context window: 1M tokens"), checked 2026-11-12
    ("claude-sonnet-5-5", 1_000_000),
    // https://platform.claude.com/docs/en/models/overview ("Context window: 1M tokens"), checked 2026-11-12
    ("claude-haiku-5-5", 1_000_000),
    // https://aistudio.google.com/docs/models/gemini-2.5-flash ("Input token limit 1,048,576"), checked 2026-11-12; Google marks it deprecated
    ("gemini-2.5-flash", 1_048_576),
    // https://aistudio.google.com/docs/models/gemini-3-flash-preview ("Input token limit 1,048,576"), checked 2026-11-12
    ("gemini-3-flash-preview", 1_048_576),
    // https://developers.openai.com/api/docs/models/gpt-4o-mini ("128,000 context window"), checked 2026-11-12; OpenAI marks it deprecated
    ("gpt-4o-mini", 128_000),
];

/// Ollama's context length per model name, read once per session (None when Ollama does not report one).
static OLLAMA: Mutex<BTreeMap<String, Option<u32>>> = Mutex::new(BTreeMap::new());

/// The context window of `model_id`, in tokens: the user's `contextTokens` on its `ai.customModels` entry, else the published
/// number above, else what Ollama reports for it. None when none of those knows it (the caller then uses `UNKNOWN_CONTEXT_TOKENS`).
pub fn context_tokens(model_id: &str, cfg: &Config) -> Option<u32> {
    let custom = cfg.get("ai.customModels").as_array().into_iter().flatten()
        .find(|m| m["id"].as_str() == Some(model_id)).and_then(|m| m["contextTokens"].as_u64()).and_then(|n| u32::try_from(n).ok());
    custom.or_else(|| PUBLISHED.iter().find(|(id, _)| *id == model_id).map(|(_, n)| *n))
        .or_else(|| ollama_context(model_id))
}

/// Ollama's reported context length for a model, asked once and then remembered for the session.
fn ollama_context(model_id: &str) -> Option<u32> {
    if let Some(n) = OLLAMA.lock().unwrap_or_else(|e| e.into_inner()).get(model_id) { return *n; }
    let n = crate::bridge::web::ollama_show(model_id).and_then(|v| context_from_show(&v));
    OLLAMA.lock().unwrap_or_else(|e| e.into_inner()).insert(model_id.to_string(), n);
    n
}

/// The context length in an Ollama /api/show reply: the `model_info` key that ends in `.context_length` (for example `llama.context_length`).
pub fn context_from_show(v: &Value) -> Option<u32> {
    v["model_info"].as_object()?.iter().find(|(k, _)| k.ends_with(".context_length"))
        .and_then(|(_, n)| n.as_u64()).and_then(|n| u32::try_from(n).ok())
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
            let r = crate::agent::run_loop_with(model, "system", 0.3, &[], "what time is it", 3, std::time::Duration::from_secs(5), usize::MAX,
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

    #[test]
    fn the_user_override_beats_the_published_window_and_unknown_is_none() {
        let cfg = Config::parse(r#"{"ai":{"customModels":[{"id":"claude-haiku-5-5","contextTokens":64000}]}}"#);
        assert_eq!(context_tokens("claude-haiku-5-5", &cfg), Some(64_000), "the override wins");
        let plain = Config::parse("{}");
        assert_eq!(context_tokens("claude-haiku-5-5", &plain), Some(1_000_000), "then the published table");
        assert_eq!(context_tokens("gpt-4o-mini", &plain), Some(128_000));
        assert_eq!(context_tokens("gemini-2.5-flash", &plain), Some(1_048_576));
        assert_eq!(context_tokens("mistral-medium-latest", &plain), None, "no confirmed number: left out, not guessed");
    }

    #[test]
    fn ollama_reports_its_context_length_under_the_architecture_key() {
        let show = json!({ "model_info": { "general.architecture": "llama", "llama.context_length": 8192, "llama.embedding_length": 4096 } });
        assert_eq!(context_from_show(&show), Some(8192));
        assert_eq!(context_from_show(&json!({ "model_info": { "llama.block_count": 32 } })), None, "no context key: unknown");
        assert_eq!(context_from_show(&json!({})), None);
    }
}
