//! Settings (settings.js + settings.html): a navigation rail and the pages of cards, in a normal window that
//! can be moved by any empty spot. Values are written with the bridge's config-set (like the JS), and shown
//! at once from `over` until config.json reloads. Re-theming actions (wallpaper, mode, scheme, presets,
//! Wallpaper Engine) run the same bridge / script commands as the JS.
//!
//! `ZORUI_RENDER_PAGE=<quick|keys|wallpapers|presets|bar|island|services|usage|about>` opens that page and, with
//! `--render settings`, skips all background loading; `ZORUI_RENDER_DEMO=1` adds sample presets / wallpapers /
//! key states so those pages can be drawn offline.

mod form;
mod layout;
mod pages;
mod registry;
mod wallpaper;
pub(crate) mod store;
pub(crate) mod picker;

use crate::{
    app::{Action, Ctx},
    conf::{self, Config},
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    hub, proc,
    ui::{self, Edit, Scroll, TextBox},
    win::{Btn, Input, Phase, View},
};
use form::{num_json, snap, Act, Pen, Sl, ACT0};
use pages::DESKTOP_WIDGETS;
use registry::{Page, PAGES};
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, rc::Rc};
use store::Inbox;

const BG: u32 = 1;
const MENU_BG: u32 = 2;
const NAV_BG: u32 = 3;
const MENU0: u32 = 10;
const NAV_ITEM_H: f32 = crate::tokens::NAV_ITEM_H;
const NAV_STEP: f32 = crate::tokens::NAV_STEP;

/// The rail's scrolling page list: its rect (from `top` down to just above Close) and the full height of all pages.
fn nav_list(top: f32, win_h: f32) -> (R, f32) {
    let bottom = win_h - 22.0 - NAV_ITEM_H - 8.0;
    (R::new(crate::tokens::NAV_X, top, crate::tokens::NAV_W, (bottom - top).max(0.0)), PAGES.len() as f32 * NAV_STEP - (NAV_STEP - NAV_ITEM_H))
}
const TOAST_MS: f64 = 3200.0;
/// How long a Claude account's Remove stays armed after its first click.
const CLAUDE_ARM_MS: f64 = 4000.0;

#[derive(Default)]
pub enum Load<T> { #[default] Idle, Loading, Ready(T) }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Text { list: bool }, Num(i64, i64), Plain, Secret }

/// A text input: its editor, the text it had when last saved, and what a commit does.
pub struct Field { pub tb: TextBox, pub orig: String, pub kind: Kind, pub mask: TextBox }
impl Field {
    pub fn new(multi: bool) -> Field { Field { tb: TextBox::new(multi), orig: String::new(), kind: Kind::Plain, mask: TextBox::new(false) } }
}

/// An open `<select>` list.
pub struct Menu { pub path: String, anchor: R, opts: Rc<Vec<(Value, String)>>, scroll: Scroll }

/// A bar layout part being dragged.
pub struct Bl { pub id: String, sx: f32, sy: f32, x: f32, y: f32, pub ghost: bool, pub target: Option<(String, usize)> }

/// What the Quick page reads from theme.json.
#[derive(Default)]
struct Info { mode: String, scheme: String, seed: String, live: bool, stamp: Option<std::time::SystemTime>, analysis: Value, profile: Value }

/// hub key a desk card sets before opening Settings: "<page key>|<section title>".
pub const GOTO: &str = "zor-ui.settingsGoto";
/// How long the highlight on a control opened from search stays visible.
const FLASH_MS: f64 = 1800.0;

/// `/` jumps to the search box unless a text field is being typed in (the picker has its own Char arm above).
/// The sidebar avatar shows the picture only when one is chosen and still on disk; otherwise the initial.
/// Made-up saved Claude logins for the screenshots (demo mode); the default is active, the profile has no usage reading.
fn demo_claude() -> Value {
    json!({ "accounts": [
        { "n": 0, "default": true, "email": "me@example.com", "plan": "max", "active": true, "dead": false, "session": 38, "week": 12, "ageMin": 0 },
        { "n": 1, "default": false, "email": "work@example.com", "plan": "pro", "active": false, "dead": false, "session": null, "week": null, "ageMin": null },
    ] })
}

/// Whether the delete icon of account `n` was clicked less than the arm time ago (the second click removes it).
fn armed(arm: &Option<(String, f64)>, n: &str, now: f64) -> bool {
    arm.as_ref().is_some_and(|(k, until)| k == n && now < *until)
}

fn avatar_initial(name: &str) -> String { name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() }
fn avatar_is_picture(file: &str) -> bool { !file.trim().is_empty() && conf::pack_dir().join("images").join(file).exists() }

fn slash_focuses_search(typing: bool, menu_open: bool) -> bool { !typing && !menu_open }

/// "<page>|<section>|<label>" (the label is optional) as the GOTO hub value.
fn parse_goto(g: &str) -> Option<(String, String, String)> {
    let mut it = g.splitn(3, '|');
    let (p, s) = (it.next()?, it.next()?);
    Some((p.into(), s.into(), it.next().unwrap_or("").into()))
}

/// Where to scroll for a section and control label: the control's y, else the section heading's.
fn goto_y(heads: &[(String, f32)], tags: &[(String, String, f32)], sect: &str, label: &str) -> Option<f32> {
    tags.iter().find(|t| t.0 == sect && t.1 == label).map(|t| t.2).or_else(|| heads.iter().find(|h| h.0 == sect).map(|h| h.1))
}
const SEARCH: &str = "search";

pub struct Settings {
    area: (f32, f32),
    page: usize,
    scroll: Scroll,
    content_h: f32,
    view_h: f32,
    /// The page list in the rail scrolls when the window is too short to show every page.
    nav_scroll: Scroll,
    nav_h: f32,
    nav_view: f32,
    /// Hit ids `ACT0..ACT0 + nav_n` belong to the rail (the wheel over them scrolls it).
    nav_n: u32,
    /// Collapsible sidebar: icon-rail mode when true or when window is compact.
    pub nav_collapsed: bool,
    /// Per-category scroll memory so navigation state is preserved across category switches.
    pub page_scrolls: HashMap<usize, f32>,
    inbox: Inbox,
    /// A copy of the config for background callbacks (they have no `Ctx`).
    cfgc: Config,
    /// Values written but not yet seen in config.json: path -> (value, written at ms).
    over: HashMap<String, (Value, f64)>,
    /// The glass preview picture and what it was made from.
    glass_prev: Option<(String, crate::gfx::Pixmap)>,
    /// The decoded pictures of each buddy slot's preview on the Buddies page.
    buddy_sprites: [crate::buddy::Sprites; 2],
    /// The creator page's latest clean-up result (see creator.rs) and the decoded picture of its preview.
    creator_prev: Option<crate::creator::Preview>,
    creator_sprites: crate::buddy::Sprites,
    acts: Vec<Act>,
    fields: HashMap<String, Field>,
    focus: Option<String>,
    /// The window has keyboard focus (else no caret).
    active: bool,
    menu: Option<Menu>,
    /// The open colour picker ("Theme from a color"). While it is open, every window painted on this thread
    /// (conf::set_preview) shows the palette the picked colour would give.
    picker: Option<picker::Picker>,
    /// Set while the picker edits a config color (the visualizer's) instead of the theme seed.
    pick_path: Option<String>,
    /// A slider being dragged: path, spec, track rect.
    sl: Option<(String, Sl, R)>,
    drag_val: Option<(String, f64)>,
    toast: Option<(String, f64)>,
    /// The control a search hit opened: (section, label, when the highlight ends in ms).
    flash: Option<(String, String, f64)>,
    busy: String,
    info: Info,
    wallpaper_draft: Option<wallpaper::Draft>,
    wallpaper_preview_cache: Vec<(String, HashMap<String, u32>)>,
    online: Load<Value>,
    local: Load<Vec<Value>>,
    we: Load<Value>,
    we_list: Load<Vec<Value>>,
    keys: Load<Value>,
    /// The Keybinds row waiting for a combo: (bind id, which of its keys).
    rec: Option<(String, usize)>,
    /// The saved Claude logins (`claude-accounts` bridge command); Idle until Claude Code is found.
    claude: Load<Value>,
    /// The account whose Remove is armed: (n, the time the arm runs out in ms). Remove needs a second click in time.
    claude_arm: Option<(String, f64)>,
    mons: Load<Vec<String>>,
    /// What each monitor shows ({ i, w, h, file, custom } from the `wall-monitors` bridge command).
    wmons: Load<Vec<Value>>,
    models: Load<Vec<(String, String)>>,
    we_type: String,
    imgs: HashMap<String, Option<tiny_skia::Pixmap>>,
    bl: Option<Bl>,
    bl_focus: Option<String>,
    bl_geo: Vec<(String, R, Vec<(String, R)>)>,
    rendering: bool,
    /// Every page's (page, section, label, sub), built when the first search is typed.
    index: Vec<(usize, String, String, String)>,
}

pub fn preview_path(name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    conf::data_dir().join("settings-previews").join(format!("{safe}.png"))
}

/// Defaults for keys core.js fills in at start (settings.js: cfg.desktop.greet / images).
fn default_for(path: &str) -> Value {
    match path {
        "desktop.greet.date" => json!(true),
        "desktop.glass.tint" => json!("transparent"),
        p if p.starts_with("desktop.glass.") => json!(crate::glass::default_of(&p["desktop.glass.".len()..])),
        p if p.starts_with("desktop.greet.") => json!(""),
        p if p.ends_with(".shape") && p.starts_with("desktop.images.") => json!("cookie"),
        p if p.ends_with(".size") && p.starts_with("desktop.images.") => json!(280),
        p if p.ends_with(".file") && p.starts_with("desktop.images.") => json!(""),
        // the creator's draft (Settings > Create a buddy)
        "buddies.make.height" => json!(64),
        "buddies.make.trim" => json!(true),
        "buddies.make.special" => json!("aura"),
        _ => Value::Null,
    }
}

/// Smallest and largest window, CSS px (the pages need about 720 x 480).
const MIN_SIZE: (f32, f32) = (720.0, 480.0);
const MAX_SIZE: (f32, f32) = (4000.0, 3000.0);
/// shared.json key holding the size the user dragged the window to, "w,h".
const SIZE_KEY: &str = "settings.size";
/// shared.json key holding the page key the user was on. Close and Esc remove it, so the next open starts on Quick.
const PAGE_KEY: &str = "settings.page";

/// Index of the saved page key in PAGES; 0 (Quick) when it is missing or unknown.
fn page_from_key(key: Option<&str>) -> usize {
    key.and_then(|k| registry::index_of_key(k)).unwrap_or(0)
}

/// The error of a preset's switchwall answer, None when the wallpaper / theme step worked (or gave no answer).
fn theme_failed(theme: &Value) -> Option<String> {
    (theme["ok"] == false).then(|| theme["error"].as_str().unwrap_or("it gave no reason").to_string())
}

fn clamp_size((w, h): (f32, f32)) -> (f32, f32) {
    (w.clamp(MIN_SIZE.0, MAX_SIZE.0), h.clamp(MIN_SIZE.1, MAX_SIZE.1))
}

/// The saved "w,h" clamped to the limits, if it parses.
fn parse_size(s: &str) -> Option<(f32, f32)> {
    let (w, h) = s.split_once(',')?;
    Some(clamp_size((w.trim().parse().ok()?, h.trim().parse().ok()?)))
}

impl Settings {
    /// `area`: the CSS-px size the popup placement gives the view (popup::area); a size the user dragged to wins.
    pub fn new(area: (f32, f32)) -> Settings {
        let area = hub::get(SIZE_KEY).and_then(|s| parse_size(&s)).unwrap_or(area);
        let env = std::env::var("ZORUI_RENDER_PAGE").ok();
        let page = env.as_deref().and_then(|e| registry::index_of_key(e)).unwrap_or_else(|| page_from_key(hub::get(PAGE_KEY).as_deref()));
        let mut s = Settings {
            area, page, scroll: Scroll::default(), content_h: 0.0, view_h: 0.0, nav_scroll: Scroll::default(), nav_h: 0.0, nav_view: 0.0, nav_n: 0,
            nav_collapsed: false, page_scrolls: HashMap::new(), inbox: Inbox::default(), cfgc: Config::parse(""),
            over: HashMap::new(), glass_prev: None, buddy_sprites: Default::default(), creator_prev: None, creator_sprites: Default::default(), acts: vec![], fields: HashMap::new(), focus: None, active: true, menu: None, picker: None, pick_path: None, sl: None, drag_val: None, toast: None, flash: None,
            busy: String::new(), info: Info::default(), wallpaper_draft: None, wallpaper_preview_cache: vec![], online: Load::Idle, local: Load::Idle, we: Load::Idle, we_list: Load::Idle, keys: Load::Idle, rec: None, claude: Load::Idle, claude_arm: None, mons: Load::Idle, wmons: Load::Idle,
            models: Load::Idle, we_type: "all".into(), imgs: HashMap::new(), bl: None, bl_focus: None, bl_geo: vec![], rendering: env.is_some(), index: vec![],
        };
        if std::env::var("ZORUI_RENDER_DEMO").is_ok() { s.demo(); }
        // `--render settings` has no pointer, so this opens the colour picker for the picture
        if let Some(c) = std::env::var("ZORUI_RENDER_PICKER").ok().and_then(|h| conf::parse_hex(&h)) { s.picker = Some(picker::Picker::new(c, area)); s.update_preview(); }
        s
    }

    /// Opens on the page `key` (Mod+/ opens "keys").
    pub fn on_page(mut self, key: &str) -> Settings {
        if let Some(i) = registry::index_of_key(key) { self.set_page(i); }
        self
    }

    /// Shows page `i` and remembers it for the next open, preserving navigation state per category.
    fn set_page(&mut self, i: usize) {
        if self.page != i {
            self.page_scrolls.insert(self.page, self.scroll.y);
            self.page = i;
            hub::set(PAGE_KEY, Some(PAGES[i].key.to_string()));
            let saved_y = self.page_scrolls.get(&i).copied().unwrap_or(0.0);
            self.scroll = Scroll::default();
            self.scroll.y = saved_y;
        }
    }

    /// Close button and Esc: forget the page so the next open starts on Quick, then close.
    fn close_settings(&mut self, cx: &mut Ctx) {
        hub::set(PAGE_KEY, None);
        self.page_scrolls.clear();
        cx.actions.push(Action::Close("settings".into()));
    }

    fn demo(&mut self) {
        self.online = Load::Ready(json!({ "presets": [{ "name": "Sakura_Night", "author": "someone" }, { "name": "Forest", "author": "" }, { "name": "Neon_City", "author": "pc" }] }));
        self.local = Load::Ready(vec![json!({ "id": "a", "name": "Work", "scheme": "tonal-spot", "mode": "dark", "wallpaper": "C:\\w\\mountain.jpg", "from": "local" }),
            json!({ "id": "b", "name": "Sakura Night", "scheme": "fruit-salad", "mode": "light", "wallpaper": "", "from": "online" })]);
        self.we = Load::Ready(json!({ "installed": true, "running": true, "current": "C:\\ws\\2\\project.json" }));
        self.we_list = Load::Ready(vec![json!({ "id": "1", "title": "Rain", "type": "scene", "origin": "workshop", "path": "C:\\ws\\1\\project.json", "thumb": "", "rating": "Everyone", "tags": [] }),
            json!({ "id": "2", "title": "Waves", "type": "video", "origin": "mine", "path": "C:\\ws\\2\\project.json", "thumb": "", "rating": "Mature", "tags": [] }),
            json!({ "id": "3", "title": "Forest", "type": "scene", "origin": "workshop", "path": "C:\\ws\\3\\project.json", "thumb": "", "rating": "Everyone", "tags": [] })]);
        self.keys = Load::Ready(json!({ "gemini": true }));
        self.claude = Load::Ready(demo_claude());
        self.models = Load::Ready(vec![("gemini-2.5-flash".into(), "Gemini 2.5 Flash".into()), ("gpt-4o".into(), "GPT-4o".into())]);
    }

    // ------------------------------------------------------------ config values

    fn lookup(&self, cfg: &Config, path: &str) -> Value {
        if let Some(key) = path.strip_prefix("$theme.") {
            if let Some(d) = &self.wallpaper_draft {
                return match key {
                    "perMonitor" => json!(d.per_monitor),
                    "selection" => d.profile[key].as_str().map(|s| json!(s)).unwrap_or(json!("automatic")),
                    "intensity" => d.profile[key].as_f64().map(|v| json!(v)).unwrap_or(json!(1.0)),
                    _ => key.split('.').try_fold(&d.profile, |v, k| v.get(k)).cloned().unwrap_or(json!("")),
                };
            }
            return Value::Null;
        }
        if let Some((v, _)) = self.over.get(path) { return v.clone(); }
        match cfg.get(path) { Value::Null => default_for(path), v => v.clone() }
    }
    pub fn get(&self, cx: &Ctx, path: &str) -> Value { self.lookup(&cx.cfg, path) }
    pub fn gb(&self, cx: &Ctx, path: &str) -> bool { self.get(cx, path).as_bool().unwrap_or(false) }
    pub fn gn(&self, cx: &Ctx, path: &str) -> f64 { self.get(cx, path).as_f64().unwrap_or(0.0) }
    pub fn gs(&self, cx: &Ctx, path: &str) -> String { self.get(cx, path).as_str().unwrap_or("").to_string() }
    pub fn garr(&self, cx: &Ctx, path: &str) -> Vec<String> {
        self.get(cx, path).as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }
    fn garr_c(&self, path: &str) -> Vec<String> {
        self.lookup(&self.cfgc, path).as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    /// set() in settings.js: remember the value, write it with the bridge.
    fn set(&mut self, path: &str, v: Value) {
        if let Some(key) = path.strip_prefix("$theme.") {
            if let Some(d) = &mut self.wallpaper_draft {
                if key == "perMonitor" { d.per_monitor = v == true; }
                else {
                    let mut at = &mut d.profile;
                    let parts: Vec<_> = key.split('.').collect();
                    for part in &parts[..parts.len() - 1] {
                        if !at[*part].is_object() { at[*part] = json!({}); }
                        at = &mut at[*part];
                    }
                    let last = parts[parts.len() - 1];
                    if key.starts_with("overrides.") && v == "" { if let Some(map) = at.as_object_mut() { map.remove(last); } }
                    else { at[last] = v; }
                }
            }
            return;
        }
        self.over.insert(path.into(), (v.clone(), hub::now_ms()));
        store::config_set(self.cfgc_bridge(), path, &v, &self.inbox);
    }
    fn cfgc_bridge(&self) -> Config {
        Config::parse(&json!({ "scriptsDir": self.cfgc.str("scriptsDir") }).to_string())
    }

    /// The config with the keys written but not yet read back from config.json laid over it (the Keybinds page reads this).
    pub fn keys_cfg(&self, cx: &Ctx) -> Config {
        let mut v = cx.cfg.0.clone();
        for (p, (val, _)) in &self.over { if let Some(id) = p.strip_prefix("keybinds.") { v["keybinds"][id] = val.clone(); } }
        Config(v)
    }

    /// Takes the combo the hook recorded for the waiting Keybinds row; gives up when the window loses focus or time runs out.
    fn rec_sync(&mut self, cx: &Ctx) {
        let got = crate::keymap::take_recorded();
        if self.rec.is_some() && got.is_none() && (!self.active || !crate::keymap::recording()) { crate::keymap::record(false); self.rec = None; return; }
        if let Some(got) = got { self.rec_done(cx, got); }
    }

    /// The combo for the waiting row: None is Esc. A key another bind uses is refused with a toast.
    fn rec_done(&mut self, cx: &Ctx, got: Option<String>) {
        let Some((id, i)) = self.rec.take() else { return };
        let Some(k) = got else { return };
        let cfg = self.keys_cfg(cx);
        match crate::keymap::taken_by(&cfg, &id, &k) {
            Some(who) => self.toast(&format!("{k} is already used by {who}")),
            None => { self.set(&format!("keybinds.{id}"), json!(crate::keymap::with_key(&cfg, &id, i, &k))); self.toast("Saved"); }
        }
    }

    pub fn toast(&mut self, msg: &str) { self.toast = Some((msg.to_string(), hub::now_ms() + TOAST_MS)); }

    /// The creator's clean-up options as they are set right now.
    pub fn creator_opts(&self, cx: &Ctx) -> crate::creator::Opts {
        crate::creator::Opts {
            clean: self.gn(cx, "buddies.make.clean").clamp(0.0, crate::creator::MAX_CLEAN as f64) as u8,
            trim: self.gb(cx, "buddies.make.trim"),
            flip: self.gb(cx, "buddies.make.flip"),
        }
    }

    /// Saves the creator's buddy as a pack folder under `root`, from the picture the preview wrote to `preview`. `images` is
    /// where the chosen picture was copied; it is deleted afterwards. The draft (picture and name) is cleared on success.
    fn make_buddy(&mut self, cx: &Ctx, root: &std::path::Path, preview: &std::path::Path, images: &std::path::Path) {
        use crate::creator;
        let (image, name) = (self.gs(cx, "buddies.make.image"), self.gs(cx, "buddies.make.name"));
        if image.is_empty() { return self.toast("Choose a picture first"); }
        if name.trim().is_empty() { return self.toast("Give the buddy a name first"); }
        // what is on disk must be the picture and options on screen
        let key = self.creator_opts(cx).key(&image);
        if !self.creator_prev.as_ref().is_some_and(|p| p.key == key && p.pack.is_some()) { return self.toast("The picture isn't ready yet. Check the preview, then try again"); }
        let Ok(png) = std::fs::read(preview.join("stand.png")) else { return self.toast("The preview picture is missing. Try again") };
        let id = creator::unique_id(root, &name);
        let manifest = creator::manifest(&name, self.gn(cx, "buddies.make.height") as u32, &self.gs(cx, "buddies.make.base"), &self.gs(cx, "buddies.make.special"));
        if let Err(e) = creator::save(root, &id, &png, &manifest) { return self.toast(&e); }
        crate::pack::forget();
        if creator::is_own_picture(&image) { let _ = std::fs::remove_file(images.join(&image)); }
        self.set("buddies.make.image", json!(""));
        self.set("buddies.make.name", json!(""));
        self.toast(&format!("Saved {}. Pick it under Buddies > Character", name.trim()));
    }

    fn sync(&mut self, cx: &Ctx) {
        self.cfgc = Config(cx.cfg.0.clone());
        let now = hub::now_ms();
        self.over.retain(|k, (v, t)| now - *t < 8000.0 && cx.cfg.get(k) != v);
        if self.toast.as_ref().is_some_and(|t| t.1 < now) { self.toast = None; }
        for f in self.inbox.take() { f(self); }
        self.rec_sync(cx);
        let path = conf::pack_dir().join("theme.json");
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if stamp != self.info.stamp || self.info.mode.is_empty() {
            // External wallpaper changes also replace the per-monitor files shown in Settings.
            self.wmons = Load::Idle;
            let v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
            let s = |k: &str, d: &str| v[k].as_str().filter(|s| !s.is_empty()).unwrap_or(d).to_string();
            if self.wallpaper_draft.as_ref().is_some_and(|d| d.analysis["identity"] != v["analysis"]["identity"]) { self.wallpaper_draft = None; }
            self.info = Info { mode: s("mode", "dark"), scheme: s("scheme", "auto"), seed: s("seed", "#6750a4"), live: v["live"].as_str().is_some_and(|l| !l.is_empty()) || v["live"] == true, stamp, analysis: v["analysis"].clone(), profile: v["profile"].clone() };
        }
    }

    // ------------------------------------------------------------ background loading (the `once` calls in render())

    fn ensure_loaded(&mut self, cx: &Ctx) {
        if self.rendering { return; }
        match self.current_page() {
            Page::Presets => {
                if matches!(self.online, Load::Idle) {
                    self.online = Load::Loading;
                    store::bridge(cx, &self.inbox, "presets-online", vec![], |s, r| {
                        let ok = if r["error"].is_string() { r } else { json!({ "presets": r["presets"].clone() }) };
                        let list: Vec<(String, String)> = ok["presets"].as_array().into_iter().flatten()
                            .filter_map(|p| Some((p["name"].as_str()?.to_string(), p["preview"].as_str()?.to_string()))).collect();
                        let inbox = s.inbox.clone();
                        std::thread::spawn(move || {
                            for (name, url) in list {
                                let path = preview_path(&name);
                                if url.is_empty() || path.exists() { continue; }
                                if let Some(b) = store::curl_bytes(&url) {
                                    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
                                    let _ = std::fs::write(&path, b);
                                    inbox.send(|_| {});
                                }
                            }
                        });
                        s.online = Load::Ready(ok);
                    });
                }
                if matches!(self.local, Load::Idle) {
                    self.local = Load::Loading;
                    store::bridge(cx, &self.inbox, "presets-local", vec![], |s, r| s.local = Load::Ready(r["presets"].as_array().cloned().unwrap_or_default()));
                }
            }
            Page::Wallpapers => {
                if matches!(self.we, Load::Idle) {
                    self.we = Load::Loading;
                    store::bridge(cx, &self.inbox, "we-status", vec![], |s, r| s.we = Load::Ready(if r["error"].is_string() || r.is_null() { json!({ "installed": false }) } else { r }));
                }
                if matches!(self.we_list, Load::Idle) && matches!(&self.we, Load::Ready(w) if w["installed"] == true) {
                    self.we_list = Load::Loading;
                    store::bridge(cx, &self.inbox, "we-list", vec![], |s, r| s.we_list = Load::Ready(r["items"].as_array().cloned().unwrap_or_default()));
                }
            }
            Page::Quick => {
                if matches!(self.wmons, Load::Idle) {
                    self.wmons = Load::Loading;
                    store::bridge(cx, &self.inbox, "wall-monitors", vec![], |s, r| s.wmons = Load::Ready(r["monitors"].as_array().cloned().unwrap_or_default()));
                }
                if matches!(self.mons, Load::Idle) {
                    self.mons = Load::Loading;
                    store::script(cx, &self.inbox, "toggles", vec!["-Action".into(), "monitors".into()], |s, r| {
                        let list = r["monitors"].as_array().cloned().unwrap_or_default();
                        // pinned in `over` (far-future stamp) so the sliders show the hardware value
                        for m in &list { s.over.insert(format!("monitor.{}", m["i"]), (m["brightness"].clone(), 1e18)); }
                        s.mons = Load::Ready(list.iter().map(|m| format!("{}|{}", m["i"], m["name"].as_str().unwrap_or(""))).collect());
                    });
                }
            }
            // the buddies page lists the same models, for "Screen help model"
            Page::Services | Page::Buddies => {
                if self.current_page() == Page::Services && matches!(self.keys, Load::Idle) {
                    self.keys = Load::Loading;
                    store::bridge(cx, &self.inbox, "secret-status", vec![], |s, r| s.keys = Load::Ready(if r["error"].is_string() || r.is_null() { json!({}) } else { r }));
                }
                if self.current_page() == Page::Services && matches!(self.claude, Load::Idle) && crate::bridge::claude_installed() {
                    self.claude = Load::Loading;
                    store::bridge(cx, &self.inbox, "claude-accounts", vec![], |s, r| s.claude = Load::Ready(if r.is_null() { json!({}) } else { r }));
                }
                if matches!(self.models, Load::Idle) {
                    self.models = Load::Loading;
                    store::bridge(cx, &self.inbox, "ai-models", vec![], |s, r| {
                        s.models = Load::Ready(r["models"].as_array().into_iter().flatten().filter_map(|m| {
                            let id = m["id"].as_str()?.to_string();
                            Some((id.clone(), m["name"].as_str().unwrap_or(&id).to_string()))
                        }).collect());
                    });
                }
            }
            _ => {}
        }
    }

    fn loading(&self) -> bool {
        matches!(self.current_page(), Page::Presets | Page::Wallpapers | Page::Services)
            && [matches!(self.online, Load::Loading), matches!(self.local, Load::Loading), matches!(self.we, Load::Loading), matches!(self.we_list, Load::Loading),
                matches!(self.keys, Load::Loading), matches!(self.claude, Load::Loading), matches!(self.models, Load::Loading)].contains(&true)
    }

    // ------------------------------------------------------------ text fields

    fn blur(&mut self, cx: &mut Ctx) {
        if let Some(k) = self.focus.take() {
            // a repaint between the press on "Rename" and its click would reset the field to the old name, so save on blur
            self.commit(cx, &k, k.starts_with("bname-"));
        }
    }

    /// The `change` event of an input: blur (submit = false) or Enter (true).
    fn commit(&mut self, cx: &mut Ctx, key: &str, submit: bool) {
        let Some(f) = self.fields.get_mut(key) else { return };
        let text = f.tb.text.clone();
        match f.kind {
            Kind::Text { list } if text != f.orig => {
                f.orig = text.clone();
                let v = if list { json!(text.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>()) } else { json!(text) };
                self.set(key, v);
                if !key.starts_with("$theme.") { self.toast("Saved"); }
            }
            Kind::Num(min, max) if text != f.orig => {
                f.orig = text.clone();
                let digits: String = text.trim().chars().take_while(char::is_ascii_digit).collect();
                let n: i64 = digits.parse().unwrap_or(0);
                self.set(key, json!(if n == 0 { min } else { n.clamp(min, max) }));
            }
            Kind::Plain | Kind::Secret if submit => match key {
                "save-name" => self.act(cx, Act::Do("save-preset", String::new())),
                "seed" => self.act(cx, Act::Do("seed", String::new())),
                "agent-name" | "agent-dir" | "agent-file" => self.act(cx, Act::Do("add-agent", String::new())),
                "contact-name" | "contact-value" => self.act(cx, Act::Do("add-contact", String::new())),
                k if k.starts_with("bname-") => { let slot = &k[6..]; let id = self.gs(cx, &format!("buddies.{slot}.character")); self.act(cx, Act::Do("rename-buddy", format!("{slot}:{id}"))) }
                k if k.starts_with("key-") => self.act(cx, Act::Do("save-key", k[4..].to_string())),
                "voice-name" => self.act(cx, Act::Do("voice-add", String::new())),
                "ref-name" => self.act(cx, Act::Do("voice-save-ref", String::new())),
                k if k.starts_with("vname-") => self.act(cx, Act::Do("voice-rename", k[6..].to_string())),
                _ => {}
            },
            _ => {}
        }
        // a number shows what was kept, not what was typed
        if let Some(f) = self.fields.get_mut(key) { if matches!(f.kind, Kind::Num(..)) { f.orig = "\u{0}".into(); } }
    }

    // ------------------------------------------------------------ actions

    fn retheme(&mut self, cx: &Ctx, args: &[&str], label: &str) {
        self.busy = label.into();
        store::script(cx, &self.inbox, "switchwall", args.iter().map(|a| a.to_string()).collect(), |s, r| {
            s.busy.clear();
            if s.picker.is_none() { conf::set_preview(None); } // the applied theme has landed
            if let Some(e) = store::failure(&r) { s.toast(&e); }
        });
    }

    /// showCard(): picking a picture turns its card on (a card is only open when it's in desktop.widgets).
    fn show_card(&mut self, id: &str) {
        let cur = self.garr_c("desktop.widgets");
        if !cur.iter().any(|c| c == id) {
            let v: Vec<&str> = DESKTOP_WIDGETS.iter().map(|w| w.0).filter(|w| cur.iter().any(|c| c == w) || *w == id).collect();
            self.set("desktop.widgets", json!(v));
        }
    }

    fn act(&mut self, cx: &mut Ctx, a: Act) {
        crate::log::line(&format!("settings: {}", a.label()));
        match a {
            Act::Page(i) => { self.set_page(i); self.menu = None; self.bl_focus = None; }
            Act::Switch(path) => {
                let v = !self.gb(cx, &path);
                self.set(&path, json!(v));
                if path == "runOnStartup" {
                    if let Err(e) = write_startup_flag(v) { self.toast(&format!("Couldn't change startup: {e}. Re-run install.ps1 as admin once")); }
                }
            }
            Act::Seg(path, v) => self.set(&path, v),
            Act::Step(path, d, min, max) => {
                let v = (self.gn(cx, &path) as i64 + d).clamp(min, max);
                self.set(&path, json!(v));
            }
            Act::Select(path, anchor, opts) => {
                self.menu = if self.menu.as_ref().is_some_and(|m| m.path == path) { None } else { Some(Menu { path, anchor, opts, scroll: Scroll::default() }) };
            }
            Act::Goto(i, sect, label) => {
                if let Some(f) = self.fields.get_mut(SEARCH) { f.tb.set(""); }
                self.focus = None;
                hub::set(GOTO, Some(format!("{}|{sect}|{label}", PAGES[i].key)));
                self.set_page(i);
            }
            Act::Slider(..) | Act::Field(..) => {}
            Act::Do(name, arg) => self.do_named(cx, name, arg),
        }
    }

    fn do_named(&mut self, cx: &mut Ctx, name: &'static str, arg: String) {
        let inbox = self.inbox.clone();
        match name {
            "toggle-nav" => self.nav_collapsed = !self.nav_collapsed,
            "wall" => { self.wmons = Load::Idle; self.retheme(cx, &[], "wall") }
            // a Keybinds chip: start recording a combo for that key, or stop if it is the one recording
            "key-rec" => {
                let again = self.rec.as_ref().is_some_and(|r| format!("{}:{}", r.0, r.1) == arg);
                crate::keymap::record(false);
                self.rec = None;
                if !again { if let Some((id, i)) = arg.rsplit_once(':').and_then(|(id, i)| Some((id.to_string(), i.parse().ok()?))) { self.rec = Some((id, i)); crate::keymap::record(true); } }
            }
            "key-reset" => self.set(&format!("keybinds.{arg}"), json!([])),
            // a picture (or back to the main one) for one monitor; the monitor list is read again afterwards
            "mon-wall" | "mon-clear" => {
                self.busy = "wall".into();
                let mut args = vec!["-Monitor".to_string(), arg];
                if name == "mon-clear" { args.push("-Clear".into()); }
                store::script(cx, &inbox, "switchwall", args, |s, r| {
                    s.busy.clear();
                    s.wmons = Load::Idle;
                    if r["cancelled"] != true { if let Some(e) = store::failure(&r) { s.toast(&e); } }
                });
            }
            "wall-palettes" | "wall-candidate" | "wall-reset" | "wall-cancel" | "wall-apply" => self.wallpaper_action(cx, name, &arg),
            "mode" | "scheme" if self.info.analysis.is_object() => self.wallpaper_action(cx, name, &arg),
            "mode" => self.retheme(cx, &["-Mode", &arg], "mode"),
            "scheme" => self.retheme(cx, &["-Scheme", &arg], "scheme"),
            "clearcolor" => self.set(&arg, json!("")),
            "pickcolor" => {
                let start = conf::parse_hex(&self.gs(cx, &arg)).or_else(|| self.wallpaper_override_color(&arg)).or_else(|| conf::parse_hex(&self.gs(cx, "visualizer.color"))).unwrap_or_else(|| cx.theme.c("primary"));
                self.menu = None;
                self.pick_path = Some(arg);
                self.picker = Some(picker::Picker::new(start, self.area));
            }
            "picker" => {
                let hex = self.fields.get("seed").map(|f| f.tb.text.trim().to_string()).unwrap_or_default();
                self.menu = None;
                self.picker = Some(picker::Picker::new(conf::parse_hex(&hex).or_else(|| conf::parse_hex(&self.info.seed)).unwrap_or(0xFF67_50A4), self.area));
                self.update_preview();
            }
            "seed" => {
                let hex = self.fields.get("seed").map(|f| f.tb.text.trim().to_string()).unwrap_or_default();
                if conf::parse_hex(&hex).is_some_and(|_| hex.len() == 7) { self.info.seed = hex.clone(); self.retheme(cx, &["-Color", &hex], "seed"); }
                else { self.toast("Enter a color like #6750a4"); }
            }
            "random-online" => {
                self.busy = "wall".into();
                let cfg = store::bcfg(cx);
                std::thread::spawn(move || {
                    let page = 1 + (hub::now_ms() as u64 / 7) % 5;
                    let r = proc::bridge(&cfg, "booru", &["wallhaven", &page.to_string()]);
                    let items = r["items"].as_array().cloned().unwrap_or_default();
                    let Some(it) = items.get((hub::now_ms() as usize / 3) % items.len().max(1)).cloned() else {
                        let e = r["error"].as_str().unwrap_or("Nothing found").to_string();
                        return inbox.send(move |s| { s.busy.clear(); s.toast(&e); });
                    };
                    proc::bridge(&cfg, "download-wallpaper", &[it["file"].as_str().unwrap_or(""), "--apply"]);
                    inbox.send(|s| s.busy.clear());
                });
            }
            "winsettings" => proc::run_bg(&cx.cfg, "toggles", &["-Action", "settings", "-Value", ""]),
            "editcfg" => proc::run_bg(&cx.cfg, "toggles", &["-Action", "editconfig"]),
            "update" => {
                self.toast("Checking for updates...");
                let inbox = self.inbox.clone();
                std::thread::spawn(move || {
                    let msg = match crate::bridge::update::check(&crate::conf::Config::load(), true) {
                        Ok(Some(tag)) => format!("Updating to {tag}. The app restarts"),
                        Ok(None) => format!("You're up to date ({})", env!("CARGO_PKG_VERSION")),
                        Err(e) => format!("Update check failed: {e}"),
                    };
                    inbox.send(move |s| s.toast(&msg));
                });
            }
            "url" => store::open_url(&arg),
            "close" => self.close_settings(cx),
            "bu" => {
                let cur = self.garr(cx, "bar.utils");
                let next: Vec<&str> = ["snip", "pick", "wall", "mode"].into_iter().filter(|v| cur.iter().any(|c| c == v) != (*v == arg)).collect();
                self.set("bar.utils", json!(next));
            }
            "pick-image" => {
                let slot = arg.clone();
                store::bridge(cx, &inbox, "pick-image", vec![arg], move |s, r| {
                    if let (Some(f), Some(k)) = (r["file"].as_str(), slot.strip_prefix("buddy-").and_then(|r| r.split('-').next())) {
                        s.set(&format!("buddies.{k}.image"), json!(f));
                    } else if let Some(f) = r["file"].as_str() {
                        s.set(&if slot == "greet" { "desktop.greet.file".to_string() } else { format!("desktop.images.{slot}.file") }, json!(f));
                        s.show_card(&slot);
                    } else if let Some(e) = r["error"].as_str() { s.toast(e); }
                });
            }
            "wall-image" => { self.set(&format!("desktop.images.{arg}.file"), json!("wallpaper.jpg")); self.show_card(&arg); }
            "tz" => {
                let mut cur = self.garr(cx, "desktop.worldClocks");
                match cur.iter().position(|c| *c == arg) { Some(i) => { cur.remove(i); } None => cur.push(arg) }
                let listed = |z: &str| pages::CITIES.iter().any(|c| c.0 == z);
                let ordered: Vec<String> = pages::CITIES.iter().map(|c| c.0.to_string()).filter(|z| cur.contains(z)).chain(cur.iter().filter(|z| !listed(z)).cloned()).collect();
                self.set("desktop.worldClocks", json!(ordered));
            }
            "dwidget" => {
                let mut cur = self.garr(cx, "desktop.widgets");
                let was = cur.contains(&arg);
                if was { cur.retain(|c| *c != arg) } else { cur.push(arg.clone()) }
                let v: Vec<&str> = DESKTOP_WIDGETS.iter().map(|w| w.0).filter(|w| cur.iter().any(|c| c == w)).collect();
                self.set("desktop.widgets", json!(v));
            }
            "new-buddy" => {
                let key = |f: &str| format!("buddies.{arg}.{f}");
                let cur = self.gs(cx, &key("character"));
                let look: Vec<(&str, String)> = ["hair", "shirt", "pants", "eyes", "bun", "style"].iter().chain(&crate::cast::TRIM_FIELDS).map(|f| (*f, self.gs(cx, &key(f)))).collect();
                let mut list = self.get(cx, "buddies.custom").as_array().cloned().unwrap_or_default();
                let id = format!("custom{}", hub::now_ms() as i64);
                let e = crate::cast::save(&list, &cur, &look.iter().map(|(f, v)| (*f, v.as_str())).collect::<Vec<_>>(), &self.gs(cx, &key("name")), &id);
                let name = e["name"].as_str().unwrap_or("").to_string();
                list.push(e);
                self.set("buddies.custom", json!(list));
                // the slot now shows the new buddy, whose look already has what its own fields held
                // the saved name replaces the slot's own, or renaming the saved buddy would change nothing on screen
                for f in look.iter().map(|(f, _)| *f).chain(["name"]) { self.set(&key(f), json!("")); }
                self.set(&key("character"), json!(id));
                self.toast(&format!("Created {name}"));
            }
            "open-buddies" => open_folder(&crate::pack::buddies_dir()),
            "make-buddy" => self.make_buddy(cx, &crate::pack::buddies_dir(), &crate::creator::preview_dir(), &conf::pack_dir().join("images")),
            "open-speech" => open_folder(&crate::conf::speech_dir()),
            "cancel-speech" => crate::bridge::listen::cancel_download(),
            "delete-speech" => {
                use crate::bridge::listen::{delete_model, MODELS};
                let Some(m) = MODELS.iter().find(|m| m.id == arg) else { return };
                match delete_model(m.id) {
                    Ok(()) => {
                        let path = m.path().to_string_lossy().into_owned();
                        if self.gs(cx, "buddies.model") == path { self.set("buddies.model", json!("")); } // Automatic again
                        self.toast("Speech model deleted");
                    }
                    Err(e) => self.toast(&format!("Could not delete it: {e}")),
                }
            }
            "download-speech" => {
                use crate::bridge::listen::{download_model, CANCELLED, MODELS};
                if !self.busy.is_empty() { return; }
                let Some(m) = MODELS.iter().find(|m| m.id == arg) else { return };
                let (id, name, path) = (m.id, m.name, m.path().to_string_lossy().into_owned());
                self.busy = format!("speech-dl:{id}");
                self.toast(&format!("Downloading {name} ({} MB). This takes a few minutes", m.mb));
                std::thread::spawn(move || {
                    let r = download_model(id);
                    inbox.send(move |s| {
                        s.busy.clear();
                        match r {
                            Ok(()) => { s.set("buddies.model", json!(path)); s.toast("Speech model installed and in use"); }
                            Err(e) if e == CANCELLED => s.toast("Download cancelled"),
                            Err(e) => s.toast(&format!("Speech model download failed: {e}")),
                        }
                    });
                });
            }
            "voice-install" => {
                if !self.busy.is_empty() { return; }
                let Ok(approvals) = crate::voice::approvals_for(&arg) else { return };
                let mb = crate::voice::download_bytes(&arg).map(|b| (b as f64 / 1_048_576.0).round() as u64).unwrap_or(0);
                self.busy = format!("voice-install:{arg}");
                self.toast(&format!("Downloading {mb} MB. This takes a few minutes"));
                std::thread::spawn(move || {
                    let id = arg.clone();
                    let r = crate::voice::install_catalog(&id, &approvals);
                    inbox.send(move |s| {
                        s.busy.clear();
                        match r {
                            Ok(()) => {
                                crate::voice::refresh_providers();
                                // a provider chosen before it was installed could not apply then; apply the saved choice now
                                let section = s.cfgc.get("voice").clone();
                                crate::voice::configure(&section, s.cfgc.str("buddies.a.character"), s.cfgc.str("buddies.b.character"));
                                s.toast("Installed");
                            }
                            Err(e) => s.toast(&format!("Install failed: {e}")),
                        }
                    });
                });
            }
            "voice-remove" => match crate::voice::remove(&arg) {
                Ok(()) => {
                    if self.gs(cx, "voice.provider") == arg { self.set("voice.provider", json!("windows")); }
                    self.toast("Removed");
                }
                Err(e) => self.toast(&format!("Could not remove it: {e}")),
            },
            "voice-test" => crate::voice::test_voice(),
            "voice-pick-ref" => {
                let inbox = self.inbox.clone();
                std::thread::spawn(move || {
                    let Some(picked) = crate::bridge::theme::pick_wav() else { return };
                    let path = std::path::PathBuf::from(picked);
                    let result = crate::voice::reference::inspect(&path);
                    inbox.send(move |s| {
                        use crate::voice::reference::{set_draft, Draft};
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        match result {
                            Ok((_, c)) => set_draft(Draft { note: format!("{name}: {:.1} s at {} Hz, ready to keep", c.duration_secs, c.rate), path: Some(path), check: Some(c) }),
                            Err(e) => set_draft(Draft { note: e, path: None, check: None }),
                        }
                        let _ = s;
                    });
                });
            }
            "voice-save-ref" => {
                use crate::voice::{library, profile::VoiceProfile, reference};
                let d = reference::draft();
                let (Some(path), Some(_)) = (d.path.clone(), d.check.clone()) else { self.toast("Choose a recording first"); return };
                let name = self.fields.get("ref-name").map(|f| f.tb.text.clone()).unwrap_or_default();
                let transcript = self.fields.get("ref-transcript").map(|f| f.tb.text.clone()).unwrap_or_default();
                let mut list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                let existing: Vec<VoiceProfile> = list.iter().filter_map(VoiceProfile::from_json).collect();
                // the name is checked before anything is copied, so a refused name leaves no file behind
                if let Err(e) = library::check_name(&existing, &name, None) { self.toast(&e); return; }
                let (bytes, check) = match reference::inspect(&path) { Ok(b) => b, Err(e) => { self.toast(&e); return; } };
                let id = crate::voice::profile::new_id();
                let kept = match reference::keep(&reference::root(), &id, &bytes, &check, &transcript) { Ok(r) => r, Err(e) => { self.toast(&e); return; } };
                if let Err(e) = library::create_reference(&mut list, &id, &name, kept) { self.toast(&e); return; }
                self.set("voice.profiles", json!(list));
                reference::set_draft(reference::Draft::default());
                for key in ["ref-name", "ref-transcript"] { if let Some(f) = self.fields.get_mut(key) { f.tb.set(""); f.orig.clear(); } }
                self.toast("Recording kept. It can be cloned once a provider can clone voices");
            }
            "voice-clone" => {
                use crate::voice::profile::VoiceProfile;
                let Some(mut profile) = self.get(cx, "voice.profiles").as_array()
                    .and_then(|l| l.iter().filter_map(VoiceProfile::from_json).find(|p| p.id == arg)) else { return };
                self.toast("Cloning. This can take a minute");
                let inbox = self.inbox.clone();
                std::thread::spawn(move || {
                    let r = crate::voice::clone_profile(&mut profile);
                    inbox.send(move |s| match r {
                        Ok(provider) => {
                            let mut list = s.cfgc.get("voice.profiles").as_array().cloned().unwrap_or_default();
                            crate::voice::library::put(&mut list, &profile);
                            s.set("voice.profiles", json!(list));
                            s.toast(&format!("Cloned with {provider}"));
                        }
                        Err(e) => s.toast(&e),
                    });
                });
            }
            "voice-preview" => { if let Err(e) = crate::voice::preview_voice(&arg) { self.toast(&e); } }
            "voice-speed" => {
                use crate::voice::library;
                let Some((id, s)) = arg.split_once(':') else { return };
                let Ok(speed) = s.parse::<f32>() else { return };
                let mut list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                match library::set_speed(&mut list, id, speed) {
                    Ok(()) => self.set("voice.profiles", json!(list)),
                    Err(e) => self.toast(&e),
                }
            }
            "voice-model" => {
                use crate::voice::library;
                let mut parts = arg.splitn(3, ':');
                let (Some(id), Some(provider), Some(model)) = (parts.next(), parts.next(), parts.next()) else { return };
                if !crate::voice::models_for(provider).contains(&model) { self.toast("That provider has no such model"); return; }
                let mut list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                match library::set_model(&mut list, id, provider, model) {
                    Ok(()) => self.set("voice.profiles", json!(list)),
                    Err(e) => self.toast(&e),
                }
            }
            "voice-rec-start" => { if let Err(e) = crate::voice::record::start() { self.toast(&e); } }
            "voice-rec-stop" => {
                let inbox = self.inbox.clone();
                std::thread::spawn(move || {
                    let result = crate::voice::record::stop()
                        .and_then(|path| crate::voice::reference::inspect(&path).map(|(_, c)| (path, c)));
                    inbox.send(move |s| {
                        use crate::voice::reference::{set_draft, Draft};
                        let _ = s;
                        match result {
                            Ok((path, c)) => set_draft(Draft { note: format!("Recorded {:.1} s, ready to keep", c.duration_secs), path: Some(path), check: Some(c) }),
                            Err(e) => set_draft(Draft { note: e, path: None, check: None }),
                        }
                    });
                });
            }
            "voice-discard" => {
                use crate::voice::reference;
                // a recording made here sits in the temp folder; a picked file is the user's and stays
                if let Some(p) = reference::draft().path.filter(|p| p.starts_with(std::env::temp_dir())) { std::fs::remove_file(p).ok(); }
                reference::set_draft(Default::default());
            }
            "voice-assign" => {
                let Some((slot, id)) = arg.split_once(':') else { return };
                let list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                if let Err(e) = crate::voice::can_speak_in(&list, id) { self.toast(&format!("That voice cannot speak yet: {e}")); return; }
                self.set(if slot == "a" { "voice.buddies.a" } else { "voice.buddies.b" }, json!(id));
                self.toast("Voice set for that Buddy");
            }
            "voice-delete" => {
                use crate::voice::library;
                let mut list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                let used = [
                    library::effective(&self.gs(cx, "voice.buddies.a"), &self.gs(cx, "buddies.a.character")),
                    library::effective(&self.gs(cx, "voice.buddies.b"), &self.gs(cx, "buddies.b.character")),
                ];
                let refs: Vec<&str> = used.iter().map(String::as_str).collect();
                // the kept recording goes with the voice, but only if it sits in the references folder
                let recording_dir = list.iter().filter_map(crate::voice::profile::VoiceProfile::from_json).find(|p| p.id == arg)
                    .and_then(|p| p.reference).and_then(|r| r.path.parent().map(std::path::PathBuf::from))
                    .filter(|dir| dir.starts_with(crate::voice::reference::root()));
                match library::delete(&mut list, &arg, &refs) {
                    Ok(()) => {
                        if let Some(dir) = recording_dir { std::fs::remove_dir_all(dir).ok(); }
                        self.set("voice.profiles", json!(list));
                        self.toast("Voice deleted");
                    }
                    Err(e) => self.toast(&e),
                }
            }
            "voice-rename" => {
                use crate::voice::library;
                let Some(name) = self.fields.get(&format!("vname-{arg}")).map(|f| f.tb.text.clone()) else { return };
                let mut list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                match library::rename(&mut list, &arg, &name) {
                    Ok(()) => { self.set("voice.profiles", json!(list)); self.toast("Renamed"); }
                    Err(e) => self.toast(&e),
                }
            }
            "voice-draft-provider" => {
                let first = crate::voice::provider_voices(&arg).first().map(|v| v.id.clone()).unwrap_or_default();
                crate::voice::library::update_draft(|d| { d.provider = arg.clone(); d.voice = first; });
            }
            "voice-draft-voice" => crate::voice::library::update_draft(|d| d.voice = arg.clone()),
            "voice-draft-speed" => { if let Ok(s) = arg.parse::<f32>() { crate::voice::library::update_draft(|d| d.speed = s); } }
            "voice-add" => {
                use crate::voice::library;
                let name = self.fields.get("voice-name").map(|f| f.tb.text.clone()).unwrap_or_default();
                let d = library::draft();
                let mut list = self.get(cx, "voice.profiles").as_array().cloned().unwrap_or_default();
                let id = crate::voice::profile::new_id();
                match library::add_custom(&mut list, &id, &name, &d.provider, &d.voice, None, d.speed) {
                    Ok(()) => {
                        self.set("voice.profiles", json!(list));
                        if let Some(f) = self.fields.get_mut("voice-name") { f.tb.set(""); f.orig.clear(); }
                        self.toast("Voice saved. Put it on a Buddy below");
                    }
                    Err(e) => self.toast(&e),
                }
            }
            "clear-buddy-image" => self.set(&format!("buddies.{arg}.image"), json!("")),
            "rename-buddy" => {
                let Some((slot, id)) = arg.split_once(':') else { return };
                let Some(f) = self.fields.get(&format!("bname-{slot}")) else { return };
                let mut list = self.get(cx, "buddies.custom").as_array().cloned().unwrap_or_default();
                if crate::cast::rename(&mut list, id, &f.tb.text) { self.set("buddies.custom", json!(list)); }
            }
            "remove-buddy" => {
                let mut list = self.get(cx, "buddies.custom").as_array().cloned().unwrap_or_default();
                if crate::cast::remove(&mut list, &arg) {
                    self.set("buddies.custom", json!(list));
                    // a slot still showing it goes back to the first built-in character
                    for k in ["a", "b"] {
                        let path = format!("buddies.{k}.character");
                        if self.gs(cx, &path) == arg { self.set(&path, json!("gojo")); }
                    }
                    self.toast("Buddy removed");
                }
            }
            "reset-desk" => {
                // native cards keep their spots in state.json
                cx.state.update(|v| v["deskPos"] = json!({}));
                self.set("desktop.layoutReset", json!(hub::now_ms() as i64));
                self.toast("Layout reset");
            }
            "league" => {
                let mut cur = self.garr(cx, "football.leagues");
                if let Some(i) = cur.iter().position(|c| *c == arg) { cur.remove(i); } else { cur.push(arg); }
                self.set("football.leagues", json!(cur));
            }
            "apply-online" => {
                self.busy = arg.clone();
                store::bridge(cx, &inbox, "preset-apply-online", vec![arg.clone()], move |s, r| {
                    s.busy.clear();
                    s.local = Load::Idle;
                    let left_out: Vec<&str> = r["unsupported"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
                    let m = if r["ok"] == true && theme_failed(&r["theme"]).is_some() { format!("Applied {}, but the wallpaper / theme failed: {}", arg.replace('_', " "), theme_failed(&r["theme"]).unwrap_or_default()) }
                        else if r["ok"] == true && !left_out.is_empty() { format!("Applied {} (no card for {})", arg.replace('_', " "), left_out.join(", ")) }
                        else if r["ok"] == true { format!("Applied {}", arg.replace('_', " ")) } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "apply-local" => {
                self.busy = arg.clone();
                store::bridge(cx, &inbox, "preset-apply-local", vec![arg], |s, r| {
                    s.busy.clear();
                    let m = if let (true, Some(e)) = (r["ok"] == true, theme_failed(&r["theme"])) { format!("Preset applied, but the wallpaper / theme failed: {e}") }
                        else if r["ok"] == true { "Preset applied".to_string() } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "save-preset" => {
                let Some(f) = self.fields.get_mut("save-name") else { return };
                let n = f.tb.text.trim().to_string();
                if n.is_empty() { return; }
                f.tb.set("");
                store::bridge(cx, &inbox, "preset-save", vec![n.clone()], move |s, r| {
                    s.local = Load::Idle;
                    let m = if r["ok"] == true { format!("Saved “{n}”") } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "delete-local" => store::bridge(cx, &inbox, "preset-delete", vec![arg], |s, _| s.local = Load::Idle),
            "we-type" => self.we_type = arg,
            "we-apply" => {
                self.busy = arg.clone();
                store::bridge(cx, &inbox, "we-apply", vec![arg], |s, r| {
                    s.busy.clear();
                    s.we = Load::Idle;
                    let m = if r["ok"] == true { "Wallpaper set".to_string() } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "we-close" => store::bridge(cx, &inbox, "we-close", vec![], |s, _| s.we = Load::Idle),
            "greet-initial" => self.set("desktop.greet.file", json!("")),
            "add-agent" => {
                let val = |s: &Self, k: &str| s.fields.get(k).map(|f| f.tb.text.trim().to_string()).unwrap_or_default();
                let (name, dir, file) = (val(self, "agent-name"), val(self, "agent-dir"), val(self, "agent-file"));
                if name.is_empty() || dir.is_empty() || file.is_empty() { self.toast("Fill in the name, folder and hook file"); return; }
                let mut cur = self.get(cx, "customAgents").as_array().cloned().unwrap_or_default();
                if cur.iter().any(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(&name))) { self.toast("That agent is already added"); return; }
                cur.push(json!({ "name": name, "dir": dir, "file": file }));
                self.set("customAgents", json!(cur.clone()));
                for k in ["agent-name", "agent-dir", "agent-file"] { if let Some(f) = self.fields.get_mut(k) { f.tb.set(""); } }
                let custom = json!(cur);
                // hook it now if its folder exists (not in tests: they would rewrite the real agent configs)
                if !cfg!(test) { std::thread::spawn(move || { crate::bridge::agent_hook::auto_install(&custom); }); }
                self.toast("Agent added. Hooks go in as soon as its folder exists");
            }
            "contact-app" => { self.fields.entry("contact-app".into()).or_insert_with(|| Field::new(false)).tb.set(&arg); }
            "add-contact" => {
                let val = |s: &Self, k: &str| s.fields.get(k).map(|f| f.tb.text.clone()).unwrap_or_default();
                let app = match val(self, "contact-app").as_str() { "" => "whatsapp".to_string(), a => a.to_string() };
                let Some(entry) = crate::assist::format_contact(&val(self, "contact-name"), &app, &val(self, "contact-value")) else {
                    self.toast("Give a name and a number or id, without = or commas");
                    return;
                };
                // after the pencil, "contact-edit" holds the entry being changed: it is replaced where it stands
                let original = Some(val(self, "contact-edit")).filter(|o| !o.is_empty());
                let cur = crate::assist::with_contact(&self.garr(cx, "buddies.contacts"), original.as_deref(), entry);
                self.set("buddies.contacts", json!(cur));
                for k in ["contact-name", "contact-value", "contact-edit"] { if let Some(f) = self.fields.get_mut(k) { f.tb.set(""); } }
                if original.is_some() { self.toast("Contact saved"); }
            }
            "edit-contact" => {
                // load the contact into the form; Save then replaces it where it stands
                let Some((name, app, value)) = crate::assist::parse_contact(&arg) else { return };
                for (k, v) in [("contact-name", name.as_str()), ("contact-value", value.as_str()), ("contact-app", app.as_str()), ("contact-edit", arg.as_str())] {
                    self.fields.entry(k.into()).or_insert_with(|| Field::new(false)).tb.set(v);
                }
            }
            "cancel-edit" => { for k in ["contact-name", "contact-value", "contact-edit"] { if let Some(f) = self.fields.get_mut(k) { f.tb.set(""); } } }
            "del-contact" => {
                let mut cur = self.garr(cx, "buddies.contacts");
                cur.retain(|e| *e != arg);
                self.set("buddies.contacts", json!(cur));
                // deleting the one being edited ends the edit, so Save can't bring it back
                if self.fields.get("contact-edit").is_some_and(|f| f.tb.text == arg) {
                    for k in ["contact-name", "contact-value", "contact-edit"] { if let Some(f) = self.fields.get_mut(k) { f.tb.set(""); } }
                }
            }
            "cancel-task" => crate::agent::cancel_running(),
            "forget-memory" => {
                if let Err(e) = arg.parse::<i64>().map_err(|e| e.to_string()).and_then(crate::agent::memory::forget_id) { crate::log::line(&format!("could not forget a memory: {e}")); }
            }
            "del-agent" => {
                let mut cur = self.get(cx, "customAgents").as_array().cloned().unwrap_or_default();
                cur.retain(|a| a["name"] != arg.as_str());
                self.set("customAgents", json!(cur));
            }
            "claude-arm" => self.claude_arm = Some((arg, hub::now_ms() + CLAUDE_ARM_MS)),
            "claude-add" | "claude-relogin" | "claude-switch" | "claude-remove" | "claude-restart" => {
                // the chip only shows while armed, but a stale click after the window must not remove
                if name == "claude-remove" {
                    let ok = armed(&self.claude_arm, &arg, hub::now_ms());
                    self.claude_arm = None;
                    if !ok { return; }
                }
                let done = match name { "claude-add" | "claude-relogin" => "Sign in in the terminal that opened", "claude-switch" => "New terminals will use this account", "claude-restart" => "No open sessions to restart", _ => "Account removed" };
                let args = if name == "claude-add" || name == "claude-restart" { vec![] } else { vec![arg] };
                store::bridge(cx, &inbox, name, args, move |s, r| {
                    s.claude = Load::Idle;
                    // a switch that also restarted sessions says how that went
                    let ok = r["message"].as_str().filter(|m| !m.is_empty()).unwrap_or(done);
                    s.toast(&if r["ok"] == true { ok.to_string() } else { r["error"].as_str().unwrap_or("Failed").to_string() });
                });
            }
            "clear-key" => store::bridge(cx, &inbox, "secret-set", vec![arg], |s, _| {
                s.keys = Load::Idle;
                // a cleared Discord token stops the bot
                crate::messaging::sync();
            }),
            // a new pairing code for the Discord bot; the card shows it until it is used or runs out
            "discord-pair" => { crate::messaging::pairing::new_code(std::time::Instant::now()); }
            // the account stops reaching the buddy from its next message
            "discord-revoke" => {
                if let Ok(id) = arg.parse::<u64>() { crate::messaging::pairing::unpair(crate::messaging::DISCORD, id); }
            }
            "save-key" => {
                let Some(f) = self.fields.get_mut(&format!("key-{arg}")) else { return };
                let v = f.tb.text.trim().to_string();
                if v.is_empty() { return; }
                f.tb.set("");
                store::bridge(cx, &inbox, "secret-set", vec![arg, v], |s, r| {
                    s.keys = Load::Idle;
                    // a new Discord token takes effect at once
                    crate::messaging::sync();
                    let m = if r["ok"] == true { "Key saved".to_string() } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "bl" => { self.bl_focus = Some(arg); self.focus = None; }
            _ => {}
        }
    }

    fn layout_now(&self) -> layout::Layout { layout::bar_layout(&self.lookup(&self.cfgc, "bar.modules"), self.cfgc.bool("showNetSpeed")) }

    /// The palette the picked colour would give, painted over the whole window until the picker closes (nothing is written).
    fn update_preview(&mut self) {
        if self.pick_path.is_some() { return; } // a visualizer color changes no theme
        conf::set_preview(self.picker.as_ref().map(|p| crate::bridge::theme::preview(p.color(), self.info.mode != "light", &self.info.scheme)));
        // the seed field is what Apply reads, and what the row shows
        if let Some(hex) = self.picker.as_ref().map(|p| p.hex()) { self.fields.entry("seed".into()).or_insert_with(|| Field::new(false)).tb.set(&hex); }
    }

    fn close_picker(&mut self) { self.picker = None; self.pick_path = None; conf::set_preview(None); }

    /// Apply: re-theme with the picked colour. The preview stays on until the real theme has landed.
    /// A config color (`pick_path`) is just written.
    fn apply_picked(&mut self, cx: &mut Ctx) {
        if let Some(path) = self.pick_path.take() {
            if let Some(p) = self.picker.take() { self.set(&path, json!(p.hex())); }
            return;
        }
        self.update_preview();
        self.picker = None;
        self.do_named(cx, "seed", String::new());
    }

    fn slider_to(&mut self, x: f32) {
        if let Some((path, sl, r)) = &self.sl { self.drag_val = Some((path.clone(), snap(sl, ui::slider_value(*r, x) as f64))); }
    }

    fn drop_target(&self, x: f32, y: f32, id: &str) -> Option<(String, usize)> {
        let (side, col, items) = self.bl_geo.iter().find(|(_, r, _)| r.contains(x, y))?;
        let _ = col;
        Some((side.clone(), items.iter().filter(|(i, _)| i != id).filter(|(_, r)| y >= r.y + r.h / 2.0).count()))
    }

    fn act_at(&self, id: u32) -> Option<Act> { id.checked_sub(ACT0).and_then(|i| self.acts.get(i as usize)).cloned() }
}

impl Drop for Settings {
    /// Closing Settings with the picker open must not leave the whole desktop in a preview palette.
    fn drop(&mut self) { if self.picker.is_some() { conf::set_preview(None); } }
}

impl View for Settings {
    fn size(&self, _: &Gfx, _: &Ctx, _: f32) -> (f32, f32) { self.area }

    // BG and MENU_BG are full-area spots; NAV_BG (3) is also picker::BG, the colour picker backdrop.
    fn hand(&self, id: u32) -> bool { !matches!(id, 0 | BG | MENU_BG | NAV_BG) }

    fn key(&self, _: &Ctx) -> String {
        // the Usage page redraws every second
        let tick = if self.current_page() == Page::Usage { (hub::now_ms() / 1000.0) as u64 } else { 0 };
        format!("{}{}{tick}{}", self.toast.is_some(), self.busy, self.flash.is_some())
    }

    fn frame_ms(&self) -> u32 {
        if self.rendering { 0 }
        else if self.scroll.moving() { 16 }
        else if self.toast.is_some() || self.flash.is_some() || !self.busy.is_empty() || self.loading() || self.bl.as_ref().is_some_and(|b| b.ghost) { 50 }
        else if self.current_page() == Page::Usage { 1000 }
        else if (self.focus.is_some() && self.active) || self.picker.is_some() { 500 }
        else { 0 }
    }

    /// Any empty spot drags the window (the JS: the rail's empty space or the window's edge).
    fn movable(&self) -> f32 { self.area.1 }

    // always: Esc reaches `input`, which closes through close_settings so the saved page is cleared
    fn captures_esc(&self) -> bool { true }

    fn limits(&self) -> Option<((f32, f32), (f32, f32))> { Some((MIN_SIZE, MAX_SIZE)) }

    fn resize(&mut self, w: f32, h: f32) {
        self.area = clamp_size((w, h));
        self.index.clear(); // search results are laid out for the old width
    }

    fn resize_end(&mut self) { hub::set(SIZE_KEY, Some(format!("{},{}", self.area.0.round(), self.area.1.round()))); }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.sync(cx);
        self.ensure_loaded(cx);
        // a desk card's "Customize in Settings": "<page>|<section title>"
        let goto = hub::get(GOTO).and_then(|g| parse_goto(&g));
        if let Some((page, _, _)) = &goto {
            hub::set(GOTO, None);
            if let Some(i) = registry::index_of_key(page) { self.set_page(i); }
        }
        let (w, h) = (c.w, c.h);
        if self.index.is_empty() && !self.query().is_empty() { self.build_index(c, cx); }
        ui::panel(c, cx, R::new(6.0, 6.0, w - 12.0, h - 12.0), 30.0);

        // navigation rail
        let collapsed = self.nav_collapsed || w < 760.0;
        let nav_w = if collapsed { crate::tokens::NAV_COLLAPSED_W } else { crate::tokens::NAV_W };
        let div_x = if collapsed { crate::tokens::DIVIDER_COLLAPSED_X } else { crate::tokens::DIVIDER_X };
        let page_x = if collapsed { crate::tokens::PAGE_COLLAPSED_X } else { crate::tokens::PAGE_X };

        c.rect(R::new(div_x, 28.0, 1.0, h - 56.0), 0.0, alpha(cx.theme.c("outlineVariant"), 0.5)); // rail | page
        let mut nav = Pen::new(c, cx, crate::tokens::NAV_X, 22.0, nav_w, 0.0, h);
        let list_top = self.nav(&mut nav);
        let mut nav_acts = std::mem::take(&mut nav.acts);
        let (lr, ch) = nav_list(list_top, h);
        let lr = R::new(crate::tokens::NAV_X, lr.y, nav_w, lr.h);
        c.hit(lr, NAV_BG);
        (self.nav_h, self.nav_view) = (ch, lr.h);
        let mut sc = std::mem::take(&mut self.nav_scroll);
        sc.region(c, cx, lr, ch, |c, top| {
            let mut pen = Pen::new(c, cx, crate::tokens::NAV_X, top, nav_w, 0.0, h);
            pen.acts = std::mem::take(&mut nav_acts);
            // rows are placed by nav position; the page index (Act::Page, self.page) stays the PAGES index
            for (row, i) in registry::nav_order().into_iter().enumerate() {
                let y = top + row as f32 * NAV_STEP;
                if y + NAV_ITEM_H < lr.y || y > lr.y + lr.h { continue; } // hit spots ignore the clip
                self.nav_item(&mut pen, R::new(crate::tokens::NAV_X, y, nav_w, NAV_ITEM_H), PAGES[i].icon, PAGES[i].title, Act::Page(i), i == self.page);
            }
            nav_acts = pen.acts;
        });
        self.nav_scroll = sc;
        self.nav_n = nav_acts.len() as u32;

        // page: measure (card heights, content height), then draw what is on screen
        let r = R::new(page_x, crate::tokens::PAGE_Y, w - page_x - 22.0, h - crate::tokens::PAGE_Y - 22.0);
        c.hit(r, BG); // the wheel needs a spot under it; it also keeps a press here from dragging the window
        let (ch, heads, tags) = {
            let mut dry = Pen::new(c, cx, r.x, r.y, r.w - crate::tokens::SCROLL_GUTTER, r.y, r.y + r.h);
            dry.dry = true;
            self.page(&mut dry);
            (dry.y - r.y, dry.heads, dry.tags)
        };
        (self.content_h, self.view_h) = (ch, r.h);
        let mut acts = nav_acts;
        let mut sc = std::mem::take(&mut self.scroll);
        let now = hub::now_ms();
        if let Some((_, s, l)) = goto {
            if let Some(y) = goto_y(&heads, &tags, &s, &l) { sc = Scroll::default(); sc.y = y - r.y - 6.0; }
            self.flash = (!l.is_empty()).then_some((s, l, now + FLASH_MS));
        }
        if self.flash.as_ref().is_some_and(|f| f.2 < now) { self.flash = None; }
        let flash = self.flash.as_ref().map(|(s, l, end)| (s.clone(), l.clone(), ((end - now) / FLASH_MS).clamp(0.0, 1.0) as f32));
        sc.region(c, cx, r, ch, |c, top| {
            let mut pen = Pen::new(c, cx, r.x, top, r.w - crate::tokens::SCROLL_GUTTER, r.y, r.y + r.h);
            pen.flash = flash;
            pen.acts = std::mem::take(&mut acts);
            self.page(&mut pen);
            acts = pen.acts;
        });
        self.scroll = sc;
        self.acts = acts;

        self.paint_menu(c, cx);
        let strip = self.pick_path.as_deref().and_then(pages::picker_strip);
        if let Some(p) = &mut self.picker { picker::paint(c, cx, p, strip.as_deref()); }
        self.paint_ghost(c, cx);
        if let Some((msg, _)) = &self.toast {
            let tw = c.g.measure(msg, txt(13.0, 400)).0.ceil() + 36.0;
            let t = R::new((w - tw) / 2.0, h - 22.0 - 38.0, tw, 38.0);
            c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(t, 19.0, col) });
            c.rect(t, 19.0, cx.theme.c("inverseSurface"));
            c.text(msg, t, txt(13.0, 400), cx.theme.c("inverseOnSurface"), Align::Center);
        }
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        self.sync(cx);
        match ev {
            Input::Hover(_) => {}
            Input::Wheel(_, d) => match &mut self.menu {
                Some(m) => { let (ch, vh) = menu_dims(m, self.area.1); m.scroll.wheel(d, ch, vh); }
                None if matches!(ev, Input::Wheel(id, _) if id == NAV_BG || (ACT0..ACT0 + self.nav_n).contains(&id)) => self.nav_scroll.glide(d, self.nav_h, self.nav_view),
                None => self.scroll.glide(d, self.content_h, self.view_h),
            },
            Input::Click(id, Btn::Left) => {
                if self.picker.is_some() {
                    match id {
                        picker::BG | picker::CANCEL => self.close_picker(),
                        picker::OK => self.apply_picked(cx),
                        i if (picker::PRESET0..picker::PRESET0 + picker::PRESETS.len() as u32).contains(&i) => {
                            if let Some(p) = &mut self.picker { p.set_color(picker::PRESETS[(i - picker::PRESET0) as usize]); }
                            self.update_preview();
                        }
                        _ => {}
                    }
                } else if id == MENU_BG { self.menu = None; }
                else if (MENU0..MENU0 + 980).contains(&id) {
                    if let Some(m) = self.menu.take() {
                        if let Some((v, _)) = m.opts.get((id - MENU0) as usize) {
                            self.set(&m.path, v.clone());
                            self.toast("Saved");
                            // a Parakeet that isn't on this PC yet is downloaded as soon as it is picked
                            let todo = (m.path == "buddies.model").then(|| crate::bridge::listen::MODELS.iter().find(|mo| !mo.installed() && v.as_str() == Some(&*mo.path().to_string_lossy()))).flatten();
                            if let Some(mo) = todo { self.do_named(cx, "download-speech", mo.id.to_string()); }
                        }
                    }
                } else if let Some(a) = self.act_at(id) { self.act(cx, a); }
            }
            Input::Click(..) => {}
            Input::Drag(id, x, y, Phase::Start) if self.picker.is_some() => {
                if let Some(p) = &mut self.picker {
                    if id == picker::HEX { p.tb.press(x, y); }
                    p.start(id, x, y);
                }
                self.update_preview();
            }
            Input::Drag(_, x, y, Phase::Move) if self.picker.is_some() => {
                if let Some(p) = &mut self.picker { p.drag(x, y); }
                self.update_preview();
            }
            Input::Drag(_, _, _, Phase::End) if self.picker.is_some() => { if let Some(p) = &mut self.picker { p.grab = 0; } }
            Input::Drag(id, x, y, Phase::Start) => {
                let a = self.act_at(id);
                if !matches!(&a, Some(Act::Field(k)) if self.focus.as_deref() == Some(k)) { self.blur(cx); }
                if !matches!(&a, Some(Act::Do("bl", _))) { self.bl_focus = None; }
                self.active = true;
                match a {
                    Some(Act::Field(k)) => {
                        self.focus = Some(k.clone());
                        if let Some(f) = self.fields.get_mut(&k) { if f.kind != Kind::Secret { f.tb.press(x, y); } else { f.tb.caret = f.tb.text.len(); } }
                    }
                    Some(Act::Slider(p, sl, r)) => { self.sl = Some((p, sl, r)); self.slider_to(x); }
                    Some(Act::Do("bl", i)) => { self.bl_focus = Some(i.clone()); self.bl = Some(Bl { id: i, sx: x, sy: y, x, y, ghost: false, target: None }); }
                    _ => {}
                }
            }
            Input::Drag(_, x, y, Phase::Move) => {
                self.slider_to(x);
                if let Some(b) = &mut self.bl {
                    (b.x, b.y) = (x, y);
                    if !b.ghost && (x - b.sx).hypot(y - b.sy) >= 4.0 { b.ghost = true; }
                }
                if let Some(id) = self.bl.as_ref().filter(|b| b.ghost).map(|b| b.id.clone()) {
                    let t = self.drop_target(x, y, &id);
                    if let Some(b) = &mut self.bl { b.target = t; }
                }
            }
            Input::Drag(_, _, _, Phase::End) => {
                if let Some((path, sl, _)) = self.sl.take() {
                    // peek: taking drag_val here would leave nothing for the config write below
                    if let (Some((_, v)), Some(i)) = (self.drag_val.clone(), path.strip_prefix("monitor.")) {
                        self.drag_val = None;
                        self.over.insert(path.clone(), (json!(v), 1e18));
                        store::script(cx, &self.inbox, "toggles", ["-Action", "brightness", "-Value", &(v as i64).to_string(), "-Monitor", i].map(String::from).to_vec(), |_, _| {});
                    } else if let Some((_, v)) = self.drag_val.take() {
                        self.set(&path, num_json(v));
                        if path == "nightLight.kelvin" && !self.rendering {
                            let (cfg, k) = (store::bcfg(cx), (v as i64).to_string());
                            std::thread::spawn(move || {
                                if proc::bridge(&cfg, "nightlight", &["status"])["on"] == true { proc::bridge(&cfg, "nightlight", &["on", &k]); }
                            });
                        }
                    }
                    let _ = sl;
                }
                if let Some(b) = self.bl.take() {
                    if let (true, Some((side, idx))) = (b.ghost, b.target) {
                        let l = layout::place_module(&self.layout_now(), &b.id, &side, idx);
                        self.set("bar.modules", layout::to_json(&l));
                    }
                }
            }
            Input::Char(ch) if self.picker.is_some() => {
                if let Some(p) = &mut self.picker { p.tb.ch(ch); p.typed(); }
                self.update_preview();
            }
            Input::Char('/') if slash_focuses_search(self.focus.is_some(), self.menu.is_some()) => {
                self.blur(cx);
                self.bl_focus = None;
                self.active = true;
                self.focus = Some(SEARCH.into());
                if let Some(f) = self.fields.get_mut(SEARCH) { f.tb.caret = f.tb.text.len(); }
            }
            Input::Char(ch) => {
                if let Some(f) = self.focus.as_ref().and_then(|k| self.fields.get_mut(k)) { f.tb.ch(ch); }
            }
            Input::Focus(on) => {
                self.active = on;
                if !on { if let Some(k) = self.focus.clone() { self.commit(cx, &k, false); } }
            }
            Input::Key { vk, ctrl, shift, alt } => {
                if self.picker.is_some() {
                    if vk == 0x1B { self.close_picker(); }
                    else {
                        let e = self.picker.as_mut().map(|p| { let e = p.tb.key(vk, ctrl, shift, alt); p.typed(); e });
                        self.update_preview();
                        if e == Some(Edit::Submit) { self.apply_picked(cx); }
                    }
                } else if ctrl && (vk == 0x21 || vk == 0x22) {
                    let n = PAGES.len();
                    self.set_page((self.page + if vk == 0x22 { 1 } else { n - 1 }) % n);
                    self.menu = None;
                } else if self.menu.is_some() {
                    if vk == 0x1B { self.menu = None; }
                } else if vk == 0x1B {
                    self.close_settings(cx);
                } else if let Some(k) = self.focus.clone() {
                    let e = self.fields.get_mut(&k).map(|f| f.tb.key(vk, ctrl, shift, alt));
                    if e == Some(Edit::Submit) { self.commit(cx, &k, true); }
                } else if let Some(id) = self.bl_focus.clone() {
                    let l = self.layout_now();
                    let next = match vk {
                        0x26 | 0x28 => Some(layout::move_module(&l, &id, if vk == 0x26 { -1 } else { 1 })),
                        0x25 | 0x27 => {
                            let cur = l.iter().find(|m| m.0 == id).and_then(|m| layout::SIDES.iter().position(|s| *s == m.1)).unwrap_or(0) as i32;
                            let j = cur + if vk == 0x25 { -1 } else { 1 };
                            (0..layout::SIDES.len() as i32).contains(&j).then(|| layout::place_module(&l, &id, layout::SIDES[j as usize], usize::MAX))
                        }
                        _ => None,
                    };
                    if let Some(n) = next { self.set("bar.modules", layout::to_json(&n)); }
                }
            }
        }
    }
}

/// (content height, visible height) of an open menu.
fn menu_dims(m: &Menu, win_h: f32) -> (f32, f32) {
    let ch = m.opts.len() as f32 * 36.0 + 8.0;
    (ch, ch.min(320.0).min(win_h - 24.0))
}

impl Settings {
    pub(crate) fn query(&self) -> String { self.fields.get(SEARCH).map(|f| f.tb.text.trim().to_lowercase()).unwrap_or_default() }

    /// Lays every page out once without drawing, to learn its labels.
    fn build_index(&mut self, c: &mut Canvas, cx: &Ctx) {
        let keep = self.page;
        for i in 0..PAGES.len() {
            self.page = i;
            let mut dry = Pen::new(c, cx, 0.0, 0.0, 600.0, 0.0, 0.0);
            dry.dry = true;
            self.body(&mut dry);
            self.index.extend(dry.index.into_iter().map(|(s, l, sub)| (i, s, l, sub)));
        }
        self.page = keep;
    }

    /// The page area while a search is typed: matching controls and sections.
    pub(crate) fn results(&mut self, p: &mut Pen) {
        let words: Vec<String> = self.query().split_whitespace().map(String::from).collect();
        let hits: Vec<_> = self.index.iter().filter(|(i, s, l, sub)| {
            let cat = pages::category_of_page(*i);
            let hay = format!("{} {} {s} {l} {sub}", cat, PAGES[*i].title).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        }).take(40).cloned().collect();
        p.h2("search", "Search results", &format!("{} matches", hits.len()));
        p.card(|p| {
            if hits.is_empty() {
                p.note("Nothing matches your search.", 13.0);
                p.note("Try keywords like wallpaper, keybinds, island, bar, theme, or voice.", 12.0);
                return;
            }
            for (i, sect, label, sub) in hits {
                let Some(r) = p.row(44.0) else { continue };
                let cat = pages::category_of_page(i);
                let sub = if sub.is_empty() { format!("{cat} · {}", PAGES[i].title) } else { format!("{cat} · {} > {sub}", PAGES[i].title) };
                let want = if label == sect { String::new() } else { label.clone() }; // a section title has no control to flash
                let id = p.act(Act::Goto(i, sect, want));
                if p.c.hit(r, id) { p.c.rect(r, 12.0, alpha(p.col("onSurface"), 0.08)); }
                p.left(r, "", &label, &sub, 0.0);
            }
        });
    }

    /// Draws the rail's header, search box and Close button; returns where the page list starts.
    fn nav(&mut self, p: &mut Pen) -> f32 {
        let (x, y0, w) = (p.x, p.y, p.w);
        let name = match p.cx.cfg.str("userName") { "" => "You", n => n }.to_string();
        if w < 60.0 {
            // Collapsed navigation rail
            let av = R::new(x + (w - 38.0) / 2.0, y0 + 6.0, 38.0, 38.0);
            p.c.circle(av.x + 19.0, av.y + 19.0, 19.0, p.col("primaryContainer"));
            let file = self.gs(p.cx, "desktop.greet.file");
            if !(avatar_is_picture(&file) && self.picture(p, &conf::pack_dir().join("images").join(&file), av, 19.0)) {
                p.c.text(&avatar_initial(&name), av, txt(16.0, 600), p.col("onPrimaryContainer"), Align::Center);
            }
            let id = p.act(Act::Do("pick-image", "greet".into()));
            p.c.hit(av, id);
            
            let cid = p.act(Act::Do("toggle-nav", String::new()));
            ui::icon_btn(p.c, p.cx, R::new(x + (w - 36.0) / 2.0, y0 + 50.0, 36.0, 36.0), cid, "menu", ui::IconKind::Plain, 20.0, false);
            
            let bid = p.act(Act::Do("editcfg", String::new()));
            ui::icon_btn(p.c, p.cx, R::new(x + (w - 36.0) / 2.0, y0 + 92.0, 36.0, 36.0), bid, "edit", ui::IconKind::Tonal, 20.0, false);
            
            self.nav_item(p, R::new(x, p.c.h - 22.0 - NAV_ITEM_H, w, NAV_ITEM_H), "close", "Close", Act::Do("close", String::new()), false);
            return y0 + 138.0;
        }

        // Expanded navigation rail
        // .me
        let av = R::new(x + 6.0, y0 + 6.0, 42.0, 42.0);
        p.c.circle(av.x + 21.0, av.y + 21.0, 21.0, p.col("primaryContainer"));
        let file = self.gs(p.cx, "desktop.greet.file"); // the Avatar picture (Desktop > Greeting card)
        // a picture that does not decode leaves the circle empty, so it falls back to the initial like no picture at all
        if !(avatar_is_picture(&file) && self.picture(p, &conf::pack_dir().join("images").join(&file), av, 21.0)) {
            p.c.text(&avatar_initial(&name), av, txt(18.0, 600), p.col("onPrimaryContainer"), Align::Center);
        }
        let id = p.act(Act::Do("pick-image", "greet".into()));
        p.c.hit(av, id); // click the avatar to choose a picture
        let top = y0 + 6.0 + (42.0 - 14.0 * ui::LH - 11.5 * ui::LH) / 2.0;
        p.c.text(&name, R::new(x + 58.0, top, w - 96.0, 14.0 * ui::LH), txt(14.0, 500), p.col("onSurface"), Align::Left);
        p.c.text("Windows", R::new(x + 58.0, top + 14.0 * ui::LH, w - 96.0, 11.5 * ui::LH), txt(11.5, 400), p.col("onSurfaceVariant"), Align::Left);
        
        let cid = p.act(Act::Do("toggle-nav", String::new()));
        ui::icon_btn(p.c, p.cx, R::new(x + w - 34.0, y0 + 10.0, 32.0, 32.0), cid, "menu_open", ui::IconKind::Plain, 18.0, false);

        let line = y0 + 62.0;
        p.c.rect(R::new(x, line, w, 1.0), 0.0, alpha(p.col("outlineVariant"), 0.5));
        let mut y = line + 1.0 + 8.0 + 4.0;
        // .cfgbtn
        let b = R::new(x, y, w, 48.0);
        p.c.rect(b, 23.0, p.col("secondaryContainer"));
        let id = p.act(Act::Do("editcfg", String::new()));
        if p.c.hit(b, id) { p.c.rect(b, 23.0, alpha(p.col("onSecondaryContainer"), 0.08)); }
        p.c.icon("edit", R::new(x + 16.0, y, 20.0, 48.0), ico(20.0, false), p.col("onSecondaryContainer"));
        p.c.text("Config file", R::new(x + 46.0, y, w - 60.0, 48.0), txt(14.0, 400), p.col("onSecondaryContainer"), Align::Left);
        y += 48.0 + 18.0; // air between the Config file button and the search box
        let sb = R::new(x, y, w, 40.0);
        self.field_ui(p, SEARCH, None, Kind::Plain, sb, "Search settings (/)", 20.0, false);
        y += 40.0 + 14.0;
        self.nav_item(p, R::new(x, p.c.h - 22.0 - NAV_ITEM_H, w, NAV_ITEM_H), "close", "Close", Act::Do("close", String::new()), false);
        y
    }

    fn nav_item(&mut self, p: &mut Pen, b: R, ic: &str, label: &str, act: Act, sel: bool) {
        use crate::tokens::NAV_R;
        if sel { p.c.rect(b, NAV_R, p.col("secondaryContainer")); }
        let id = p.act(act);
        if p.c.hit(b, id) && !sel { p.c.rect(b, NAV_R, alpha(p.col("onSurface"), crate::tokens::HOVER_A)); }
        let fg = p.col(if sel { "onSecondaryContainer" } else { "onSurfaceVariant" });
        let icon_col = if sel { ui::accent(p.cx) } else { fg }; // the open page's icon takes the accent
        if b.w < 60.0 {
            p.c.icon(ic, R::new(b.x + (b.w - 20.0) / 2.0, b.y, 20.0, b.h), ico(20.0, false), icon_col);
        } else {
            p.c.icon(ic, R::new(b.x + 16.0, b.y, 20.0, b.h), ico(20.0, false), icon_col);
            p.c.text(label, R::new(b.x + 50.0, b.y, b.w - 60.0, b.h), txt(13.5, 400), fg, Align::Left);
        }
    }

    /// The open `<select>` list over everything.
    fn paint_menu(&mut self, c: &mut Canvas, cx: &Ctx) {
        let (w, h) = (c.w, c.h);
        let cur = self.menu.as_ref().map(|m| self.lookup(&cx.cfg, &m.path));
        let Some(m) = &mut self.menu else { return };
        c.hit(R::new(0.0, 0.0, w, h), MENU_BG);
        let (ch, vh) = menu_dims(m, h);
        let width = m.anchor.w.max(180.0);
        let x = m.anchor.x.min(w - width - 8.0);
        let below = m.anchor.y + m.anchor.h + 4.0;
        let y = if below + vh > h - 8.0 { (m.anchor.y - 4.0 - vh).max(8.0) } else { below };
        let box_ = R::new(x, y, width, vh);
        c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(box_, 17.0, col) });
        c.rect(box_, 17.0, alpha(cx.theme.c("outlineVariant"), 0.45));
        c.rect(R::new(x + 1.0, y + 1.0, width - 2.0, vh - 2.0), 16.0, cx.theme.c("surfaceContainerHigh"));
        let inner = R::new(x + 4.0, y + 4.0, width - 8.0, vh - 8.0);
        let opts = m.opts.clone();
        m.scroll.region(c, cx, inner, ch - 8.0, |c, top| {
            for (i, (v, label)) in opts.iter().enumerate().take(980) {
                let r = R::new(inner.x, top + i as f32 * 36.0, inner.w, 36.0);
                if r.y + r.h < inner.y || r.y > inner.y + inner.h { continue; }
                let sel = cur.as_ref() == Some(v) || (v == &json!("") && cur.as_ref().is_some_and(Value::is_null));
                if sel { c.rect(r, 12.0, cx.theme.c("secondaryContainer")); }
                if c.hit(r, MENU0 + i as u32) && !sel { c.rect(r, 12.0, alpha(cx.theme.c("onSurface"), 0.08)); }
                c.text(label, R::new(r.x + 12.0, r.y, r.w - 24.0, r.h), txt(14.0, 400), cx.theme.c(if sel { "onSecondaryContainer" } else { "onSurface" }), Align::Left);
            }
        });
    }

    /// The bar part that follows the pointer while dragging.
    fn paint_ghost(&mut self, c: &mut Canvas, cx: &Ctx) {
        let Some(b) = self.bl.as_ref().filter(|b| b.ghost) else { return };
        let (label, ic) = layout::info(&b.id);
        let r = R::new(b.x - 20.0, b.y - 18.0, 170.0, 34.0);
        c.shadow(8.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(r, 12.0, col) });
        c.rect(r, 12.0, alpha(cx.theme.c("secondaryContainer"), 0.92));
        let fg = cx.theme.c("onSecondaryContainer");
        c.icon("drag_indicator", R::new(r.x + 4.0, r.y, 16.0, 34.0), ico(16.0, false), fg);
        c.icon(ic, R::new(r.x + 26.0, r.y, 18.0, 34.0), ico(18.0, false), fg);
        c.text(label, R::new(r.x + 50.0, r.y, r.w - 56.0, 34.0), txt(13.0, 400), fg, Align::Left);
    }
}

/// What zorui-svc reads to decide whether to start Zor UI and GlazeWM at login (%ProgramData%\zor-ui\startup.json).
fn startup_flag_json(on: bool) -> String { json!({ "run_on_startup": on }).to_string() }

/// install.ps1 lets Users write this one file (svc.json stays admin-only); it holds only the on/off flag.
fn write_startup_flag(on: bool) -> std::io::Result<()> {
    let dir = PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("zor-ui");
    std::fs::write(dir.join("startup.json"), startup_flag_json(on))
}

/// Opens `dir` in File Explorer, making it first so there is something to open.
fn open_folder(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_claude_accounts_show_both_row_states() {
        let d = super::demo_claude();
        let rows = d["accounts"].as_array().unwrap();
        assert_eq!(rows.iter().filter(|r| r["active"] == true).count(), 1, "exactly one active login");
        assert_eq!(rows[0]["n"], 0, "the default row comes first, with n 0");
        assert_eq!(rows[0]["default"], true);
        assert_eq!(rows[1]["default"], false);
        assert_eq!(crate::bridge::claude_acct::summary(&rows[0]), "Max · 5h 38% · 7d 12% · just now");
        assert_eq!(crate::bridge::claude_acct::summary(&rows[1]), "Pro · usage unavailable");
    }

    #[test]
    fn remove_is_armed_only_for_the_same_account_within_the_window() {
        let arm = Some(("2".to_string(), 5000.0));
        assert!(super::armed(&arm, "2", 4999.0));
        assert!(!super::armed(&arm, "2", 5000.0), "the window has closed at its end time");
        assert!(!super::armed(&arm, "3", 1000.0), "another account's icon does not arm this one");
        assert!(!super::armed(&None, "2", 1000.0));
    }

    #[test]
    fn startup_flag_file_holds_the_switch() {
        assert_eq!(super::startup_flag_json(false), r#"{"run_on_startup":false}"#);
        assert_eq!(super::startup_flag_json(true), r#"{"run_on_startup":true}"#);
    }

    #[test]
    fn a_failed_theme_step_is_reported() {
        assert_eq!(super::theme_failed(&serde_json::json!({ "ok": false, "error": "no image" })).as_deref(), Some("no image"));
        assert_eq!(super::theme_failed(&serde_json::json!({ "ok": false })).as_deref(), Some("it gave no reason"));
        assert_eq!(super::theme_failed(&serde_json::json!({ "ok": true, "mode": "dark" })), None);
        assert_eq!(super::theme_failed(&serde_json::Value::Null), None);
    }

    #[test]
    fn saved_size_is_parsed_and_clamped() {
        assert_eq!(super::parse_size("1000,700"), Some((1000.0, 700.0)));
        assert_eq!(super::parse_size(" 1000 , 700 "), Some((1000.0, 700.0)));
        assert_eq!(super::parse_size("10,10"), Some(super::MIN_SIZE));
        assert_eq!(super::parse_size("99999,99999"), Some(super::MAX_SIZE));
        assert_eq!(super::parse_size("wide"), None);
        assert_eq!(super::parse_size("1000,x"), None);
    }

    #[test]
    fn saved_page_falls_back_to_quick() {
        let last = PAGES.len() - 1;
        assert_eq!(super::page_from_key(Some(PAGES[last].key)), last);
        assert_eq!(super::page_from_key(Some("nope")), 0);
        assert_eq!(super::page_from_key(None), 0);
        assert_eq!(PAGES[0].key, "quick");
    }

    #[test]
    fn esc_and_close_clear_the_saved_page_and_close() {
        for vk in [Some(0x1B), None] {
            let mut st = Settings::new((1200.0, 800.0));
            let mut cx = Ctx::new(false);
            match vk {
                Some(vk) => st.input(Input::Key { vk, ctrl: false, shift: false, alt: false }, &mut cx),
                None => st.do_named(&mut cx, "close", String::new()),
            }
            assert!(cx.actions.iter().any(|a| matches!(a, Action::Close(n) if n == "settings")));
            assert_eq!(hub::get(PAGE_KEY), None);
        }
    }

    #[test]
    fn every_settings_widget_is_ported() {
        for (id, _) in DESKTOP_WIDGETS.iter() { assert!(crate::desk::PORTED.contains(id), "{id}"); }
    }

    use super::*;

    fn s() -> Settings { Settings::new((1200.0, 800.0)) }

    #[test]
    fn hand_skips_background_spots() {
        let st = s();
        assert!(!st.hand(0) && !st.hand(BG) && !st.hand(MENU_BG) && !st.hand(NAV_BG), "background spots keep the arrow");
        assert!(st.hand(NAV_BG + 1), "a control gets the hand");
    }

    #[test]
    fn contacts_are_added_replaced_and_deleted() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        let add = |st: &mut Settings, cx: &mut Ctx, name: &str, app: &str, value: &str| {
            for (k, v) in [("contact-name", name), ("contact-value", value)] { st.fields.entry(k.into()).or_insert_with(|| Field::new(false)).tb.set(v); }
            st.do_named(cx, "contact-app", app.into());
            st.do_named(cx, "add-contact", String::new());
        };
        add(&mut st, &mut cx, "Ali", "whatsapp", "+92 300");
        add(&mut st, &mut cx, "Sam", "discord", "123");
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Ali=whatsapp:+92 300", "Sam=discord:123"]);
        assert_eq!(st.fields["contact-name"].tb.text, "", "the form clears after adding");
        add(&mut st, &mut cx, "ali", "discord", "9");
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Sam=discord:123", "ali=discord:9"], "same name replaces");
        add(&mut st, &mut cx, "", "discord", "9");
        assert_eq!(st.garr(&cx, "buddies.contacts").len(), 2, "an empty name adds nothing");
        st.do_named(&mut cx, "del-contact", "Sam=discord:123".into());
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["ali=discord:9"]);
    }

    fn put_contact(st: &mut Settings, cx: &mut Ctx, name: &str, app: &str, value: &str) {
        for (k, v) in [("contact-name", name), ("contact-value", value)] { st.fields.entry(k.into()).or_insert_with(|| Field::new(false)).tb.set(v); }
        st.do_named(cx, "contact-app", app.into());
        st.do_named(cx, "add-contact", String::new());
    }

    fn two_contacts() -> (Settings, Ctx) {
        let (mut st, mut cx) = (s(), Ctx::new(false));
        put_contact(&mut st, &mut cx, "Ali", "whatsapp", "+92 300");
        put_contact(&mut st, &mut cx, "Sam", "discord", "123");
        (st, cx)
    }

    fn field(st: &Settings, k: &str) -> String { st.fields.get(k).map(|f| f.tb.text.clone()).unwrap_or_default() }

    #[test]
    fn the_pencil_loads_a_contact_into_the_form() {
        let (mut st, mut cx) = two_contacts();
        st.do_named(&mut cx, "edit-contact", "Sam=discord:123".into());
        assert_eq!((field(&st, "contact-name"), field(&st, "contact-app"), field(&st, "contact-value")), ("Sam".into(), "discord".into(), "123".into()));
        assert_eq!(field(&st, "contact-edit"), "Sam=discord:123", "it remembers which entry it is changing");
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Ali=whatsapp:+92 300", "Sam=discord:123"], "loading changes nothing yet");
        st.do_named(&mut cx, "edit-contact", "not a contact".into());
        assert_eq!(field(&st, "contact-name"), "Sam", "an entry that doesn't parse is ignored");
    }

    #[test]
    fn saving_an_edit_changes_the_contact_where_it_stands_and_clears_the_form() {
        let (mut st, mut cx) = two_contacts();
        st.do_named(&mut cx, "edit-contact", "Ali=whatsapp:+92 300".into());
        st.fields.get_mut("contact-name").unwrap().tb.set("Alice");
        st.do_named(&mut cx, "contact-app", "whatsappweb".into());
        st.do_named(&mut cx, "add-contact", String::new());
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Alice=whatsappweb:+92 300", "Sam=discord:123"], "same place, new name and app");
        assert_eq!((field(&st, "contact-name"), field(&st, "contact-value"), field(&st, "contact-edit")), (String::new(), String::new(), String::new()));
        // the next save is an ordinary add, not another edit of Ali
        put_contact(&mut st, &mut cx, "Zed", "discord", "9");
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Alice=whatsappweb:+92 300", "Sam=discord:123", "Zed=discord:9"]);
    }

    #[test]
    fn renaming_a_contact_onto_another_replaces_that_other_one() {
        let (mut st, mut cx) = two_contacts();
        st.do_named(&mut cx, "edit-contact", "Sam=discord:123".into());
        st.fields.get_mut("contact-name").unwrap().tb.set("ali");
        st.do_named(&mut cx, "add-contact", String::new());
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["ali=discord:123"]);
    }

    #[test]
    fn cancel_leaves_the_list_alone_and_empties_the_form() {
        let (mut st, mut cx) = two_contacts();
        st.do_named(&mut cx, "edit-contact", "Sam=discord:123".into());
        st.fields.get_mut("contact-name").unwrap().tb.set("Changed");
        st.do_named(&mut cx, "cancel-edit", String::new());
        assert_eq!((field(&st, "contact-name"), field(&st, "contact-value"), field(&st, "contact-edit")), (String::new(), String::new(), String::new()));
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Ali=whatsapp:+92 300", "Sam=discord:123"]);
    }

    #[test]
    fn deleting_the_contact_being_edited_ends_the_edit() {
        let (mut st, mut cx) = two_contacts();
        st.do_named(&mut cx, "edit-contact", "Sam=discord:123".into());
        st.do_named(&mut cx, "del-contact", "Sam=discord:123".into());
        assert_eq!(field(&st, "contact-edit"), "", "Save can't bring it back");
        assert_eq!(field(&st, "contact-name"), "");
        st.do_named(&mut cx, "add-contact", String::new());
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Ali=whatsapp:+92 300"], "nothing came back");
        // deleting a different one leaves the edit going
        st.do_named(&mut cx, "edit-contact", "Ali=whatsapp:+92 300".into());
        put_contact(&mut st, &mut cx, "", "discord", "");
        st.do_named(&mut cx, "del-contact", "Other=discord:1".into());
        assert_eq!(field(&st, "contact-edit"), "Ali=whatsapp:+92 300");
    }

    #[test]
    fn an_edit_with_an_empty_name_changes_nothing_and_keeps_editing() {
        let (mut st, mut cx) = two_contacts();
        st.do_named(&mut cx, "edit-contact", "Ali=whatsapp:+92 300".into());
        st.fields.get_mut("contact-name").unwrap().tb.set("");
        st.do_named(&mut cx, "add-contact", String::new());
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Ali=whatsapp:+92 300", "Sam=discord:123"]);
        assert_eq!(field(&st, "contact-edit"), "Ali=whatsapp:+92 300", "still editing, so the user can fix it");
    }

    #[test]
    fn contacts_have_their_own_page_right_after_buddies() {
        let keys: Vec<&str> = PAGES.iter().map(|p| p.key).collect();
        let b = keys.iter().position(|k| *k == "buddies").unwrap();
        assert_eq!(keys[b + 1], "contacts");
        assert_eq!(PAGES[b + 1].title, "Contacts");
        assert_eq!(super::page_from_key(Some("contacts")), b + 1);
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "every page key is unique");
    }

    #[test]
    fn a_whatsapp_web_contact_is_saved_under_its_own_app() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        for (k, v) in [("contact-name", "Mum"), ("contact-value", "+44 7700 900123")] { st.fields.entry(k.into()).or_insert_with(|| Field::new(false)).tb.set(v); }
        st.do_named(&mut cx, "contact-app", "whatsappweb".into());
        st.do_named(&mut cx, "add-contact", String::new());
        assert_eq!(st.garr(&cx, "buddies.contacts"), ["Mum=whatsappweb:+44 7700 900123"]);
        // what the buddy reads back is that app and number, not a plain WhatsApp contact
        let c = crate::assist::parse_contact(&st.garr(&cx, "buddies.contacts")[0]).unwrap();
        assert_eq!(c, ("Mum".to_string(), "whatsappweb".to_string(), "+44 7700 900123".to_string()));
    }

    #[test]
    fn nav_list_scrolls_only_in_a_short_window() {
        let (r, ch) = nav_list(209.0, 480.0);
        assert!(ch > r.h && r.h > 0.0, "13 pages do not fit in the smallest window: {ch} in {}", r.h);
        assert!(r.y + r.h <= 480.0 - 22.0 - NAV_ITEM_H, "the list ends above Close");
        let (r, ch) = nav_list(209.0, 1200.0);
        assert!(ch <= r.h, "a tall window shows every page");
        assert_eq!(nav_list(209.0, 100.0).0.h, 0.0, "never a negative height");
    }

    #[test]
    fn the_nav_is_as_tall_as_its_pages_and_scrolls_when_short() {
        // The list scrolls rather than clipping: every page's row fits in the scroll height, at any window height.
        for h in [870.0, 1200.0] {
            let (r, ch) = nav_list(209.0, h);
            assert_eq!(ch, PAGES.len() as f32 * NAV_STEP - (NAV_STEP - NAV_ITEM_H), "scroll height at window {h}");
            assert!(r.h > 0.0, "the list has room at window {h}");
        }
    }

    #[test]
    fn avatar_initial_is_the_first_letter_capitalised() {
        assert_eq!(avatar_initial("anas"), "A");
        assert_eq!(avatar_initial(""), "");
    }

    #[test]
    fn avatar_needs_a_chosen_file_that_exists() {
        assert!(!avatar_is_picture(""));
        assert!(!avatar_is_picture("  "));
        assert!(!avatar_is_picture("definitely-not-here-0123.png"));
    }

    #[test]
    fn config_values_fall_back_like_core_js() {
        let st = s();
        let cx = Ctx::new(false);
        assert_eq!(st.get(&cx, "desktop.images.image2.size"), json!(280));
        assert_eq!(st.get(&cx, "desktop.images.image1.shape"), json!("cookie"));
        assert_eq!(st.get(&cx, "desktop.greet.date"), json!(true));
        assert_eq!(st.gs(&cx, "desktop.greet.file"), "");
        assert_eq!(st.gn(&cx, "island.textWidth"), 170.0);
        assert!(st.gb(&cx, "island.panel.media"));
        assert_eq!(st.garr(&cx, "bar.utils"), ["snip", "pick", "wall", "mode"]);
    }

    #[test]
    fn written_values_show_at_once_until_the_file_catches_up() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.over.insert("island.textWidth".into(), (json!(200), hub::now_ms()));
        assert_eq!(st.gn(&cx, "island.textWidth"), 200.0);
        cx.cfg = Config::parse(r#"{"island": {"textWidth": 200}}"#);
        st.sync(&cx);
        assert!(st.over.is_empty()); // config.json has it now
        assert_eq!(st.gn(&cx, "island.textWidth"), 200.0);
    }

    #[test]
    fn a_picked_visualizer_color_is_written_without_retheming() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.do_named(&mut cx, "pickcolor", "island.eqColor".into());
        assert!(st.picker.is_some() && st.pick_path.as_deref() == Some("island.eqColor"));
        st.picker.as_mut().unwrap().set_color(0xFF12_3456);
        st.update_preview();
        assert!(conf::set_preview(None).is_none()); // no theme preview was painted
        st.apply_picked(&mut cx);
        assert_eq!(st.over["island.eqColor"].0, json!("#123456"));
        assert!(st.picker.is_none() && st.pick_path.is_none() && st.busy.is_empty());
        st.do_named(&mut cx, "clearcolor", "island.eqColor".into());
        assert_eq!(st.over["island.eqColor"].0, json!(""));
    }

    /// What the preview paints: an override, else the shared color, else the theme's; edits not yet in config.json count.
    #[test]
    fn visualizer_preview_colors_follow_pending_edits() {
        let mut st = s();
        let cx = Ctx::new(false);
        let theme = cx.theme.c("primary");
        assert_eq!(st.viz_color(&cx, "island.eqColor"), theme);
        st.over.insert("visualizer.color".into(), (json!("#112233"), hub::now_ms()));
        assert_eq!(st.viz_color(&cx, "island.eqColor"), 0xFF11_2233);
        st.over.insert("island.eqColor".into(), (json!("#445566"), hub::now_ms()));
        assert_eq!((st.viz_color(&cx, "island.eqColor"), st.viz_color(&cx, "visualizer.barColor")), (0xFF44_5566, 0xFF11_2233));
        st.over.insert("island.eqColor".into(), (json!("nope"), hub::now_ms()));
        assert_eq!(st.viz_color(&cx, "island.eqColor"), 0xFF11_2233);
    }

    #[test]
    fn the_preview_follows_the_picker_while_picking_and_reverts_on_cancel() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        let theme = cx.theme.c("primary");
        st.do_named(&mut cx, "pickcolor", "island.eqColor".into());
        st.picker.as_mut().unwrap().set_color(0xFF12_3456);
        assert_eq!((st.viz_color(&cx, "island.eqColor"), st.viz_color(&cx, "visualizer.barColor")), (0xFF12_3456, theme)); // only the picked setting
        st.close_picker();
        assert_eq!(st.viz_color(&cx, "island.eqColor"), theme);
    }

    #[test]
    fn only_visualizer_colors_get_a_picker_strip() {
        for p in ["island.eqColor", "visualizer.barColor", "visualizer.desktopColor", "visualizer.color"] { assert!(pages::picker_strip(p).is_some(), "{p}"); }
        assert!(pages::picker_strip("buddy.color").is_none());
    }

    #[test]
    fn cancelling_the_picker_forgets_its_config_path() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.do_named(&mut cx, "pickcolor", "visualizer.color".into());
        st.close_picker();
        assert!(st.picker.is_none() && st.pick_path.is_none() && !st.over.contains_key("visualizer.color"));
    }

    #[test]
    fn number_fields_clamp_and_zero_means_minimum() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        for (typed, want) in [("12", 12), ("99", 30), ("0", 1), ("abc", 1), (" 7x", 7)] {
            let mut f = Field::new(false);
            f.kind = Kind::Num(1, 30);
            f.tb.set(typed);
            f.orig = "x".into();
            st.fields.insert("island.notifySeconds".into(), f);
            st.commit(&mut cx, "island.notifySeconds", true);
            assert_eq!(st.over["island.notifySeconds"].0, json!(want), "{typed}");
        }
    }

    #[test]
    fn list_fields_split_on_commas() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        let mut f = Field::new(false);
        f.kind = Kind::Text { list: true };
        f.tb.set("一, 二 ,, 三");
        f.orig = String::new();
        st.fields.insert("workspaceLabels".into(), f);
        st.commit(&mut cx, "workspaceLabels", true);
        assert_eq!(st.over["workspaceLabels"].0, json!(["一", "二", "三"]));
        assert!(st.toast.is_some());
    }

    #[test]
    fn finishing_a_slider_drag_keeps_the_value() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        let sl = Sl { min: 0.0, max: 20.0, step: 1.0, fmt: form::Fmt::Px };
        st.sl = Some(("desktop.glass.haze".into(), sl, R::new(0.0, 0.0, 100.0, 10.0)));
        st.drag_val = Some(("desktop.glass.haze".into(), 12.0));
        st.input(Input::Drag(0, 0.0, 0.0, Phase::End), &mut cx);
        assert_eq!(st.over["desktop.glass.haze"].0, json!(12));
        assert!(st.drag_val.is_none() && st.sl.is_none());
    }

    #[test]
    fn drop_target_counts_items_above_the_pointer() {
        let mut st = s();
        let col = R::new(0.0, 0.0, 100.0, 300.0);
        st.bl_geo = vec![("left".into(), col, vec![("a".into(), R::new(0.0, 10.0, 100.0, 34.0)), ("b".into(), R::new(0.0, 50.0, 100.0, 34.0)), ("c".into(), R::new(0.0, 90.0, 100.0, 34.0))])];
        assert_eq!(st.drop_target(5.0, 5.0, "x"), Some(("left".into(), 0)));
        assert_eq!(st.drop_target(5.0, 70.0, "x"), Some(("left".into(), 2)));
        assert_eq!(st.drop_target(5.0, 70.0, "a"), Some(("left".into(), 1))); // the dragged part doesn't count
        assert_eq!(st.drop_target(500.0, 5.0, "x"), None);
        
    }

    #[test]
    fn slash_only_focuses_search_when_nothing_is_being_typed() {
        assert!(slash_focuses_search(false, false));
        assert!(!slash_focuses_search(true, false));
        assert!(!slash_focuses_search(false, true));
    }
    #[test]
    fn slash_key_focuses_search_without_typing_it() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.fields.insert(SEARCH.into(), Field::new(false));
        st.input(Input::Char('/'), &mut cx);
        assert_eq!(st.focus.as_deref(), Some(SEARCH));
        assert_eq!(st.fields[SEARCH].tb.text, "");
        st.input(Input::Char('/'), &mut cx); // now typing: a slash is text
        assert_eq!(st.fields[SEARCH].tb.text, "/");
    }
    #[test]
    fn goto_value_parses_with_or_without_a_label() {
        assert_eq!(parse_goto("bar|Look|Corner | radius"), Some(("bar".into(), "Look".into(), "Corner | radius".into())));
        assert_eq!(parse_goto("bar|Look"), Some(("bar".into(), "Look".into(), String::new())));
        assert_eq!(parse_goto("bar"), None);
    }
    #[test]
    fn goto_lands_on_the_control_then_the_heading() {
        let heads = vec![("Look".to_string(), 100.0)];
        let tags = vec![("Look".to_string(), "Radius".to_string(), 340.0), ("Other".to_string(), "Radius".to_string(), 900.0)];
        assert_eq!(goto_y(&heads, &tags, "Look", "Radius"), Some(340.0));
        assert_eq!(goto_y(&heads, &tags, "Look", ""), Some(100.0));
        assert_eq!(goto_y(&heads, &tags, "Look", "Missing"), Some(100.0));
        assert_eq!(goto_y(&heads, &tags, "Nope", "Radius"), None);
    }
    #[test]
    fn search_query_is_trimmed_and_lowercase() {
        let mut st = s();
        assert_eq!(st.query(), "");
        let mut f = Field::new(false);
        f.tb.set("  Dock ");
        st.fields.insert(SEARCH.into(), f);
        assert_eq!(st.query(), "dock");
    }

    #[test]
    fn preview_names_are_file_safe() {
        assert!(preview_path("a/b c").ends_with("a_b_c.png"));
    }

    #[test]
    fn custom_agents_are_added_validated_and_removed() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        let fill = |st: &mut Settings, name: &str, dir: &str, file: &str| for (k, v) in [("agent-name", name), ("agent-dir", dir), ("agent-file", file)] {
            let mut f = Field::new(false);
            f.tb.set(v);
            st.fields.insert(k.into(), f);
        };
        fill(&mut st, "My Agent", ".zorui-test-no-such-folder", "hooks.json");
        st.do_named(&mut cx, "add-agent", String::new());
        assert_eq!(st.get(&cx, "customAgents"), json!([{ "name": "My Agent", "dir": ".zorui-test-no-such-folder", "file": "hooks.json" }]));
        assert!(st.fields["agent-name"].tb.text.is_empty()); // the form clears for the next one
        fill(&mut st, "my agent", ".x", "a.json");
        st.do_named(&mut cx, "add-agent", String::new());
        assert_eq!(st.get(&cx, "customAgents").as_array().unwrap().len(), 1); // same name, any case: refused
        fill(&mut st, "Other", "", "a.json");
        st.do_named(&mut cx, "add-agent", String::new());
        assert_eq!(st.get(&cx, "customAgents").as_array().unwrap().len(), 1); // a field left empty: refused
        st.do_named(&mut cx, "del-agent", "My Agent".into());
        assert_eq!(st.get(&cx, "customAgents"), json!([]));
    }

    #[test]
    fn voice_has_its_own_page_after_the_creator() {
        let keys: Vec<&str> = PAGES.iter().map(|p| p.key).collect();
        let c = keys.iter().position(|k| *k == "creator").unwrap();
        assert_eq!((keys[c + 1], PAGES[c + 1].title), ("voice", "Voice"));
        assert_eq!(super::page_from_key(Some("voice")), c + 1);
    }

    #[test]
    fn the_creator_has_its_own_page_after_contacts_and_sensible_defaults() {
        let keys: Vec<&str> = PAGES.iter().map(|p| p.key).collect();
        let c = keys.iter().position(|k| *k == "contacts").unwrap();
        assert_eq!((keys[c + 1], PAGES[c + 1].title), ("creator", "Create a buddy"));
        assert_eq!(super::page_from_key(Some("creator")), c + 1);
        let mut st = s();
        let cx = Ctx::new(false);
        assert_eq!((st.gn(&cx, "buddies.make.height"), st.gb(&cx, "buddies.make.trim"), st.gb(&cx, "buddies.make.flip"), st.gs(&cx, "buddies.make.special")), (64.0, true, false, "aura".to_string()));
        assert_eq!(st.creator_opts(&cx), crate::creator::Opts { clean: 0, trim: true, flip: false });
        st.set("buddies.make.clean", json!(500));
        st.set("buddies.make.flip", json!(true));
        assert_eq!(st.creator_opts(&cx), crate::creator::Opts { clean: 80, trim: true, flip: true }, "the tolerance is held to what the control offers");
    }

    #[test]
    fn saving_a_created_buddy_checks_everything_then_writes_a_pack_and_clears_the_draft() {
        let base = std::env::temp_dir().join(format!("zorui-make-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, preview, images) = (base.join("buddies"), base.join("preview"), base.join("images"));
        for d in [&preview, &images] { std::fs::create_dir_all(d).unwrap(); }
        let (mut st, cx) = (s(), Ctx::new(false));
        let said = |st: &Settings| st.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
        st.make_buddy(&cx, &root, &preview, &images);
        assert_eq!(said(&st), "Choose a picture first");
        st.set("buddies.make.image", json!("buddy-make-1.png"));
        st.make_buddy(&cx, &root, &preview, &images);
        assert_eq!(said(&st), "Give the buddy a name first");
        st.set("buddies.make.name", json!("  "));
        st.make_buddy(&cx, &root, &preview, &images);
        assert_eq!(said(&st), "Give the buddy a name first", "a blank name is no name");
        st.set("buddies.make.name", json!("Son Goku"));
        st.make_buddy(&cx, &root, &preview, &images);
        assert!(said(&st).contains("isn't ready"), "no preview has been made yet");
        let ready = |st: &mut Settings, opts: &crate::creator::Opts| {
            let pk = crate::pack::parse("creator", &preview, &crate::creator::manifest("Preview", 64, "", "aura"), 1).unwrap();
            st.creator_prev = Some(crate::creator::Preview { key: opts.key("buddy-make-1.png"), pack: Some(pk), error: String::new(), stamp: 1 });
        };
        let opts = st.creator_opts(&cx);
        ready(&mut st, &opts);
        st.make_buddy(&cx, &root, &preview, &images);
        assert!(said(&st).contains("missing"), "the preview says it is ready but its file is gone: {}", said(&st));
        std::fs::write(preview.join("stand.png"), b"PNGDATA").unwrap();
        std::fs::write(images.join("buddy-make-1.png"), b"source").unwrap();
        std::fs::write(images.join("wallpaper.jpg"), b"not mine").unwrap();
        st.set("buddies.make.clean", json!(20)); // an option moved since the preview was made
        st.make_buddy(&cx, &root, &preview, &images);
        assert!(said(&st).contains("isn't ready") && !root.exists(), "a stale preview saves nothing");
        st.set("buddies.make.clean", json!(0));
        st.set("buddies.make.height", json!(90));
        st.set("buddies.make.base", json!("yuji"));
        st.set("buddies.make.special", json!("beam"));
        st.make_buddy(&cx, &root, &preview, &images);
        assert_eq!(said(&st), "Saved Son Goku. Pick it under Buddies > Character");
        assert_eq!(std::fs::read(root.join("son-goku").join("stand.png")).unwrap(), b"PNGDATA");
        let p = crate::pack::read(&root, "son-goku").expect("the bar finds it");
        assert_eq!((p.name.as_str(), p.height, p.base.as_str(), p.moves.get("special").map(String::as_str)), ("Son Goku", 90.0, "yuji", Some("beam")));
        assert!(!images.join("buddy-make-1.png").exists() && images.join("wallpaper.jpg").exists(), "only the creator's own picture is deleted");
        assert_eq!((st.gs(&cx, "buddies.make.image"), st.gs(&cx, "buddies.make.name")), (String::new(), String::new()), "the draft is cleared");
        // the same name again makes a second buddy, never overwriting the first
        st.set("buddies.make.image", json!("buddy-make-1.png"));
        st.set("buddies.make.name", json!("Son Goku"));
        let opts = st.creator_opts(&cx);
        ready(&mut st, &opts);
        std::fs::write(preview.join("stand.png"), b"SECOND").unwrap();
        st.make_buddy(&cx, &root, &preview, &images);
        assert_eq!(std::fs::read(root.join("son-goku").join("stand.png")).unwrap(), b"PNGDATA");
        assert_eq!(std::fs::read(root.join("son-goku-2").join("stand.png")).unwrap(), b"SECOND");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn new_buddy_saves_from_either_slot() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.do_named(&mut cx, "new-buddy", "b".into());
        assert_eq!(st.get(&cx, "buddies.custom").as_array().map(Vec::len), Some(1));
        assert!(crate::tokens::SCROLL_GUTTER >= 12.0);
    }

    #[test]
    fn new_buddy_keeps_its_picture_and_extras_then_frees_the_slot() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.set("buddies.a.image", json!("buddy-a-5.png"));
        st.set("buddies.a.glasses", json!("yes"));
        st.set("buddies.a.name", json!("Rin"));
        st.do_named(&mut cx, "new-buddy", "a".into());
        let e = st.get(&cx, "buddies.custom")[0].clone();
        assert_eq!((e["image"].as_str(), e["glasses"].as_str()), (Some("buddy-a-5.png"), Some("yes")));
        assert_eq!((e["name"].as_str(), st.gs(&cx, "buddies.a.name").as_str()), (Some("Rin"), ""), "the name moves to the saved buddy");
        assert_eq!(st.gs(&cx, "buddies.a.image"), "", "the slot's own picture is cleared, the saved buddy has it");
        st.set("buddies.a.image", json!("x.png"));
        st.do_named(&mut cx, "clear-buddy-image", "a".into());
        assert_eq!(st.gs(&cx, "buddies.a.image"), "");
    }

    #[test]
    fn leaving_the_buddy_name_field_renames_the_buddy() {
        let mut st = s();
        let mut cx = Ctx::new(false);
        st.set("buddies.a.character", json!("c1"));
        st.set("buddies.custom", json!([{ "id": "c1", "name": "Old" }]));
        let mut f = Field::new(false);
        f.tb.set("New");
        st.fields.insert("bname-a".into(), f);
        st.focus = Some("bname-a".into());
        st.blur(&mut cx);
        assert_eq!(st.get(&cx, "buddies.custom"), json!([{ "id": "c1", "name": "New" }]));
    }

    #[test]
    fn a_recorded_key_is_saved_refused_or_dropped() {
        let (mut st, cx) = (s(), Ctx::new(false));
        let keys = |st: &Settings| st.keys_cfg(&cx).strs("keybinds.files");
        st.rec = Some(("files".into(), 0));
        st.rec_done(&cx, Some("Alt+Y".into()));
        assert_eq!(keys(&st), ["Alt+Y"], "shows at once, before config.json catches up");
        assert!(st.rec.is_none());
        st.rec = Some(("files".into(), 0));
        st.rec_done(&cx, Some("Alt+I".into()));
        assert_eq!(keys(&st), ["Alt+Y"], "Alt+I belongs to Settings");
        assert!(st.toast.as_ref().is_some_and(|t| t.0.contains("Settings")));
        st.rec = Some(("files".into(), 0));
        st.rec_done(&cx, None);
        assert_eq!(keys(&st), ["Alt+Y"], "Esc changes nothing");
        st.rec_done(&cx, Some("Alt+U".into()));
        assert_eq!(keys(&st), ["Alt+Y"], "no row was waiting");
    }

    #[test]
    fn a_reset_key_goes_back_to_its_default() {
        let (mut st, mut cx) = (s(), Ctx::new(false));
        st.set("keybinds.files", json!(["Alt+Y"]));
        st.do_named(&mut cx, "key-reset", "files".into());
        let cfg = st.keys_cfg(&cx);
        let b = crate::keymap::BINDS.iter().find(|b| b.id == "files").unwrap();
        assert_eq!(crate::keymap::keys_of(&cfg, b), ["Alt+E"]);
    }

    #[test]
    fn navigation_collapses_and_restores() {
        let (mut st, mut cx) = (s(), Ctx::new(false));
        assert!(!st.nav_collapsed);
        st.do_named(&mut cx, "toggle-nav", String::new());
        assert!(st.nav_collapsed);
        st.do_named(&mut cx, "toggle-nav", String::new());
        assert!(!st.nav_collapsed);
    }

    #[test]
    fn category_switching_preserves_scroll_position() {
        let mut st = s();
        st.page = 0;
        st.scroll.y = 120.0;
        st.set_page(4); // Switch to Bar
        assert_eq!(st.scroll.y, 0.0);
        st.scroll.y = 250.0;
        st.set_page(0); // Switch back to Quick
        assert_eq!(st.scroll.y, 120.0, "should restore page 0 scroll position");
        st.set_page(4); // Switch back to Bar
        assert_eq!(st.scroll.y, 250.0, "should restore page 4 scroll position");
    }
}

#[cfg(test)]
mod voice_actions_tests;
