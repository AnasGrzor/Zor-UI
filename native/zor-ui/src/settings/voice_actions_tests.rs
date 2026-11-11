//! Tests for the Voice page's actions, run against the Settings state. Config writes are skipped in tests
//! (store::config_set), and reads see the pending writes, so each test reads back what an action saved.

use super::*;

fn st() -> Settings { Settings::new((1200.0, 800.0)) }

/// The Voice page's draft and the engine are process-wide, so these tests take turns.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|p| p.into_inner()) }

/// Creates a voice called `name` through the Studio's own form, on `provider`/`voice`, and returns its id.
fn create_on(st: &mut Settings, cx: &mut Ctx, name: &str, provider: &str, voice: &str) -> String {
    let (p, v) = (provider.to_string(), voice.to_string());
    crate::voice::library::update_draft(move |d| { d.provider = p; d.voice = v; d.speed = 1.0; });
    st.fields.insert("voice-name".into(), Field::new(false));
    st.fields.get_mut("voice-name").unwrap().tb.set(name);
    st.do_named(cx, "voice-add", String::new());
    let list = st.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
    list.iter().find(|v| v["name"] == name).and_then(|v| v["id"].as_str()).map(String::from).expect("voice saved")
}

fn create(st: &mut Settings, cx: &mut Ctx, name: &str) -> String { create_on(st, cx, name, "windows", "male") }

fn saved(st: &Settings, cx: &Ctx, id: &str) -> crate::voice::profile::VoiceProfile {
    let list = st.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
    let entry = list.iter().find(|v| v["id"] == id).expect("still saved");
    crate::voice::profile::VoiceProfile::from_json(entry).unwrap()
}

#[test]
fn a_voice_made_on_the_page_can_be_put_on_a_buddy_straight_away() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    let id = create(&mut st, &mut cx, "Calm");
    st.do_named(&mut cx, "voice-assign", format!("a:{id}"));
    assert_eq!(st.gs(&cx, "voice.buddies.a"), id, "assigned without waiting for the engine to reload");
}

#[test]
fn an_unknown_voice_is_refused_and_the_slot_keeps_its_voice() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    let before = st.gs(&cx, "voice.buddies.b");
    st.do_named(&mut cx, "voice-assign", "b:no-such-voice".into());
    assert_eq!(st.gs(&cx, "voice.buddies.b"), before);
}

#[test]
fn a_voice_on_a_provider_that_is_not_active_is_refused() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    // an OpenAI voice has no binding for the Windows voice that is active in tests
    let id = create_on(&mut st, &mut cx, "Remote", "openai", "alloy");
    let before = st.gs(&cx, "voice.buddies.a");
    st.do_named(&mut cx, "voice-assign", format!("a:{id}"));
    assert_eq!(st.gs(&cx, "voice.buddies.a"), before, "refused: no voice for the active provider");
}

#[test]
fn speed_is_saved_and_clamped_to_the_range_the_providers_take() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    let id = create(&mut st, &mut cx, "Quick");
    st.do_named(&mut cx, "voice-speed", format!("{id}:9"));
    assert_eq!(saved(&st, &cx, &id).settings.speed, 4.0);
    st.do_named(&mut cx, "voice-speed", format!("{id}:0.85"));
    assert!((saved(&st, &cx, &id).settings.speed - 0.85).abs() < 1e-4);
}

#[test]
fn a_model_is_saved_only_when_the_provider_offers_it() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    let windows = create(&mut st, &mut cx, "Plain");
    st.do_named(&mut cx, "voice-model", format!("{windows}:windows:tts-1"));
    assert!(saved(&st, &cx, &windows).binding_for("windows").unwrap().model.is_none(), "Windows has no models");

    let remote = create_on(&mut st, &mut cx, "Remote", "openai", "alloy");
    st.do_named(&mut cx, "voice-model", format!("{remote}:openai:tts-1-hd"));
    assert_eq!(saved(&st, &cx, &remote).binding_for("openai").unwrap().model.as_deref(), Some("tts-1-hd"));
    st.do_named(&mut cx, "voice-model", format!("{remote}:openai:not-a-model"));
    assert_eq!(saved(&st, &cx, &remote).binding_for("openai").unwrap().model.as_deref(), Some("tts-1-hd"), "an unknown model changes nothing");
}

#[test]
fn a_voice_a_buddy_uses_cannot_be_deleted_until_the_buddy_lets_go() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    let id = create(&mut st, &mut cx, "Held");
    st.do_named(&mut cx, "voice-assign", format!("a:{id}"));
    st.do_named(&mut cx, "voice-delete", id.clone());
    assert_eq!(st.get(&cx, "voice.profiles").as_array().map(Vec::len), Some(1), "refused while Buddy A uses it");
    st.set("voice.buddies.a", json!("builtin:gojo"));
    st.do_named(&mut cx, "voice-delete", id);
    assert_eq!(st.get(&cx, "voice.profiles").as_array().map(Vec::len), Some(0), "deleted once no Buddy uses it");
}

#[test]
fn built_in_voices_cannot_be_deleted_or_given_a_speed() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    st.do_named(&mut cx, "voice-delete", "builtin:gojo".into());
    st.do_named(&mut cx, "voice-speed", "builtin:gojo:0.5".into());
    assert!(st.get(&cx, "voice.profiles").as_array().map_or(true, Vec::is_empty), "nothing saved for built-ins");
}

#[test]
fn discard_clears_the_studio_recording_and_leaves_a_picked_file_alone() {
    let _g = serial();
    let mut st = st();
    let mut cx = Ctx::new(false);
    let picked = std::path::PathBuf::from(r"C:\Users\someone\Music\take1.wav");
    crate::voice::reference::set_draft(crate::voice::reference::Draft { path: Some(picked.clone()), check: None, note: "x".into() });
    st.do_named(&mut cx, "voice-discard", String::new());
    assert_eq!(crate::voice::reference::draft(), Default::default());
    assert!(!picked.starts_with(std::env::temp_dir()), "a file the user picked is not in the temp folder, so it is kept");
}
