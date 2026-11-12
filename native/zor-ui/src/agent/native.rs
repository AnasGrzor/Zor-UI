//! The buddy's tools in native form: one JSON schema per tool, for providers that call tools themselves. A native call goes
//! back through `parse_step`, the same validator the JSON protocol uses, so both paths accept and refuse the same things.
//! `run_native_loop` runs it for a Claude model when native tools are on (`agent::native_on`).

use super::{delegate, fit_to_budget, parse_step, Call, Step, MAX_SAY};
use crate::assist::Turn;
use crate::bridge::native_tools;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// One tool: its name, what it does, its properties (name, meaning), and which properties are required.
fn tool(name: &str, description: &str, props: &[(&str, &str)], required: &[&str]) -> Value {
    let properties: serde_json::Map<String, Value> = props.iter().map(|(k, d)| (k.to_string(), json!({ "type": "string", "description": d }))).collect();
    native_tools::anthropic_tool(name, description, json!({ "type": "object", "properties": properties, "required": required }))
}

/// Every tool the buddy has, as native tool definitions. Names and fields match the JSON protocol in `SYSTEM`.
pub fn tool_schemas() -> Vec<Value> {
    vec![
        tool("search", "Shows a web search to the user in the browser. Only when they ask to see or open a search.", &[("arg", "the search query")], &["arg"]),
        tool("web_search", "Looks the query up and returns the top results as text, for facts, news, prices and anything uncertain.", &[("arg", "the search query")], &["arg"]),
        tool("read_url", "Returns the text of a web page, for example one from web_search.", &[("arg", "an https:// address")], &["arg"]),
        tool("open", "Starts an installed app or game.", &[("arg", "the app or game name")], &["arg"]),
        tool("find_app", "Lists installed apps and games matching a name.", &[("arg", "the name")], &["arg"]),
        tool("open_url", "Opens a web page in the browser.", &[("arg", "an https:// address")], &["arg"]),
        tool("play", "Searches for music and plays it. Without the Spotify app it uses the browser.", &[("query", "song or artist"), ("service", "spotify, youtube, youtube music, or empty")], &["query"]),
        tool("media", "Controls the player that is already running.", &[("arg", "play, pause, next or previous")], &["arg"]),
        tool("message", "Opens a chat with the text ready. The user presses Enter to send. The text is exactly what the user asked to say, in their own words; never write a different message.",&[("to", "contact name"), ("text", "the message"), ("via", "app or empty")], &["to", "text"]),
        tool("volume", "Sets the speaker volume.", &[("arg", "up, down, mute, unmute, or 0 to 100")], &["arg"]),
        tool("brightness", "Sets the screen brightness.", &[("arg", "up, down, or 1 to 100")], &["arg"]),
        tool("radio", "Switches a radio.", &[("which", "wifi or bluetooth"), ("state", "on, off or toggle")], &["which"]),
        tool("power", "Lock, sleep, restart, shutdown or logout. The user is asked before anything but lock.", &[("arg", "lock, sleep, restart, shutdown or logout")], &["arg"]),
        tool("clipboard", "Puts text on the clipboard.", &[("arg", "the text")], &["arg"]),
        tool("find_file", "Finds files in Desktop, Documents, Downloads, Pictures, Videos and Music.", &[("arg", "words in the name")], &["arg"]),
        tool("open_file", "Opens a file or folder.", &[("arg", "full path from find_file")], &["arg"]),
        tool("wallpaper", "Sets the desktop wallpaper.", &[("arg", "full path of a picture from find_file, or random")], &["arg"]),
        tool("see_screen", "Looks at the user's screen and answers. Only when they ask about what is on it.", &[("arg", "the question")], &["arg"]),
        tool("remember", "Keeps a fact about the user for later: when they ask you to remember something, or when it will matter next time. Never a password, key or account number.", &[("arg", "the fact in one short sentence")], &["arg"]),
        tool("forget", "Deletes the remembered fact that matches, when the user asks you to forget something.", &[("arg", "words from the fact to forget")], &["arg"]),
        tool("take_over","Controls the mouse and keyboard for a task no other tool can reach. The user is asked first; use it last.", &[("arg", "the task")], &["arg"]),
        tool("list_tree", "Lists the files and folders in the developer workspace set in Settings. Only for a request about the project or its code.", &[("arg", "folder inside the workspace, or empty for its top")], &[]),
        tool("read_file", "Reads a text file in the developer workspace. Secret files are refused.", &[("arg", "file path inside the workspace")], &["arg"]),
        tool("search_code", "Finds the lines in the developer workspace that contain the words.", &[("arg", "the words")], &["arg"]),
        tool("git_status", "Shows the git status and a diff summary of the developer workspace.", &[], &[]),
        tool("run_check", "Runs one fixed check in the developer workspace and says whether it passed. The user is asked once per workspace. Only when asked to build, test or check the project.", &[("arg", "cargo check, cargo test, cargo build, cargo build --release or cargo clippy")], &["arg"]),
    ]
}

/// The step a native tool call means: its name and input go through the same validator as a JSON `do` reply.
pub fn step_from_tool(name: &str, input: &Value) -> Step {
    let mut v = if input.is_object() { input.clone() } else { json!({}) };
    v["do"] = json!(name);
    parse_step(&v.to_string())
}

/// Answers `said` with native tool calls. Each reply either calls tools, whose results go back as `tool_result` blocks, or
/// is plain text, which ends the request. Calls that can't be read are answered with an error so the model can try again.
/// `ask` sends one conversation (`convo` carries the tool list) and returns the reply; `run` runs one tool.
#[allow(clippy::too_many_arguments)]
pub fn run_native_loop(model: &str, system: &str, temp: f64, history: &[Value], said: &str, max_steps: usize, limit: Duration, budget: usize, mut ask: impl FnMut(&Value) -> Result<Turn, String>, mut run: impl FnMut(&Call) -> Result<String, String>, mut summarize: impl FnMut(&str, &str) -> Option<String>) -> Result<String, String> {
    let start = Instant::now();
    let mut msgs = history.to_vec();
    msgs.push(json!({ "role": "user", "content": said }));
    let keep = history.len();
    for _ in 0..max_steps {
        if start.elapsed() > limit { return Err("That took too long, so I stopped.".into()); }
        fit_to_budget(&mut msgs, keep, budget);
        let convo = json!({ "model": model, "system": system, "temperature": temp, "messages": msgs, "tools": tool_schemas() });
        let turn = ask(&convo).map_err(|e| crate::assist::short(&e, 110))?;
        crate::log::line(&format!("buddy native turn: {} calls, {} bad, text {:?}", turn.calls.len(), turn.bad.len(), crate::assist::short(&turn.text, 120)));
        if turn.calls.is_empty() && turn.bad.is_empty() {
            let text = turn.text.trim();
            if text.is_empty() { return Err("I got no answer from the AI.".into()); }
            return Ok(crate::assist::short(text, MAX_SAY));
        }
        // the assistant's turn goes back as it was sent: its text, then each tool call (a call that failed keeps an empty input)
        let mut content = vec![];
        if !turn.text.trim().is_empty() { content.push(json!({ "type": "text", "text": turn.text })); }
        for c in &turn.calls { content.push(json!({ "type": "tool_use", "id": c["id"], "name": c["name"], "input": c["input"] })); }
        for b in &turn.bad { content.push(json!({ "type": "tool_use", "id": b["id"], "name": b["name"], "input": {} })); }
        msgs.push(json!({ "role": "assistant", "content": content }));
        let mut results = vec![];
        for c in &turn.calls {
            let (text, is_error) = match step_from_tool(c["name"].as_str().unwrap_or(""), &c["input"]) {
                Step::Call { call, .. } => match run(&call) { Ok(r) => (delegate::for_call(said, &call, &r, &mut summarize), false), Err(e) => (format!("Failed: {e}"), true) },
                Step::Bad(why) => (why, true),
                Step::Say(_) => ("Use a tool, or answer in plain text.".into(), true),
            };
            crate::log::line(&format!("buddy native tool {}: {}", c["name"], crate::assist::short(&text, 200)));
            results.push(json!({ "type": "tool_result", "tool_use_id": c["id"], "content": text, "is_error": is_error }));
        }
        for b in &turn.bad {
            let text = b["message"].as_str().unwrap_or("The tool input could not be read.").to_string();
            results.push(json!({ "type": "tool_result", "tool_use_id": b["id"], "content": text, "is_error": true }));
        }
        msgs.push(json!({ "role": "user", "content": results }));
    }
    Err("I couldn't finish that in a few steps.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn text(t: &str) -> Turn { Turn { text: t.into(), ..Turn::default() } }
    fn calls(list: &[(&str, &str, Value)]) -> Turn {
        Turn { calls: list.iter().map(|(id, name, input)| json!({ "id": id, "name": name, "input": input })).collect(), ..Turn::default() }
    }

    #[test]
    fn a_tool_result_goes_back_to_the_model_with_its_id() {
        let turns = RefCell::new(vec![text("Opened Spotify."), calls(&[("toolu_1", "open", json!({ "arg": "Spotify" }))])]);
        let seen = RefCell::new(Vec::<Value>::new());
        let r = run_native_loop("claude-sonnet-5-5", "SYS", 0.4, &[], "open spotify", 4, Duration::from_secs(5), usize::MAX,
            |convo| { seen.borrow_mut().push(convo.clone()); Ok(turns.borrow_mut().pop().unwrap()) },
            |c| { assert!(matches!(c, Call::Open(a) if a == "Spotify")); Ok("Opened Spotify.".into()) }, |_, _| None);
        assert_eq!(r, Ok("Opened Spotify.".into()));
        let second = seen.borrow()[1].clone();
        assert_eq!(second["temperature"], 0.4);
        assert_eq!(second["tools"].as_array().unwrap().len(), 26, "the tool list goes with every turn");
        let msgs = second["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["id"], "toolu_1");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(msgs[2]["content"][0]["is_error"], false);
    }

    #[test]
    fn a_failed_tool_is_sent_back_as_an_error_and_the_model_goes_on() {
        let turns = RefCell::new(vec![text("Sorry, it failed."), calls(&[("toolu_2", "open", json!({ "arg": "Nope" }))])]);
        let seen = RefCell::new(Vec::<Value>::new());
        let r = run_native_loop("m", "SYS", 0.3, &[], "x", 4, Duration::from_secs(5), usize::MAX,
            |convo| { seen.borrow_mut().push(convo.clone()); Ok(turns.borrow_mut().pop().unwrap()) },
            |_| Err("No app called Nope.".into()), |_, _| None);
        assert_eq!(r, Ok("Sorry, it failed.".into()));
        let result = seen.borrow()[1]["messages"][2]["content"][0].clone();
        assert_eq!(result["is_error"], true);
        assert_eq!(result["content"], "Failed: No app called Nope.");
    }

    #[test]
    fn a_call_with_unreadable_input_is_answered_without_running_anything() {
        let bad = Turn { bad: vec![json!({ "id": "toolu_3", "name": "open", "message": "tool input is not JSON" })], ..Turn::default() };
        let turns = RefCell::new(vec![text("Done."), bad]);
        let seen = RefCell::new(Vec::<Value>::new());
        let ran = RefCell::new(0);
        let r = run_native_loop("m", "SYS", 0.3, &[], "x", 4, Duration::from_secs(5), usize::MAX,
            |convo| { seen.borrow_mut().push(convo.clone()); Ok(turns.borrow_mut().pop().unwrap()) },
            |_| { *ran.borrow_mut() += 1; Ok(String::new()) }, |_, _| None);
        assert_eq!(r, Ok("Done.".into()));
        assert_eq!(*ran.borrow(), 0);
        let assistant = seen.borrow()[1]["messages"][1]["content"][0].clone();
        assert_eq!(assistant["id"], "toolu_3", "the bad call stays in the history with its id");
        let result = seen.borrow()[1]["messages"][2]["content"][0].clone();
        assert_eq!(result["tool_use_id"], "toolu_3");
        assert_eq!(result["is_error"], true);
    }

    #[test]
    fn an_unknown_tool_name_is_refused_as_an_error() {
        let turns = RefCell::new(vec![text("Okay."), calls(&[("toolu_4", "delete_everything", json!({ "arg": "x" }))])]);
        let seen = RefCell::new(Vec::<Value>::new());
        let r = run_native_loop("m", "SYS", 0.3, &[], "x", 4, Duration::from_secs(5), usize::MAX,
            |convo| { seen.borrow_mut().push(convo.clone()); Ok(turns.borrow_mut().pop().unwrap()) },
            |_| panic!("an unknown tool must not run"), |_, _| None);
        assert_eq!(r, Ok("Okay.".into()));
        assert_eq!(seen.borrow()[1]["messages"][2]["content"][0]["is_error"], true);
    }

    #[test]
    fn a_plain_text_reply_ends_the_request_and_empty_text_is_an_error() {
        let r = run_native_loop("m", "SYS", 0.3, &[], "hi", 4, Duration::from_secs(5), usize::MAX, |_| Ok(text("Hi there.")), |_| Ok(String::new()), |_, _| None);
        assert_eq!(r, Ok("Hi there.".into()));
        let r = run_native_loop("m", "SYS", 0.3, &[], "hi", 4, Duration::from_secs(5), usize::MAX, |_| Ok(text("  ")), |_| Ok(String::new()), |_, _| None);
        assert_eq!(r, Err("I got no answer from the AI.".into()));
    }

    #[test]
    fn a_long_tool_result_goes_back_as_its_summary_after_one_summary_call() {
        let turns = RefCell::new(vec![text("Done."), calls(&[("toolu_5", "web_search", json!({ "arg": "news" }))])]);
        let seen = RefCell::new(Vec::<Value>::new());
        let sums = RefCell::new(0);
        let r = run_native_loop("claude-sonnet-5-5", "SYS", 0.3, &[], "news", 4, Duration::from_secs(5), usize::MAX,
            |convo| { seen.borrow_mut().push(convo.clone()); Ok(turns.borrow_mut().pop().unwrap()) },
            |_| Ok("n".repeat(7000)),
            |_, _| { *sums.borrow_mut() += 1; Some("short summary".into()) });
        assert_eq!(r, Ok("Done.".into()));
        assert_eq!(*sums.borrow(), 1, "one summary call");
        let result = seen.borrow()[1]["messages"][2]["content"][0].clone();
        assert_eq!(result["content"], "Summary of the tool result: short summary");
        assert_eq!(result["is_error"], false);
    }

    #[test]
    fn the_loop_stops_after_the_step_budget() {
        let r = run_native_loop("m", "SYS", 0.3, &[], "x", 2, Duration::from_secs(5), usize::MAX, |_| Ok(calls(&[("t", "media", json!({ "arg": "next" }))])), |_| Ok("Skipped.".into()), |_, _| None);
        assert_eq!(r, Err("I couldn't finish that in a few steps.".into()));
    }

    #[test]
    fn a_stop_from_the_user_ends_the_native_loop_before_any_tool_runs() {
        let ran = RefCell::new(0);
        let r = run_native_loop("m", "SYS", 0.3, &[], "x", 4, Duration::from_secs(5), usize::MAX,
            |_| Err(crate::browse::STOPPED.into()),
            |_| { *ran.borrow_mut() += 1; Ok(String::new()) }, |_, _| None);
        assert_eq!(r, Err(crate::browse::STOPPED.into()));
        assert_eq!(*ran.borrow(), 0);
    }

    #[test]
    fn every_schema_names_a_tool_the_protocol_knows() {
        for schema in tool_schemas() {
            let name = schema["name"].as_str().unwrap();
            let mut input = json!({});
            for r in schema["input_schema"]["required"].as_array().unwrap() {
                input[r.as_str().unwrap()] = json!("sample");
            }
            if name == "radio" { input["which"] = json!("wifi"); }
            assert!(!matches!(step_from_tool(name, &input), Step::Bad(_)), "{name} is refused with its required fields");
        }
    }

    #[test]
    fn a_native_call_becomes_the_same_call_as_the_json_protocol() {
        assert!(matches!(step_from_tool("open", &json!({ "arg": "Spotify" })), Step::Call { call: Call::Open(ref a), .. } if a == "Spotify"));
        assert!(matches!(step_from_tool("media", &json!({ "arg": "next" })), Step::Call { call: Call::Media(ref a), .. } if a == "next"));
        assert!(matches!(step_from_tool("play", &json!({ "query": "lofi", "service": "youtube" })), Step::Call { call: Call::Play { ref query, ref service }, .. } if query == "lofi" && service == "youtube"));
    }

    #[test]
    fn a_missing_field_or_an_unknown_tool_is_refused() {
        assert!(matches!(step_from_tool("open", &json!({})), Step::Bad(_)));
        assert!(matches!(step_from_tool("delete_everything", &json!({ "arg": "x" })), Step::Bad(_)));
    }

    #[test]
    fn a_power_action_still_asks_first_through_the_native_path() {
        assert!(matches!(step_from_tool("power", &json!({ "arg": "shutdown" })), Step::Call { call: Call::Power(ref a), .. } if a == "shutdown"));
    }

    #[test]
    fn the_schema_lists_the_same_tools_as_the_system_text() {
        let names: Vec<String> = tool_schemas().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names.len(), 26);
        for n in &names {
            assert!(super::super::SYSTEM.contains(&format!("\"do\":\"{n}\"")), "{n} is not in SYSTEM");
        }
    }
}
