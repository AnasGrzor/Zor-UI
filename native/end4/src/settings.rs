//! Settings (settings.js + settings.html): a navigation rail and the pages of cards, in a normal window that
//! can be moved by any empty spot. Values are written with the bridge's config-set (like the JS), and shown
//! at once from `over` until config.json reloads. Re-theming actions (wallpaper, mode, scheme, presets,
//! Wallpaper Engine) run the same bridge / script commands as the JS.
//!
//! `END4_RENDER_PAGE=<quick|keys|wallpapers|presets|bar|island|services|about>` opens that page and, with
//! `--render settings`, skips all background loading; `END4_RENDER_DEMO=1` adds sample presets / wallpapers /
//! key states so those pages can be drawn offline.

mod form;
mod layout;
mod pages;
pub(crate) mod store;

use crate::{
    app::{Action, Ctx},
    conf::{self, Config},
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    hub, proc,
    ui::{self, Edit, Scroll, TextBox},
    win::{Btn, Input, Phase, View},
};
use form::{num_json, snap, Act, Pen, Sl, ACT0};
use pages::{DESKTOP_WIDGETS, PAGES};
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf, rc::Rc};
use store::Inbox;

const BG: u32 = 1;
const MENU_BG: u32 = 2;
const MENU0: u32 = 10;
const TOAST_MS: f64 = 3200.0;

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
struct Info { mode: String, scheme: String, seed: String, live: bool, stamp: Option<std::time::SystemTime> }

/// hub key a desk card sets before opening Settings: "<page key>|<section title>".
pub const GOTO: &str = "end4.settingsGoto";
const SEARCH: &str = "search";

pub struct Settings {
    area: (f32, f32),
    page: usize,
    scroll: Scroll,
    content_h: f32,
    view_h: f32,
    inbox: Inbox,
    /// A copy of the config for background callbacks (they have no `Ctx`).
    cfgc: Config,
    native: [bool; 3],
    /// Values written but not yet seen in config.json: path -> (value, written at ms).
    over: HashMap<String, (Value, f64)>,
    acts: Vec<Act>,
    fields: HashMap<String, Field>,
    focus: Option<String>,
    /// The window has keyboard focus (else no caret).
    active: bool,
    menu: Option<Menu>,
    /// A slider being dragged: path, spec, track rect.
    sl: Option<(String, Sl, R)>,
    drag_val: Option<(String, f64)>,
    toast: Option<(String, f64)>,
    busy: String,
    info: Info,
    online: Load<Value>,
    local: Load<Vec<Value>>,
    we: Load<Value>,
    we_list: Load<Vec<Value>>,
    keys: Load<Value>,
    mons: Load<Vec<String>>,
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
        p if p.starts_with("desktop.greet.") => json!(""),
        p if p.ends_with(".shape") && p.starts_with("desktop.images.") => json!("cookie"),
        p if p.ends_with(".size") && p.starts_with("desktop.images.") => json!(280),
        p if p.ends_with(".file") && p.starts_with("desktop.images.") => json!(""),
        _ => Value::Null,
    }
}

impl Settings {
    /// `area`: the CSS-px size the popup placement gives the view (popup::area).
    pub fn new(area: (f32, f32)) -> Settings {
        let env = std::env::var("END4_RENDER_PAGE").ok();
        let page = env.as_deref().and_then(|e| PAGES.iter().position(|p| p.0 == e)).unwrap_or(0);
        let mut s = Settings {
            area, page, scroll: Scroll::default(), content_h: 0.0, view_h: 0.0, inbox: Inbox::default(), cfgc: Config::parse(""), native: [false; 3],
            over: HashMap::new(), acts: vec![], fields: HashMap::new(), focus: None, active: true, menu: None, sl: None, drag_val: None, toast: None,
            busy: String::new(), info: Info::default(), online: Load::Idle, local: Load::Idle, we: Load::Idle, we_list: Load::Idle, keys: Load::Idle, mons: Load::Idle,
            models: Load::Idle, we_type: "all".into(), imgs: HashMap::new(), bl: None, bl_focus: None, bl_geo: vec![], rendering: env.is_some(), index: vec![],
        };
        if std::env::var("END4_RENDER_DEMO").is_ok() { s.demo(); }
        s
    }

    /// Opens on the page `key` (Mod+/ opens "keys").
    pub fn on_page(mut self, key: &str) -> Settings {
        if let Some(i) = PAGES.iter().position(|p| p.0 == key) { self.page = i; }
        self
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
        self.models = Load::Ready(vec![("gemini-2.5-flash".into(), "Gemini 2.5 Flash".into()), ("gpt-4o".into(), "GPT-4o".into())]);
    }

    // ------------------------------------------------------------ config values

    fn lookup(&self, cfg: &Config, path: &str) -> Value {
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
        self.over.insert(path.into(), (v.clone(), hub::now_ms()));
        store::config_set(self.cfgc_bridge(), path, &v, &self.inbox);
    }
    fn cfgc_bridge(&self) -> Config {
        Config::parse(&json!({ "pythonPath": self.cfgc.str("pythonPath"), "bridgePath": self.cfgc.str("bridgePath"), "scriptsDir": self.cfgc.str("scriptsDir") }).to_string())
    }

    pub fn toast(&mut self, msg: &str) { self.toast = Some((msg.to_string(), hub::now_ms() + TOAST_MS)); }

    fn sync(&mut self, cx: &Ctx) {
        self.cfgc = Config(cx.cfg.0.clone());
        self.native = [cx.native("desktop"), cx.native("visualizer"), cx.native("dock")];
        let now = hub::now_ms();
        self.over.retain(|k, (v, t)| now - *t < 8000.0 && cx.cfg.get(k) != v);
        if self.toast.as_ref().is_some_and(|t| t.1 < now) { self.toast = None; }
        for f in self.inbox.take() { f(self); }
        let path = conf::pack_dir().join("theme.json");
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if stamp != self.info.stamp || self.info.mode.is_empty() {
            let v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
            let s = |k: &str, d: &str| v[k].as_str().filter(|s| !s.is_empty()).unwrap_or(d).to_string();
            self.info = Info { mode: s("mode", "dark"), scheme: s("scheme", "auto"), seed: s("seed", "#6750a4"), live: v["live"].as_str().is_some_and(|l| !l.is_empty()) || v["live"] == true, stamp };
        }
    }

    // ------------------------------------------------------------ background loading (the `once` calls in render())

    fn ensure_loaded(&mut self, cx: &Ctx) {
        if self.rendering { return; }
        match PAGES[self.page].0 {
            "presets" => {
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
            "wallpapers" => {
                if matches!(self.we, Load::Idle) {
                    self.we = Load::Loading;
                    store::bridge(cx, &self.inbox, "we-status", vec![], |s, r| s.we = Load::Ready(if r["error"].is_string() || r.is_null() { json!({ "installed": false }) } else { r }));
                }
                if matches!(self.we_list, Load::Idle) && matches!(&self.we, Load::Ready(w) if w["installed"] == true) {
                    self.we_list = Load::Loading;
                    store::bridge(cx, &self.inbox, "we-list", vec![], |s, r| s.we_list = Load::Ready(r["items"].as_array().cloned().unwrap_or_default()));
                }
            }
            "quick" => if matches!(self.mons, Load::Idle) {
                self.mons = Load::Loading;
                store::script(cx, &self.inbox, "toggles", vec!["-Action".into(), "monitors".into()], |s, r| {
                    let list = r["monitors"].as_array().cloned().unwrap_or_default();
                    // pinned in `over` (far-future stamp) so the sliders show the hardware value
                    for m in &list { s.over.insert(format!("monitor.{}", m["i"]), (m["brightness"].clone(), 1e18)); }
                    s.mons = Load::Ready(list.iter().map(|m| format!("{}|{}", m["i"], m["name"].as_str().unwrap_or(""))).collect());
                });
            },
            "services" => {
                if matches!(self.keys, Load::Idle) {
                    self.keys = Load::Loading;
                    store::bridge(cx, &self.inbox, "secret-status", vec![], |s, r| s.keys = Load::Ready(if r["error"].is_string() || r.is_null() { json!({}) } else { r }));
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
        matches!(PAGES[self.page].0, "presets" | "wallpapers" | "services")
            && [matches!(self.online, Load::Loading), matches!(self.local, Load::Loading), matches!(self.we, Load::Loading), matches!(self.we_list, Load::Loading),
                matches!(self.keys, Load::Loading), matches!(self.models, Load::Loading)].contains(&true)
    }

    // ------------------------------------------------------------ text fields

    fn blur(&mut self, cx: &mut Ctx) {
        if let Some(k) = self.focus.take() { self.commit(cx, &k, false); }
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
                self.toast("Saved");
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
                k if k.starts_with("bname-") => { let slot = &k[6..]; let id = self.gs(cx, &format!("buddies.{slot}.character")); self.act(cx, Act::Do("rename-buddy", format!("{slot}:{id}"))) }
                k if k.starts_with("key-") => self.act(cx, Act::Do("save-key", k[4..].to_string())),
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
        if !self.native[0] { proc::zebar_preset(&format!("desk-{id}")); }
    }

    fn act(&mut self, cx: &mut Ctx, a: Act) {
        match a {
            Act::Page(i) => { self.page = i; self.menu = None; self.bl_focus = None; }
            Act::Switch(path) => {
                let v = !self.gb(cx, &path);
                self.set(&path, json!(v));
                if v && !self.rendering {
                    match path.as_str() {
                        "visualizer.desktop" if !self.native[1] => proc::zebar_preset("visualizer"),
                        "desktopWidgets" if !self.native[0] => proc::zebar_preset("desktop"),
                        "dock.enabled" if !self.native[2] => proc::zebar_preset("dock"), // turning it off: it closes itself
                        _ => {}
                    }
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
            Act::Goto(i, sect) => {
                if let Some(f) = self.fields.get_mut(SEARCH) { f.tb.set(""); }
                self.focus = None;
                hub::set(GOTO, Some(format!("{}|{sect}", PAGES[i].0)));
                self.page = i;
            }
            Act::Slider(..) | Act::Field(..) => {}
            Act::Do(name, arg) => self.do_named(cx, name, arg),
        }
    }

    fn do_named(&mut self, cx: &mut Ctx, name: &'static str, arg: String) {
        let inbox = self.inbox.clone();
        match name {
            "wall" => self.retheme(cx, &[], "wall"),
            "mode" => self.retheme(cx, &["-Mode", &arg], "mode"),
            "scheme" => self.retheme(cx, &["-Scheme", &arg], "scheme"),
            "seed" => {
                let hex = self.fields.get("seed").map(|f| f.tb.text.trim().to_string()).unwrap_or_default();
                if conf::parse_hex(&hex).is_some_and(|_| hex.len() == 7) { self.retheme(cx, &["-Color", &hex], "seed"); }
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
            "url" => store::open_url(&arg),
            "close" => cx.actions.push(Action::Close("settings".into())),
            "bu" => {
                let cur = self.garr(cx, "bar.utils");
                let next: Vec<&str> = ["snip", "pick", "wall", "mode"].into_iter().filter(|v| cur.iter().any(|c| c == v) != (*v == arg)).collect();
                self.set("bar.utils", json!(next));
            }
            "pick-image" => {
                let slot = arg.clone();
                store::bridge(cx, &inbox, "pick-image", vec![arg], move |s, r| {
                    if let Some(f) = r["file"].as_str() {
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
                if !was && !self.native[0] { proc::zebar_preset(&format!("desk-{arg}")); } // turned-off cards close themselves
            }
            "new-buddy" => {
                let key = |f: &str| format!("buddies.{arg}.{f}");
                let cur = self.gs(cx, &key("character"));
                let look: Vec<(&str, String)> = ["hair", "shirt", "pants", "eyes", "bun", "style"].iter().map(|f| (*f, self.gs(cx, &key(f)))).collect();
                let mut list = self.get(cx, "buddies.custom").as_array().cloned().unwrap_or_default();
                let id = format!("custom{}", hub::now_ms() as i64);
                let e = crate::cast::save(&list, &cur, &look.iter().map(|(f, v)| (*f, v.as_str())).collect::<Vec<_>>(), &self.gs(cx, &key("name")), &id);
                let name = e["name"].as_str().unwrap_or("").to_string();
                list.push(e);
                self.set("buddies.custom", json!(list));
                // the slot now shows the new buddy, whose look already has what its own fields held
                for (f, _) in &look { self.set(&key(f), json!("")); }
                self.set(&key("character"), json!(id));
                self.toast(&format!("Created {name}"));
            }
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
                // native cards keep their spots in state.json; Zebar's are in the webview's localStorage (not reachable from here)
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
                    let m = if r["ok"] == true { format!("Applied {}", arg.replace('_', " ")) } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "apply-local" => {
                self.busy = arg.clone();
                store::bridge(cx, &inbox, "preset-apply-local", vec![arg], |s, r| {
                    s.busy.clear();
                    let m = if r["ok"] == true { "Preset applied".to_string() } else { r["error"].as_str().unwrap_or("Failed").to_string() };
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
            "clear-key" => store::bridge(cx, &inbox, "secret-set", vec![arg], |s, _| s.keys = Load::Idle),
            "save-key" => {
                let Some(f) = self.fields.get_mut(&format!("key-{arg}")) else { return };
                let v = f.tb.text.trim().to_string();
                if v.is_empty() { return; }
                f.tb.set("");
                store::bridge(cx, &inbox, "secret-set", vec![arg, v], |s, r| {
                    s.keys = Load::Idle;
                    let m = if r["ok"] == true { "Key saved".to_string() } else { r["error"].as_str().unwrap_or("Failed").to_string() };
                    s.toast(&m);
                });
            }
            "bl" => { self.bl_focus = Some(arg); self.focus = None; }
            _ => {}
        }
    }

    fn layout_now(&self) -> layout::Layout { layout::bar_layout(&self.lookup(&self.cfgc, "bar.modules"), self.cfgc.bool("showNetSpeed")) }

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

impl View for Settings {
    fn size(&self, _: &Gfx, _: &Ctx, _: f32) -> (f32, f32) { self.area }

    fn key(&self, _: &Ctx) -> String { format!("{}{}", self.toast.is_some(), self.busy) }

    fn frame_ms(&self) -> u32 {
        if self.rendering { 0 }
        else if self.scroll.moving() { 16 }
        else if self.toast.is_some() || !self.busy.is_empty() || self.loading() || self.bl.as_ref().is_some_and(|b| b.ghost) { 50 }
        else if self.focus.is_some() && self.active { 500 }
        else { 0 }
    }

    /// Any empty spot drags the window (the JS: the rail's empty space or the window's edge).
    fn movable(&self) -> f32 { self.area.1 }

    fn captures_esc(&self) -> bool { self.menu.is_some() }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.sync(cx);
        self.ensure_loaded(cx);
        // a desk card's "Customize in Settings": "<page>|<section title>"
        let goto = hub::get(GOTO).and_then(|g| g.split_once('|').map(|(p, s)| (p.to_string(), s.to_string())));
        if let Some((page, _)) = &goto {
            hub::set(GOTO, None);
            if let Some(i) = PAGES.iter().position(|p| p.0 == page) { self.page = i; }
        }
        let (w, h) = (c.w, c.h);
        if self.index.is_empty() && !self.query().is_empty() { self.build_index(c, cx); }
        ui::panel(c, cx, R::new(6.0, 6.0, w - 12.0, h - 12.0), 30.0);

        // navigation rail
        let mut nav = Pen::new(c, cx, 22.0, 22.0, 210.0, 0.0, h);
        self.nav(&mut nav);
        let nav_acts = std::mem::take(&mut nav.acts);

        // page: measure (card heights, content height), then draw what is on screen
        let r = R::new(246.0, 22.0, w - 268.0, h - 44.0);
        c.hit(r, BG); // the wheel needs a spot under it; it also keeps a press here from dragging the window
        let (cards, ch, heads) = {
            let mut dry = Pen::new(c, cx, r.x, r.y, r.w - 6.0, r.y, r.y + r.h);
            dry.dry = true;
            self.page(&mut dry);
            (std::mem::take(&mut dry.measured), dry.y - r.y, dry.heads)
        };
        (self.content_h, self.view_h) = (ch, r.h);
        let mut acts = nav_acts;
        let mut sc = std::mem::take(&mut self.scroll);
        if let Some((_, y)) = goto.and_then(|(_, s)| heads.into_iter().find(|h| h.0 == s)) { sc = Scroll::default(); sc.y = y - r.y - 6.0; }
        sc.region(c, cx, r, ch, |c, top| {
            let mut pen = Pen::new(c, cx, r.x, top, r.w - 6.0, r.y, r.y + r.h);
            pen.cards = cards;
            pen.acts = std::mem::take(&mut acts);
            self.page(&mut pen);
            acts = pen.acts;
        });
        self.scroll = sc;
        self.acts = acts;

        self.paint_menu(c, cx);
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
                None => self.scroll.glide(d, self.content_h, self.view_h),
            },
            Input::Click(id, Btn::Left) => {
                if id == MENU_BG { self.menu = None; }
                else if (MENU0..MENU0 + 980).contains(&id) {
                    if let Some(m) = self.menu.take() {
                        if let Some((v, _)) = m.opts.get((id - MENU0) as usize) { self.set(&m.path, v.clone()); self.toast("Saved"); }
                    }
                } else if let Some(a) = self.act_at(id) { self.act(cx, a); }
            }
            Input::Click(..) => {}
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
                    if let (Some((_, v)), Some(i)) = (self.drag_val.take(), path.strip_prefix("monitor.")) {
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
            Input::Char(ch) => {
                if let Some(f) = self.focus.as_ref().and_then(|k| self.fields.get_mut(k)) { f.tb.ch(ch); }
            }
            Input::Focus(on) => {
                self.active = on;
                if !on { if let Some(k) = self.focus.clone() { self.commit(cx, &k, false); } }
            }
            Input::Key { vk, ctrl, shift, alt } => {
                if ctrl && (vk == 0x21 || vk == 0x22) {
                    let n = PAGES.len();
                    self.page = (self.page + if vk == 0x22 { 1 } else { n - 1 }) % n;
                    self.menu = None;
                } else if self.menu.is_some() {
                    if vk == 0x1B { self.menu = None; }
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
            let hay = format!("{} {s} {l} {sub}", PAGES[*i].2).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        }).take(40).cloned().collect();
        p.h2("search", "Search results", "");
        p.card(|p| {
            if hits.is_empty() { return p.note("Nothing matches.", 13.0); }
            for (i, sect, label, sub) in hits {
                let Some(r) = p.row(44.0) else { continue };
                let sub = if sub.is_empty() { PAGES[i].2.to_string() } else { format!("{} > {sub}", PAGES[i].2) };
                let id = p.act(Act::Goto(i, sect));
                if p.c.hit(r, id) { p.c.rect(r, 12.0, alpha(p.col("onSurface"), 0.08)); }
                p.left(r, "", &label, &sub, 0.0);
            }
        });
    }

    fn nav(&mut self, p: &mut Pen) {
        let (x, y0, w) = (p.x, p.y, p.w);
        let name = match p.cx.cfg.str("userName") { "" => "You", n => n }.to_string();
        // .me
        p.c.circle(x + 6.0 + 21.0, y0 + 6.0 + 21.0, 21.0, p.col("primaryContainer"));
        let init: String = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        p.c.text(&init, R::new(x + 6.0, y0 + 6.0, 42.0, 42.0), txt(18.0, 600), p.col("onPrimaryContainer"), Align::Center);
        let top = y0 + 6.0 + (42.0 - 14.0 * ui::LH - 11.5 * ui::LH) / 2.0;
        p.c.text(&name, R::new(x + 58.0, top, w - 64.0, 14.0 * ui::LH), txt(14.0, 500), p.col("onSurface"), Align::Left);
        p.c.text("Windows", R::new(x + 58.0, top + 14.0 * ui::LH, w - 64.0, 11.5 * ui::LH), txt(11.5, 400), p.col("onSurfaceVariant"), Align::Left);
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
        y += 48.0 + 10.0 + 4.0;
        let sb = R::new(x, y, w, 40.0);
        self.field_ui(p, SEARCH, None, Kind::Plain, sb, "Search settings", 20.0, false);
        y += 40.0 + 10.0;
        for (i, (_, ic, label)) in PAGES.iter().enumerate() {
            self.nav_item(p, R::new(x, y, w, 46.0), ic, label, Act::Page(i), i == self.page);
            y += 50.0;
        }
        self.nav_item(p, R::new(x, p.c.h - 22.0 - 46.0, w, 46.0), "close", "Close", Act::Do("close", String::new()), false);
    }

    fn nav_item(&mut self, p: &mut Pen, b: R, ic: &str, label: &str, act: Act, sel: bool) {
        if sel { p.c.rect(b, 23.0, p.col("secondaryContainer")); }
        let id = p.act(act);
        if p.c.hit(b, id) && !sel { p.c.rect(b, 23.0, alpha(p.col("onSurface"), 0.08)); }
        let fg = p.col(if sel { "onSecondaryContainer" } else { "onSurfaceVariant" });
        p.c.icon(ic, R::new(b.x + 16.0, b.y, 20.0, b.h), ico(20.0, false), fg);
        p.c.text(label, R::new(b.x + 50.0, b.y, b.w - 60.0, b.h), txt(13.5, 400), fg, Align::Left);
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

#[cfg(test)]
mod tests {
    #[test]
    fn every_settings_widget_is_ported() {
        for (id, _) in DESKTOP_WIDGETS.iter() { assert!(crate::desk::PORTED.contains(id), "{id}"); }
    }

    use super::*;

    fn s() -> Settings { Settings::new((1200.0, 800.0)) }

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
}
