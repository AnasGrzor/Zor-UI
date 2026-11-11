//! config.json and theme.json from the pack folder (~.glzrzor-ui) (Settings and end4_theme.py write them).

use crate::gfx::Color;
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::PathBuf, time::SystemTime};

/// DEFAULTS in lib/core.js, plus "native": the parts this app draws (all on by default except the buddy; false turns one off).
fn defaults() -> Value {
    serde_json::from_str(r#"{
        "scriptsDir": "", "userName": "", "workspaces": 10, "workspacesMain": 5, "workspacesRebind": true, "clockFormat": "EEE, dd/MM '•' HH:mm", "use24h": true,
        "tempUnit": "C", "showKeyboardLayout": false, "showNetSpeed": false, "lyrics": true, "autoGameMode": true, "autoTrackAgents": true, "runOnStartup": true, "customAgents": [],
        "workspaceLabels": [], "alwaysShowNumbers": false, "transparency": 0, "desktopWidgets": true, "syncLockScreen": true,
        "ai": { "model": "gemini-2.5-flash", "temperature": 0.5, "systemPrompt": "", "claudeCli": true, "nativeTools": false, "memory": true },
        "claude": { "autoSwitch": false, "threshold": 95 },
        "translator": { "source": "auto", "target": "en" },
        "booru": { "source": "yandere", "allowNsfw": false },
        "football": { "leagues": [] },
        "visualizer": { "desktop": true, "bar": false, "bars": 64, "color": "", "desktopColor": "", "barColor": "" },
        "nightLight": { "kelvin": 4500 },
        "record": { "audio": "both", "mic": "" },
        "wallpaperEngine": { "follow": true, "path": "" },
        "wallpaperTheme": { "perMonitor": false, "adaptiveGlass": true, "reducedMotion": false },
        "timer": { "focus": 25, "short": 5, "long": 15, "cycles": 4 },
        "frame": { "enabled": false, "thickness": 5, "radius": 20 },
        "dock": { "enabled": false, "side": "bottom", "style": "hug", "iconSize": 30, "radius": 30, "pinned": [] },
        "bar": { "style": "float", "groups": "filled", "spacing": "cozy", "modules": null, "utils": ["snip", "pick", "wall", "mode"], "titleWidth": 34, "trayVisible": 3, "trayMatchWindows": true },
        "font": { "family": "", "size": 1, "weight": 0 },
        "island": {
            "notifications": true, "notifySeconds": 5, "volume": true, "mic": true, "osdSeconds": 2,
            "agents": true, "agentWorking": true, "recording": true, "timer": true, "privacy": true,
            "media": true, "art": true, "progress": true, "eq": true, "eqColor": "", "controls": true, "trackSeconds": 3,
            "idle": "auto", "status": true, "badges": true, "textWidth": 170, "wideWidth": 280,
            "click": "panel", "rightClick": "sidebar-left", "scroll": "volume",
            "panel": { "media": true, "lyrics": true, "recording": true, "timer": true, "clock": true, "agents": true, "notifications": true, "notifCount": 3 }
        },
        "desktop": { "widgets": ["clock", "weather", "week", "greet", "media"], "clock": "cookie", "clockVertical": false, "snap": true, "snapLines": true,
            "worldClocks": ["Europe/London", "America/New_York", "Asia/Tokyo"], "text": "" },
        "buddies": { "pair": true, "primary": "a", "contacts": [], "model": "", "listenMode": "auto", "mic": "", "breaks": true, "breakMins": 50, "watch": true,
            "listen": true, "listenAi": true, "screen": false, "screenModel": "", "drive": false, "browser": false, "driveRounds": 25, "driveSecs": 180, "allow": { "search": true, "open": true, "message": true, "media": true, "system": true },
            "a": { "character": "gojo", "name": "", "hair": "", "shirt": "", "pants": "", "eyes": "", "bun": "", "chatty": "normal", "walk": true },
            "b": { "on": false, "character": "geto", "name": "", "hair": "", "shirt": "", "pants": "", "eyes": "", "bun": "", "chatty": "normal", "walk": true } },
        "voice": { "provider": "windows" },
        "native": { "bar": true, "desktop": true, "dock": true, "island-panel": true, "overview": true, "settings": true, "sidebar-left": true, "sidebar-right": true, "theme": true, "tray-flyout": true, "visualizer": true }
    }"#).expect("defaults are valid JSON")
}

/// Objects merge key by key (core.js spreads each section over its defaults); anything else is replaced.
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => for (k, v) in o { merge(b.entry(k).or_insert(Value::Null), v) },
        (b, o) => *b = o,
    }
}

#[derive(Clone)]
pub struct Config(pub Value);

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut v = defaults();
        if let Ok(file @ Value::Object(_)) = serde_json::from_str::<Value>(text) { merge(&mut v, file); }
        Config(v)
    }
    /// config.json as it is on disk now (many callers hold a copy with only the script paths).
    pub fn load() -> Config { Config::parse(&fs::read_to_string(pack_dir().join("config.json")).unwrap_or_default()) }
    pub fn get(&self, path: &str) -> &Value {
        path.split('.').try_fold(&self.0, |v, k| v.get(k)).unwrap_or(&Value::Null)
    }
    /// A visualizer's color: its own setting (`own`), else the shared `visualizer.color`, else None (use the theme).
    pub fn viz_color(&self, own: &str) -> Option<Color> {
        parse_hex(self.str(own)).or_else(|| parse_hex(self.str("visualizer.color")))
    }
    pub fn bool(&self, path: &str) -> bool { self.get(path).as_bool().unwrap_or(false) }
    pub fn str(&self, path: &str) -> &str { self.get(path).as_str().unwrap_or("") }
    pub fn strs(&self, path: &str) -> Vec<String> {
        self.get(path).as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }
}

/// base.css :root fallback palette, used until theme.json is readable.
const FALLBACK: &[(&str, Color)] = &[
    ("background", 0xFF161217), ("onBackground", 0xFFEAE0E7), ("surface", 0xFF161217),
    ("surfaceContainerLowest", 0xFF110D12), ("surfaceContainerLow", 0xFF1F1A1F), ("surfaceContainer", 0xFF231E23),
    ("surfaceContainerHigh", 0xFF2D282E), ("surfaceContainerHighest", 0xFF383339), ("onSurface", 0xFFEAE0E7),
    ("onSurfaceVariant", 0xFFCFC3CD), ("outline", 0xFF988E97), ("outlineVariant", 0xFF4C444C),
    ("primary", 0xFFE5B6F2), ("onPrimary", 0xFF452152), ("primaryContainer", 0xFF5D386A), ("onPrimaryContainer", 0xFFF9D8FF),
    ("secondary", 0xFFD5C0D7), ("onSecondary", 0xFF392C3D), ("secondaryContainer", 0xFF534457), ("onSecondaryContainer", 0xFFF2DCF3),
    ("tertiary", 0xFFF5B7B5), ("onTertiary", 0xFF4C2523), ("tertiaryContainer", 0xFF663A39), ("onTertiaryContainer", 0xFFFFDAD8),
    ("error", 0xFFFFB4AB), ("onError", 0xFF690005), ("inverseSurface", 0xFFEAE0E7), ("inverseOnSurface", 0xFF342F34),
];

mod theme_effects;
pub use theme_effects::{surface_scope, hide_preview};
pub struct Theme {
    colors: HashMap<String, Color>, pub dark: bool,
    regions: Vec<(f64, f64)>,
    monitors: Vec<theme_effects::MonitorTheme>,
    transition: Option<theme_effects::Transition>,
}

impl Theme {
    pub fn parse(text: &str) -> Theme {
        let mut colors: HashMap<String, Color> = FALLBACK.iter().map(|&(k, v)| (k.to_string(), v)).collect();
        let raw: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        if let Some(map) = raw.get("colors").and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(c) = v.as_str().and_then(parse_hex) { colors.insert(k.clone(), c); }
            }
        }
        Theme { colors, dark: raw.get("mode").and_then(Value::as_str) != Some("light"),
            regions: theme_effects::regions(&raw["analysis"]), monitors: theme_effects::monitors(&raw), transition: None }
    }
    /// A Material colour by its theme.json name ("onSurface"); magenta if a name is mistyped.
    pub fn c(&self, name: &str) -> Color {
        PREVIEW.with(|p| p.borrow().as_ref().and_then(|m| m.get(name).copied()))
            .or_else(|| self.monitor().and_then(|m| m.colors.get(name).copied()))
            .or_else(|| self.colors.get(name).copied()).unwrap_or(0xFFFF_00FF)
    }
}

thread_local! { static PREVIEW: std::cell::RefCell<Option<HashMap<String, Color>>> = const { std::cell::RefCell::new(None) }; }

/// Paints every `Theme::c` on this thread from `colors` (a palette being previewed) until it is set to None; returns the previous one.
pub fn set_preview(colors: Option<HashMap<String, Color>>) -> Option<HashMap<String, Color>> { PREVIEW.with(|p| p.replace(colors)) }

pub fn parse_hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok().map(|c| 0xFF00_0000 | c),
        8 => u32::from_str_radix(h, 16).ok().map(|c| c.rotate_right(8)), // #RRGGBBAA -> AARRGGBB
        _ => None,
    }
}

/// Re-reads a file only when its modified time changes.
pub struct Watched { pub path: PathBuf, stamp: Option<SystemTime> }
impl Watched {
    pub fn new(path: PathBuf) -> Watched { Watched { path, stamp: None } }
    /// The new contents if the file changed since the last call (the first call always reads).
    pub fn changed(&mut self) -> Option<String> {
        let stamp = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if stamp.is_some() && stamp == self.stamp { return None; }
        if stamp.is_none() && self.stamp.is_none() { self.stamp = Some(SystemTime::UNIX_EPOCH); return Some(String::new()); }
        self.stamp = stamp;
        Some(fs::read_to_string(&self.path).unwrap_or_default())
    }
}

/// Small JSON state the widgets keep between runs (card positions, ...).
pub struct State { path: PathBuf, pub v: Value }
impl State {
    pub fn load(path: PathBuf) -> State {
        let v = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
        State { path, v }
    }
    /// Re-reads the file: the theme scripts and preset apply write it from other processes.
    pub fn reload(&mut self) { self.v = State::load(self.path.clone()).v; }
    /// Applies `f` to the file as it is now and writes it back. The theme scripts write this file too
    /// (live wallpaper, image, mode), so saving the copy read at start-up would erase their keys.
    pub fn update(&mut self, f: impl FnOnce(&mut Value)) {
        self.v = State::load(self.path.clone()).v;
        f(&mut self.v);
        // write then rename, so a crash mid-write never leaves half a file
        let tmp = self.path.with_extension("tmp");
        if fs::write(&tmp, self.v.to_string()).is_ok() { let _ = fs::rename(&tmp, &self.path); }
    }
}

pub fn pack_dir() -> PathBuf { home().join(r".glzr\zor-ui") }
pub fn home() -> PathBuf { PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()) }
pub fn data_dir() -> PathBuf {
    let d = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("zor-ui");
    let _ = fs::create_dir_all(&d);
    d
}
/// Where speech models go: one folder per model, each with its tokens.txt. Not created until something needs it.
pub fn speech_dir() -> PathBuf { data_dir().join("speech-models") }

#[cfg(test)]
mod tests {
    #[test]
    fn speech_models_live_in_the_app_folder_not_in_another_program() {
        let p = speech_dir().to_string_lossy().to_lowercase();
        assert!(p.ends_with(r"zor-ui\speech-models") && !p.contains("orca"));
    }

    #[test]
    fn every_part_is_native_unless_turned_off() {
        let c = Config::parse("{\"native\": {\"bar\": false}}");
        assert!(c.bool("native.theme") && c.bool("native.settings") && !c.bool("native.bar") && !c.bool("native.gojo"));
        assert!(Config::parse("").bool("native.bar"));
    }

    use super::*;
    #[test]
    fn the_settings_folder_is_the_zor_ui_folder() {
        let p = pack_dir().to_string_lossy().to_lowercase();
        assert!(p.ends_with(r".glzr\zor-ui") && !p.contains("zebar"));
    }

    #[test]
    fn a_preview_palette_overrides_the_theme_until_cleared() {
        let t = Theme::parse("");
        let real = t.c("primary");
        assert!(set_preview(Some(HashMap::from([("primary".to_string(), 0xFF11_2233)]))).is_none());
        assert_eq!((t.c("primary"), t.c("onSurface")), (0xFF11_2233, Theme::parse("").c("onSurface"))); // unnamed colours still come from the theme
        assert_eq!(set_preview(None).map(|m| m.len()), Some(1));
        assert_eq!(t.c("primary"), real);
    }

    #[test]
    fn state_update_keeps_keys_written_since_load() {
        let path = std::env::temp_dir().join(format!("zor-ui-state-test-{}.json", std::process::id()));
        fs::write(&path, r#"{"deskPos": {}}"#).unwrap();
        let mut s = State::load(path.clone());
        fs::write(&path, r#"{"deskPos": {}, "live": "we:x"}"#).unwrap(); // the theme script, after start-up
        s.update(|v| v["leftTab"] = json!("ai"));
        let disk: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(disk["live"], "we:x");
        assert_eq!(disk["leftTab"], "ai");
    }
    #[test]
    fn a_visualizer_uses_its_own_color_then_the_shared_one() {
        let c = Config::parse(r##"{"visualizer": {"color": "#112233"}, "island": {"eqColor": "#445566"}}"##);
        assert_eq!(c.viz_color("island.eqColor"), Some(0xFF44_5566)); // its own wins
        assert_eq!(c.viz_color("visualizer.desktopColor"), Some(0xFF11_2233)); // unset: the shared color
        let bad = Config::parse(r##"{"visualizer": {"color": "red"}, "island": {"eqColor": "#12"}}"##);
        assert_eq!(bad.viz_color("island.eqColor"), None); // invalid everywhere: the theme
        assert_eq!(Config::parse("").viz_color("island.eqColor"), None);
        let b = Config::parse(r##"{"visualizer": {"barColor": "#abcdef"}}"##);
        assert_eq!((b.viz_color("visualizer.barColor"), b.viz_color("visualizer.desktopColor")), (Some(0xFFAB_CDEF), None));
    }
    #[test]
    fn config_merges_sections() {
        let c = Config::parse(r#"{"use24h": false, "desktop": {"clock": "digital"}, "island": {"panel": {"media": false}}}"#);
        assert!(!c.bool("use24h"));
        assert_eq!(c.str("desktop.clock"), "digital");
        assert_eq!(c.strs("desktop.worldClocks").len(), 3); // kept from defaults
        assert!(!c.bool("island.panel.media"));
        assert!(c.bool("island.panel.lyrics"));
        assert_eq!(Config::parse("not json").str("tempUnit"), "C");
    }
    #[test]
    fn theme_colours() {
        let t = Theme::parse(r##"{"mode": "light", "colors": {"primary": "#112233"}}"##);
        assert_eq!(t.c("primary"), 0xFF11_2233);
        assert_eq!(t.c("onSurface"), 0xFFEA_E0E7);
        assert_eq!(parse_hex("#11223380"), Some(0x8011_2233));
    }
}
