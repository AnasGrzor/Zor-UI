//! Which model answers a request. The configured model is the default. A request that looks multi-step goes to the strong
//! model when one is set (`ai.strongModel`). If a model fails before any tool has run, the next model in `ai.fallbackModels`
//! tries the same request. A failure after a tool ran is never retried on another model: the tool may already have acted.

use super::provider;
use crate::conf::Config;

/// Words that mark a request as several steps or a comparison.
const MULTI_STEP: [&str; 9] = ["then", "after", "compare", "research", "plan", "steps", "step", "first", "comparison"];

/// Whether a request is several steps or a comparison. A planning call is made only for these (plan.rs).
pub fn looks_multi_step(said: &str) -> bool {
    let words: Vec<String> = said.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
    let text = format!(" {} ", words.join(" "));
    MULTI_STEP.iter().any(|w| text.contains(&format!(" {w} ")))
}

/// Words that mark a request as coding work.
const CODING: [&str; 14] = ["code", "function", "bug", "compile", "refactor", "rust", "python", "script", "repo", "repository", "debug", "test", "tests", "implement"];

/// Whether a request is about writing or fixing code. Such a request keeps the strong model, like a multi-step one.
pub fn looks_coding(said: &str) -> bool {
    said.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| CODING.contains(&w))
}

/// Why a model was picked. Logged as a word, never with the request's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason { Simple, Complex, Vision, Cooldown, Latency }

impl Reason {
    pub fn word(self) -> &'static str {
        match self { Reason::Simple => "simple", Reason::Complex => "complex", Reason::Vision => "vision", Reason::Cooldown => "cooldown", Reason::Latency => "latency" }
    }
}

/// What the request needs.
#[derive(Clone, Copy, Debug)]
pub struct Need {
    /// Multi-step or coding: keeps the strong model.
    pub complex: bool,
    /// Pictures will be sent, so the model must see.
    pub vision: bool,
    /// Rough size of the request in tokens, to leave out a model whose known window is smaller.
    pub tokens: u32,
}

/// What is known about the models, by model id. Closures, so a test can set each fact.
pub struct Facts<'a> {
    /// The provider can be used now (its key is saved).
    pub usable: &'a dyn Fn(&str) -> bool,
    /// The model is resting after failures.
    pub cooling: &'a dyn Fn(&str) -> bool,
    /// Cost rank (`Price::score`); None when the price is not known.
    pub cost: &'a dyn Fn(&str) -> Option<f64>,
    /// Recent latency in ms; None when there is no figure.
    pub latency: &'a dyn Fn(&str) -> Option<f64>,
    pub can_see: &'a dyn Fn(&str) -> bool,
    /// The known context window; None when unknown.
    pub window: &'a dyn Fn(&str) -> Option<u32>,
}

/// A candidate must be faster than the user's model by this much to take a request, so noise never moves it.
const FASTER_BY: f64 = 0.8;

/// The model for a request, from the user's model, the strong model (may be empty) and the other models of the user's chain.
///
/// - Complex or coding: the strong model when it is usable, not resting and big enough.
/// - Vision: the user's model when it can see, else the first other model that can.
/// - Simple: the user's model, unless another chain model has a known, strictly lower price (an unknown price never counts), or the
///   same price and a clearly lower latency.
/// - A resting user's model gives way to another usable model, and stays in the chain after it (`fallback_chain`).
/// The user's model is the answer whenever nothing above applies, including when its key is missing.
pub fn decide(primary: &str, strong: &str, others: &[String], need: &Need, f: &Facts) -> (String, Reason) {
    let ok = |m: &str| (f.usable)(m) && !(f.cooling)(m) && (!need.vision || (f.can_see)(m)) && (f.window)(m).is_none_or(|w| need.tokens <= w);
    let base = if need.vision { Reason::Vision } else if need.complex { Reason::Complex } else { Reason::Simple };
    if need.complex && !need.vision && !strong.is_empty() && strong != primary && ok(strong) { return (strong.to_string(), Reason::Complex); }
    let primary_ok = (f.usable)(primary) && !(f.cooling)(primary) && (!need.vision || (f.can_see)(primary));
    let candidates = || others.iter().filter(|m| m.as_str() != primary && ok(m));
    if !primary_ok {
        if !(f.usable)(primary) { return (primary.to_string(), base); } // no saved key: the user's choice stays, the chain falls back
        // cheapest known price first, else the first in the chain
        let pick = candidates().min_by(|a, b| (f.cost)(a).unwrap_or(f64::MAX).total_cmp(&(f.cost)(b).unwrap_or(f64::MAX)));
        let why = if (f.cooling)(primary) { Reason::Cooldown } else { Reason::Vision };
        return pick.map_or((primary.to_string(), base), |m| (m.clone(), why));
    }
    if base == Reason::Simple {
        if let Some(p0) = (f.cost)(primary) {
            let cheapest = candidates().filter_map(|m| (f.cost)(m).map(|c| (m, c))).min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((m, c)) = cheapest {
                if c < p0 - 1e-9 { return (m.clone(), Reason::Simple); }
                if (c - p0).abs() <= 1e-9 {
                    if let (Some(lm), Some(lp)) = ((f.latency)(m), (f.latency)(primary)) {
                        if lm < lp * FASTER_BY { return (m.clone(), Reason::Latency); }
                    }
                }
            }
        }
    }
    (primary.to_string(), base)
}

/// The fallbacks the user picked in Settings, then any list in config.json, without empty names.
fn configured_fallbacks(cfg: &Config) -> Vec<String> {
    let mut fallbacks: Vec<String> = ["ai.fallback1", "ai.fallback2"].iter().map(|k| cfg.str(k).trim().to_string()).collect();
    fallbacks.extend(cfg.get("ai.fallbackModels").as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|m| m.trim().to_string()));
    fallbacks.retain(|m| !m.is_empty());
    fallbacks
}

/// The model for this request (`decide`) with the live facts: saved keys (`usable`, by model id), the health figures, the price table
/// and the known windows. No model call is made. The route is logged with a reason word and no request text.
pub fn route_model(cfg: &Config, said: &str, primary: &str, usable: &dyn Fn(&str) -> bool) -> String {
    let need = Need { complex: looks_multi_step(said) || looks_coding(said), vision: false, tokens: (said.len() / 3) as u32 };
    // opt-in: by default the user's own model answers every request it can, as before
    let cost_on = cfg.get("ai.routeByCost").as_bool().unwrap_or(false);
    let cost = |m: &str| if cost_on { provider::price(m).map(|p| p.score()) } else { None };
    let latency = |m: &str| provider::health_of(m).and_then(|h| h.latency_ms());
    let can_see = |m: &str| crate::bridge::model_can_see(m);
    let window = |m: &str| provider::known_window(m, cfg);
    let facts = Facts { usable, cooling: &provider::cooling, cost: &cost, latency: &latency, can_see: &can_see, window: &window };
    let (model, why) = decide(primary, cfg.str("ai.strongModel").trim(), &configured_fallbacks(cfg), &need, &facts);
    crate::log::line(&format!("buddy route: {model} ({})", why.word()));
    model
}

/// The models to try, in order: the chosen one, then the two fallbacks picked in Settings, then any list in config.json, then the
/// user's own model if the route left it out (a strong or cheaper pick went first). Empty and repeated names are skipped, and so is
/// a fallback whose provider can't be used now (`usable`, by model id) or that is resting after failures. When every fallback has a
/// latency figure, the faster one goes first.
pub fn fallback_chain(cfg: &Config, first: &str, usable: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut rest: Vec<String> = vec![];
    for m in configured_fallbacks(cfg) {
        if m == first || rest.contains(&m) { continue; }
        if !usable(&m) { crate::log::line(&format!("buddy route: skipping {m}, no saved key")); }
        else if provider::cooling(&m) { crate::log::line(&format!("buddy route: skipping {m}, resting after failures")); }
        else { rest.push(m); }
    }
    let lat: Vec<Option<f64>> = rest.iter().map(|m| provider::health_of(m).and_then(|h| h.latency_ms())).collect();
    // the user's fallback order stands unless they opted in to routing (`ai.routeByCost`)
    if cfg.get("ai.routeByCost").as_bool().unwrap_or(false) && rest.len() > 1 && lat.iter().all(Option::is_some) {
        let mut pairs: Vec<(String, f64)> = rest.into_iter().zip(lat.into_iter().flatten()).collect();
        pairs.sort_by(|a, b| a.1.total_cmp(&b.1));
        rest = pairs.into_iter().map(|p| p.0).collect();
    }
    let mut chain = vec![first.to_string()];
    chain.extend(rest);
    let own = cfg.str("ai.model").trim();
    if !own.is_empty() && !chain.iter().any(|m| m == own) { chain.push(own.to_string()); }
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
        let cfg = Config::parse(r#"{"ai":{"model":"claude-haiku-5-5","fallbackModels":["gpt-4o-mini","","claude-haiku-5-5","gpt-4o-mini"]}}"#);
        assert_eq!(fallback_chain(&cfg, "claude-haiku-5-5", &all), ["claude-haiku-5-5", "gpt-4o-mini"]);
        assert_eq!(fallback_chain(&Config::parse(r#"{"ai":{"model":"a"}}"#), "a", &all), ["a"], "no fallbacks: just the one model");
        let picked = Config::parse(r#"{"ai":{"model":"claude-sonnet-5-5","fallback1":"gpt-4o-mini","fallback2":"gpt-4o-mini"}}"#);
        assert_eq!(fallback_chain(&picked, "claude-sonnet-5-5", &all), ["claude-sonnet-5-5", "gpt-4o-mini"], "the two Settings picks, with a repeat dropped");
        let off = Config::parse(r#"{"ai":{"model":"claude-sonnet-5-5","fallback1":"","fallback2":"claude-haiku-5-5"}}"#);
        assert_eq!(fallback_chain(&off, "claude-sonnet-5-5", &all), ["claude-sonnet-5-5", "claude-haiku-5-5"], "Off leaves no blank model");
    }

    #[test]
    fn a_fallback_without_a_saved_key_is_skipped_but_the_chosen_model_is_kept() {
        let own_openai = Config::parse(r#"{"ai":{"model":"gpt-4o-mini","fallback1":"gpt-4o-mini","fallback2":"claude-haiku-5-5"}}"#);
        let own_sonnet = Config::parse(r#"{"ai":{"model":"claude-sonnet-5-5","fallback1":"gpt-4o-mini","fallback2":"claude-haiku-5-5"}}"#);
        let no_openai = |id: &str| id != "gpt-4o-mini";
        assert_eq!(fallback_chain(&own_openai, "gpt-4o-mini", &no_openai), ["gpt-4o-mini", "claude-haiku-5-5"], "the user's own choice is never skipped");
        assert_eq!(fallback_chain(&own_sonnet, "claude-sonnet-5-5", &no_openai), ["claude-sonnet-5-5", "claude-haiku-5-5"], "the unusable fallback is left out");
    }

    /// One row of the route table: the facts a case sets, and the model and reason it must come to.
    struct Case { name: &'static str, complex: bool, vision: bool, tokens: u32, no_key: &'static [&'static str], cooling: &'static [&'static str],
        latency: &'static [(&'static str, f64)], want: (&'static str, Reason) }

    /// The user's model `big` costs 16, `mid` 4, `small` 1, `twin` 4 (same as mid), `mystery` has no price. `big` and `mid` can see.
    fn route(c: &Case, primary: &str, strong: &str, others: &[&str]) -> (String, Reason) {
        let others: Vec<String> = others.iter().map(|s| s.to_string()).collect();
        let usable = |m: &str| !c.no_key.contains(&m);
        let cooling = |m: &str| c.cooling.contains(&m);
        let cost = |m: &str| match m { "big" => Some(16.0), "mid" | "twin" => Some(4.0), "small" => Some(1.0), "tiny" => Some(0.5), _ => None };
        let latency = |m: &str| c.latency.iter().find(|(n, _)| *n == m).map(|(_, l)| *l);
        let can_see = |m: &str| m == "big" || m == "mid";
        let window = |m: &str| if m == "tiny" { Some(100) } else { None };
        let f = Facts { usable: &usable, cooling: &cooling, cost: &cost, latency: &latency, can_see: &can_see, window: &window };
        decide(primary, strong, &others, &Need { complex: c.complex, vision: c.vision, tokens: c.tokens }, &f)
    }

    const NONE: &[&str] = &[];
    fn case(name: &'static str, want: (&'static str, Reason)) -> Case {
        Case { name, complex: false, vision: false, tokens: 10, no_key: NONE, cooling: NONE, latency: &[], want }
    }

    #[test]
    fn the_route_table_picks_the_expected_model() {
        use Reason::*;
        // (case, primary, strong, others)
        let table: Vec<(Case, &str, &str, Vec<&str>)> = vec![
            (case("simple goes to the cheapest priced model", ("small", Simple)), "big", "big", vec!["mid", "small"]),
            (case("simple with no cheaper model stays", ("mid", Simple)), "mid", "", vec!["big", "twin"]),
            (Case { complex: true, ..case("complex keeps the strong model", ("big", Complex)) }, "mid", "big", vec!["small"]),
            (Case { complex: true, no_key: &["big"], ..case("a strong model without a key is not used", ("mid", Complex)) }, "mid", "big", vec!["small"]),
            (Case { complex: true, cooling: &["big"], ..case("a resting strong model is not used", ("mid", Complex)) }, "mid", "big", vec!["small"]),
            (Case { complex: true, ..case("complex never goes cheaper", ("mid", Complex)) }, "mid", "", vec!["small"]),
            (Case { vision: true, ..case("vision keeps a seeing user model", ("mid", Vision)) }, "mid", "", vec!["big"]),
            (Case { vision: true, ..case("vision moves off a blind model to one that sees", ("big", Vision)) }, "small", "", vec!["mystery", "big"]),
            (Case { vision: true, ..case("vision with nobody able to see stays", ("small", Vision)) }, "small", "", vec!["mystery"]),
            (case("an unknown price is never the cheapest", ("mid", Simple)), "mid", "", vec!["mystery"]),
            (case("an unknown price on the user's model means no cost routing", ("mystery", Simple)), "mystery", "", vec!["small"]),
            (Case { no_key: &["small"], ..case("the cheapest without a key is passed over", ("mid", Simple)) }, "big", "", vec!["small", "mid"]),
            (Case { no_key: &["big"], ..case("the user's model without a key is kept", ("big", Simple)) }, "big", "", vec!["small"]),
            (Case { cooling: &["small"], ..case("a resting cheap model is passed over", ("mid", Simple)) }, "big", "", vec!["small", "mid"]),
            (Case { cooling: &["big"], ..case("a resting user model gives way", ("small", Cooldown)) }, "big", "", vec!["mystery", "small"]),
            (Case { cooling: &["big"], ..case("a resting user model with no other choice stays", ("big", Simple)) }, "big", "", vec![]),
            (Case { tokens: 500, ..case("a model whose window is too small is passed over", ("big", Simple)) }, "big", "", vec!["tiny"]),
            (Case { latency: &[("mid", 900.0), ("twin", 400.0)], ..case("equal price, clearly faster wins", ("twin", Latency)) }, "mid", "", vec!["twin"]),
            (Case { latency: &[("mid", 500.0), ("twin", 450.0)], ..case("equal price, a small gap is noise", ("mid", Simple)) }, "mid", "", vec!["twin"]),
            (Case { latency: &[("twin", 100.0)], ..case("no figure for the user's model: no switch on speed", ("mid", Simple)) }, "mid", "", vec!["twin"]),
            (Case { latency: &[("small", 50.0), ("big", 9000.0)], ..case("speed never beats a lower price", ("small", Simple)) }, "big", "", vec!["small"]),
        ];
        for (c, primary, strong, others) in &table {
            assert_eq!(route(c, primary, strong, others), (c.want.0.to_string(), c.want.1), "{}", c.name);
        }
    }

    #[test]
    fn coding_and_multi_step_requests_are_complex_and_plain_ones_are_not() {
        assert!(looks_coding("fix the bug in my rust function"));
        assert!(!looks_coding("turn the volume up"));
        assert!(!looks_coding("what is the contest about"), "a word that only contains a coding word is not one");
    }

    #[test]
    fn the_live_route_keeps_the_user_model_without_prices_and_a_resting_fallback_is_dropped() {
        let all = |_: &str| true;
        let cfg = Config::parse(r#"{"ai":{"model":"user-model-z","fallback1":"rest-model-z","fallback2":"ok-model-z"}}"#);
        assert_eq!(route_model(&cfg, "turn the volume up", "user-model-z", &all), "user-model-z", "no known price: no cost routing");
        for _ in 0..provider::FAILS_TO_COOL { provider::note_call("rest-model-z", false, None, 10); }
        assert!(provider::cooling("rest-model-z"));
        assert_eq!(fallback_chain(&cfg, "user-model-z", &all), ["user-model-z", "ok-model-z"], "the resting fallback is skipped");
    }

    #[test]
    fn a_route_that_leaves_the_users_model_first_keeps_it_in_the_chain() {
        let all = |_: &str| true;
        let cfg = Config::parse(r#"{"ai":{"model":"claude-haiku-5-5","fallback1":"gpt-4o-mini"}}"#);
        assert_eq!(fallback_chain(&cfg, "claude-sonnet-5-5", &all), ["claude-sonnet-5-5", "gpt-4o-mini", "claude-haiku-5-5"], "tried last, never dropped");
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
