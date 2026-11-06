//! The pages of settings.js, in the same order with the same labels and config keys.

use super::{
    form::{chip_w, Act, Fmt, Pen, Sl, ACT0},
    layout::{bar_layout, info, SIDES},
    Load, Settings,
};
use crate::{
    conf,
    gfx::{alpha, ico, txt, Align, R},
    ui::{self, IconKind, LH},
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::OnceLock};

/// World clock cities: (time zone, name).
pub const CITIES: [(&str, &str); 20] = [
    ("Europe/London", "London"), ("Europe/Paris", "Paris"), ("Europe/Berlin", "Berlin"), ("Europe/Istanbul", "Istanbul"), ("Africa/Cairo", "Cairo"),
    ("Asia/Dubai", "Dubai"), ("Asia/Karachi", "Karachi"), ("Asia/Kolkata", "Delhi"), ("Asia/Dhaka", "Dhaka"), ("Asia/Bangkok", "Bangkok"),
    ("Asia/Singapore", "Singapore"), ("Asia/Shanghai", "Shanghai"), ("Asia/Tokyo", "Tokyo"), ("Asia/Seoul", "Seoul"), ("Australia/Sydney", "Sydney"),
    ("Pacific/Auckland", "Auckland"), ("America/New_York", "New York"), ("America/Chicago", "Chicago"), ("America/Los_Angeles", "Los Angeles"), ("America/Sao_Paulo", "Sao Paulo"),
];

/// Buddy colors: (hex, name). Empty keeps the character's own.
const COLORS: [(&str, &str); 13] = [
    ("", "Character's own"), ("#16161D", "Black"), ("#F5F5F5", "White"), ("#B0B7C3", "Silver"), ("#D6303A", "Red"), ("#F48FB1", "Pink"), ("#F28C28", "Orange"),
    ("#F2D13A", "Yellow"), ("#4CAF50", "Green"), ("#26A69A", "Teal"), ("#4A7BE0", "Blue"), ("#8E5BD0", "Purple"), ("#6B4423", "Brown"),
];

const CLOCK_FORMATS: [&str; 7] = ["EEE, dd/MM '•' HH:mm", "HH:mm", "h:mm a", "EEE d MMM, HH:mm", "EEE d MMM, h:mm a", "dd/MM/yyyy HH:mm", "EEEE, d MMMM"];

static FONTS: OnceLock<Vec<String>> = OnceLock::new();

pub const PAGES: [(&str, &str, &str); 10] = [
    ("quick", "tune", "Quick"), ("keys", "keyboard", "Keybinds"), ("wallpapers", "animated_images", "Wallpapers"), ("presets", "palette", "Presets"),
    ("bar", "toolbar", "Bar & desktop"), ("island", "pill", "Dynamic Island"), ("buddies", "pets", "Buddies"), ("services", "hub", "Services"), ("usage", "memory", "Usage"), ("about", "info", "About"),
];
const SCHEMES: [(&str, &str, &str); 10] = [
    ("auto", "auto_awesome", "Auto"), ("content", "image", "Content"), ("expressive", "palette", "Expressive"),
    ("fidelity", "tonality", "Fidelity"), ("fruit-salad", "nutrition", "Fruit Salad"), ("monochrome", "contrast", "Monochrome"),
    ("neutral", "circle", "Neutral"), ("rainbow", "looks", "Rainbow"), ("tonal-spot", "radio_button_checked", "Tonal Spot"),
    ("vibrant", "flare", "Vibrant"),
];
const LEAGUES: [(&str, &str); 14] = [("esp.1", "LaLiga"), ("eng.1", "Premier League"), ("ita.1", "Serie A"), ("fra.1", "Ligue 1"), ("por.1", "Primeira Liga"),
    ("tur.1", "Süper Lig"), ("ned.1", "Eredivisie"), ("ger.1", "Bundesliga"), ("mex.1", "Liga MX"), ("arg.1", "Liga Profesional"),
    ("bra.1", "Brasileirão"), ("ksa.1", "Saudi Pro League"), ("uefa.champions", "Champions League"), ("uefa.europa", "Europa League")];
pub const DESKTOP_WIDGETS: [(&str, &str); 18] = [("clock", "Clock"), ("weather", "Weather"), ("week", "Week"), ("greet", "Greeting"), ("media", "Media"),
    ("lyrics", "Lyrics"), ("cover", "Now playing"), ("image1", "Image 1"), ("image2", "Image 2"), ("image3", "Image 3"), ("sticker", "Sticker"),
    ("calendar", "Calendar"), ("todo", "To do"), ("notes", "Notes"), ("timer", "Timer"), ("worldclock", "World clock"),
    ("resources", "Resources"), ("text", "Custom text")];
const IMAGE_SHAPES: [(&str, &str); 7] = [("cookie", "Cookie"), ("flower", "Flower"), ("clover", "Clover"), ("squircle", "Squircle"), ("circle", "Circle"), ("rounded", "Rounded"), ("square", "Square")];
const KEY_PROVIDERS: [(&str, &str); 6] = [("gemini", "Google Gemini"), ("anthropic", "Anthropic (Claude)"), ("openai", "OpenAI"), ("mistral", "Mistral"),
    ("openrouter", "OpenRouter"), ("wallhaven", "Wallhaven (for NSFW search)")];
const ISLAND_ACTIONS: [(&str, &str); 6] = [("panel", "Expand the island"), ("sidebar-left", "Left sidebar"), ("sidebar-right", "Right sidebar"),
    ("overview", "Overview"), ("play", "Play / pause"), ("none", "Nothing")];
/// LANGS in lib/tab-translate.js, without "auto".
const LANGS: [(&str, &str); 49] = [("af", "Afrikaans"), ("ar", "Arabic"), ("bn", "Bengali"), ("bg", "Bulgarian"), ("ca", "Catalan"), ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"), ("hr", "Croatian"), ("cs", "Czech"), ("da", "Danish"), ("nl", "Dutch"), ("en", "English"), ("et", "Estonian"), ("fil", "Filipino"),
    ("fi", "Finnish"), ("fr", "French"), ("de", "German"), ("el", "Greek"), ("gu", "Gujarati"), ("he", "Hebrew"), ("hi", "Hindi"), ("hu", "Hungarian"), ("id", "Indonesian"),
    ("it", "Italian"), ("ja", "Japanese"), ("ko", "Korean"), ("lv", "Latvian"), ("lt", "Lithuanian"), ("ms", "Malay"), ("mr", "Marathi"), ("no", "Norwegian"), ("fa", "Persian"),
    ("pl", "Polish"), ("pt", "Portuguese"), ("pa", "Punjabi"), ("ro", "Romanian"), ("ru", "Russian"), ("sr", "Serbian"), ("sk", "Slovak"), ("es", "Spanish"), ("sw", "Swahili"),
    ("sv", "Swedish"), ("ta", "Tamil"), ("te", "Telugu"), ("th", "Thai"), ("tr", "Turkish"), ("uk", "Ukrainian"), ("ur", "Urdu"), ("vi", "Vietnamese")];

fn page_blurb(key: &str) -> &'static str {
    match key {
        "quick" => "Wallpaper, colors and the switches you reach for most",
        "keys" => "Every keyboard shortcut",
        "wallpapers" => "Pick a picture or a live wallpaper",
        "presets" => "Save and swap whole looks",
        "bar" => "The bar, the desktop cards and how they are laid out",
        "island" => "What the island shows and how it reacts",
        "buddies" => "The characters that live on your desktop",
        "services" => "API keys, models and sign-ins",
        "usage" => "What the AI tools have used",
        _ => "Version and updates",
    }
}

fn opts(list: &[(&str, &str)]) -> Vec<(Value, String)> { list.iter().map(|(v, l)| (json!(v), l.to_string())).collect() }
fn px(min: f64, max: f64, step: f64) -> Sl { Sl { min, max, step, fmt: Fmt::Px } }

/// One tile of the wallpaper / preset galleries.
pub struct Tile { pub img: Option<PathBuf>, pub badge: String, pub title: String, pub sub: String, pub busy: bool, pub icon: &'static str, pub act: Act, pub current: bool }

impl Settings {
    pub fn page(&mut self, p: &mut Pen) {
        if self.query().is_empty() {
            p.title(PAGES[self.page].2, page_blurb(PAGES[self.page].0));
            self.body(p)
        } else { self.results(p) }
    }

    pub fn body(&mut self, p: &mut Pen) {
        match PAGES[self.page].0 {
            "quick" => self.quick(p),
            "keys" => self.keys(p),
            "wallpapers" => self.wallpapers(p),
            "presets" => self.presets(p),
            "bar" => self.bar(p),
            "island" => self.island(p),
            "buddies" => self.buddies(p),
            "services" => self.services(p),
            "usage" => self.usage(p),
            _ => self.about(p),
        }
    }

    /// Add or remove agents beyond Claude Code, Codex and Gemini CLI: any tool whose hooks live in a Claude-style JSON file.
    fn custom_agents(&mut self, p: &mut Pen) {
        p.note("Your own agents. Give a name, its config folder under your user folder (like .myagent) and the hook file in it (like hooks.json). Zor UI adds its hooks there once the folder exists.", 12.0);
        for (key, ic, label, sub, ph) in [("agent-name", "badge", "Name", "Shown in the island", "My Agent"), ("agent-dir", "folder", "Config folder", "Under your user folder", ".myagent"), ("agent-file", "description", "Hook file", "Inside that folder", "hooks.json")] {
            if let Some(r) = p.row(44.0) {
                p.left(r, ic, label, sub, 240.0);
                self.field_ui(p, key, None, super::Kind::Plain, R::new(r.x + r.w - 240.0, r.y + 2.0, 240.0, 40.0), ph, 17.0, false);
            }
        }
        if let Some(r) = p.row(44.0) {
            p.left(r, "add_circle", "Add this agent", "", 80.0);
            p.chip_right(r, None, "Add", Act::Do("add-agent", String::new()));
        }
        for a in self.get(p.cx, "customAgents").as_array().cloned().unwrap_or_default() {
            let name = a["name"].as_str().unwrap_or("").to_string();
            let Some(r) = p.row(44.0) else { continue };
            p.left(r, "smart_toy", &name, &format!("{} / {}", a["dir"].as_str().unwrap_or(""), a["file"].as_str().unwrap_or("")), 40.0);
            let id = p.act(Act::Do("del-agent", name));
            ui::icon_btn(p.c, p.cx, R::new(r.x + r.w - 36.0, r.y + 4.0, 36.0, 36.0), id, "delete", IconKind::Plain, 20.0, false);
        }
    }

    fn island(&mut self, p: &mut Pen) {
        p.h2("pill", "What the island shows", "");
        p.card(|p| {
            p.note("The island picks the most important of these. Move it or hide it in Bar & desktop > Bar layout. Anything turned off here shows on the bar as usual.", 12.0);
            self.sw(p, "island.notifications", "New notifications", "The app and title, for a few seconds", "notifications");
            self.num(p, "island.notifySeconds", "Notification seconds", 1, 30, "timer");
            self.sw(p, "island.volume", "Volume changes", "The new level, with a bar", "volume_up");
            self.sw(p, "island.mic", "Microphone mute", "When the mic is muted or turned back on", "mic");
            self.num(p, "island.osdSeconds", "Volume and mic seconds", 1, 10, "timer");
            self.sw(p, "island.agents", "AI agents that need you", "A permission prompt or a question from Claude, Codex or any agent reporting to Zor UI", "smart_toy");
            self.sw(p, "autoTrackAgents", "Track coding agents automatically", "Adds hooks to Claude Code, Codex and Gemini CLI when found on this PC, and checks every 5 minutes", "radar");
            self.sw(p, "island.agentWorking", "AI agents at work", "The agent and its 5-hour usage, when nothing else is showing", "pending");
            self.sw(p, "island.recording", "Screen recording", "A red pill with the recording time", "screen_record");
            self.sw(p, "island.timer", "Timer and stopwatch", "Pomodoro countdown or stopwatch", "timer");
            self.sw(p, "island.media", "Now playing", "The song, with its cover and progress", "music_note");
        });
        p.h2("smart_toy", "Your own agents", "");
        p.card(|p| self.custom_agents(p));
        p.h2("music_note", "Now playing", "");
        p.card(|p| {
            self.sw(p, "island.art", "Album art", "Otherwise a music note", "image");
            self.sw(p, "island.progress", "Progress ring", "Around the cover", "progress_activity");
            self.sw(p, "island.controls", "Controls on hover", "Previous, play / pause and next, with the artist", "play_pause");
            self.num(p, "island.trackSeconds", "Expand on a new song (seconds)", 0, 15, "expand");
        });
        p.h2("bedtime", "When nothing's going on", "");
        p.card(|p| {
            self.seg(p, "island.idle", &[("auto", "Auto"), ("time", "Time"), ("date", "Date"), ("weather", "Weather"), ("greeting", "Greeting"), ("none", "Nothing")], "Show", "pill");
            p.note("Auto shows whatever the bar doesn't: the time, else the weather, else a greeting. Nothing hides the island until something happens.", 12.0);
            self.sw(p, "island.status", "Status icons", "Muted speaker or mic, no network", "wifi_off");
        });
        p.h2("touch_app", "Look and actions", "");
        p.card(|p| {
            self.sw(p, "island.badges", "Badges for the rest", "Small icons for what else is going on; click one to show it", "more_horiz");
            self.slider(p, "island.textWidth", "Text width", px(80.0, 400.0, 10.0), "width");
            self.slider(p, "island.wideWidth", "Expanded text width", px(160.0, 600.0, 10.0), "width_wide");
            self.pick(p, "island.click", opts(&ISLAND_ACTIONS), "Click", "", "left_click", 190.0);
            self.pick(p, "island.rightClick", opts(&ISLAND_ACTIONS), "Right-click", "", "right_click", 190.0);
            self.pick(p, "island.scroll", opts(&[("volume", "Volume"), ("track", "Previous / next song"), ("none", "Nothing")]), "Scroll", "", "swipe_vertical", 190.0);
        });
        p.h2("expand_content", "Expanded island", "");
        p.card(|p| {
            self.sw(p, "island.panel.media", "Now playing", "Cover, progress and controls", "music_note");
            self.sw(p, "island.panel.lyrics", "Lyrics", "Under the song, when lyrics are on (Quick)", "lyrics");
            self.sw(p, "island.panel.recording", "Screen recording", "Time and a stop button", "screen_record");
            self.sw(p, "island.panel.timer", "Timer and stopwatch", "Pause, resume and reset", "timer");
            self.sw(p, "island.panel.clock", "Clock and weather", "When nothing else is going on", "schedule");
            self.sw(p, "island.panel.agents", "AI agents", "What each needs, and its usage limits", "smart_toy");
            self.sw(p, "island.panel.notifications", "Notifications", "The latest ones, with Clear all", "notifications");
            self.num(p, "island.panel.notifCount", "Notifications shown", 1, 10, "format_list_numbered");
        });
    }

    fn quick(&mut self, p: &mut Pen) {
        let (mode, scheme) = (self.info.mode.clone(), self.info.scheme.clone());
        p.h2("wallpaper", "Wallpaper & colors", "");
        p.card(|p| {
            if let Some(r) = p.row(248.0) {
                let lw = ((r.w - 10.0) * 1.4 / 2.4).floor();
                let img = R::new(r.x, r.y, lw, r.h);
                p.c.rect(img, 23.0, p.col("surfaceContainer"));
                let wall = conf::pack_dir().join("wallpaper.jpg");
                self.picture(p, &wall, img, 23.0);
                if self.info.live {
                    let id = 0;
                    ui::chip(p.c, p.cx, img.x + 10.0, img.y + 10.0, id, Some("movie"), "Live wallpaper", false);
                }
                let by = img.y + img.h - 46.0;
                let bx = img.x + img.w - 10.0 - 36.0 * 3.0 - 12.0;
                for (i, (ic, kind, act)) in [("image", IconKind::Filled, Act::Do("wall", String::new())), ("shuffle", IconKind::Tonal, Act::Do("random-online", String::new())),
                    ("animated_images", IconKind::Tonal, Act::Page(2))].into_iter().enumerate() {
                    let id = p.act(act);
                    ui::icon_btn(p.c, p.cx, R::new(bx + i as f32 * 42.0, by, 36.0, 36.0), id, ic, kind, 20.0, false);
                }
                let rx = r.x + lw + 10.0;
                let rw = r.w - lw - 10.0;
                let bw = (rw - 6.0) / 2.0;
                for (i, (v, ic, l)) in [("light", "light_mode", "Light"), ("dark", "dark_mode", "Dark")].into_iter().enumerate() {
                    let b = R::new(rx + i as f32 * (bw + 6.0), r.y, bw, 72.0);
                    let sel = mode == v;
                    p.c.rect(b, 23.0, p.col(if sel { "primaryContainer" } else { "surfaceContainer" }));
                    let id = p.act(Act::Do("mode", v.into()));
                    if p.c.hit(b, id) { p.c.rect(b, 23.0, alpha(p.col("onSurface"), 0.08)); }
                    let fg = p.col(if sel { "onPrimaryContainer" } else { "onSurface" });
                    p.c.icon(ic, R::new(b.x, b.y + 16.0, b.w, 20.0), ico(20.0, false), fg);
                    p.c.text(l, R::new(b.x, b.y + 40.0, b.w, 16.0), txt(12.0, 400), fg, Align::Center);
                }
                let sw_ = (rw - 12.0) / 3.0;
                for (i, (v, ic, l)) in SCHEMES.iter().take(9).enumerate() {
                    let b = R::new(rx + (i % 3) as f32 * (sw_ + 6.0), r.y + 80.0 + (i / 3) as f32 * 58.0, sw_, 52.0);
                    let sel = scheme == *v;
                    p.c.rect(b, 17.0, p.col(if sel { "primaryContainer" } else { "surfaceContainer" }));
                    let id = p.act(Act::Do("scheme", v.to_string()));
                    if p.c.hit(b, id) { p.c.rect(b, 17.0, alpha(p.col("onSurface"), 0.08)); }
                    let fg = p.col(if sel { "onPrimaryContainer" } else { "onSurface" });
                    p.c.icon(ic, R::new(b.x + 10.0, b.y + 8.0, 17.0, 17.0), ico(17.0, false), fg);
                    p.c.text(l, R::new(b.x + 10.0, b.y + 28.0, b.w - 14.0, 16.0), txt(11.5, 400), fg, Align::Left);
                }
            }
            if let Some(r) = p.row(40.0) {
                p.left(r, "colorize", "Theme from a color", "Keeps your wallpaper", 48.0 + 8.0 + 96.0 + 12.0 + 80.0);
                let ax = r.x + r.w - chip_w(p.c, "Apply", false);
                let id = p.act(Act::Do("seed", String::new()));
                ui::chip(p.c, p.cx, ax, r.y + 4.0, id, None, "Apply", false);
                let hex = self.fields.get("seed").map(|f| f.tb.text.clone()).unwrap_or_else(|| self.info.seed.clone());
                let fr = R::new(ax - 12.0 - 96.0, r.y + 2.0, 96.0, 36.0);
                self.field_ui(p, "seed", Some(&self.info.seed.clone()), super::Kind::Plain, fr, "#6750a4", 12.0, false);
                let col = conf::parse_hex(hex.trim()).unwrap_or(0xFF67_50A4);
                p.c.rect(R::new(fr.x - 8.0 - 48.0, r.y + 2.0, 48.0, 36.0), 6.0, col);
            }
            if !self.busy.is_empty() { p.spinner_block(); }
        });
        if let Load::Ready(mons) = &self.mons {
            let mons = mons.clone();
            if !mons.is_empty() {
                p.h2("brightness_6", "Brightness", "");
                p.card(|p| for m in mons {
                    let (i, name) = m.split_once('|').unwrap_or(("0", ""));
                    self.slider(p, &format!("monitor.{i}"), name, Sl { min: 1.0, max: 100.0, step: 1.0, fmt: Fmt::PctInt }, "brightness_6");
                });
            }
        }
        p.h2("style", "Look & feel", "");
        p.card(|p| {
            self.slider(p, "transparency", "Bar & panel transparency", Sl { min: 0.0, max: 0.6, step: 0.02, fmt: Fmt::Pct }, "opacity");
            let fonts = FONTS.get_or_init(|| p.c.g.families());
            let mut opts = vec![(json!(""), "Default (Google Sans Flex)".to_string())];
            opts.extend(fonts.iter().map(|f| (json!(f), f.clone())));
            self.pick(p, "font.family", opts, "Font", "Every font installed on this PC", "font_download", 260.0);
            self.slider(p, "font.size", "Bar & desktop text size", Sl { min: 0.8, max: 1.3, step: 0.05, fmt: Fmt::Pct }, "format_size");
            self.slider(p, "font.weight", "Bar & desktop font thickness", Sl { min: -300.0, max: 300.0, step: 50.0, fmt: Fmt::Weight }, "format_bold");
            self.slider(p, "nightLight.kelvin", "Night light warmth", Sl { min: 2500.0, max: 6000.0, step: 100.0, fmt: Fmt::Kelvin }, "nightlight");
            self.sw(p, "syncLockScreen", "Use the wallpaper on the lock screen", "Windows may block this on managed PCs", "lock");
        });
    }

    fn wallpapers(&mut self, p: &mut Pen) {
        let Load::Ready(we) = &self.we else { return p.spinner_block() };
        let we = we.clone();
        if we["installed"] != true {
            p.h2("animated_images", "Wallpaper Engine", "");
            return p.card(|p| p.note("Wallpaper Engine wasn't found in your Steam libraries. Videos picked with Change wallpaper play in Lively instead (install it with install.ps1 -WithLively).", 14.0));
        }
        let list: Vec<Value> = match &self.we_list { Load::Ready(l) => l.clone(), _ => vec![] };
        let cur = we["current"].as_str().unwrap_or("").to_lowercase();
        let is_cur = |it: &Value| {
            let path = it["path"].as_str().unwrap_or("").to_lowercase();
            !cur.is_empty() && (cur == path || cur.starts_with(path.strip_suffix("project.json").unwrap_or(&path)))
        };
        let q = self.fields.get("we-q").map(|f| f.tb.text.to_lowercase()).unwrap_or_default();
        let wt = self.we_type.clone();
        let shown: Vec<&Value> = list.iter().filter(|it| (wt == "all" || it["type"] == wt.as_str())
            && (q.is_empty() || it["title"].as_str().unwrap_or("").to_lowercase().contains(&q)
                || it["tags"].as_array().is_some_and(|t| t.iter().any(|x| x.as_str().unwrap_or("").to_lowercase().contains(&q))))).collect();
        let mut types = vec!["all".to_string()];
        for it in &list { if let Some(t) = it["type"].as_str().filter(|t| !t.is_empty() && !types.contains(&t.to_string())) { types.push(t.into()); } }
        p.h2("animated_images", "Wallpaper Engine", "");
        let playing = if cur.is_empty() { "No live wallpaper from Zor UI".to_string() } else {
            format!("Playing: {}", list.iter().find(|i| is_cur(i)).and_then(|i| i["title"].as_str().map(String::from))
                .unwrap_or_else(|| { let c = we["current"].as_str().unwrap_or(""); c.rsplitn(3, '\\').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\\") }))
        };
        p.card(|p| {
            self.sw(p, "wallpaperEngine.follow", "Follow Wallpaper Engine", "Re-theme Zor UI when you change wallpaper in Wallpaper Engine or a playlist moves on", "sync");
            if let Some(r) = p.row(40.0) {
                let running = we["running"] == true;
                let w = chip_w(p.c, "Stop live wallpaper", true);
                p.left(r, if running { "play_circle" } else { "pause_circle" }, if running { "Running" } else { "Not running" }, &playing, w);
                p.chip_right(r, Some("stop"), "Stop live wallpaper", Act::Do("we-close", String::new()));
            }
        });
        p.card(|p| {
            if let Some(r) = p.row(40.0) {
                let widths: Vec<f32> = types.iter().map(|t| 28.0 + p.c.g.measure(&cap(t), txt(12.5, 400)).0.ceil()).collect();
                let total: f32 = widths.iter().sum::<f32>() + 2.0 * (types.len() as f32 - 1.0);
                let fr = R::new(r.x, r.y, (r.w - total - 12.0).max(80.0), 40.0);
                let ph = format!("Search {} wallpapers by name or tag", if matches!(self.we_list, Load::Ready(_)) { list.len().to_string() } else { String::new() });
                self.field_ui(p, "we-q", None, super::Kind::Plain, fr, &ph, 17.0, false);
                let mut x = r.x + r.w - total;
                let group = R::new(x, r.y + 2.0, total, 36.0);
                p.c.clip_round(group, 18.0, |c| {
                    for (t, w) in types.iter().zip(&widths) {
                        let b = R::new(x, r.y + 2.0, *w, 36.0);
                        let sel = *t == wt;
                        c.rect(b, 6.0, p.cx.theme.c(if sel { "primary" } else { "surfaceContainer" }));
                        let id = ACT0 + p.acts.len() as u32;
                        p.acts.push(Act::Do("we-type", t.clone()));
                        if c.hit(b, id) && !sel { c.rect(b, 6.0, alpha(p.cx.theme.c("onSurface"), 0.08)); }
                        c.text(&cap(t), R::new(b.x, b.y, b.w, b.h), txt(12.5, 400), p.cx.theme.c(if sel { "onPrimary" } else { "onSurface" }), Align::Center);
                        x += w + 2.0;
                    }
                });
            }
            if matches!(self.we_list, Load::Ready(_)) {
                let tiles: Vec<Tile> = shown.iter().take(120).map(|it| {
                    let id = it["id"].as_str().unwrap_or("").to_string();
                    let c = is_cur(it);
                    Tile { img: it["thumb"].as_str().filter(|t| !t.is_empty()).map(|t| conf::pack_dir().join(t)),
                        badge: it["type"].as_str().unwrap_or("").to_string(), title: it["title"].as_str().unwrap_or("").to_string(),
                        sub: format!("{}{}", if it["origin"] == "workshop" { "Workshop" } else { "My projects" },
                            match it["rating"].as_str() { Some(r) if !r.is_empty() && r != "Everyone" => format!(" · {r}"), _ => String::new() }),
                        busy: self.busy == id, icon: if c { "check" } else { "wallpaper" }, act: Act::Do("we-apply", id), current: c }
                }).collect();
                self.grid(p, tiles);
                if shown.len() > 120 { p.note(&format!("Showing 120 of {}. Search to narrow it down.", shown.len()), 14.0); }
            } else { p.spinner_block(); }
        });
    }

    fn presets(&mut self, p: &mut Pen) {
        p.h2("bookmark", "Your presets", "");
        p.card(|p| {
            if let Some(r) = p.row(40.0) {
                let w = chip_w(p.c, "Save", false);
                p.left(r, "add_circle", "Save current setup", "Wallpaper, colors, bar and widget options", 220.0 + 12.0 + w);
                p.chip_right(r, None, "Save", Act::Do("save-preset", String::new()));
                self.field_ui(p, "save-name", None, super::Kind::Plain, R::new(r.x + r.w - w - 12.0 - 220.0, r.y, 220.0, 40.0), "Preset name", 17.0, false);
            }
            match &self.local {
                Load::Ready(l) if l.is_empty() => p.note("No saved presets yet.", 14.0),
                Load::Ready(l) => for it in l.clone() {
                    let id = it["id"].as_str().unwrap_or("").to_string();
                    let Some(r) = p.row(40.0) else { continue };
                    let wall = it["wallpaper"].as_str().unwrap_or("").rsplit('\\').next().unwrap_or("").to_string();
                    let sub = [it["scheme"].as_str().unwrap_or(""), it["mode"].as_str().unwrap_or(""), &wall].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
                    p.left(r, if it["from"] == "online" { "cloud_done" } else { "bookmark" }, it["name"].as_str().unwrap_or(""), &sub, 36.0 + 12.0 + 60.0);
                    let did = p.act(Act::Do("delete-local", id.clone()));
                    ui::icon_btn(p.c, p.cx, R::new(r.x + r.w - 36.0, r.y + 2.0, 36.0, 36.0), did, "delete", IconKind::Plain, 20.0, false);
                    if self.busy == id { p.spinner(r.x + r.w - 36.0 - 12.0 - 30.0, r.y + 20.0, 20.0); }
                    else { p.chip_right(R::new(r.x, r.y, r.w - 48.0, r.h), None, "Apply", Act::Do("apply-local", id)); }
                },
                _ => p.spinner_block(),
            }
        });
        p.h2("public", "Community gallery", "github.com/pctrade/end4-pCpresets");
        p.card(|p| {
            p.note("Applies each preset's wallpaper, color scheme, workspace labels and transparency. Linux-only parts (Hyprland, Quickshell layouts) are skipped.", 12.0);
            match &self.online {
                Load::Ready(o) if o["error"].is_string() => {
                    let msg = o["error"].as_str().unwrap_or("").to_string();
                    let h = p.c.g.measure_wrapped(&msg, txt(13.0, 400), p.w - 24.0).ceil() + 20.0;
                    if let Some(r) = p.row(h) {
                        p.c.rect(r, 17.0, alpha(p.col("error"), 0.18));
                        p.c.para(&msg, R::new(r.x + 12.0, r.y + 10.0, r.w - 24.0, h - 20.0), txt(13.0, 400), p.col("onSurface"), Align::Left);
                    }
                }
                Load::Ready(o) => {
                    let tiles = o["presets"].as_array().cloned().unwrap_or_default().iter().map(|pr| {
                        let name = pr["name"].as_str().unwrap_or("").to_string();
                        Tile { img: Some(super::preview_path(&name)), badge: String::new(), title: name.replace('_', " "),
                            sub: pr["author"].as_str().filter(|a| !a.is_empty()).map(|a| format!("by {a}")).unwrap_or_default(),
                            busy: self.busy == name, icon: "download", act: Act::Do("apply-online", name), current: false }
                    }).collect();
                    self.grid(p, tiles);
                }
                _ => p.spinner_block(),
            }
        });
    }

    fn bar(&mut self, p: &mut Pen) {
        p.h2("toolbar", "Bar", "");
        p.card(|p| {
            self.num(p, "workspaces", "Workspaces shown", 1, 20, "grid_view");
            self.num(p, "workspacesMain", "Workspaces on the main monitor", 1, 9, "monitor");
            self.sw(p, "workspacesRebind", "Rebind GlazeWM to match", "Changing the number above also edits and reloads GlazeWM's config. Off: only the bar changes", "sync_alt");
            let many = |a: &[&str]| json!(a);
            let labels = vec![
                (json!([]), "Numbers (1 2 3)".to_string()),
                (many(&["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"]), "Kanji (一 二 三)".to_string()),
                (many(&["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"]), "Roman (I II III)".to_string()),
                (many(&["①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩"]), "Circled (① ② ③)".to_string()),
            ];
            self.pick(p, "workspaceLabels", labels, "Workspace labels", "What the active workspace shows", "label", 260.0);
            self.sw(p, "alwaysShowNumbers", "Always show workspace labels", "Otherwise only the active one shows a label", "pin");
            let now = crate::util::now();
            let formats: Vec<(Value, String)> = CLOCK_FORMATS.iter().map(|f| (json!(f), crate::sys::luxon(f, &now))).collect();
            self.pick(p, "clockFormat", formats, "Clock format", "Shown as it looks right now", "schedule", 260.0);
            self.seg(p, "tempUnit", &[("C", "°C"), ("F", "°F")], "Temperature", "thermostat");
            self.sw(p, "showKeyboardLayout", "Keyboard layout indicator", "", "keyboard");
            self.sw(p, "visualizer.bar", "Visualizer in the bar", "Small cava-style bars next to the media", "graphic_eq");
        });
        p.h2("dashboard_customize", "Bar layout", "");
        p.card(|p| {
            self.seg(p, "bar.position", &[("top", "Top"), ("bottom", "Bottom"), ("left", "Left"), ("right", "Right")], "Bar position", "vertical_align_top");
            self.sw(p, "bar.hideFullscreen", "Hide in full screen", "The bar hides on a monitor while an app (a game, a video) covers it", "fullscreen");
            p.note("Side bars show icons and short text only (no window title). Moving the bar frees or takes screen space for windows after zorui restarts.", 12.0);
            self.seg(p, "bar.style", &[("float", "Float"), ("flat", "Flat"), ("islands", "Islands")], "Bar style", "style");
            self.seg(p, "bar.groups", &[("filled", "Filled"), ("outlined", "Outlined"), ("none", "None")], "Group backgrounds", "rounded_corner");
            self.slider(p, "bar.titleWidth", "Window title width", Sl { min: 10.0, max: 40.0, step: 1.0, fmt: Fmt::PctInt }, "width");
            p.note("Drag a part up or down to change its order (top shows first), or into another column. Or click it and use the arrow keys.", 12.0);
            self.bar_columns(p);
            p.lead_label("Utility buttons");
            let cur = self.garr(p.cx, "bar.utils");
            p.chips([("snip", "Screen snip"), ("pick", "Color picker"), ("wall", "Wallpaper"), ("mode", "Dark mode")].iter()
                .map(|(v, l)| (l.to_string(), None, cur.iter().any(|c| c == v), Act::Do("bu", v.to_string()))).collect());
        });
        p.h2("desktop_windows", "Desktop", "");
        p.card(|p| {
            self.sw(p, "desktopWidgets", "Desktop widgets", "Pick the cards below, then drag them anywhere on the desktop", "widgets");
            let cur = self.garr(p.cx, "desktop.widgets");
            p.chips(DESKTOP_WIDGETS.iter().map(|(id, n)| (n.to_string(), None, cur.iter().any(|c| c == id), Act::Do("dwidget", id.to_string()))).collect());
            self.seg(p, "desktop.clock", &[("cookie", "Cookie"), ("analog", "Analog"), ("digital", "Digital"), ("pixel", "Pixel")], "Clock style", "schedule");
            self.sw(p, "desktop.clockVertical", "Stacked digital clock", "Hours over minutes, for the Digital style", "view_agenda");
            p.note("World clock cities: pick as many as you like.", 12.0);
            let cur = self.garr(p.cx, "desktop.worldClocks");
            p.chips(CITIES.iter().map(|(tz, n)| (n.to_string(), None, cur.iter().any(|c| c == tz), Act::Do("tz", tz.to_string()))).collect());
            self.text(p, "desktop.text", "Custom text", "Shown by the Custom text card", "text_fields");
            if let Some(r) = p.row(40.0) {
                let w = chip_w(p.c, "Reset layout", false);
                p.left(r, "restart_alt", "Card positions", "Put every card back in one column on the left", w);
                p.chip_right(r, None, "Reset layout", Act::Do("reset-desk", String::new()));
            }
            self.sw(p, "frame.enabled", "Screen frame", "A thin rounded frame around the screen", "crop_free");
            self.seg(p, "dock.style", &[("hug", "Hug"), ("float", "Float"), ("plain", "Plain")], "Dock style", "style");
            self.slider(p, "dock.iconSize", "Dock icon size", px(20.0, 56.0, 2.0), "aspect_ratio");
            self.slider(p, "dock.radius", "Dock corner radius", px(0.0, 40.0, 2.0), "rounded_corner");
            self.slider(p, "frame.thickness", "Frame thickness", px(1.0, 40.0, 1.0), "line_weight");
            self.slider(p, "frame.radius", "Frame corner radius", px(0.0, 80.0, 2.0), "rounded_corner");
            self.seg(p, "dock.side", &[("bottom", "Bottom"), ("right", "Right")], "Dock edge", "dock_to_right");
            self.sw(p, "dock.enabled", "Dock", "Pinned apps and open windows along the bottom. Right-click an app to pin it", "dock_to_bottom");
            self.sw(p, "visualizer.desktop", "Desktop visualizer", "Audio bars along the bottom of the screen (cava)", "equalizer");
            self.num(p, "visualizer.bars", "Visualizer bars", 16, 160, "bar_chart");
            self.text(p, "userName", "Your name", "Shown in the greeting and sidebar", "person");
        });
        p.h2("image", "Image cards", "");
        p.card(|p| {
            for (i, id) in ["image1", "image2", "image3", "sticker"].into_iter().enumerate() {
                let file = self.gs(p.cx, &format!("desktop.images.{id}.file"));
                let on = self.garr(p.cx, "desktop.widgets").iter().any(|w| w == id);
                if let Some(r) = p.row(40.0) {
                    let sub = if file == "wallpaper.jpg" { "Your wallpaper" } else if !file.is_empty() { "Picture chosen" } else { "No picture yet" };
                    let (w1, w2, w3) = (chip_w(p.c, "Choose", true), chip_w(p.c, "Wallpaper", true), chip_w(p.c, if on { "On" } else { "Off" }, true));
                    p.left(r, "image", &if id == "sticker" { "Sticker".into() } else { format!("Image {}", i + 1) }, sub, 130.0 + w1 + w2 + w3 + 36.0);
                    let mut x = r.x + r.w;
                    x -= w3; let a = p.act(Act::Do("dwidget", id.into()));
                    ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some(if on { "visibility" } else { "visibility_off" }), if on { "On" } else { "Off" }, on);
                    x -= w2 + 12.0; let a = p.act(Act::Do("wall-image", id.into()));
                    ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("wallpaper"), "Wallpaper", false);
                    x -= w1 + 12.0; let a = p.act(Act::Do("pick-image", id.into()));
                    ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("folder_open"), "Choose", false);
                    self.select_box(p, &format!("desktop.images.{id}.shape"), opts(&IMAGE_SHAPES), R::new(x - 12.0 - 130.0, r.y, 130.0, 40.0));
                }
                self.slider(p, &format!("desktop.images.{id}.size"), &if id == "sticker" { "Sticker size".into() } else { format!("Image {} size", i + 1) }, px(120.0, 800.0, 10.0), "aspect_ratio");
            }
            p.note("Choosing a picture turns its card on. On/Off shows or hides the card.", 12.0);
        });
        p.h2("waving_hand", "Greeting card", "");
        p.card(|p| {
            self.text(p, "desktop.greet.text", "Greeting", "Empty for \"Hi, {name}!\". {name} is your name", "title");
            self.text(p, "desktop.greet.sub", "Second line", "Empty for Good Morning / Afternoon / Evening. {name} works here too", "short_text");
            self.sw(p, "desktop.greet.date", "Date line", "Today • weekday, day and month", "today");
            let file = self.gs(p.cx, "desktop.greet.file");
            if let Some(r) = p.row(40.0) {
                let (w1, w2) = (chip_w(p.c, "Choose", true), chip_w(p.c, "Use initial", true));
                let right = 130.0 + 12.0 + w1 + if file.is_empty() { 0.0 } else { 12.0 + w2 };
                p.left(r, "account_circle", "Avatar", if file.is_empty() { "Your initial" } else { "Picture chosen" }, right);
                let mut x = r.x + r.w;
                if !file.is_empty() {
                    x -= w2; let a = p.act(Act::Do("greet-initial", String::new()));
                    ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("close"), "Use initial", false);
                    x -= 12.0;
                }
                x -= w1; let a = p.act(Act::Do("pick-image", "greet".into()));
                ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("folder_open"), "Choose", false);
                let mut o = vec![(json!(""), "Default".to_string())];
                o.extend(opts(&IMAGE_SHAPES));
                self.select_box(p, "desktop.greet.shape", o, R::new(x - 12.0 - 130.0, r.y, 130.0, 40.0));
            }
        });
        p.h2("timer", "Pomodoro", "");
        p.card(|p| {
            self.num(p, "timer.focus", "Focus minutes", 1, 180, "center_focus_strong");
            self.num(p, "timer.short", "Short break minutes", 1, 60, "coffee");
            self.num(p, "timer.long", "Long break minutes", 1, 120, "weekend");
            self.num(p, "timer.cycles", "Focus sessions before a long break", 1, 12, "repeat");
        });
    }

    /// Two buddy slots on the bar. Slot 1 is the original `native.gojo` flag, slot 2 is `buddies.b.on`.
    fn buddies(&mut self, p: &mut Pen) {
        let custom = self.gb(p.cx, "buddies.customEnabled"); // feature flag, off by default
        let casts: Vec<(Value, String)> = crate::cast::all(&if custom { self.get(p.cx, "buddies.custom") } else { json!([]) }).into_iter().map(|(id, name)| (json!(id), name)).collect();
        let chatty = [("quiet", "Quiet"), ("normal", "Normal"), ("chatty", "Chatty")];
        for (k, title, on) in [("a", "Buddy 1", "native.gojo"), ("b", "Buddy 2", "buddies.b.on")] {
            p.h2("pets", title, "");
            p.card(|p| {
                self.sw(p, on, "Show on the bar", "Turn the buddy off and it goes away", "visibility");
                self.pick(p, &format!("buddies.{k}.character"), casts.clone(), "Character", "Their look and what they say", "face", 190.0);
                self.text_ph(p, &format!("buddies.{k}.name"), "Name", "What their partner calls them", "badge", "Character's own name");
                self.seg(p, &format!("buddies.{k}.chatty"), &chatty, "Talks", "chat");
                self.sw(p, &format!("buddies.{k}.walk"), "Walks around", "Off: they stay where they are and only sit or stand", "directions_walk");
                for (f, label, ic) in [("hair", "Hair color", "brush"), ("shirt", "Shirt color", "checkroom"), ("pants", "Pants color", "styler"), ("eyes", "Eye color", "visibility")] {
                    let colors: Vec<(Value, String)> = COLORS.iter().map(|(h, n)| (json!(h), n.to_string())).collect();
                    self.pick(p, &format!("buddies.{k}.{f}"), colors, label, "", ic, 190.0);
                }
                let bun = vec![(json!(""), "Character's own".to_string()), (json!("yes"), "Yes".to_string()), (json!("no"), "No".to_string())];
                self.pick(p, &format!("buddies.{k}.bun"), bun, "Bun", "Hair tied up on top", "face_3", 190.0);
                if custom {
                let flag = |yes: &str| vec![(json!(""), "Character's own".to_string()), (json!("yes"), yes.to_string()), (json!("no"), "No".to_string())];
                self.pick(p, &format!("buddies.{k}.glasses"), flag("Yes"), "Glasses", "", "eyeglasses", 190.0);
                self.pick(p, &format!("buddies.{k}.ears"), flag("Yes"), "Cat ears", "", "pets", 190.0);
                let colors: Vec<(Value, String)> = COLORS.iter().map(|(h, n)| (json!(h), n.to_string())).collect();
                self.pick(p, &format!("buddies.{k}.skin"), colors, "Skin color", "", "palette", 190.0);
                let img = self.gs(p.cx, &format!("buddies.{k}.image"));
                if let Some(r) = p.row(40.0) {
                    let (w1, w2) = (chip_w(p.c, "Choose", true), chip_w(p.c, "Clear", true));
                    p.left(r, "image", "Use a picture", if img.is_empty() { "A PNG with a transparent background replaces the drawn figure" } else { "Picture chosen. It replaces the drawn figure" }, 130.0 + w1 + w2 + 24.0);
                    let mut x = r.x + r.w;
                    if !img.is_empty() { x -= w2; let a = p.act(Act::Do("clear-buddy-image", k.into())); ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("close"), "Clear", false); x -= 12.0; }
                    x -= w1; let a = p.act(Act::Do("pick-image", format!("buddy-{k}-{}", crate::hub::now_ms() as i64)));
                    ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("folder_open"), "Choose", false);
                }
                }
                let mut styles = vec![(json!(""), "Character's own".to_string())];
                styles.extend(crate::cast::HAIR_STYLES.iter().map(|(v, n)| (json!(v), n.to_string())));
                self.pick(p, &format!("buddies.{k}.style"), styles, "Hairstyle", "Spiky, short, long or a mohawk", "content_cut", 190.0);
                let cur = self.gs(p.cx, &format!("buddies.{k}.character"));
                let saved = self.get(p.cx, "buddies.custom").as_array().and_then(|a| a.iter().find(|e| e["id"] == cur.as_str()).map(|e| e["name"].as_str().unwrap_or("").to_string()));
                if let Some((name, r)) = saved.filter(|_| custom).and_then(|n| p.row(40.0).map(|r| (n, r))) {
                    let (wr, wn) = (chip_w(p.c, "Remove", false), chip_w(p.c, "Rename", false));
                    p.left(r, "edit", "Saved buddy", "Rename it, or remove it from the list", 200.0 + wr + wn + 24.0);
                    p.chip_right(r, None, "Remove", Act::Do("remove-buddy", cur.clone()));
                    p.chip_right(R::new(r.x, r.y, r.w - wr - 12.0, r.h), None, "Rename", Act::Do("rename-buddy", format!("{k}:{cur}")));
                    self.field_ui(p, &format!("bname-{k}"), Some(&name), super::Kind::Plain, R::new(r.x + r.w - wr - wn - 24.0 - 200.0, r.y, 200.0, 40.0), "Buddy name", 17.0, false);
                }
            });
        }
        if custom { p.h2("person_add", "Create a new buddy", "");
        p.card(|p| {
            p.note("Customize a buddy above (name, colors, hairstyle, picture), then save it as a new character. Every saved character is in both slots' Character lists.", 12.0);
            for (k, title) in [("a", "Buddy 1"), ("b", "Buddy 2")] {
                if let Some(r) = p.row(44.0) {
                    p.left(r, "person_add", &format!("Save {title} as a new character"), "", 100.0);
                    p.chip_right(r, None, "Create", Act::Do("new-buddy", k.into()));
                }
            }
        }); }
        p.h2("mic", "Voice commands", "");
        p.card(|p| {
            self.sw(p, "buddies.listen", "Talk to the primary buddy", "The hotkey (default Alt+B) records you and the buddy acts on what you say", "mic");
            self.sw(p, "buddies.allow.search", "Search the web", "\"search for ...\" opens a Google search in your browser", "search");
            self.sw(p, "buddies.allow.open", "Open apps and games", "\"open <name>\" starts a Start Menu app or a Steam game", "rocket_launch");
            self.sw(p, "buddies.allow.message", "Start messages", "\"message <name> saying ...\" opens the chat with the text ready. You press Enter to send", "chat");
            self.sw(p, "buddies.listenAi", "Ask the AI for other requests", "Sends what you said to the sidebar's AI model. Off: only the phrases above work", "auto_awesome");
            self.sw(p, "native.gojoVoice", "Speak replies aloud", "Uses Windows' own voice", "volume_up");
            let mut mics = vec![(json!(""), "Windows default".to_string())];
            mics.extend(crate::assist::mics().into_iter().map(|m| (json!(m), m)));
            self.pick(p, "buddies.mic", mics, "Microphone", "The one the buddy listens with", "mic", 260.0);
            let mut models = vec![(json!(""), "Automatic".to_string())];
            models.extend(crate::assist::speech_models().into_iter().map(|(dir, name)| (json!(dir), name)));
            self.pick(p, "buddies.model", models, "Speech model", "Found on this PC (Orca's Parakeet). Automatic uses the first one", "model_training", 260.0);
            self.text_ph(p, "buddies.contacts", "Contacts", "name=app:number, comma separated", "contacts", "ali=whatsapp:+923001234567, sam=discord:1234567890");
            p.note("Contacts are the one thing to type, since nobody else knows them: whatsapp takes a phone number with country code, discord takes the user id (Developer Mode, right-click, Copy User ID). Voice needs the hotkey in GlazeWM's config; re-run install.ps1 to add it.", 12.0);
        });
        p.h2("favorite", "Partners", "");
        p.card(|p| {
            self.sw(p, "buddies.pair", "Make them partners", "With both on, they walk to each other, face each other and trade lines", "group");
            self.seg(p, "buddies.primary", &[("a", "Buddy 1"), ("b", "Buddy 2")], "Primary buddy", "star");
            self.sw(p, "buddies.breaks", "Remind me to take breaks", "The primary buddy speaks up after a long stretch of work", "coffee");
            self.num(p, "buddies.breakMins", "Minutes of work between breaks", 1, 240, "timer");
            self.sw(p, "buddies.watch", "Comment on what I'm doing", "The primary buddy reacts to the app in front, like an editor, a video or a game", "visibility");
            p.note("Talk to the primary buddy with the hotkey (default Alt+B): say \"search for ...\", \"open <app or game>\" or \"message <name> saying ...\". Names come from buddies.contacts in config.json, e.g. {\"ali\": {\"app\": \"whatsapp\", \"phone\": \"+923001234567\"}} or {\"app\": \"discord\", \"id\": \"<user id>\"}. Messages open in the app and wait for you to press Enter.", 12.0);
            p.note("Click a buddy to hear them, double-click for their move, drag to move them. With two on, only the primary buddy comments on the desktop and reminds you to take breaks. Quiet turns a buddy's comments and chat off.", 12.0);
        });
    }

    fn services(&mut self, p: &mut Pen) {
        let keys = match &self.keys { Load::Ready(k) => Some(k.clone()), _ => None };
        let current_model = self.gs(p.cx, "ai.model");
        let models: Vec<(Value, String)> = match &self.models {
            Load::Ready(m) if !m.is_empty() => m.iter().map(|(i, n)| (json!(i), n.clone())).collect(),
            _ => vec![(json!(current_model), current_model.clone())],
        };
        p.h2("neurology", "AI", "");
        p.card(|p| {
            self.sw(p, "ai.claudeCli", "Use my Claude subscription", "Adds Claude Haiku, Sonnet and Opus through Claude Code, no API key. Needs the claude command, signed in. Counts against your plan", "workspace_premium");
            self.pick(p, "ai.model", models, "Default model", "Used by the sidebar and the buddies. Ollama models appear when Ollama is running", "model_training", 260.0);
            self.slider(p, "ai.temperature", "Temperature", Sl { min: 0.0, max: 2.0, step: 0.1, fmt: Fmt::Fixed1 }, "thermostat");
            if let Some(r) = p.row(80.0) {
                p.left(R::new(r.x, r.y, r.w, 40.0), "description", "System prompt", "Leave empty for the default", 300.0);
                let init = self.gs(p.cx, "ai.systemPrompt");
                self.field_ui(p, "ai.systemPrompt", Some(&init), super::Kind::Text { list: false }, R::new(r.x + r.w - 300.0, r.y, 300.0, 80.0), "", 17.0, false);
            }
            for (id, name) in KEY_PROVIDERS {
                let Some(r) = p.row(40.0) else { continue };
                let has = keys.as_ref().is_some_and(|k| k[id] == true);
                let status = if keys.is_none() { "…" } else if has { "Key saved (encrypted with Windows DPAPI)" } else { "No key" };
                let wsave = chip_w(p.c, "Save", false);
                p.left(r, "key", "", "", 240.0 + 12.0 + wsave + if has { 48.0 } else { 0.0 });
                // the status line replaces `small`, in the "ok" / "warn" colours
                let (h1, h2) = (13.5 * LH, 12.0 * LH);
                let top = r.y + (r.h - h1 - h2) / 2.0;
                p.c.text(name, R::new(r.x + 32.0, top, 300.0, h1), txt(13.5, 400), p.col("onSurface"), Align::Left);
                p.c.text(status, R::new(r.x + 32.0, top + h1, 300.0, h2), txt(12.0, 400), p.col(if has { "primary" } else { "onSurfaceVariant" }), Align::Left);
                let mut right = r.x + r.w;
                if has {
                    let a = p.act(Act::Do("clear-key", id.into()));
                    ui::icon_btn(p.c, p.cx, R::new(right - 36.0, r.y + 2.0, 36.0, 36.0), a, "delete", IconKind::Plain, 20.0, false);
                    right -= 48.0;
                }
                let a = p.act(Act::Do("save-key", id.into()));
                ui::chip(p.c, p.cx, right - wsave, r.y + 4.0, a, None, "Save", false);
                let ph = if has { "••••••••  (enter to replace)" } else { "Paste API key" };
                self.field_ui(p, &format!("key-{id}"), None, super::Kind::Secret, R::new(right - wsave - 12.0 - 240.0, r.y, 240.0, 40.0), ph, 17.0, false);
            }
        });
        p.h2("translate", "Translator", "");
        p.card(|p| self.pick(p, "translator.target", LANGS.iter().map(|(k, v)| (json!(k), v.to_string())).collect(), "Translate into", "", "language", 260.0));
        p.h2("screen_record", "Screen recording", "");
        p.card(|p| {
            self.pick(p, "record.audio", crate::bridge::rec_audio::MODES.iter().map(|(k, v, _)| (json!(k), v.to_string())).collect(), "Record", "System sound is what your speakers play", "graphic_eq", 260.0);
            let mut mics = vec![(json!(""), "Windows default".to_string())];
            mics.extend(crate::assist::mics().into_iter().map(|m| (json!(m), m)));
            self.pick(p, "record.mic", mics, "Microphone", "Used when Record includes the microphone", "mic", 260.0);
            p.note("The sound is joined to the video when you stop, so stopping a long recording takes a few seconds.", 12.0);
        });
        p.h2("photo_library", "Anime & wallpapers", "");
        p.card(|p| {
            self.seg(p, "booru.source", &[("yandere", "yande.re"), ("konachan", "Konachan"), ("danbooru", "Danbooru"), ("wallhaven", "Wallhaven")], "Default source", "image_search");
            self.sw(p, "booru.allowNsfw", "Allow NSFW results", "Off by default. Applies to every source.", "explicit");
        });
        p.h2("sports_soccer", "Football", "");
        p.card(|p| {
            p.lead_label("Leagues to show (none selected = all)");
            let cur = self.garr(p.cx, "football.leagues");
            p.chips(LEAGUES.iter().map(|(id, n)| (n.to_string(), None, cur.iter().any(|c| c == id), Act::Do("league", id.to_string()))).collect());
        });
        p.h2("sports_esports", "Games", "");
        p.card(|p| self.sw(p, "autoGameMode", "Pause while a game is full screen", "Hides the bars and windows and stops polling, then brings them back", "sports_esports"));
        p.h2("lyrics", "Media", "");
        p.card(|p| self.sw(p, "lyrics", "Synced lyrics", "From lrclib.net, in the left sidebar", "lyrics"));
    }

    fn keys(&mut self, p: &mut Pen) {
        let lwin = p.cx.cfg.str("modKey") == "lwin";
        let m = if lwin { "Win" } else { "Alt" };
        let f = |s: &str| s.replace("{m}", m);
        let groups: Vec<(&str, &str, Vec<(String, &str)>)> = vec![
            ("dashboard", "Shell", vec![(f("{m}+A"), "Left sidebar: home, AI, translator, anime, football"), (f("{m}+N"), "Right sidebar: toggles, notifications, calendar"),
                (f("{m}+I"), "Settings"), (f("{m}+/"), "This keybinds list"), (if lwin { "Win+Tab".into() } else { "Alt+`".into() }, "Overview"), ("Alt+Space".into(), "Launcher"),
                (f("{m}+Ctrl+T"), "Pick a wallpaper and regenerate colors"), (f("{m}+Ctrl+D"), "Dark / light"), (f("{m}+Ctrl+R"), "Restart the bar and reload GlazeWM"),
                (f("{m}+Shift+G"), "Game mode: free RAM (press again to restore)"),
                ("Ctrl+PgUp / PgDn".into(), "Switch tabs in a sidebar or here"), ("Esc".into(), "Close a sidebar or this window")]),
            ("apps", "Apps", vec![(f("{m}+Enter / {m}+T"), "Terminal"), (f("{m}+E"), "File Explorer"), (f("{m}+W"), "Edge"), (f("{m}+C"), "VS Code"),
                (f("{m}+Shift+S"), "Screen snip"), (f("{m}+K"), "On-screen keyboard")]),
            ("view_quilt", "Windows", vec![(f("{m}+Arrows"), "Focus the window in that direction"), (f("{m}+Shift+Arrows"), "Move the window in that direction (also to another monitor)"),
                (f("{m}+V"), "Next window opens beside / below"), (f("{m}+R"), "Resize mode: arrows to resize, Esc or Enter to finish"), (f("{m}+- / {m}+="), "Narrower / wider"),
                (f("{m}+Q"), "Close"), (f("{m}+F"), "Fullscreen"), (f("{m}+Shift+Space"), "Float / tile"), (f("{m}+Shift+T"), "Tile a floating window"), (f("{m}+M"), "Minimize")]),
            ("grid_view", "Workspaces", vec![(f("{m}+1…0"), "Go to workspace 1–10"), (f("{m}+Shift+1…0"), "Send the window to workspace 1–10"),
                (f("{m}+Ctrl+← / →"), "Previous / next workspace"), (f("{m}+PgUp / PgDn"), "Previous / next workspace"), (f("{m}+Ctrl+Shift+← / →"), "Move the workspace to another monitor")]),
            ("build", "GlazeWM", vec![(f("{m}+Shift+P"), "Pause tiling (press again to resume)"), (f("{m}+Shift+W"), "Redraw all windows"), (f("{m}+Shift+E"), "Quit GlazeWM")]),
        ];
        for (ic, title, list) in groups {
            p.h2(ic, title, "");
            p.card(|p| {
                let kw = list.iter().map(|(k, _)| p.c.g.measure(k, txt(13.0, 400)).0.ceil()).fold(0.0, f32::max) + 16.0;
                let rh = 13.0 * LH + 6.0;
                let h = list.len() as f32 * rh + (list.len() as f32 - 1.0) * 6.0;
                if let Some(r) = p.row(h) {
                    for (i, (k, d)) in list.iter().enumerate() {
                        let y = r.y + i as f32 * (rh + 6.0);
                        let kr = R::new(r.x, y, p.c.g.measure(k, txt(13.0, 400)).0.ceil() + 16.0, rh);
                        p.c.rect(kr, 6.0, p.col("surfaceContainer"));
                        p.c.text(k, R::new(kr.x + 8.0, y, kr.w, rh), txt(13.0, 400), p.col("onSurface"), Align::Left);
                        p.c.text(d, R::new(r.x + kw + 14.0, y, r.w - kw - 14.0, rh), txt(13.0, 400), p.col("onSurface"), Align::Left);
                    }
                }
            });
        }
        p.lead_gap(12.0);
        let note = "Games are left alone by GlazeWM (window rule in %USERPROFILE%\\.glzr\\glazewm\\config.yaml).";
        let h = p.c.g.measure_wrapped(note, txt(14.0, 400), p.w - 8.0).ceil();
        if p.show(h) { p.c.para(note, R::new(p.x + 4.0, p.y, p.w - 8.0, h), txt(14.0, 400), p.col("onSurfaceVariant"), Align::Left); }
        p.y += h + 12.0;
    }

    fn usage(&mut self, p: &mut Pen) {
        let (me, kids) = crate::perf::processes();
        let parts = crate::perf::parts();
        let row = |p: &mut Pen, ic: &str, label: &str, sub: &str, value: String, frac: f32| {
            if let Some(r) = p.row(40.0) {
                let vw = p.c.g.measure(&value, txt(13.0, 500)).0.ceil() + 4.0;
                p.left(r, ic, label, sub, vw);
                p.c.text(&value, R::new(r.x + r.w - vw, r.y, vw, r.h - 6.0), txt(13.0, 500), p.col("onSurface"), Align::Right);
                let bar = R::new(r.x, r.y + r.h - 4.0, r.w, 3.0);
                p.c.rect(bar, 1.5, alpha(p.col("onSurface"), 0.08));
                p.c.rect(R::new(bar.x, bar.y, (bar.w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 3.0 } else { 0.0 }), bar.h), 1.5, p.col("primary"));
            }
        };
        let kids_ws: u64 = kids.iter().map(|k| k.ws).sum();
        let kids_cpu = kids.iter().fold(0.0f32, |a, k| a + k.cpu);
        p.h2("memory", "Zor UI", "CPU is the share of one core. Updates every second");
        p.card(|p| {
            row(p, "memory", "Memory", "Working set of zorui.exe", crate::perf::mb(me.ws), me.ws as f32 / (600u64 << 20) as f32);
            row(p, "database", "Private memory", "Committed to the app alone", crate::perf::mb(me.private), me.private as f32 / (600u64 << 20) as f32);
            row(p, "speed", "CPU", "All windows and background work", format!("{:.1}%", me.cpu), me.cpu / 100.0);
            row(p, "terminal", "Helper programs", &format!("{} running (scripts, bridge, tools)", kids.len()), format!("{} · {:.1}%", crate::perf::mb(kids_ws), kids_cpu), kids_cpu / 100.0);
        });
        p.h2("dashboard", "Parts", "Drawing time and pixel buffers of each part");
        p.card(|p| {
            if parts.is_empty() { p.note("Nothing has drawn yet.", 12.0); }
            let top = parts.iter().map(|x| x.cpu).fold(0.1f32, f32::max);
            for x in &parts {
                let sub = format!("{} window{} · {:.0} frames/s · {} buffer", x.windows, if x.windows == 1 { "" } else { "s" }, x.fps, crate::perf::mb(x.bytes));
                row(p, "widgets", &x.name, &sub, format!("{:.2}%", x.cpu), x.cpu / top);
            }
        });
        p.h2("terminal", "Helper programs", "Started by Zor UI");
        p.card(|p| {
            if kids.is_empty() { p.note("None running right now.", 12.0); }
            let top = kids.iter().map(|k| k.ws).max().unwrap_or(1).max(1) as f32;
            for k in &kids {
                row(p, "terminal", &k.name, &format!("pid {}", k.pid), format!("{} · {:.1}%", crate::perf::mb(k.ws), k.cpu), k.ws as f32 / top);
            }
        });
    }

    fn about(&mut self, p: &mut Pen) {
        p.h2("info", "About", "");
        p.card(|p| {
            if let Some(r) = p.row(40.0) {
                let w = chip_w(p.c, "zor-ui", true);
                p.left(r, "", "Zor UI", "A native shell for GlazeWM on Windows: bar, Dynamic Island, sidebars, buddies, dock. Some parts are ported from end4-pC by pctrade (GPL-3.0)", w);
                p.chip_right(r, Some("open_in_new"), "Credit: end4-pC", Act::Do("url", "https://github.com/pctrade/end4-pC".into()));
            }
            if let Some(r) = p.row(40.0) {
                let w = chip_w(p.c, "Open", true);
                p.left(r, "", "Windows settings", "", w);
                p.chip_right(r, Some("settings"), "Open", Act::Do("winsettings", String::new()));
            }
            if let Some(r) = p.row(40.0) {
                let w = chip_w(p.c, "Check", true);
                p.left(r, "", "Updates", &format!("Version {}. Installs a newer release if there is one", env!("CARGO_PKG_VERSION")), w);
                p.chip_right(r, Some("update"), "Check", Act::Do("update", String::new()));
            }
            if let Some(r) = p.row(40.0) {
                let w = chip_w(p.c, "Edit", true);
                p.left(r, "", "Config file", if p.cx.cfg.str("scriptsDir").is_empty() { "" } else { "%USERPROFILE%\\.glzr\\zebar\\zor-ui\\config.json" }, w);
                p.chip_right(r, Some("edit"), "Edit", Act::Do("editcfg", String::new()));
            }
        });
    }

    // ------------------------------------------------------------ pieces

    /// The cover-cropped picture at `path` decoded for a `w` x `h` CSS px box (cached), if it exists and decodes.
    pub fn pic(&mut self, g: &crate::gfx::Gfx, scale: f32, path: &std::path::Path, w: f32, h: f32) -> Option<&tiny_skia::Pixmap> {
        let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
        let key = format!("{}|{pw}x{ph}", path.display());
        if !self.imgs.contains_key(&key) {
            if !path.exists() { return None; }
            if self.imgs.len() > 200 { self.imgs.clear(); }
            self.imgs.insert(key.clone(), g.load_image(path, pw, ph));
        }
        self.imgs.get(&key)?.as_ref()
    }

    /// `pic` drawn into `r` with rounded corners.
    pub fn picture(&mut self, p: &mut Pen, path: &std::path::Path, r: R, radius: f32) {
        let scale = p.c.scale;
        let g = p.c.g;
        if let Some(img) = self.pic(g, scale, path, r.w, r.h) { p.c.clip_round(r, radius, |c| c.image(img, r, 1.0)); }
    }

    /// `.grid`: auto-fill columns of at least 190 px, 10 px apart.
    pub fn grid(&mut self, p: &mut Pen, tiles: Vec<Tile>) {
        p.lead_gap(12.0);
        let cols = (((p.w + 10.0) / 200.0).floor() as usize).max(1);
        let tw = (p.w - 10.0 * (cols as f32 - 1.0)) / cols as f32;
        let th = (tw * 9.0 / 16.0).round() + 52.0;
        let rows = tiles.len().div_ceil(cols);
        let h = if rows == 0 { 0.0 } else { rows as f32 * th + (rows as f32 - 1.0) * 10.0 };
        if p.show(h) {
            for (i, t) in tiles.into_iter().enumerate() {
                let r = R::new(p.x + (i % cols) as f32 * (tw + 10.0), p.y + (i / cols) as f32 * (th + 10.0), tw, th);
                if r.y + r.h < p.lo || r.y > p.hi { continue; }
                if t.current { p.c.rect(R::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0), 25.0, p.col("primary")); }
                p.c.rect(r, 23.0, p.col("surfaceContainer"));
                let pv = R::new(r.x, r.y, r.w, th - 52.0);
                let (scale, g) = (p.c.scale, p.c.g);
                let img = t.img.as_ref().and_then(|i| self.pic(g, scale, i, pv.w, pv.h));
                p.c.clip_round(r, 23.0, |c| {
                    c.rect(pv, 0.0, p.cx.theme.c("surfaceContainerHigh"));
                    if let Some(img) = img { c.image(img, pv, 1.0); }
                });
                if !t.badge.is_empty() {
                    let bw = p.c.g.measure(&cap(&t.badge), txt(11.0, 400)).0.ceil() + 16.0;
                    let b = R::new(r.x + 8.0, r.y + 8.0, bw, 22.0);
                    p.c.rect(b, 11.0, alpha(ui::layer0(p.cx), 0.8));
                    p.c.text(&cap(&t.badge), b, txt(11.0, 400), p.col("onSurface"), Align::Center);
                }
                let m = R::new(r.x, r.y + th - 52.0, r.w, 52.0);
                p.c.text(&t.title, R::new(m.x + 10.0, m.y + 11.0, m.w - 10.0 - 36.0 - 6.0 - 10.0, 13.0 * LH), txt(13.0, 500), p.col("onSurface"), Align::Left);
                p.c.text(&t.sub, R::new(m.x + 10.0, m.y + 11.0 + 13.0 * LH, m.w - 62.0, 11.0 * LH), txt(11.0, 400), p.col("onSurfaceVariant"), Align::Left);
                let b = R::new(m.x + m.w - 10.0 - 36.0, m.y + 8.0, 36.0, 36.0);
                if t.busy { p.c.circle(b.x + 18.0, b.y + 18.0, 18.0, p.col("secondaryContainer")); p.spinner(b.x + 18.0, b.y + 18.0, 20.0); }
                else { let id = p.act(t.act); ui::icon_btn(p.c, p.cx, b, id, t.icon, IconKind::Tonal, 20.0, false); }
            }
        }
        p.y += h;
    }

    /// The four bar layout columns with draggable parts.
    fn bar_columns(&mut self, p: &mut Pen) {
        let net = p.cx.cfg.bool("showNetSpeed");
        let lay = bar_layout(&self.get(p.cx, "bar.modules"), net);
        let cw = (p.w - 24.0) / 4.0;
        let most = SIDES.iter().map(|s| lay.iter().filter(|m| m.1 == *s).count()).max().unwrap_or(0);
        let ch = (8.0 + 17.0 + most as f32 * 40.0 + 8.0).max(140.0);
        let Some(r) = p.row(ch) else { self.bl_geo.clear(); return };
        self.bl_geo.clear();
        let dragging = self.bl.as_ref().filter(|b| b.ghost).map(|b| b.id.clone());
        for (k, side) in SIDES.iter().enumerate() {
            let col = R::new(r.x + k as f32 * (cw + 8.0), r.y, cw, ch);
            let over = self.bl.as_ref().is_some_and(|b| b.ghost && b.target.as_ref().is_some_and(|t| t.0 == *side));
            if over { p.c.rect(col, 17.0, p.col("primary")); p.c.rect(R::new(col.x + 2.0, col.y + 2.0, col.w - 4.0, col.h - 4.0), 15.0, p.col("surfaceContainer")); }
            else { p.c.rect(col, 17.0, p.col("surfaceContainer")); }
            let title = ["Left", "Center", "Right", "Hidden"][k];
            p.c.text(title, R::new(col.x + 12.0, col.y + 8.0, col.w - 16.0, 15.0), txt(12.0, 500), p.col("onSurfaceVariant"), Align::Left);
            let mut items = vec![];
            let mut y = col.y + 8.0 + 17.0 + 6.0;
            for (id, _) in lay.iter().filter(|m| m.1 == *side) {
                let ir = R::new(col.x + 8.0, y, col.w - 16.0, 34.0);
                y += 40.0;
                items.push((id.clone(), ir));
                let (label, ic) = info(id);
                let ghosted = dragging.as_deref() == Some(id.as_str());
                let a = if ghosted { 0.35 } else { 1.0 };
                p.c.rect(ir, 12.0, alpha(p.col("surfaceContainerHigh"), a));
                if self.bl_focus.as_deref() == Some(id.as_str()) {
                    p.c.rect(ir, 12.0, p.col("primary"));
                    p.c.rect(R::new(ir.x + 2.0, ir.y + 2.0, ir.w - 4.0, ir.h - 4.0), 10.0, p.col("surfaceContainerHigh"));
                }
                if ir.y + ir.h > p.lo && ir.y < p.hi {
                    let aid = p.act(Act::Do("bl", id.clone()));
                    p.c.hit(ir, aid);
                }
                p.c.icon("drag_indicator", R::new(ir.x + 4.0, ir.y, 16.0, 34.0), ico(16.0, false), alpha(p.col("onSurfaceVariant"), a));
                p.c.icon(ic, R::new(ir.x + 26.0, ir.y, 18.0, 34.0), ico(18.0, false), alpha(p.col("onSurface"), a));
                p.c.text(label, R::new(ir.x + 50.0, ir.y, ir.w - 56.0, 34.0), txt(13.0, 400), alpha(p.col("onSurface"), a), Align::Left);
            }
            if over {
                if let Some((_, idx)) = self.bl.as_ref().and_then(|b| b.target.clone()) {
                    let rest: Vec<&R> = items.iter().filter(|(id, _)| dragging.as_deref() != Some(id.as_str())).map(|(_, r)| r).collect();
                    let ly = rest.get(idx).map_or_else(|| rest.last().map_or(col.y + 8.0 + 17.0 + 3.0, |l| l.y + 37.0), |n| n.y - 3.0);
                    p.c.rect(R::new(col.x + 12.0, ly, col.w - 24.0, 3.0), 1.5, p.col("primary"));
                }
            }
            self.bl_geo.push((side.to_string(), col, items));
        }
    }
}

impl<'p, 'a> Pen<'p, 'a> {
    /// `.lbl` caption inside a card (12 px muted).
    pub fn lead_label(&mut self, s: &str) {
        if let Some(r) = self.row(12.0 * LH) { self.c.text(s, r, txt(12.0, 400), self.col("onSurfaceVariant"), Align::Left); }
    }
    /// Adds spacing before something that isn't a card child (`dy`).
    pub fn lead_gap(&mut self, dy: f32) { self.y += dy; }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions() {
        assert_eq!(cap("all"), "All");
        assert_eq!(cap("video"), "Video");
        assert_eq!(cap(""), "");
    }

    #[test]
    fn every_page_has_its_id() {
        assert_eq!(PAGES.iter().map(|p| p.0).collect::<Vec<_>>(), ["quick", "keys", "wallpapers", "presets", "bar", "island", "buddies", "services", "usage", "about"]);
        assert!(crate::settings::layout::BAR_MODULES.iter().all(|m| !m.2.is_empty()));
        assert_eq!(LANGS.len(), 49);
    }
}

#[cfg(test)]
mod page_header_tests {
    use super::*;

    fn buddies_height(saved: bool) -> f32 {
        use crate::{app::Ctx, gfx::{Canvas, Gfx}};
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&conf::pack_dir().join(r"assetsonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 800 * 800 * 4];
        let mut c = Canvas::new(&g, &mut px, 800, 800, 1.0, 0).unwrap();
        let mut cx = Ctx::new(false);
        if saved { cx.cfg = conf::Config::parse(r#"{"buddies": {"customEnabled": true, "a": {"character": "c1"}, "custom": [{"id": "c1", "name": "X"}]}}"#); }
        let mut st = Settings::new((800.0, 800.0)).on_page("buddies");
        let mut p = Pen::new(&mut c, &cx, 0.0, 0.0, 600.0, 0.0, 0.0);
        p.dry = true;
        st.buddies(&mut p);
        p.y
    }

    #[test]
    fn saved_buddy_row_only_takes_space_when_shown() {
        assert!(buddies_height(true) > buddies_height(false) + 30.0);
    }

    #[test]
    fn every_page_has_a_blurb() {
        for (key, _, _) in PAGES.iter() { assert!(!page_blurb(key).is_empty(), "{key}"); }
        assert_ne!(page_blurb("quick"), page_blurb("about"));
    }
}
