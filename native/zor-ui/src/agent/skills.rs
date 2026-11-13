//! Skills: how to finish a kind of job with several tools. A skill names its tools and gives short guidance. The guidance goes
//! into the request only when the request's words point at that skill, so most requests carry none of it.
//!
//! Versioning: each skill has a version, starting at 1. Bump it when the skill's guidance or tools change, so a change in
//! behavior shows as a new number. Skill names are unique (tested).
//!
//! Each skill also states its permission (the highest policy risk its tools reach, checked against `policy::risk_of`), the
//! context it needs, and an eval: requests that must select it and requests that must not. The tests check that every skill
//! tool is a tool the buddy has, that versions and names are sound, and that each eval holds against `select`.
//!
//! The `workflow` skill adds no execution path. Its guidance chains existing tools in order.

// The fields below that the buddy never reads are read by the tests, which check each skill's versioning, permission and eval.
#[allow(dead_code)]
pub struct Eval {
    /// Requests that must select the skill.
    pub selects: &'static [&'static str],
    /// Requests that must not select the skill.
    pub must_not: &'static [&'static str],
}

#[allow(dead_code)]
pub struct Skill {
    pub name: &'static str,
    /// Starts at 1. Bump it when the guidance or the tools change.
    pub version: u32,
    /// One line: what the skill does.
    pub capability: &'static str,
    /// The tools this skill uses, by name. Each must be a tool the buddy has (checked by a test).
    pub tools: &'static [&'static str],
    /// Words in the request that point at this skill.
    pub words: &'static [&'static str],
    pub guidance: &'static str,
    /// The highest `policy::Risk` any of `tools` reaches, by name: "ReadOnly", "Reversible" or "Consequential".
    pub permission: &'static str,
    /// What the skill needs in context, or "none".
    pub context: &'static str,
    pub eval: Eval,
}

pub const SKILLS: [Skill; 7] = [
    Skill {
        name: "research",
        version: 1,
        capability: "Looks things up on the web and reads the most useful page.",
        tools: &["web_search", "read_url", "search"],
        words: &["research", "compare", "find out", "look up", "search", "news", "price", "prices", "score", "scores", "review", "reviews"],
        guidance: "Search, then read the most useful page before answering. Compare sources, and say when they disagree. Keep the answer short.",
        permission: "ReadOnly",
        context: "none",
        eval: Eval {
            selects: &["research the best budget laptop", "compare the prices of two laptops", "look up the score of the match", "find out when the next eclipse is", "what is the news about the election", "search for cheap flights to Rome", "any reviews of that restaurant"],
            must_not: &["hi, how are you", "what is the capital of France", "turn the volume up", "remember that my sister is called Lina", "check my email", "play the next track"],
        },
    },
    Skill {
        name: "desktop",
        version: 1,
        capability: "Opens apps and changes the PC's settings, and looks at the screen when asked.",
        tools: &["open", "find_app", "volume", "brightness", "media", "wallpaper", "clipboard", "see_screen", "take_over"],
        words: &["open", "close", "volume", "brightness", "screen", "wallpaper", "clipboard", "app", "window", "click", "fill"],
        guidance: "Use the app or setting directly. Check the result it reports. Use take_over only for what no other tool reaches.",
        permission: "Consequential",
        context: "none",
        eval: Eval {
            selects: &["turn the volume up", "open Spotify", "set the brightness to 50", "what is on my screen", "close this window", "put this picture on the wallpaper", "copy this to the clipboard"],
            must_not: &["hi, how are you", "what is the capital of France", "compare the prices of two laptops", "remember that my sister is called Lina", "find out when the next eclipse is", "run the tests on the code", "I'm ready for lunch", "go to sleep"],
        },
    },
    Skill {
        name: "personal",
        version: 1,
        capability: "Keeps facts the user asks for, finds and opens their files, sends messages and plays music.",
        tools: &["remember", "forget", "find_file", "open_file", "message", "play"],
        words: &["remember", "forget", "message", "text", "tell", "file", "document", "song", "music", "play"],
        guidance: "Facts the user asks you to keep go in memory. Messages use their own words. Files are found before they are opened.",
        permission: "Reversible",
        context: "none",
        eval: Eval {
            selects: &["remember that my sister is called Lina", "forget what I said about the dentist", "text Sam that I am late", "send a message to mum", "find the document about the lease", "play some music", "find the file called budget", "play the next track"],
            must_not: &["hi, how are you", "what is the capital of France", "look up the score of the match", "turn the volume up", "run the tests on the code", "make the app ready for release", "turn the page", "go to sleep"],
        },
    },
    Skill {
        name: "developer",
        version: 1,
        capability: "Looks through the developer workspace: its files, its code and its git status, and runs its build and tests.",
        tools: &["list_tree", "read_file", "search_code", "git_status", "run_check"],
        words: &["code", "codebase", "test", "tests", "build", "bug", "bugs", "git", "repo", "compile"],
        guidance: "Look before you answer: list_tree to find the file, read_file for its text, search_code for the words. Run run_check only for a build or test, and quote its result. The user is asked once per workspace before a check runs. Say when a file is missing or secret.",
        permission: "Consequential",
        context: "developer workspace, set by hand in config.json (buddies.developerWorkspace)",
        eval: Eval {
            selects: &["run the tests on the code", "check the build", "there is a bug in the parser", "show me the error from the build", "what changed in git", "list the files in the codebase", "find the cache function in the code"],
            must_not: &["hi, how are you", "what is the capital of France", "remember that my sister is called Lina", "turn the volume up", "look up the score of the match", "play some music", "check my email", "there was an error on my phone"],
        },
    },
    Skill {
        name: "project",
        version: 1,
        capability: "Lists the active project's files and keeps track of its open work.",
        tools: &["list_tree", "remember", "forget"],
        words: &["project", "progress", "tracking", "todo", "backlog", "milestone", "tasks", "unfinished"],
        guidance: "List the project's files with list_tree when asked what is in it. Keep open work as one short fact with remember, and name the project in it, for example: Zor UI: the bar animation is unfinished. Forget the fact when the user says it is done. There is no tracker: the memory is the list.",
        permission: "Reversible",
        context: "active project",
        eval: Eval {
            selects: &["what is left on the project", "track my progress on the redesign", "add a todo for the login screen", "what is on the backlog", "list the tasks for this project", "what milestone are we on", "where is my project at"],
            must_not: &["hi, how are you", "what is the capital of France", "remember that my sister is called Lina", "turn the volume up", "compare the prices of two laptops", "run the tests on the code", "play the next track", "what time is it then"],
        },
    },
    Skill {
        name: "browser_research",
        version: 1,
        capability: "Drives the browser to visit a website, read a page and act on it, with web search for facts.",
        tools: &[
            "browser_navigate", "browser_snapshot", "browser_find", "browser_click", "browser_type", "browser_press_key", "browser_select_option",
            "browser_navigate_back", "browser_wait_for", "browser_handle_dialog", "browser_close", "web_search", "read_url",
        ],
        words: &["website", "websites", "site", "browse", "browser", "web page", "webpage", "this page", "that page", "open the page", "read the page"],
        guidance: "Open the page with browser_navigate and read it with browser_snapshot. Use browser_find on a long page. Close the browser page with browser_close when the task is done. The browser asks the user before it clicks or types on a risky element.",
        permission: "Consequential",
        context: "browser turned on in Settings",
        eval: Eval {
            selects: &["browse to the bbc website", "visit example.com in the browser", "find the contact page on that site", "click the login button on the website", "close the browser", "open the page and read it", "browse the shop"],
            must_not: &["hi, how are you", "what is the capital of France", "turn the volume up", "remember that my sister is called Lina", "run the tests on the code", "compare the prices of two laptops", "go to sleep", "turn the page"],
        },
    },
    Skill {
        name: "workflow",
        version: 1,
        capability: "Chains several existing tools in order for one job, such as checking a project and then reporting.",
        tools: &["list_tree", "read_file", "search_code", "git_status", "web_search", "read_url", "remember"],
        words: &["prepare", "steps", "step by step", "workflow", "and then", "after that"],
        guidance: "Name the steps, run them in order, and check each result before the next step. Stop at the first failed step and report it. Use only the tools listed for the job; a workflow adds no tool of its own.",
        permission: "Reversible",
        context: "developer workspace, for a job about a project",
        eval: Eval {
            selects: &["prepare my project for the release", "prepare the release steps", "do this step by step", "check the code and then report back", "after that send me the summary", "run my weekly routine as a workflow", "prepare the app to share"],
            must_not: &["hi, how are you", "what is the capital of France", "turn the volume up", "remember that my sister is called Lina", "compare the prices of two laptops", "look up the score of the match", "what time is it then", "I'm ready"],
        },
    },
];

/// The skills a request points at, in table order. Matching is on whole words, so "app" does not match "apparently".
pub fn select(said: &str) -> Vec<&'static Skill> {
    let words: Vec<String> = said.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
    let text = format!(" {} ", words.join(" "));
    SKILLS.iter().filter(|s| s.words.iter().any(|w| text.contains(&format!(" {w} ")))).collect()
}

/// The guidance lines for the skills a request points at. Empty when none does.
pub fn guidance_for(said: &str) -> String {
    select(said).iter().map(|s| format!("Skill {}: {}", s.name, s.guidance)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::native::{step_from_tool, tool_schemas};
    use crate::agent::policy::{risk_of, Risk};
    use crate::agent::{Call, Step, SYSTEM};
    use serde_json::json;

    /// Every name the buddy can call: the native tools, the JSON protocol names in `SYSTEM`, and the browser tools.
    fn buddy_tools() -> (Vec<String>, Vec<String>, Vec<String>) {
        let native: Vec<String> = tool_schemas().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        let json_protocol: Vec<String> = SYSTEM.split("{\"do\":\"").skip(1).map(|s| s.split('"').next().unwrap().to_string()).collect();
        let browser: Vec<String> = crate::browse::TOOLS.iter().map(|(n, _)| n.to_string()).collect();
        (native, json_protocol, browser)
    }

    /// The skill tools that no source lists. Empty when every tool exists.
    fn missing(tools: &[&str]) -> Vec<String> {
        let (native, json_protocol, browser) = buddy_tools();
        let have = |t: &str| native.iter().chain(&json_protocol).chain(&browser).any(|n| n == t);
        tools.iter().filter(|t| !have(t)).map(|t| t.to_string()).collect()
    }

    fn risk_named(s: &str) -> Risk {
        match s {
            "ReadOnly" => Risk::ReadOnly,
            "Reversible" => Risk::Reversible,
            "Consequential" => Risk::Consequential,
            "Destructive" => Risk::Destructive,
            other => panic!("unknown permission {other:?}"),
        }
    }

    fn rank(r: Risk) -> u8 {
        match r {
            Risk::ReadOnly => 0,
            Risk::Reversible => 1,
            Risk::Consequential => 2,
            Risk::Destructive => 3,
        }
    }

    /// The policy risk of one tool, built from the call the AI would make with it.
    fn risk_of_tool(name: &str) -> Risk {
        if name.starts_with("browser_") {
            return risk_of(&Call::Browser { tool: name.into(), args: json!({}) });
        }
        let input = json!({ "arg": "x", "query": "x", "to": "x", "text": "x", "which": "wifi" });
        match step_from_tool(name, &input) {
            Step::Call { call, .. } => risk_of(&call),
            _ => panic!("{name} does not build a call"),
        }
    }

    #[test]
    fn every_skill_tool_is_a_tool_the_buddy_has() {
        for s in &SKILLS {
            let gone = missing(s.tools);
            assert!(gone.is_empty(), "{} uses tools the buddy does not have: {gone:?}", s.name);
        }
    }

    #[test]
    fn a_skill_naming_a_missing_tool_is_reported() {
        assert_eq!(missing(&["list_tree", "no_such_tool"]), ["no_such_tool"]);
    }

    #[test]
    fn the_json_protocol_is_read_for_developer_tools() {
        let (_, json_protocol, _) = buddy_tools();
        for t in ["list_tree", "read_file", "search_code", "git_status"] {
            assert!(json_protocol.iter().any(|n| n == t), "{t} is not in the JSON protocol");
        }
    }

    #[test]
    fn a_request_picks_the_skills_its_words_point_at() {
        let names = |said: &str| select(said).iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names("compare the prices of two laptops"), ["research"]);
        assert_eq!(names("remember that my sister is called Lina"), ["personal"]);
        assert_eq!(names("turn the volume up"), ["desktop"]);
        assert_eq!(names("what is on my screen"), ["desktop"]);
        assert!(names("hi, how are you").is_empty(), "a chat gets no skill guidance");
    }

    #[test]
    fn a_word_only_matches_as_a_whole_word() {
        assert!(select("apparently the weather is fine").is_empty(), "'app' inside 'apparently' is not the desktop skill");
    }

    #[test]
    fn the_guidance_is_empty_when_no_skill_applies() {
        assert_eq!(guidance_for("hi there"), "");
        assert!(guidance_for("turn the volume up").starts_with("Skill desktop:"));
    }

    #[test]
    fn every_skill_has_a_version_and_names_are_unique() {
        for s in &SKILLS {
            assert!(s.version >= 1, "{} has version {}", s.name, s.version);
        }
        let mut names: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), SKILLS.len(), "two skills share a name");
    }

    #[test]
    fn each_skill_selects_its_requests_and_refuses_the_others() {
        for s in &SKILLS {
            assert!(s.eval.selects.len() >= 6, "{} needs at least 6 selecting requests", s.name);
            assert!(s.eval.must_not.len() >= 4, "{} needs at least 4 refused requests", s.name);
            assert!(s.eval.must_not.iter().filter(|r| !CHAT.contains(r)).count() >= 2, "{} needs at least 2 near-miss refusals", s.name);
            for req in s.eval.selects {
                assert!(select(req).iter().any(|x| x.name == s.name), "{} must select {req:?}, got {:?}", s.name, select(req).iter().map(|x| x.name).collect::<Vec<_>>());
            }
            for req in s.eval.must_not {
                assert!(!select(req).iter().any(|x| x.name == s.name), "{} must not select {req:?}", s.name);
            }
        }
    }

    /// Ordinary chat. No skill may select these.
    const CHAT: [&str; 6] = ["hi, how are you", "what is the capital of France", "good morning", "thanks, that was helpful", "what is 2 plus 2", "how was your day"];

    #[test]
    fn ordinary_chat_selects_no_skill() {
        for chat in CHAT {
            assert!(select(chat).is_empty(), "{chat:?} selected {:?}", select(chat).iter().map(|s| s.name).collect::<Vec<_>>());
        }
    }

    #[test]
    fn each_skill_permission_is_the_highest_risk_of_its_tools() {
        for s in &SKILLS {
            let perm = risk_named(s.permission);
            let mut top = Risk::ReadOnly;
            for t in s.tools {
                let r = risk_of_tool(t);
                assert!(rank(r) <= rank(perm), "{} declares {} but {t} is {r:?}", s.name, s.permission);
                if rank(r) > rank(top) {
                    top = r;
                }
            }
            assert_eq!(top, perm, "{} declares {} but its highest tool risk is {top:?}", s.name, s.permission);
        }
    }

    #[test]
    fn developer_read_tools_are_read_only_and_power_moves_are_not() {
        assert_eq!(risk_of_tool("list_tree"), Risk::ReadOnly);
        assert_eq!(risk_of_tool("git_status"), Risk::ReadOnly);
        assert_eq!(risk_of_tool("take_over"), Risk::Consequential);
        assert_eq!(risk_of_tool("run_check"), Risk::Consequential, "a check asks once per workspace");
    }
}
