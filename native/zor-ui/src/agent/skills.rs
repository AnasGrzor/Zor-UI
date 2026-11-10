//! Skills: how to finish a kind of job with several tools. A skill names its tools and gives short guidance. The guidance goes
//! into the request only when the request's words point at that skill, so most requests carry none of it. The developer skill
//! (inspect a codebase, run tests) is not here: the buddy has no code tools yet.

use super::native::tool_schemas;

pub struct Skill {
    pub name: &'static str,
    /// The tools this skill uses, by name. Each must be a tool the buddy has (checked by a test).
    pub tools: &'static [&'static str],
    /// Words in the request that point at this skill.
    pub words: &'static [&'static str],
    pub guidance: &'static str,
}

pub const SKILLS: [Skill; 3] = [
    Skill {
        name: "research",
        tools: &["web_search", "read_url", "search"],
        words: &["research", "compare", "find out", "look up", "search", "news", "price", "prices", "score", "scores", "review", "reviews"],
        guidance: "Search, then read the most useful page before answering. Compare sources, and say when they disagree. Keep the answer short.",
    },
    Skill {
        name: "desktop",
        tools: &["open", "find_app", "volume", "brightness", "media", "wallpaper", "clipboard", "see_screen", "take_over"],
        words: &["open", "close", "volume", "brightness", "screen", "wallpaper", "clipboard", "app", "window", "click", "fill"],
        guidance: "Use the app or setting directly. Check the result it reports. Use take_over only for what no other tool reaches.",
    },
    Skill {
        name: "personal",
        tools: &["remember", "forget", "find_file", "open_file", "message", "play"],
        words: &["remember", "forget", "message", "text", "tell", "file", "document", "song", "music", "play"],
        guidance: "Facts the user asks you to keep go in memory. Messages use their own words. Files are found before they are opened.",
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

    #[test]
    fn every_skill_tool_is_a_tool_the_buddy_has() {
        let names: Vec<String> = tool_schemas().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        for s in &SKILLS {
            for t in s.tools {
                assert!(names.iter().any(|n| n == t), "{} uses {t}, which is not a tool", s.name);
            }
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
}
