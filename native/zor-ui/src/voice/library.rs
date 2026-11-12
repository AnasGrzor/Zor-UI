//! The voice library: custom voices, their names, and which voice each Buddy uses. A custom voice is one provider
//! voice plus speech settings, kept in config.json under voice.profiles. The rules here are pure so the tests
//! can cover them; the Settings page calls them and writes the result.

use super::contract::{SpeechSettings, VoiceInfo, VoiceReference};
use super::profile::{default_voice, ProviderBinding, VoiceProfile, EXT_VERSION};
#[cfg(test)]
use super::profile::builtin_id;
use serde_json::{json, Value};
use std::sync::Mutex;

pub const MAX_NAME: usize = 40;

/// A trimmed name of 1 to 40 characters that is not already used by another custom voice.
pub fn check_name(list: &[VoiceProfile], name: &str, except: Option<&str>) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() { return Err("give the voice a name".into()); }
    if name.chars().count() > MAX_NAME { return Err(format!("the name can be at most {MAX_NAME} characters")); }
    let taken = list.iter().any(|p| Some(p.id.as_str()) != except && p.name.eq_ignore_ascii_case(name));
    if taken { return Err(format!("a voice called '{name}' already exists")); }
    Ok(name.to_string())
}

/// Appends a custom voice: `voice` on `provider`, with `speed`. Built-in ids are never used for custom voices.
pub fn add_custom(list: &mut Vec<Value>, id: &str, name: &str, provider: &str, voice: &str, model: Option<&str>, speed: f32) -> Result<(), String> {
    let existing: Vec<VoiceProfile> = list.iter().filter_map(VoiceProfile::from_json).collect();
    let name = check_name(&existing, name, None)?;
    if voice.is_empty() { return Err("pick a voice".into()); }
    let profile = VoiceProfile {
        id: id.to_string(),
        name,
        description: format!("{provider} voice {voice}"),
        tags: vec!["custom".into()],
        settings: SpeechSettings { speed: speed.clamp(0.25, 4.0), ..Default::default() },
        reference: None,
        bindings: vec![ProviderBinding { provider: provider.into(), voice: voice.into(), model: model.map(String::from), ext: json!({ "v": EXT_VERSION }) }],
    };
    list.push(profile.to_json());
    Ok(())
}

/// Renames custom voice `id`. Built-in voices cannot be renamed.
pub fn rename(list: &mut [Value], id: &str, name: &str) -> Result<(), String> {
    if id.starts_with("builtin:") { return Err("built-in voices keep their names".into()); }
    let existing: Vec<VoiceProfile> = list.iter().filter_map(VoiceProfile::from_json).collect();
    let name = check_name(&existing, name, Some(id))?;
    let entry = list.iter_mut().find(|v| v["id"].as_str() == Some(id)).ok_or("no such voice")?;
    entry["name"] = json!(name);
    Ok(())
}

/// Removes custom voice `id`. Refused while a Buddy uses it, so no Buddy is left silent or switched to another voice
/// without the user choosing it.
pub fn delete(list: &mut Vec<Value>, id: &str, assigned_to: &[&str]) -> Result<(), String> {
    if id.starts_with("builtin:") { return Err("built-in voices cannot be deleted".into()); }
    if assigned_to.contains(&id) { return Err("a Buddy uses this voice. Pick another voice for it first".into()); }
    let before = list.len();
    list.retain(|v| v["id"].as_str() != Some(id));
    if list.len() == before { return Err("no such voice".into()); }
    Ok(())
}

/// The voice a Buddy uses: its saved choice, else the built-in voice of its character.
pub fn effective(saved: &str, character: &str) -> String {
    if saved.is_empty() { default_voice(character) } else { saved.to_string() }
}

/// Keeps a recording as a custom voice with no provider yet: it can be cloned once a provider can clone.
pub fn create_reference(list: &mut Vec<Value>, id: &str, name: &str, reference: VoiceReference) -> Result<(), String> {
    let existing: Vec<VoiceProfile> = list.iter().filter_map(VoiceProfile::from_json).collect();
    let name = check_name(&existing, name, None)?;
    let profile = VoiceProfile {
        id: id.to_string(),
        name,
        description: "Recording kept for cloning".into(),
        tags: vec!["custom".into(), "reference".into()],
        settings: SpeechSettings::default(),
        reference: Some(reference),
        bindings: vec![],
    };
    list.push(profile.to_json());
    Ok(())
}

/// Stores `profile` in the list, replacing the entry with its id, or appending it when new.
pub fn put(list: &mut Vec<Value>, profile: &VoiceProfile) {
    let json = profile.to_json();
    match list.iter().position(|v| v["id"].as_str() == Some(profile.id.as_str())) {
        Some(i) => list[i] = json,
        None => list.push(json),
    }
}

/// Provider line for a voice row: "Windows · Piper (not installed) · OpenAI".
pub fn provider_note(v: &VoiceProfile, installed: &dyn Fn(&str) -> bool) -> String {
    if v.bindings.is_empty() {
        return if v.reference.is_some() { "Recording only. Needs a provider that can clone voices".into() } else { "No provider".into() };
    }
    v.bindings.iter().map(|b| {
        let name = provider_name(&b.provider);
        if b.provider == "piper" && !installed("piper") { format!("{name} (not installed)") } else { name.to_string() }
    }).collect::<Vec<_>>().join(" · ")
}

pub fn provider_name(id: &str) -> &str {
    match id { "windows" => "Windows", "piper" => "Piper", "openai" => "OpenAI", "elevenlabs" => "ElevenLabs", "endpoint" => "Custom endpoint", other => other }
}

/// Sets the speed of custom voice `id`, held to 0.25 to 4 as on creation.
pub fn set_speed(list: &mut [Value], id: &str, speed: f32) -> Result<(), String> {
    if id.starts_with("builtin:") { return Err("built-in voices keep their speed".into()); }
    let entry = list.iter_mut().find(|v| v["id"].as_str() == Some(id)).ok_or("no such voice")?;
    entry["settings"]["speed"] = json!(speed.clamp(0.25, 4.0));
    Ok(())
}

/// Sets the model custom voice `id` uses on `provider`.
pub fn set_model(list: &mut [Value], id: &str, provider: &str, model: &str) -> Result<(), String> {
    if id.starts_with("builtin:") { return Err("built-in voices keep their model".into()); }
    let entry = list.iter_mut().find(|v| v["id"].as_str() == Some(id)).ok_or("no such voice")?;
    let mut p = VoiceProfile::from_json(entry).ok_or("the voice is damaged")?;
    let b = p.bindings.iter_mut().find(|b| b.provider == provider).ok_or("this voice has no binding for that provider")?;
    b.model = Some(model.to_string());
    *entry = p.to_json();
    Ok(())
}

/// The voices a provider offers, as they appear in the creation picker.
pub fn choices(voices: &[VoiceInfo]) -> Vec<(String, String)> {
    voices.iter().map(|v| (v.id.clone(), if v.name.is_empty() { v.id.clone() } else { v.name.clone() })).collect()
}

/// The creation form's state: which provider and voice, and the speed.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub provider: String,
    pub voice: String,
    pub speed: f32,
}

impl Default for Draft {
    fn default() -> Self { Draft { provider: "windows".into(), voice: "male".into(), speed: 1.0 } }
}

static DRAFT: Mutex<Option<Draft>> = Mutex::new(None);

pub fn draft() -> Draft { DRAFT.lock().ok().and_then(|d| d.clone()).unwrap_or_default() }

pub fn update_draft(f: impl FnOnce(&mut Draft)) {
    if let Ok(mut d) = DRAFT.lock() {
        let mut cur = d.clone().unwrap_or_default();
        f(&mut cur);
        *d = Some(cur);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(id: &str, name: &str) -> Value {
        json!({ "id": id, "name": name, "bindings": [{ "provider": "windows", "voice": "male" }] })
    }

    #[test]
    fn names_are_trimmed_bounded_and_unique_ignoring_case() {
        let list = vec![VoiceProfile::from_json(&custom("u1", "Calm")).unwrap()];
        assert_eq!(check_name(&list, "  Warm  ", None).unwrap(), "Warm");
        assert!(check_name(&list, "   ", None).is_err());
        assert!(check_name(&list, &"x".repeat(MAX_NAME + 1), None).is_err());
        assert!(check_name(&list, "calm", None).is_err(), "same name, other case");
        assert!(check_name(&list, "calm", Some("u1")).is_ok(), "keeping its own name is fine");
    }

    #[test]
    fn adding_stores_a_profile_with_its_binding_and_speed() {
        let mut list = vec![];
        add_custom(&mut list, "u1", "Slow Sam", "openai", "onyx", Some("tts-1"), 9.0).unwrap();
        let p = VoiceProfile::from_json(&list[0]).unwrap();
        assert_eq!(p.name, "Slow Sam");
        assert_eq!(p.settings.speed, 4.0, "speed is clamped to the range the providers accept");
        let b = p.binding_for("openai").unwrap();
        assert_eq!((b.voice.as_str(), b.model.as_deref()), ("onyx", Some("tts-1")));
    }

    #[test]
    fn adding_needs_a_name_and_a_voice() {
        let mut list = vec![];
        assert!(add_custom(&mut list, "u1", "", "windows", "male", None, 1.0).is_err());
        assert!(add_custom(&mut list, "u1", "Name", "windows", "", None, 1.0).is_err());
        assert!(list.is_empty(), "nothing saved when refused");
    }

    #[test]
    fn rename_changes_only_custom_voices() {
        let mut list = vec![custom("u1", "Calm")];
        rename(&mut list, "u1", "Quiet").unwrap();
        assert_eq!(list[0]["name"], "Quiet");
        assert!(rename(&mut list, "builtin:gojo", "x").is_err());
    }

    #[test]
    fn a_voice_a_buddy_uses_cannot_be_deleted() {
        let mut list = vec![custom("u1", "Calm"), custom("u2", "Other")];
        assert!(delete(&mut list, "u1", &["u1"]).is_err(), "assigned: refused");
        assert_eq!(list.len(), 2);
        delete(&mut list, "u1", &["builtin:gojo"]).unwrap();
        assert_eq!(list.len(), 1);
        assert!(delete(&mut list, "builtin:gojo", &[]).is_err(), "built-ins are never deleted");
    }

    #[test]
    fn a_buddy_with_no_saved_choice_uses_its_characters_voice() {
        assert_eq!(effective("", "gojo"), "builtin:gojo");
        assert_eq!(effective("", "unknown-pack"), "builtin:neutral");
        assert_eq!(effective("user-7", "gojo"), "user-7", "a saved choice wins");
    }

    #[test]
    fn the_provider_line_says_when_piper_is_not_installed() {
        let gojo = builtin_profile("gojo");
        assert_eq!(provider_note(&gojo, &|_| false), "Windows · Piper (not installed) · OpenAI");
        assert_eq!(provider_note(&gojo, &|_| true), "Windows · Piper · OpenAI");
    }

    fn builtin_profile(c: &str) -> VoiceProfile {
        crate::voice::profile::builtin_profiles().into_iter().find(|p| p.id == builtin_id(c)).unwrap()
    }

    #[test]
    fn a_recording_becomes_a_voice_with_no_provider_until_cloned() {
        let mut list = vec![];
        let r = VoiceReference { path: "C:/refs/x/reference.wav".into(), transcript: "Hi.".into(), duration_secs: 5.0 };
        create_reference(&mut list, "r1", "My voice", r.clone()).unwrap();
        let p = VoiceProfile::from_json(&list[0]).unwrap();
        assert!(p.bindings.is_empty(), "no provider yet");
        assert_eq!(p.reference, Some(r));
        assert!(provider_note(&p, &|_| true).starts_with("Recording only"));
        assert!(create_reference(&mut list, "r2", "my voice", VoiceReference { path: "x".into(), transcript: String::new(), duration_secs: 0.0 }).is_err(), "names stay unique");
    }

    #[test]
    fn put_replaces_by_id_and_appends_new_ones() {
        let mut list = vec![];
        let mut p = VoiceProfile { id: "u1".into(), name: "A".into(), description: String::new(), tags: vec![], settings: SpeechSettings::default(), reference: None, bindings: vec![] };
        put(&mut list, &p);
        p.name = "B".into();
        put(&mut list, &p);
        assert_eq!(list.len(), 1, "same id: replaced, not duplicated");
        assert_eq!(list[0]["name"], "B");
    }

    #[test]
    fn speed_is_clamped_and_a_model_is_set_on_the_right_binding() {
        let mut list = vec![];
        add_custom(&mut list, "u1", "Voice", "openai", "onyx", None, 1.0).unwrap();
        set_speed(&mut list, "u1", 9.0).unwrap();
        assert_eq!(VoiceProfile::from_json(&list[0]).unwrap().settings.speed, 4.0);
        set_model(&mut list, "u1", "openai", "tts-1-hd").unwrap();
        assert_eq!(VoiceProfile::from_json(&list[0]).unwrap().binding_for("openai").unwrap().model.as_deref(), Some("tts-1-hd"));
        assert!(set_model(&mut list, "u1", "piper", "x").is_err(), "no binding for that provider");
        assert!(set_speed(&mut list, "builtin:gojo", 1.0).is_err());
    }

    #[test]
    fn the_draft_keeps_its_last_edit() {
        update_draft(|d| { d.provider = "openai".into(); d.speed = 1.2; });
        assert_eq!(draft().provider, "openai");
        assert_eq!(draft().speed, 1.2);
        update_draft(|d| { d.provider = "windows".into(); d.speed = 1.0; });
    }
}
