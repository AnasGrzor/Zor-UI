//! Provider adapters: each provider's request shape and stream events, behind one trait. `chat_request` and `sse_events` in `web.rs`
//! go through `for_provider`, so the chat code never names a provider's wire format. A new provider is one impl and one arm in
//! `for_provider`. The helpers the requests use (`gemini_parts`, `blocks_of`, `openai_like`, `UA`) stay in `web.rs`.

use super::{blocks_of, gemini_parts, openai_like, UA};
use crate::bridge::native_tools::{self, ToolParser};
use serde_json::{json, Value};

/// One provider's wire format.
pub trait Adapter {
    /// The provider's name, as the model list gives it.
    fn name(&self) -> &str;
    /// The request for one chat turn: URL, headers and JSON body.
    fn request(&self, model: &Value, key: &str, msgs: &[Value], system: &str, temp: f64) -> (String, Vec<(String, String)>, Value);
    /// The lines to emit for one parsed stream event.
    fn events(&self, ev: &Value) -> Vec<Value>;
    /// Adds the caller's tool list to a request body. Providers without native tools leave it as it is.
    fn add_tools(&self, _body: &mut Value, _tools: &Value) {}
    /// A reader for the tool calls in this provider's stream, when it has native tools.
    fn new_tools(&self) -> Option<ToolParser> { None }
}

/// The adapter for a provider. Any name without its own wire format uses the OpenAI-style one (with its endpoint, if the model has one).
pub fn for_provider(provider: &str) -> Box<dyn Adapter> {
    match provider {
        "gemini" => Box::new(Gemini),
        "anthropic" => Box::new(Anthropic),
        other => Box::new(OpenAiLike(other.to_string())),
    }
}

/// The headers every request carries.
fn base_headers() -> Vec<(String, String)> {
    let h = |k: &str, v: &str| (k.to_string(), v.to_string());
    vec![h("User-Agent", UA), h("Content-Type", "application/json"), h("Accept", "text/event-stream")]
}

fn header(k: &str, v: &str) -> (String, String) { (k.to_string(), v.to_string()) }

/// A message's role as the providers name it: "user" unless it is the assistant's.
fn role(m: &Value) -> String { m["role"].as_str().unwrap_or("user").to_string() }

/// The text of a stream event's delta, when it has one.
fn delta(t: &Value) -> Option<Value> { t.as_str().filter(|t| !t.is_empty()).map(|t| json!({ "delta": t })) }

struct Gemini;

impl Adapter for Gemini {
    fn name(&self) -> &str { "gemini" }

    fn request(&self, model: &Value, key: &str, msgs: &[Value], system: &str, temp: f64) -> (String, Vec<(String, String)>, Value) {
        let id = model["id"].as_str().unwrap_or("");
        let mut headers = base_headers();
        headers.push(header("x-goog-api-key", key));
        let contents: Vec<Value> = msgs.iter().map(|m| json!({ "role": if role(m) == "assistant" { "model" } else { "user" }, "parts": gemini_parts(m) })).collect();
        (format!("https://generativelanguage.googleapis.com/v1beta/models/{id}:streamGenerateContent?alt=sse"), headers,
         json!({ "contents": contents, "systemInstruction": { "parts": [{ "text": system }] }, "generationConfig": { "temperature": temp } }))
    }

    fn events(&self, ev: &Value) -> Vec<Value> {
        ev["candidates"][0]["content"]["parts"].as_array().map(|p| p.iter().filter_map(|x| delta(&x["text"])).collect()).unwrap_or_default()
    }
}

struct Anthropic;

impl Adapter for Anthropic {
    fn name(&self) -> &str { "anthropic" }

    fn request(&self, model: &Value, key: &str, msgs: &[Value], system: &str, temp: f64) -> (String, Vec<(String, String)>, Value) {
        let id = model["id"].as_str().unwrap_or("");
        let mut headers = base_headers();
        headers.push(header("x-api-key", key));
        headers.push(header("anthropic-version", "2023-06-01"));
        let messages: Vec<Value> = msgs.iter().map(|m| json!({ "role": role(m), "content": blocks_of(m, false) })).collect();
        ("https://api.anthropic.com/v1/messages".into(), headers,
         json!({ "model": id, "max_tokens": 4096, "system": system, "temperature": temp, "stream": true, "messages": messages }))
    }

    fn events(&self, ev: &Value) -> Vec<Value> {
        match ev["type"].as_str() {
            Some("content_block_delta") => delta(&ev["delta"]["text"]).into_iter().collect(),
            Some("error") => vec![json!({ "error": ev["error"]["message"].as_str().unwrap_or("error") })],
            _ => vec![],
        }
    }

    fn add_tools(&self, body: &mut Value, tools: &Value) { native_tools::add_to_body(body, tools); }

    fn new_tools(&self) -> Option<ToolParser> { Some(ToolParser::default()) }
}

/// OpenAI's wire format, which Mistral, OpenRouter, Ollama, custom endpoints and unknown names also use.
struct OpenAiLike(String);

impl Adapter for OpenAiLike {
    fn name(&self) -> &str { &self.0 }

    fn request(&self, model: &Value, key: &str, msgs: &[Value], system: &str, temp: f64) -> (String, Vec<(String, String)>, Value) {
        let id = model["id"].as_str().unwrap_or("");
        let provider = self.0.as_str();
        let mut headers = base_headers();
        if !key.is_empty() { headers.push(header("Authorization", &format!("Bearer {key}"))); }
        if provider == "openrouter" {
            headers.push(header("HTTP-Referer", "https://github.com/pctrade/end4-pC"));
            headers.push(header("X-Title", "Zor UI"));
        }
        let url = model["endpoint"].as_str().filter(|e| !e.is_empty()).unwrap_or(openai_like(provider)).to_string();
        let mut messages = vec![json!({ "role": "system", "content": system })];
        messages.extend(msgs.iter().map(|m| json!({ "role": role(m), "content": blocks_of(m, true) })));
        (url, headers, json!({ "model": id, "stream": true, "temperature": temp, "messages": messages }))
    }

    fn events(&self, ev: &Value) -> Vec<Value> {
        delta(&ev["choices"][0]["delta"]["content"]).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_name_gets_its_own_adapter_and_unknown_names_get_the_openai_one() {
        assert_eq!(for_provider("gemini").name(), "gemini");
        assert_eq!(for_provider("anthropic").name(), "anthropic");
        assert_eq!(for_provider("mistral").name(), "mistral");
        assert_eq!(for_provider("some-new-host").name(), "some-new-host");
    }

    #[test]
    fn only_anthropic_takes_a_tool_list_and_reads_tool_calls() {
        let tools = json!([{ "name": "open", "description": "d", "input_schema": {} }]);
        let mut a = json!({ "model": "m" });
        for_provider("anthropic").add_tools(&mut a, &tools);
        assert_eq!(a["tools"], tools, "anthropic gets the tools");
        assert!(for_provider("anthropic").new_tools().is_some());
        for p in ["gemini", "openai", "mistral"] {
            let mut b = json!({ "model": "m" });
            for_provider(p).add_tools(&mut b, &tools);
            assert!(b.get("tools").is_none(), "{p} sends no tools");
            assert!(for_provider(p).new_tools().is_none(), "{p} has no tool calls to read");
        }
    }

    #[test]
    fn the_openai_adapter_is_the_default_for_a_name_it_does_not_know() {
        let (url, _, body) = for_provider("some-new-host").request(&json!({ "id": "m" }), "K", &[], "sys", 0.5);
        assert_eq!(url, "https://api.openai.com/v1/chat/completions", "no endpoint: the OpenAI URL");
        assert_eq!(body["messages"][0], json!({ "role": "system", "content": "sys" }));
    }
}
