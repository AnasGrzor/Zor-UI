//! Which model answers a request. The configured model is the default. A request that looks multi-step goes to the strong
//! model when one is set (`ai.strongModel`). If a model fails before any tool has run, the next model in `ai.fallbackModels`
//! tries the same request. A failure after a tool ran is never retried on another model: the tool may already have acted.

use crate::conf::Config;

/// Words that mark a request as several steps or a comparison.
const MULTI_STEP: [&str; 9] = ["then", "after", "compare", "research", "plan", "steps", "step", "first", "comparison"];

/// Whether a request is several steps or a comparison. A planning call is made only for these (plan.rs).
pub fn looks_multi_step(said: &str) -> bool {
    let words: Vec<String> = said.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
    let text = format!(" {} ", words.join(" "));
    MULTI_STEP.iter().any(|w| text.contains(&format!(" {w} ")))
}

/// The model for this request: the strong one for a multi-step request when it is set and its provider can be used now
/// (`usable`, by model id), else the primary. The primary is always the user's choice, so it is never skipped here.
pub fn route_model(cfg: &Config, said: &str, primary: &str, usable: &dyn Fn(&str) -> bool) -> String {
    let strong = cfg.str("ai.strongModel").trim();
    if !strong.is_empty() && looks_multi_step(said) {
        if usable(strong) {
            crate::log::line(&format!("buddy route: multi-step request to {strong}"));
            return strong.to_string();
        }
        crate::log::line(&format!("buddy route: {strong} has no saved key, staying on {primary}"));
    }
    primary.to_string()
}

/// The models to try, in order: the chosen one, then the two fallbacks picked in Settings, then any list in config.json.
/// Empty and repeated names are skipped, and so is a fallback whose provider can't be used now (`usable`, by model id).
pub fn fallback_chain(cfg: &Config, first: &str, usable: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut fallbacks: Vec<String> = ["ai.fallback1", "ai.fallback2"].iter().map(|k| cfg.str(k).trim().to_string()).collect();
    fallbacks.extend(cfg.get("ai.fallbackModels").as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|m| m.trim().to_string()));
    let mut chain = vec![first.to_string()];
    for m in fallbacks {
        if m.is_empty() || chain.contains(&m) { continue; }
        if usable(&m) { chain.push(m); } else { crate::log::line(&format!("buddy route: skipping {m}, no saved key")); }
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
        let all = |_: &str| true;
        assert_eq!(route_model(&none, "compare two laptops and then book one", "haiku", &all), "haiku", "no strong model set");
        assert_eq!(route_model(&strong, "compare two laptops and then book one", "haiku", &all), "claude-sonnet-5-5");
        assert_eq!(route_model(&strong, "turn the volume up", "haiku", &all), "haiku", "a simple request stays on the primary");
    }

    #[test]
    fn a_strong_model_without_a_saved_key_is_not_chosen() {
        let strong = Config::parse(r#"{"ai":{"strongModel":"gpt-4o-mini"}}"#);
        let no_openai = |id: &str| id != "gpt-4o-mini";
        assert_eq!(route_model(&strong, "compare two laptops and then book one", "claude-haiku-5-5", &no_openai), "claude-haiku-5-5");
    }

    #[test]
    fn the_fallback_chain_starts_with_the_chosen_model_and_skips_repeats() {
        let all = |_: &str| true;
        let cfg = Config::parse(r#"{"ai":{"fallbackModels":["gpt-4o-mini","","claude-haiku-5-5","gpt-4o-mini"]}}"#);
        assert_eq!(fallback_chain(&cfg, "claude-haiku-5-5", &all), ["claude-haiku-5-5", "gpt-4o-mini"]);
        assert_eq!(fallback_chain(&Config::parse("{}"), "a", &all), ["a"], "no fallbacks: just the one model");
        let picked = Config::parse(r#"{"ai":{"fallback1":"gpt-4o-mini","fallback2":"gpt-4o-mini"}}"#);
        assert_eq!(fallback_chain(&picked, "claude-sonnet-5-5", &all), ["claude-sonnet-5-5", "gpt-4o-mini"], "the two Settings picks, with a repeat dropped");
        let off = Config::parse(r#"{"ai":{"fallback1":"","fallback2":"claude-haiku-5-5"}}"#);
        assert_eq!(fallback_chain(&off, "claude-sonnet-5-5", &all), ["claude-sonnet-5-5", "claude-haiku-5-5"], "Off leaves no blank model");
    }

    #[test]
    fn a_fallback_without_a_saved_key_is_skipped_but_the_chosen_model_is_kept() {
        let cfg = Config::parse(r#"{"ai":{"fallback1":"gpt-4o-mini","fallback2":"claude-haiku-5-5"}}"#);
        let no_openai = |id: &str| id != "gpt-4o-mini";
        assert_eq!(fallback_chain(&cfg, "gpt-4o-mini", &no_openai), ["gpt-4o-mini", "claude-haiku-5-5"], "the user's own choice is never skipped");
        assert_eq!(fallback_chain(&cfg, "claude-sonnet-5-5", &no_openai), ["claude-sonnet-5-5", "claude-haiku-5-5"], "the unusable fallback is left out");
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
