//! Buddy packs: a folder `~\.glzr\zor-ui\buddies\<name>\` holding `buddy.json` and the pictures it names.
//! A pack gives a buddy its own look (pictures per state) and its own lines, so a buddy can look and talk
//! like any character. Only `buddy.json` and plain picture file names inside the folder are ever read.
//!
//! ```json
//! { "name": "Goku", "height": 80, "fps": 8,
//!   "frames": { "stand": ["stand.png"], "walk": ["walk1.png", "walk2.png"], "sit": ["sit.png"] },
//!   "lines": { "greet": "Yo!", "click": ["I'm hungry.", "Wanna spar?"] },
//!   "colors": { "hair": "#101010", "eye": "#202020" },
//!   "moves": { "special": "beam", "alt": "none" } }
//! ```
//! Pictures face right, stand with their bottom centre on the feet, and should have no empty margin.

use crate::conf;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

/// The states a pack can give pictures for, in the order the last-resort fallback tries them.
pub const MODES: [&str; 8] = ["stand", "walk", "sit", "sleep", "fall", "held", "point", "talk"];
/// Most pictures per state, lines per category, characters per line, and bytes of buddy.json.
pub const MAX_FRAMES: usize = 24;
pub const MAX_LINES: usize = 64;
pub const MAX_LINE: usize = 200;
const MAX_JSON: u64 = 256 * 1024;
pub const MIN_H: f32 = 16.0;
pub const MAX_H: f32 = 160.0;
/// The height of the drawn figure, CSS px: what a pack gets when it names none.
pub const DEFAULT_H: f32 = 52.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    /// Folder name, or "image:<file>" for a single picture chosen in Settings.
    pub id: String,
    pub dir: PathBuf,
    pub name: String,
    /// Built-in character whose lines the pack inherits; empty: neutral lines.
    pub base: String,
    /// Height of the standing picture in CSS px.
    pub height: f32,
    pub fps: f32,
    /// Picture file names per state (a key of `MODES`).
    pub frames: HashMap<String, Vec<String>>,
    /// Lines by category (see `cast::apply_lines`); a category left out keeps the base's.
    pub lines: HashMap<String, Vec<String>>,
    /// Colour fields ("hair", "eye", "orbs") as typed in the manifest.
    pub colors: HashMap<String, String>,
    /// The moves of a double-click ("special") and a right-click ("alt"), by name (see `cast::Move::parse`).
    pub moves: HashMap<String, String>,
    /// Changes when buddy.json does, so cached pictures are dropped.
    pub stamp: u64,
}

/// A file name that stays inside the folder: letters, digits, space, `.`, `_`, `-`, a picture extension, no `..`.
pub fn safe_file(f: &str) -> bool {
    let ok_ext = ["png", "webp", "jpg", "jpeg", "gif", "bmp"].iter().any(|e| f.to_lowercase().ends_with(&format!(".{e}")));
    ok_ext && f.len() <= 64 && !f.starts_with('.') && !f.contains("..") && f.chars().all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c))
}

/// A folder name that is one path component: letters, digits, space, `_`, `-`.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && !id.starts_with(' ') && id.chars().all(|c| c.is_ascii_alphanumeric() || " _-".contains(c))
}

/// A manifest value as a list of strings: a lone string is a list of one; anything else is empty.
fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        _ => vec![],
    }
}

fn clip(s: &str) -> String { s.chars().take(MAX_LINE).collect() }

/// Reads `json` as the manifest of the pack in folder `id` (`dir`). Unknown keys and states are ignored; bad
/// picture names, empty states and lines past the caps are dropped. Err when nothing could be drawn.
pub fn parse(id: &str, dir: &Path, json: &str, stamp: u64) -> Result<Pack, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("buddy.json: {e}"))?;
    let mut frames = HashMap::new();
    for m in MODES {
        let list: Vec<String> = strings(&v["frames"][m]).into_iter().filter(|f| safe_file(f)).take(MAX_FRAMES).collect();
        if !list.is_empty() { frames.insert(m.to_string(), list); }
    }
    if frames.is_empty() { return Err("buddy.json names no usable pictures under \"frames\"".into()); }
    let mut lines = HashMap::new();
    if let Some(o) = v["lines"].as_object() {
        for (k, val) in o {
            // an empty string is kept: it silences a one-line category
            let list: Vec<String> = strings(val).iter().take(MAX_LINES).map(|s| clip(s)).collect();
            if !list.is_empty() { lines.insert(k.clone(), list); }
        }
    }
    let mut colors = HashMap::new();
    if let Some(o) = v["colors"].as_object() {
        for (k, val) in o {
            let s = match val { Value::Array(_) => strings(val).join(","), _ => val.as_str().unwrap_or("").to_string() };
            if !s.is_empty() { colors.insert(k.clone(), s.chars().take(64).collect()); }
        }
    }
    let mut moves = HashMap::new();
    if let Some(o) = v["moves"].as_object() {
        for (k, val) in o { if let Some(s) = val.as_str() { moves.insert(k.clone(), s.chars().take(16).collect()); } }
    }
    let name = v["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(id);
    Ok(Pack {
        id: id.to_string(),
        dir: dir.to_path_buf(),
        name: name.chars().take(40).collect(),
        base: v["base"].as_str().unwrap_or("").trim().to_string(),
        height: (v["height"].as_f64().unwrap_or(DEFAULT_H as f64) as f32).clamp(MIN_H, MAX_H),
        fps: (v["fps"].as_f64().unwrap_or(8.0) as f32).clamp(1.0, 30.0),
        frames, lines, colors, moves, stamp,
    })
}

/// A single picture from Settings > Buddies > "Use a picture" as a pack with one standing frame.
pub fn single(file: &str) -> Pack {
    Pack {
        id: format!("image:{file}"), dir: conf::pack_dir().join("images"), name: String::new(), base: String::new(),
        height: DEFAULT_H, fps: 8.0, frames: HashMap::from([("stand".to_string(), vec![file.to_string()])]),
        lines: HashMap::new(), colors: HashMap::new(), moves: HashMap::new(), stamp: 0,
    }
}

impl Pack {
    /// "3 states, 8 pictures, 80 px tall": what Settings lists under the pack's name.
    pub fn summary(&self) -> String {
        let (states, pics) = (self.frames.len(), self.frames.values().map(Vec::len).sum::<usize>());
        let s = |n: usize| if n == 1 { "" } else { "s" };
        format!("{states} state{}, {pics} picture{}, {} px tall", s(states), s(pics), self.height)
    }

    /// The state whose pictures to draw for `want`, and whether it is `want` itself (or a stand-in). The
    /// stand-ins: sleep sits, a held or fallen buddy stands in each other's pose, everything else stands.
    pub fn resolve(&self, want: &str) -> Option<(&'static str, bool)> {
        let chain: &[&str] = match want {
            "sleep" => &["sleep", "sit", "stand"],
            "held" => &["held", "fall", "stand"],
            "walk" => &["walk", "stand"],
            "sit" => &["sit", "stand"],
            "fall" => &["fall", "stand"],
            "point" => &["point", "stand"],
            "talk" => &["talk", "stand"],
            _ => &["stand"],
        };
        let own = |m: &str| self.frames.contains_key(m);
        chain.iter().chain(MODES.iter()).find(|m| own(m)).map(|m| {
            let m: &'static str = MODES.iter().find(|x| *x == m).copied().unwrap_or("stand");
            (m, m == want)
        })
    }
}

/// Which of `n` frames shows at `t` seconds at `fps`; always 0 for one frame or a bad rate.
pub fn frame_index(n: usize, fps: f32, t: f32) -> usize {
    if n <= 1 || !(fps > 0.0) || !(t >= 0.0) { return 0; }
    (t * fps) as usize % n
}

/// Scale (x, y) of a picture that has no animation of its own for this moment: it breathes, squashes onto a
/// ledge as `sit` goes 0 to 1, and pulses while `talk`.
pub fn squash(t: f32, sit: f32, talk: bool) -> (f32, f32) {
    let breath = (t * 2.0).sin() * 0.012;
    let pulse = if talk { (t * 14.0).sin().abs() * 0.04 } else { 0.0 };
    (1.0 - breath * 0.5 + pulse * 0.5, 1.0 + breath - 0.06 * sit.clamp(0.0, 1.0) + pulse)
}

/// `~\.glzr\zor-ui\buddies`.
pub fn buddies_dir() -> PathBuf { conf::pack_dir().join("buddies") }

/// The pack in folder `id` under `root`: None when the folder has no buddy.json (it isn't a pack), Err with the reason
/// when it has one that can't be used.
fn read_result(root: &Path, id: &str) -> Option<Result<Pack, String>> {
    if !safe_id(id) { return None; }
    let dir = root.join(id);
    let file = dir.join("buddy.json");
    let meta = std::fs::metadata(&file).ok().filter(|m| m.is_file())?;
    if meta.len() > MAX_JSON { return Some(Err(format!("buddy.json is over {} KB", MAX_JSON / 1024))); }
    let stamp = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    match std::fs::read_to_string(&file) { Ok(text) => Some(parse(id, &dir, &text, stamp)), Err(e) => Some(Err(e.to_string())) }
}

/// Reads the pack in folder `id` under `root` (None: no such pack, or its manifest is unusable, which is logged).
pub fn read(root: &Path, id: &str) -> Option<Pack> {
    // the tests make broken packs on purpose: they must not write to the real log
    read_result(root, id)?.map_err(|e| if !cfg!(test) { crate::log::line(&format!("buddy pack {id}: {e}")) }).ok()
}

/// The pack in folder `id` of the buddies folder, re-read only when its buddy.json changes.
pub fn load(id: &str) -> Option<Arc<Pack>> {
    static CACHE: Mutex<Vec<(String, Option<Arc<Pack>>)>> = Mutex::new(Vec::new());
    let stamp = std::fs::metadata(buddies_dir().join(id).join("buddy.json")).ok().and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    let mut cache = CACHE.lock().ok()?;
    if let Some((_, p)) = cache.iter().find(|(k, _)| k == id) {
        if p.as_ref().map_or(stamp == 0, |p| p.stamp == stamp) { return p.clone(); }
    }
    let p = read(&buddies_dir(), id).map(Arc::new);
    cache.retain(|(k, _)| k != id);
    cache.push((id.to_string(), p.clone()));
    p
}

/// Every folder under `root` that has a buddy.json, by folder name: its pack, or why it can't be used.
pub type Found = Vec<(String, Result<Pack, String>)>;

pub fn scan_in(root: &Path) -> Found {
    let mut v: Found = std::fs::read_dir(root).into_iter().flatten().flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| { let id = e.file_name().to_string_lossy().to_string(); read_result(root, &id).map(|r| (id, r)) })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// (id, name) of each usable pack in `found`; ids are "pack:<folder>".
fn names(found: Found) -> Vec<(String, String)> {
    found.into_iter().filter_map(|(id, r)| r.ok().map(|p| (format!("pack:{id}"), p.name))).collect()
}

static KEPT: Mutex<Option<(Instant, Found)>> = Mutex::new(None);

/// Drops the kept scan, so the next `scan` or `list` sees a folder that was just made.
pub fn forget() { if let Ok(mut k) = KEPT.lock() { *k = None; } }

/// `scan_in` for the real folder, kept for 2 s: Settings asks on every frame.
pub fn scan() -> Found {
    let Ok(mut k) = KEPT.lock() else { return vec![] };
    if let Some((at, v)) = k.as_ref() { if at.elapsed() < Duration::from_secs(2) { return v.clone(); } }
    let v = scan_in(&buddies_dir());
    *k = Some((Instant::now(), v.clone()));
    v
}

/// (id, name) of each usable pack in the real folder, sorted by folder name.
pub fn list() -> Vec<(String, String)> { names(scan()) }

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(json: &str) -> Result<Pack, String> { parse("goku", Path::new("x"), json, 1) }

    #[test]
    fn a_manifest_gives_frames_lines_and_limits() {
        let p = pk(r##"{"name":" Goku ","height":80,"fps":12,"base":"yuji",
            "frames":{"stand":["stand.png"],"walk":["w1.png","w2.png","../evil.png","c:\\x.png","a/b.png","no.txt"],"junk":["x.png"]},
            "lines":{"greet":"Yo!","click":["a","b"],"empty":[]},"colors":{"hair":"#101010","orbs":["#111111","#222222","#333333"]},
            "moves":{"special":"beam","alt":7}}"##).unwrap();
        assert_eq!((p.name.as_str(), p.height, p.fps, p.base.as_str()), ("Goku", 80.0, 12.0, "yuji"));
        assert_eq!(p.frames["walk"], ["w1.png", "w2.png"], "names that leave the folder or aren't pictures are dropped");
        assert!(!p.frames.contains_key("junk"), "unknown states are ignored");
        assert_eq!((p.lines["greet"].as_slice(), p.lines["click"].len()), (&["Yo!".to_string()][..], 2));
        assert!(!p.lines.contains_key("empty"));
        assert_eq!(p.colors["orbs"], "#111111,#222222,#333333");
        assert_eq!((p.moves.get("special").map(String::as_str), p.moves.get("alt")), (Some("beam"), None), "only names are kept");
    }

    #[test]
    fn defaults_clamps_and_caps() {
        let p = pk(r#"{"frames":{"stand":["a.png"]},"height":9999,"fps":0}"#).unwrap();
        assert_eq!((p.name.as_str(), p.height, p.fps), ("goku", MAX_H, 1.0), "the folder names it; size and rate are clamped");
        assert_eq!(pk(r#"{"frames":{"stand":["a.png"]},"height":1}"#).unwrap().height, MIN_H);
        assert_eq!(pk(r#"{"frames":{"stand":["a.png"]}}"#).unwrap().height, DEFAULT_H);
        let many: Vec<String> = (0..100).map(|i| format!("\"f{i}.png\"")).collect();
        assert_eq!(pk(&format!(r#"{{"frames":{{"walk":[{}]}}}}"#, many.join(","))).unwrap().frames["walk"].len(), MAX_FRAMES);
        let long = "x".repeat(500);
        assert_eq!(pk(&format!(r#"{{"frames":{{"stand":["a.png"]}},"lines":{{"click":["{long}"]}}}}"#)).unwrap().lines["click"][0].len(), MAX_LINE);
    }

    #[test]
    fn a_pack_with_nothing_to_draw_is_an_error() {
        assert!(pk("not json").is_err());
        assert!(pk("{}").is_err());
        assert!(pk(r#"{"frames":{"stand":["../a.png"],"walk":[]}}"#).is_err());
    }

    #[test]
    fn a_silenced_line_is_kept() {
        let p = pk(r#"{"frames":{"stand":["a.png"]},"lines":{"landed":""}}"#).unwrap();
        assert_eq!(p.lines["landed"], [""], "an empty string means say nothing");
    }

    #[test]
    fn file_and_folder_names_cannot_escape() {
        for bad in ["", "..", "../a.png", "a/b.png", "a\\b.png", ".hidden.png", "a..png", "c:a.png", "a.exe", "a.png.exe", &"x".repeat(70)] { assert!(!safe_file(bad), "{bad:?}"); }
        for ok in ["a.png", "Stand 1.PNG", "walk_2-b.webp"] { assert!(safe_file(ok), "{ok:?}"); }
        for bad in ["", "..", "a/b", "a\\b", " a", "a:b", "a.b"] { assert!(!safe_id(bad), "{bad:?}"); }
        assert!(safe_id("goku") && safe_id("My Buddy_2"));
    }

    #[test]
    fn a_missing_state_stands_in_with_the_nearest() {
        let p = pk(r#"{"frames":{"stand":["s.png"],"walk":["w.png"],"fall":["f.png"],"sit":["t.png"]}}"#).unwrap();
        assert_eq!(p.resolve("walk"), Some(("walk", true)));
        assert_eq!(p.resolve("sleep"), Some(("sit", false)), "a buddy with no sleep pictures sleeps sitting");
        assert_eq!(p.resolve("held"), Some(("fall", false)));
        assert_eq!(p.resolve("point"), Some(("stand", false)));
        assert_eq!(p.resolve("talk"), Some(("stand", false)));
        assert_eq!(p.resolve("stand"), Some(("stand", true)));
        let q = pk(r#"{"frames":{"walk":["w.png"]}}"#).unwrap();
        assert_eq!(q.resolve("stand"), Some(("walk", false)), "no standing pictures: the first state there is");
        assert_eq!(q.resolve("sleep"), Some(("walk", false)));
    }

    #[test]
    fn frames_advance_with_time_and_wrap() {
        assert_eq!((frame_index(4, 8.0, 0.0), frame_index(4, 8.0, 0.13), frame_index(4, 8.0, 0.25), frame_index(4, 8.0, 0.5)), (0, 1, 2, 0));
        assert_eq!(frame_index(1, 8.0, 9.0), 0);
        assert_eq!(frame_index(0, 8.0, 9.0), 0);
        assert_eq!((frame_index(3, 0.0, 5.0), frame_index(3, f32::NAN, 5.0), frame_index(3, 8.0, -1.0), frame_index(3, 8.0, f32::NAN)), (0, 0, 0, 0));
    }

    #[test]
    fn a_still_picture_breathes_and_squashes_when_sitting() {
        let (x0, y0) = squash(0.0, 0.0, false);
        assert_eq!((x0, y0), (1.0, 1.0));
        let ys: Vec<f32> = (0..100).map(|i| squash(i as f32 * 0.05, 0.0, false).1).collect();
        assert!(ys.iter().all(|y| (0.98..1.02).contains(y)) && ys.iter().any(|y| *y > 1.005) && ys.iter().any(|y| *y < 0.995), "a gentle breath");
        assert!(squash(0.0, 1.0, false).1 < squash(0.0, 0.0, false).1, "sitting squashes");
        assert!(squash(0.1, 0.0, true).1 > squash(0.1, 0.0, false).1, "talking pulses");
        assert_eq!(squash(0.0, 9.0, false).1, squash(0.0, 1.0, false).1, "sit is clamped");
    }

    #[test]
    fn packs_are_read_from_a_folder_and_listed_by_name() {
        let root = std::env::temp_dir().join(format!("zorui-pack-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (d, json) in [("bb", r#"{"name":"Zed","frames":{"stand":["a.png"]}}"#), ("aa", r#"{"frames":{"stand":["a.png"]}}"#), ("bad", "{"), ("none", "")] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            if !json.is_empty() { std::fs::write(root.join(d).join("buddy.json"), json).unwrap(); }
        }
        std::fs::write(root.join("loose.json"), "{}").unwrap();
        assert_eq!(names(scan_in(&root)), [("pack:aa".to_string(), "aa".to_string()), ("pack:bb".to_string(), "Zed".to_string())], "broken and empty folders are skipped");
        let p = read(&root, "bb").unwrap();
        assert_eq!((p.dir, p.stamp > 0), (root.join("bb"), true));
        assert!(read(&root, "..").is_none() && read(&root, "a/b").is_none() && read(&root, "nope").is_none());
        std::fs::write(root.join("bb").join("buddy.json"), "x".repeat(300_000)).unwrap();
        assert!(read(&root, "bb").is_none(), "an oversized manifest is not read");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pack_is_summed_up_in_one_line() {
        let p = pk(r#"{"height":80,"frames":{"stand":["a.png"],"walk":["w1.png","w2.png"]}}"#).unwrap();
        assert_eq!(p.summary(), "2 states, 3 pictures, 80 px tall");
        assert_eq!(pk(r#"{"frames":{"stand":["a.png"]}}"#).unwrap().summary(), "1 state, 1 picture, 52 px tall");
    }

    #[test]
    fn a_folder_with_a_broken_manifest_is_reported_not_hidden() {
        let root = std::env::temp_dir().join(format!("zorui-pack-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (d, json) in [("good", r#"{"frames":{"stand":["a.png"]}}"#), ("typo", r#"{"frames": {"#), ("empty", "{}"), ("big", "")] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("buddy.json"), if d == "big" { "x".repeat(300_000) } else { json.to_string() }).unwrap();
        }
        std::fs::create_dir_all(root.join("no manifest")).unwrap();
        std::fs::write(root.join("loose.json"), "{}").unwrap();
        let found = scan_in(&root);
        let folders: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(folders, ["big", "empty", "good", "typo"], "only folders with a buddy.json, sorted");
        let err = |n: &str| found.iter().find(|(id, _)| id == n).unwrap().1.clone().err();
        assert!(err("good").is_none());
        assert!(err("typo").unwrap().starts_with("buddy.json:"), "the parse error says where");
        assert!(err("empty").unwrap().contains("no usable pictures"));
        assert!(err("big").unwrap().contains("over 256 KB"));
        assert_eq!(names(scan_in(&root)), [("pack:good".to_string(), "good".to_string())], "the picker lists only what works");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_single_picture_is_a_pack_of_one_frame() {
        let p = single("buddy-a-1.png");
        assert_eq!((p.id.as_str(), p.height, p.resolve("walk")), ("image:buddy-a-1.png", DEFAULT_H, Some(("stand", false))));
        assert!(p.dir.ends_with("images"));
    }
}
