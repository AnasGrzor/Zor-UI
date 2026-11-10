//! Voice profiles: one logical voice (a Buddy's voice, or a custom one) with a binding per provider.
//! A profile outlives provider changes. Its provider-specific parts sit in each binding's `ext` object.

use super::contract::{SpeechSettings, VoiceReference};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Version of the binding `ext` object. Bump it when a provider changes what it stores there.
pub const EXT_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderBinding {
    pub provider: String,
    /// The provider's own voice id.
    pub voice: String,
    pub model: Option<String>,
    /// Provider-specific data, stored as `{"v": EXT_VERSION, ...}`.
    pub ext: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub settings: SpeechSettings,
    /// The user's authorised recording, kept so the voice can be rebuilt on another provider.
    pub reference: Option<VoiceReference>,
    pub bindings: Vec<ProviderBinding>,
}

impl VoiceProfile {
    pub fn binding_for(&self, provider: &str) -> Option<&ProviderBinding> {
        self.bindings.iter().find(|b| b.provider == provider)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "tags": self.tags,
            "settings": settings_json(&self.settings),
            "reference": self.reference.as_ref().map(|r| json!({
                "path": r.path.to_string_lossy(),
                "transcript": r.transcript,
                "duration": r.duration_secs,
            })),
            "bindings": self.bindings.iter().map(|b| json!({
                "provider": b.provider,
                "voice": b.voice,
                "model": b.model,
                "ext": b.ext,
            })).collect::<Vec<_>>(),
        })
    }

    /// Reads a profile written by `to_json`. Returns None when the id or name is missing, so a damaged entry
    /// is skipped instead of breaking the whole list.
    pub fn from_json(v: &Value) -> Option<VoiceProfile> {
        let id = v.get("id")?.as_str()?.to_string();
        let name = v.get("name")?.as_str()?.to_string();
        if id.is_empty() || name.is_empty() { return None; }
        let s = v.get("settings").unwrap_or(&Value::Null);
        let reference = v.get("reference").filter(|r| r.is_object()).map(|r| VoiceReference {
            path: PathBuf::from(r.get("path").and_then(Value::as_str).unwrap_or("")),
            transcript: r.get("transcript").and_then(Value::as_str).unwrap_or("").to_string(),
            duration_secs: r.get("duration").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        });
        let bindings = v.get("bindings").and_then(Value::as_array).map(|list| list.iter().filter_map(|b| {
            Some(ProviderBinding {
                provider: b.get("provider")?.as_str()?.to_string(),
                voice: b.get("voice")?.as_str()?.to_string(),
                model: b.get("model").and_then(Value::as_str).map(String::from),
                ext: b.get("ext").cloned().unwrap_or_else(|| json!({ "v": EXT_VERSION })),
            })
        }).collect()).unwrap_or_default();
        Some(VoiceProfile {
            id,
            name,
            description: v.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
            tags: v.get("tags").and_then(Value::as_array).map(|t| t.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default(),
            settings: SpeechSettings {
                speed: s.get("speed").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                pitch: s.get("pitch").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                expressiveness: s.get("expressiveness").and_then(Value::as_f64).unwrap_or(0.5) as f32,
                emotion: s.get("emotion").and_then(Value::as_str).map(String::from),
            },
            reference,
            bindings,
        })
    }
}

fn settings_json(s: &SpeechSettings) -> Value {
    json!({ "speed": s.speed, "pitch": s.pitch, "expressiveness": s.expressiveness, "emotion": s.emotion })
}

/// The characters the app ships with, and the Windows voice each one uses. Every one maps to the same
/// Windows voice, as before this platform existed, so Buddy sounds the same.
pub const BUILTIN_CHARACTERS: [&str; 4] = ["neutral", "gojo", "geto", "yuji"];

/// Profile id of a built-in character voice, e.g. `builtin:gojo`.
pub fn builtin_id(character: &str) -> String { format!("builtin:{character}") }

/// The built-in profiles: one per character, bound to the Windows voice.
pub fn builtin_profiles() -> Vec<VoiceProfile> {
    BUILTIN_CHARACTERS.iter().map(|c| VoiceProfile {
        id: builtin_id(c),
        name: c.to_string(),
        description: "Built-in character voice".into(),
        tags: vec!["built-in".into()],
        settings: SpeechSettings::default(),
        reference: None,
        bindings: vec![ProviderBinding {
            provider: "windows".into(),
            voice: "male".into(),
            model: None,
            ext: json!({ "v": EXT_VERSION }),
        }],
    }).collect()
}

/// A random UUID (version 4 layout) for a new profile. Not cryptographic; it only needs to be unique here.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut state = nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ ((std::process::id() as u64) << 32);
    let a = splitmix(&mut state);
    let b = splitmix(&mut state);
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32, a as u16, (a >> 16) as u16 & 0x0fff, ((b >> 48) as u16 & 0x3fff) | 0x8000, b & 0xffff_ffff_ffff)
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> VoiceProfile {
        VoiceProfile {
            id: "p1".into(),
            name: "Mine".into(),
            description: "warm".into(),
            tags: vec!["calm".into()],
            settings: SpeechSettings { speed: 1.2, pitch: -1.0, expressiveness: 0.8, emotion: Some("happy".into()) },
            reference: Some(VoiceReference { path: PathBuf::from("C:/ref.wav"), transcript: "hello".into(), duration_secs: 4.5 }),
            bindings: vec![ProviderBinding { provider: "windows".into(), voice: "male".into(), model: None, ext: json!({ "v": 1, "x": 2 }) }],
        }
    }

    #[test]
    fn json_round_trip_keeps_every_field() {
        let p = sample();
        assert_eq!(VoiceProfile::from_json(&p.to_json()), Some(p));
    }

    #[test]
    fn damaged_entries_are_skipped() {
        assert_eq!(VoiceProfile::from_json(&json!({ "name": "no id" })), None);
        assert_eq!(VoiceProfile::from_json(&json!({ "id": "", "name": "x" })), None);
    }

    #[test]
    fn a_binding_is_found_by_provider_and_bad_bindings_are_dropped() {
        let mut p = sample();
        assert!(p.binding_for("windows").is_some());
        assert!(p.binding_for("elevenlabs").is_none(), "no binding means the provider cannot speak it");
        let v = json!({ "id": "p", "name": "n", "bindings": [ { "provider": "windows" }, { "provider": "x", "voice": "y" } ] });
        p = VoiceProfile::from_json(&v).unwrap();
        assert_eq!(p.bindings.len(), 1, "a binding without a voice is dropped");
    }

    #[test]
    fn builtins_cover_every_character_on_the_windows_voice() {
        let b = builtin_profiles();
        assert_eq!(b.len(), BUILTIN_CHARACTERS.len());
        assert!(b.iter().all(|p| p.binding_for("windows").map(|x| x.voice.as_str()) == Some("male")));
        assert_eq!(builtin_id("gojo"), "builtin:gojo");
    }

    #[test]
    fn new_ids_are_uuid_shaped_and_differ() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4'), "version 4");
        assert!("89ab".contains(parts[3].chars().next().unwrap()), "RFC variant");
    }
}
