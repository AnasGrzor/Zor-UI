//! Which model answers a request. The configured model is the default. A request that looks multi-step goes to the strong
//! model when one is set (`ai.strongModel`). If a model fails before any tool has run, the next model in `ai.fallbackModels`
//! tries the same request. A failure after a tool ran is never retried on another model: the tool may already have acted.

use crate::conf::Config;

/// Words that mark a request as several steps or a comparison.
const MULTI_STEP: [&str; 9] = ["then", "after", "compare", "research", "plan", "steps", "step", "first", "comparison"];

fn looks_multi_step(said: &str) -> bool {
    let words: Vec<String> = said.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
    let text = format!(" {} ", words.join(" "));
    MULTI_STEP.iter().any(|w| text.contains(&format!(" {w} ")))
}

/// The model for this request: the strong one for a multi-step request when it is set, else the primary.
pub fn route_model(cfg: &Config, said: &str, primary: &str) -> String {
    let strong = cfg.str("ai.strongModel").trim();
    if !strong.is_empty() && looks_multi_step(said) {
        crate::log::line(&format!("buddy route: multi-step request to {strong}"));
        return strong.to_string();
    }
    primary.to_string()
}

/// The models to try, in order: the chosen one, then the two fallbacks picked in Settings, then any list in config.json.
/// Empty and repeated names are skipped.
pub fn fallback_chain(cfg: &Config, first: &str) -> Vec<String> {
    let mut fallbacks: Vec<String> = ["ai.fallback1", "ai.fallback2"].iter().map(|k| cfg.str(k).trim().to_string()).collect();
    fallbacks.extend(cfg.get("ai.fallbackModels").as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|m| m.trim().to_string()));
    let mut chain = vec![first.to_string()];
    for m in fallbacks {
        if !m.is_empty() && !chain.iter().any(|c| *c == m) { chain.push(m); }
    }
    chain
}

/// Whether to try the next model: the request failed, no tool ran, and it wasn't the user stopping it.
pub fn should_fall_back(result: &Result<String, String>, a_tool_ran: bool) -> bool {
    match result {
        Err(e) => !a_tool_ran && e != crate::browse::STOPPED && e != crate::agent::CANCELLED,
        Ok(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_step_request_goes_to_the_strong_model_only_when_one_is_set() {
        let none = Config::parse("{}");
        let strong = Config::parse(r#"{"ai":{"strongModel":"claude-sonnet-5-5"}}"#);
        assert_eq!(route_model(&none, "compare two laptops and then book one", "haiku"), "haiku", "no strong model set");
        assert_eq!(route_model(&strong, "compare two laptops and then book one", "haiku"), "claude-sonnet-5-5");
        assert_eq!(route_model(&strong, "turn the volume up", "haiku"), "haiku", "a simple request stays on the primary");
    }

    #[test]
    fn the_fallback_chain_starts_with_the_chosen_model_and_skips_repeats() {
        let cfg = Config::parse(r#"{"ai":{"fallbackModels":["gpt-4o-mini","","claude-haiku-5-5","gpt-4o-mini"]}}"#);
        assert_eq!(fallback_chain(&cfg, "claude-haiku-5-5"), ["claude-haiku-5-5", "gpt-4o-mini"]);
        assert_eq!(fallback_chain(&Config::parse("{}"), "a"), ["a"], "no fallbacks: just the one model");
        let picked = Config::parse(r#"{"ai":{"fallback1":"gpt-4o-mini","fallback2":"gpt-4o-mini"}}"#);
        assert_eq!(fallback_chain(&picked, "claude-sonnet-5-5"), ["claude-sonnet-5-5", "gpt-4o-mini"], "the two Settings picks, with a repeat dropped");
        let off = Config::parse(r#"{"ai":{"fallback1":"","fallback2":"claude-haiku-5-5"}}"#);
        assert_eq!(fallback_chain(&off, "claude-sonnet-5-5"), ["claude-sonnet-5-5", "claude-haiku-5-5"], "Off leaves no blank model");
    }

    #[test]
    fn a_failure_falls_back_only_before_any_tool_ran() {
        let down: Result<String, String> = Err("claude returned 503".into());
        assert!(should_fall_back(&down, false), "the model was down before anything ran");
        assert!(!should_fall_back(&down, true), "a tool already acted: never try again on another model");
        assert!(!should_fall_back(&Err(crate::browse::STOPPED.into()), false), "the user stopped it");
        assert!(!should_fall_back(&Ok("Done.".into()), false), "an answer is not a failure");
    }
}
