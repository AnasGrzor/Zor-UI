//! The "voice" section of config.json and its versioned migrations. Old installs have no section: their
//! Buddies keep the built-in voices they had, and the on/off switch stays `native.gojoVoice`.

use super::endpoint::EndpointCfg;
use super::profile::{builtin_profiles, default_voice, VoiceProfile};
use serde_json::Value;

/// Schema of the "voice" section. Bump it with a new arm in `migrate` when the layout changes.
pub const SCHEMA: u64 = 1;

#[derive(Debug, PartialEq)]
pub struct VoiceConfig {
    /// Id of the provider to run ("windows" unless the user picked another).
    pub provider: String,
    /// Voice profile id for Buddy slot a and slot b.
    pub buddies: [String; 2],
    /// Built-in profiles first, then the user's.
    pub profiles: Vec<VoiceProfile>,
    /// The custom OpenAI-compatible endpoint, when the user has set one up.
    pub endpoint: Option<EndpointCfg>,
}

impl VoiceConfig {
    /// Reads the section, migrating it first. `a_char` and `b_char` are the characters the two Buddies
    /// already wear, so a migrated install keeps each one's built-in voice.
    pub fn load(section: &Value, a_char: &str, b_char: &str) -> VoiceConfig {
        let migrated = migrate(section, a_char, b_char);
        let mut profiles = builtin_profiles();
        for p in migrated.get("profiles").and_then(Value::as_array).into_iter().flatten().filter_map(VoiceProfile::from_json) {
            if !profiles.iter().any(|b| b.id == p.id) { profiles.push(p); }
        }
        let slot = |key: &str| migrated["buddies"][key].as_str().filter(|s| !s.is_empty()).map(String::from);
        VoiceConfig {
            provider: migrated["provider"].as_str().filter(|s| !s.is_empty()).unwrap_or("windows").to_string(),
            buddies: [slot("a").unwrap_or_else(|| default_voice(a_char)), slot("b").unwrap_or_else(|| default_voice(b_char))],
            profiles,
            endpoint: endpoint_of(&migrated["endpoint"]),
        }
    }
}

/// The endpoint from its config entry: {url, model, voices: [...]}. A missing address means no endpoint.
pub fn endpoint_of(e: &Value) -> Option<EndpointCfg> {
    let url = e.get("url")?.as_str()?.trim();
    if url.is_empty() { return None; }
    // a list (what the field saves once it has one) or the comma-separated text it starts as
    let voices: Vec<String> = match e.get("voices") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => vec![],
    };
    Some(EndpointCfg {
        url: url.to_string(),
        model: e.get("model").and_then(Value::as_str).filter(|m| !m.is_empty()).unwrap_or("tts-1").to_string(),
        voices,
    })
}

/// Brings a "voice" section to `SCHEMA`. A section from a newer app is returned as it is, since a rewrite
/// would drop fields this build does not know.
pub fn migrate(section: &Value, a_char: &str, b_char: &str) -> Value {
    let schema = section.get("schema").and_then(Value::as_u64).unwrap_or(0);
    if schema >= SCHEMA { return section.clone(); }
    // 0 -> 1: the section did not exist; buddies start on their built-in character voices.
    let mut out = section.as_object().cloned().unwrap_or_default();
    out.insert("schema".into(), Value::from(SCHEMA));
    out.entry("provider").or_insert_with(|| Value::from("windows"));
    out.entry("profiles").or_insert_with(|| Value::Array(vec![]));
    let mut buddies = out.get("buddies").and_then(Value::as_object).cloned().unwrap_or_default();
    buddies.entry("a").or_insert_with(|| Value::from(default_voice(a_char)));
    buddies.entry("b").or_insert_with(|| Value::from(default_voice(b_char)));
    out.insert("buddies".into(), Value::Object(buddies));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::profile;
    use serde_json::json;

    #[test]
    fn an_install_without_the_section_keeps_its_built_in_voices() {
        let c = VoiceConfig::load(&Value::Null, "gojo", "geto");
        assert_eq!(c.provider, "windows");
        assert_eq!(c.buddies, ["builtin:gojo".to_string(), "builtin:geto".to_string()]);
        assert_eq!(c.profiles.len(), profile::BUILTIN_CHARACTERS.len(), "only the built-ins exist yet");
    }

    #[test]
    fn migration_writes_the_current_schema_and_does_not_touch_saved_choices() {
        let saved = json!({ "schema": 1, "provider": "elevenlabs", "buddies": { "a": "user:7", "b": "builtin:yuji" } });
        assert_eq!(migrate(&saved, "gojo", "geto"), saved);
        let c = VoiceConfig::load(&saved, "gojo", "geto");
        assert_eq!(c.provider, "elevenlabs");
        assert_eq!(c.buddies[0], "user:7");
    }

    #[test]
    fn a_newer_schema_is_left_alone() {
        let future = json!({ "schema": 9, "provider": "x" });
        assert_eq!(migrate(&future, "gojo", "geto"), future);
    }

    #[test]
    fn an_endpoint_needs_an_address_and_takes_its_voices() {
        assert!(endpoint_of(&json!({})).is_none());
        let e = endpoint_of(&json!({ "url": "http://localhost:8880/v1", "voices": ["af_sky", " "] })).unwrap();
        assert_eq!(e.voices, vec!["af_sky".to_string()]);
        assert_eq!(e.model, "tts-1", "model defaults to tts-1");
        let typed = endpoint_of(&json!({ "url": "https://x.test", "voices": "a, b ,," })).unwrap();
        assert_eq!(typed.voices, vec!["a".to_string(), "b".to_string()], "comma-separated text works too");
    }

    #[test]
    fn an_unknown_character_falls_back_to_the_neutral_voice() {
        let c = VoiceConfig::load(&Value::Null, "custom-pack", "gojo");
        assert_eq!(c.buddies[0], "builtin:neutral");
    }

    #[test]
    fn user_profiles_load_and_damaged_ones_are_skipped() {
        let section = json!({ "schema": 1, "profiles": [
            { "id": "user-1", "name": "Mine", "bindings": [] },
            { "name": "no id" },
            { "id": "builtin:gojo", "name": "shadow" }
        ]});
        let c = VoiceConfig::load(&section, "gojo", "geto");
        assert_eq!(c.profiles.len(), profile::BUILTIN_CHARACTERS.len() + 1);
        assert!(c.profiles.iter().any(|p| p.id == "user-1"));
        assert!(c.profiles.iter().any(|p| p.id == "builtin:gojo" && p.name == "gojo"), "a user entry cannot replace a built-in");
    }
}
