//! Native tool calling for the Anthropic Messages API: the tool list a request carries, and the parser for the tool calls
//! that come back in the stream. Not wired into the buddy's loop yet; the loop still reads its JSON protocol.

use serde_json::{json, Value};

/// One tool the model may call, in the shape Anthropic's `tools` field takes.
pub fn anthropic_tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "input_schema": input_schema })
}

/// Adds the caller's tool list to an Anthropic request body. An empty or missing list leaves the body as it was,
/// so a plain text chat sends exactly what it always did.
pub fn add_to_body(body: &mut Value, tools: &Value) {
    if tools.as_array().is_some_and(|a| !a.is_empty()) { body["tools"] = tools.clone(); }
}

/// A complete tool call read from the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// A tool call whose input could not be read. The id and name are kept so the answer can still go back for it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolFailure {
    pub id: String,
    pub name: String,
    pub message: String,
}

/// Collects one tool call at a time. Anthropic sends a tool block as: content_block_start (id, name), then
/// input_json_delta pieces that make up the JSON input, then content_block_stop. Text blocks are ignored.
#[derive(Default)]
pub struct ToolParser {
    open: Option<(u64, String, String, String)>, // (block index, id, name, JSON text so far)
}

impl ToolParser {
    /// Feeds one parsed stream event. Returns the finished call when its block stops, or a failure when the input isn't JSON.
    pub fn feed(&mut self, ev: &Value) -> Option<Result<ToolUse, ToolFailure>> {
        let index = ev["index"].as_u64();
        match ev["type"].as_str()? {
            "content_block_start" if ev["content_block"]["type"] == "tool_use" => {
                let block = &ev["content_block"];
                self.open = Some((index?, block["id"].as_str().unwrap_or("").into(), block["name"].as_str().unwrap_or("").into(), String::new()));
                None
            }
            "content_block_delta" if ev["delta"]["type"] == "input_json_delta" => {
                if let (Some((i, _, _, text)), Some(part)) = (self.open.as_mut(), ev["delta"]["partial_json"].as_str()) {
                    if Some(*i) == index { text.push_str(part); }
                }
                None
            }
            "content_block_stop" => {
                if self.open.as_ref().map(|(i, ..)| Some(*i)) != Some(index) { return None; }
                let (_, id, name, text) = self.open.take()?;
                let input = if text.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str::<Value>(&text) };
                Some(match input {
                    Ok(input) => Ok(ToolUse { id, name, input }),
                    Err(e) => Err(ToolFailure { id, name, message: format!("tool input is not JSON: {e}") }),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(index: u64, id: &str, name: &str) -> Value {
        json!({ "type": "content_block_start", "index": index, "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} } })
    }
    fn delta(index: u64, part: &str) -> Value {
        json!({ "type": "content_block_delta", "index": index, "delta": { "type": "input_json_delta", "partial_json": part } })
    }
    fn stop(index: u64) -> Value { json!({ "type": "content_block_stop", "index": index }) }

    #[test]
    fn a_tool_call_is_put_together_from_its_pieces() {
        let mut p = ToolParser::default();
        let text = json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } });
        assert_eq!(p.feed(&text), None);
        assert_eq!(p.feed(&start(1, "toolu_1", "open")), None);
        assert_eq!(p.feed(&delta(1, "{\"arg\":")), None);
        assert_eq!(p.feed(&delta(1, " \"Spotify\"}")), None);
        assert_eq!(p.feed(&stop(1)), Some(Ok(ToolUse { id: "toolu_1".into(), name: "open".into(), input: json!({ "arg": "Spotify" }) })));
    }

    #[test]
    fn a_tool_with_no_input_gets_an_empty_object() {
        let mut p = ToolParser::default();
        p.feed(&start(0, "toolu_2", "media"));
        assert_eq!(p.feed(&stop(0)), Some(Ok(ToolUse { id: "toolu_2".into(), name: "media".into(), input: json!({}) })));
    }

    #[test]
    fn broken_json_input_is_an_error_not_a_call() {
        let mut p = ToolParser::default();
        p.feed(&start(0, "toolu_3", "open"));
        p.feed(&delta(0, "{\"arg\":"));
        assert!(matches!(p.feed(&stop(0)), Some(Err(_))));
    }

    #[test]
    fn a_stop_for_another_block_does_not_end_the_call() {
        let mut p = ToolParser::default();
        p.feed(&start(1, "toolu_4", "open"));
        assert_eq!(p.feed(&stop(0)), None);
        assert!(p.feed(&stop(1)).is_some());
    }

    #[test]
    fn tools_are_added_to_the_body_only_when_given() {
        let mut plain = json!({ "model": "m", "stream": true });
        add_to_body(&mut plain, &json!([]));
        add_to_body(&mut plain, &Value::Null);
        assert_eq!(plain, json!({ "model": "m", "stream": true }), "no tools, no change");
        let tools = json!([anthropic_tool("open", "Starts an app", json!({ "type": "object" }))]);
        add_to_body(&mut plain, &tools);
        assert_eq!(plain["tools"], tools);
    }

    #[test]
    fn a_tool_definition_has_the_anthropic_fields() {
        let t = anthropic_tool("open", "Starts an app", json!({ "type": "object", "properties": { "arg": { "type": "string" } } }));
        assert_eq!(t["name"], "open");
        assert_eq!(t["input_schema"]["type"], "object");
    }
}
