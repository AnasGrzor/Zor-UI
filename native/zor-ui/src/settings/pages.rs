//! The pages of settings.js, in the same order with the same labels and config keys.

use super::{
    form::{chip_w, Act, Fmt, Pen, Sl, ACT0, ROW_GAP},
    layout::{bar_layout, info, SIDES},
    Load, Settings,
};
use crate::{
    conf, keymap,
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
/// The visualizers the Settings preview can show.
#[derive(Clone, Copy)]
enum Viz { Island, Bar, Desktop }

/// Which visualizer a color setting paints (the shared color shows the bar).
fn viz_of(path: &str) -> Option<Viz> {
    match path { "island.eqColor" => Some(Viz::Island), "visualizer.barColor" | "visualizer.color" => Some(Viz::Bar), "visualizer.desktopColor" => Some(Viz::Desktop), _ => None }
}

/// Sample bars in `area` (they stand on its bottom edge), drawn with the code the real visualizer uses.
fn draw_viz_sample(c: &mut crate::gfx::Canvas, k: Viz, area: R, col: crate::gfx::Color) {
    match k {
        Viz::Island => crate::bar::draw_eq(c, area.x + (area.w - 15.0) / 2.0, area.y + area.h - 9.0, col, &crate::bar::EQ_SAMPLE),
        Viz::Bar => crate::bar::draw_viz(c, area.x + (area.w - 60.0) / 2.0, area.y + area.h - 22.0, col, &crate::viz::sample_levels(20)),
        Viz::Desktop => {
            let lv = crate::viz::sample_levels(28);
            let (bw, gp) = (area.w / lv.len() as f32 * 0.78, area.w / lv.len() as f32 * 0.22);
            for (j, v) in lv.iter().enumerate() {
                let (bh, cap) = crate::viz::bar_geometry(*v, (*v + 0.08).min(1.0), area.h);
                let x = area.x + j as f32 * (bw + gp);
                c.rect(R::new(x, area.y + area.h - bh, bw, bh), bw.min(4.0) / 2.0, alpha(col, 1.0 - 0.6 * bh / area.h));
                c.rect(R::new(x, area.y + area.h - cap, bw, 2.0), 0.0, col);
            }
        }
    }
}

/// The strip inside the color picker for config `path`: the visualizer it colors, in the color being picked.
/// `None` when the path is no visualizer color.
pub fn picker_strip(path: &str) -> Option<Box<dyn Fn(&mut crate::gfx::Canvas, R, crate::gfx::Color)>> {
    let k = viz_of(path)?;
    Some(Box::new(move |c, r, col| draw_viz_sample(c, k, R::new(r.x + 10.0, r.y + 6.0, r.w - 20.0, r.h - 12.0), col)))
}

const COLORS: [(&str, &str); 13] = [
    ("", "Character's own"), ("#16161D", "Black"), ("#F5F5F5", "White"), ("#B0B7C3", "Silver"), ("#D6303A", "Red"), ("#F48FB1", "Pink"), ("#F28C28", "Orange"),
    ("#F2D13A", "Yellow"), ("#4CAF50", "Green"), ("#26A69A", "Teal"), ("#4A7BE0", "Blue"), ("#8E5BD0", "Purple"), ("#6B4423", "Brown"),
];

const CLOCK_FORMATS: [&str; 7] = ["EEE, dd/MM '•' HH:mm", "HH:mm", "h:mm a", "EEE d MMM, HH:mm", "EEE d MMM, h:mm a", "dd/MM/yyyy HH:mm", "EEEE, d MMMM"];

static FONTS: OnceLock<Vec<String>> = OnceLock::new();

pub const PAGES: [(&str, &str, &str); 13] = [
    ("quick", "tune", "Quick"), ("keys", "keyboard", "Keybinds"), ("wallpapers", "animated_images", "Wallpapers"), ("presets", "palette", "Presets"),
    ("bar", "toolbar", "Bar & desktop"), ("island", "pill", "Dynamic Island"), ("buddies", "pets", "Buddies"), ("contacts", "contacts", "Contacts"),
    ("creator", "person_add", "Create a buddy"), ("voice", "mic", "Voice"), ("services", "hub", "Services"), ("usage", "memory", "Usage"), ("about", "info", "About"),
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
/// Grid sizes ("<rows>x<cols>") for `desktop.sizes.<id>`; "" is the card's own size.
const CARD_SIZES: [(&str, &str); 9] = [("", "Default"), ("1x1", "1 × 1"), ("1x2", "1 × 2"), ("1x3", "1 × 3"), ("2x1", "2 × 1"), ("2x2", "2 × 2"),
    ("2x3", "2 × 3"), ("3x2", "3 × 2"), ("3x3", "3 × 3")];
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
        "contacts" => "The people the buddy can message by voice",
        "creator" => "Turn a picture into a buddy that looks like your character",
        "voice" => "Talk to the buddy, what it may do for you, and the speech model",
        "services" => "API keys, models and sign-ins",
        "usage" => "What the AI tools have used",
        _ => "Version and updates",
    }
}

/// A speech model's line in the dropdown: its name, size on disk, and "download" when it isn't on this PC yet.
fn speech_option_label(name: &str, mb: u32, installed: bool) -> String {
    format!("{name} · {mb} MB{}", if installed { "" } else { " · download" })
}

fn opts(list: &[(&str, &str)]) -> Vec<(Value, String)> { list.iter().map(|(v, l)| (json!(v), l.to_string())).collect() }

/// The icon and name a contact's app is listed under. Any spelling of WhatsApp Web gets its own; unknown apps show as WhatsApp,
/// as they did before.
fn contact_app_label(app: &str) -> (&'static str, &'static str) {
    match app {
        "discord" => ("forum", "Discord"),
        "whatsappweb" | "whatsapp-web" | "whatsapp web" => ("language", "WhatsApp Web"),
        _ => ("chat", "WhatsApp"),
    }
}

/// The "Screen help model" choices: the sidebar's model first, then every model that can read images. One that can't is left
/// out, unless it is the one already chosen, so the picker never shows a blank.
fn screen_model_choices(models: &[(String, String)], current: &str) -> Vec<(Value, String)> {
    let mut v = vec![(json!(""), "Same as the sidebar's AI model".to_string())];
    v.extend(models.iter().filter(|(id, _)| id == current || crate::bridge::model_id_can_see(id)).map(|(id, name)| (json!(id), name.clone())));
    v
}
fn px(min: f64, max: f64, step: f64) -> Sl { Sl { min, max, step, fmt: Fmt::Px } }

/// One tile of the wallpaper / preset galleries.
pub struct Tile { pub img: Option<PathBuf>, pub badge: String, pub title: String, pub sub: String, pub busy: bool, pub icon: &'static str, pub act: Act, pub current: bool, pub link: Option<String> }

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
            "contacts" => self.contacts_page(p),
            "creator" => self.creator_page(p),
            "voice" => self.voice(p),
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

    /// The Contacts page: the people the buddy can message by voice.
    fn contacts_page(&mut self, p: &mut Pen) {
        p.h2("contacts", "People the buddy can message", "");
        p.card(|p| {
            self.contacts(p);
            p.note("Say \"message <name> saying ...\". The chat opens with the text ready and you press Enter to send. WhatsApp uses its app if it is installed, otherwise WhatsApp Web. Discord needs the user id (Developer Mode, right-click, Copy User ID).", 12.0);
        });
    }

    /// The people the buddy can message: one row each with edit and delete buttons, then the form that adds one or, after the
    /// pencil, saves changes to that one.
    fn contacts(&mut self, p: &mut Pen) {
        let list = self.garr(p.cx, "buddies.contacts");
        let editing = self.fields.get("contact-edit").map(|f| f.tb.text.clone()).unwrap_or_default();
        let people: Vec<(String, (String, String, String))> = list.into_iter().filter_map(|e| crate::assist::parse_contact(&e).map(|c| (e, c))).collect();
        if people.is_empty() { p.note("No contacts yet. Add one below, then say \"message <name> saying ...\".", 12.0); }
        for (entry, (name, app, value)) in people {
            let Some(r) = p.row(44.0) else { continue };
            let (ic, app_name) = contact_app_label(&app);
            p.left(r, ic, &name, &format!("{app_name}  \u{2022}  {value}"), 80.0);
            let id = p.act(Act::Do("edit-contact", entry.clone()));
            ui::icon_btn(p.c, p.cx, R::new(r.x + r.w - 76.0, r.y + 4.0, 36.0, 36.0), id, "edit", IconKind::Plain, 20.0, entry == editing); // filled while it is the one being edited
            let id = p.act(Act::Do("del-contact", entry));
            ui::icon_btn(p.c, p.cx, R::new(r.x + r.w - 36.0, r.y + 4.0, 36.0, 36.0), id, "delete", IconKind::Plain, 20.0, false);
        }
        if !editing.is_empty() {
            if let Some(r) = p.row(44.0) {
                let who = crate::assist::parse_contact(&editing).map(|c| c.0).unwrap_or_default();
                p.left(r, "edit", &format!("Editing {who}"), "Change anything below, then save", 100.0);
                p.chip_right(r, None, "Cancel", Act::Do("cancel-edit", String::new()));
            }
        }
        let app = match self.fields.get("contact-app").map(|f| f.tb.text.as_str()) { Some("discord") => "discord", Some("whatsappweb") => "whatsappweb", _ => "whatsapp" };
        if let Some(r) = p.row(44.0) {
            p.left(r, "person_add", "Name", "What you say to the buddy", 240.0);
            self.field_ui(p, "contact-name", None, super::Kind::Plain, R::new(r.x + r.w - 240.0, r.y + 2.0, 240.0, 40.0), "Ali", 17.0, false);
        }
        if let Some(r) = p.row(44.0) {
            p.left(r, "apps", "App", "", 340.0);
            let mut x = r.x + r.w - 340.0;
            for (key, label) in [("whatsapp", "WhatsApp"), ("whatsappweb", "WhatsApp Web"), ("discord", "Discord")] {
                let id = p.act(Act::Do("contact-app", key.into()));
                x += ui::chip(p.c, p.cx, x, r.y + 6.0, id, None, label, app == key).w + 8.0;
            }
        }
        let (sub, ph) = match app {
            "discord" => ("User id: Developer Mode, right-click, Copy User ID", "1234567890"),
            "whatsappweb" => ("Number with country code, opens in your browser", "+923001234567"),
            _ => ("Phone number with country code", "+923001234567"),
        };
        if let Some(r) = p.row(44.0) {
            p.left(r, "tag", if app == "discord" { "User id" } else { "Number" }, sub, 240.0);
            self.field_ui(p, "contact-value", None, super::Kind::Plain, R::new(r.x + r.w - 240.0, r.y + 2.0, 240.0, 40.0), ph, 17.0, false);
        }
        if let Some(r) = p.row(44.0) {
            let (ic, title, button) = if editing.is_empty() { ("add_circle", "Add this contact", "Add") } else { ("save", "Save changes", "Save") };
            p.left(r, ic, title, "", 80.0);
            p.chip_right(r, None, button, Act::Do("add-contact", String::new()));
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
            self.sw(p, "island.privacy", "Mic and camera in use", "An orange or green dot while an app listens or films, and a note when one starts", "videocam");
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
            self.sw(p, "island.eq", "Equalizer", "Moving bars next to the song while it plays", "equalizer");
            self.color(p, "island.eqColor", "Equalizer color", "Empty follows the visualizer color, then the theme", "palette");
            self.viz_preview(p, &[Viz::Island]);
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
                // while the picker is open the field holds the picked colour, so it must not be reset from theme.json
                let init = self.picker.is_none().then(|| self.info.seed.clone());
                self.field_ui(p, "seed", init.as_deref(), super::Kind::Plain, fr, "#6750a4", 12.0, false);
                let col = conf::parse_hex(hex.trim()).unwrap_or(0xFF67_50A4);
                let sw = R::new(fr.x - 8.0 - 48.0, r.y + 2.0, 48.0, 36.0);
                p.c.rect(sw, 6.0, col);
                let id = p.act(Act::Do("picker", String::new()));
                if p.c.hit(sw, id) { p.c.rect(sw, 6.0, alpha(p.col("onSurface"), 0.12)); }
            }
            if !self.busy.is_empty() { p.spinner_block(); }
        });
        let merged = self.monitor_wallpapers(p);
        if let (false, Load::Ready(mons)) = (merged, &self.mons) {
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
            self.sw(p, "runOnStartup", "Start Zor UI and GlazeWM at login", "Turn off to start them yourself. Takes effect at the next login", "power_settings_new");
        });
    }

    /// One card row per monitor: its picture drawn in that monitor's own shape inside a fixed area, its name, and
    /// buttons; then its brightness slider. The picture is the file the bridge cut with `focus_crop` and handed to
    /// Windows, so the preview can't differ from the real thing. False when there is nothing to show (one monitor),
    /// so the caller keeps the plain brightness list.
    fn monitor_wallpapers(&mut self, p: &mut Pen) -> bool {
        let Load::Ready(list) = &self.wmons else { return false };
        let list = list.clone();
        if list.len() < 2 { return false; }
        let bright: Vec<(String, String)> = match &self.mons { Load::Ready(m) => m.iter().filter_map(|s| s.split_once('|')).map(|(i, n)| (n.to_string(), i.to_string())).collect(), _ => vec![] };
        p.h2("desktop_windows", "Monitors", "");
        p.card(|p| {
            for m in &list {
                let (i, w, h) = (m["i"].as_u64().unwrap_or(0), m["w"].as_f64().unwrap_or(1920.0) as f32, m["h"].as_f64().unwrap_or(1080.0) as f32);
                let custom = m["custom"] == true;
                let name = m["name"].as_str().unwrap_or("").to_string();
                // the slider below must run even when this row is off screen (or in the measuring pass), or the card is measured short
                if let Some(r) = p.row(PREVIEW_H + 24.0) {
                let (bw, bh) = monitor_box(w, h);
                let b = R::new(r.x + 12.0 + (PREVIEW_W - bw) / 2.0, r.y + 12.0 + (PREVIEW_H - bh) / 2.0, bw, bh);
                p.c.rect(b, 10.0, p.col("surfaceContainer"));
                self.picture(p, std::path::Path::new(m["file"].as_str().unwrap_or("")), b, 10.0);
                let (title, sub) = monitor_caption(&name, i as usize, w as i32, h as i32, custom);
                let tx = r.x + 12.0 + PREVIEW_W + 20.0;
                let tw = r.w - (tx - r.x) - 12.0;
                let ty = r.y + (PREVIEW_H + 24.0 - 82.0) / 2.0;
                p.c.text(&title, R::new(tx, ty, tw, 22.0), txt(15.0, 500), p.col("onSurface"), Align::Left);
                p.c.text(&sub, R::new(tx, ty + 24.0, tw, 16.0), txt(12.0, 400), p.col("onSurfaceVariant"), Align::Left);
                let w1 = chip_w(p.c, "Choose", true);
                let a = p.act(Act::Do("mon-wall", i.to_string()));
                ui::chip(p.c, p.cx, tx, ty + 50.0, a, Some("folder_open"), "Choose", false);
                if custom {
                    let a = p.act(Act::Do("mon-clear", i.to_string()));
                    ui::chip(p.c, p.cx, tx + w1 + 12.0, ty + 50.0, a, Some("restart_alt"), "Use main", false);
                }
                }
                if let Some((_, bi)) = bright.iter().find(|(n, _)| *n == name) {
                    self.slider(p, &format!("monitor.{bi}"), "Brightness", Sl { min: 1.0, max: 100.0, step: 1.0, fmt: Fmt::PctInt }, "brightness_6");
                }
            }
            p.note("A picture that is not the shape of its monitor is cut to fit, keeping the busiest part.", 12.0);
        });
        true
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
                        busy: self.busy == id, icon: if c { "check" } else { "wallpaper" }, act: Act::Do("we-apply", id), current: c, link: None }
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
        p.h2("public", "Community gallery", "github.com/pctrade/end4-pCpresets");        p.card(|p| {
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
                            busy: self.busy == name, icon: "download", link: Some(super::store::preset_url(&name)), act: Act::Do("apply-online", name), current: false }
                    }).collect();
                    self.grid(p, tiles);
                }
                _ => p.spinner_block(),
            }
        });
    }

    /// A sample card in the current glass settings over your wallpaper; follows a slider while it is dragged.
    fn glass_preview(&mut self, p: &mut Pen) {
        const H: f32 = 150.0;
        let Some(r) = p.row(H + 16.0) else { return };
        let v = |s: &Self, k: &str| { let path = format!("desktop.glass.{k}"); match &s.drag_val { Some((q, x)) if *q == path => *x, _ => s.gn(p.cx, &path) } };
        let tint = crate::glass::tint_rgb(&self.gs(p.cx, "desktop.glass.tint"));
        let g = crate::glass::Glass { frost: v(self, "frost") as f32, haze: v(self, "haze") as f32, pull: v(self, "pull") as f32, rim: v(self, "rimWidth") as f32, sheen: v(self, "sheen") as f32,
            split: v(self, "split") as f32, tint: tint.unwrap_or((0.0, 0.0, 0.0)), tint_strength: if tint.is_some() { v(self, "tintStrength") as f32 } else { 0.0 }, vivid: v(self, "vividness") as f32 };
        let Some(img) = p.cx.state.v["image"].as_str().filter(|s| !s.is_empty()).map(std::path::PathBuf::from) else { return p.note("Pick a wallpaper picture to see the preview.", 12.0) };
        let (box_, k) = (R::new(r.x + 12.0, r.y + 8.0, r.w - 24.0, H), p.c.scale);
        let (w, h) = ((box_.w * k).round() as u32, (H * k).round() as u32);
        let key = format!("{g:?}|{img:?}|{w}x{h}");
        if self.glass_prev.as_ref().map_or(true, |(c, _)| *c != key) {
            // the face area of a 1920x1080 wallpaper, where a card preview is most telling
            let Some(mut pic) = crate::glass::backdrop(p.c.g, &img, (0, 0, 1920, 1080), (900, 250, w, h)) else { return };
            let card = (w as f32 * 0.2, h as f32 * 0.15, w as f32 * 0.6, h as f32 * 0.7, 23.0 * k);
            let Some(glass) = crate::glass::render(&pic, &[card], &g, k) else { return };
            // lay the glass over the plain picture so the preview is one image
            if let Some(mut m) = tiny_skia::PixmapMut::from_bytes(pic.data_mut(), w, h) {
                m.draw_pixmap(0, 0, glass.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
            }
            self.glass_prev = Some((key, pic));
        }
        if let Some((_, pic)) = &self.glass_prev { p.c.clip_round(box_, 17.0, |c| c.image(pic, box_, 1.0)); }
        let card = R::new(box_.x + box_.w * 0.2, box_.y + H * 0.15, box_.w * 0.6, H * 0.7);
        p.c.text("27\u{b0}C", R::new(card.x + 18.0, card.y + 10.0, card.w - 36.0, 38.0), txt(30.0, 500), 0xFFFFFFFF, Align::Left);
        p.c.text("Karachi  \u{2022}  clear sky", R::new(card.x + 18.0, card.y + 52.0, card.w - 36.0, 18.0), txt(13.0, 400), 0xFFFFFFFF, Align::Left);
    }

    /// Sample bars, drawn the way each visualizer draws them, in the color its setting gives right now.
    fn viz_preview(&mut self, p: &mut Pen, which: &[Viz]) {
        const H: f32 = 76.0;
        let Some(r) = p.row(H + 12.0) else { return };
        let gap = 10.0;
        let w = (r.w - 24.0 - gap * (which.len() - 1) as f32) / which.len() as f32;
        for (i, k) in which.iter().enumerate() {
            let (label, own) = match k { Viz::Island => ("Island", "island.eqColor"), Viz::Bar => ("Bar", "visualizer.barColor"), Viz::Desktop => ("Desktop", "visualizer.desktopColor") };
            let col = self.viz_color(p.cx, own);
            let b = R::new(r.x + 12.0 + i as f32 * (w + gap), r.y + 6.0, w, H);
            p.c.rect(b, 12.0, p.col("surfaceContainer"));
            p.c.text(label, R::new(b.x + 10.0, b.y + 6.0, b.w - 20.0, 14.0), txt(11.0, 400), p.col("onSurfaceVariant"), Align::Left);
            let area = R::new(b.x + 10.0, b.y + 24.0, b.w - 20.0, H - 34.0); // bars stand on its bottom edge
            draw_viz_sample(p.c, *k, area, col);
        }
    }

    /// Sample tray icons as the bar draws them: the first N and the chevron that holds the rest.
    fn tray_preview(&mut self, p: &mut Pen) {
        const H: f32 = 56.0;
        let Some(r) = p.row(H + 12.0) else { return };
        let b = R::new(r.x + 12.0, r.y + 6.0, r.w - 24.0, H);
        p.c.rect(b, 12.0, p.col("surfaceContainerHigh"));
        p.c.text("Bar", R::new(b.x + 10.0, b.y + 6.0, b.w - 20.0, 14.0), txt(11.0, 400), p.col("onSurfaceVariant"), Align::Left);
        let edge = crate::bar::Edge::parse(&self.gs(p.cx, "bar.position"));
        let visible = self.gn(p.cx, "bar.trayVisible").max(0.0) as usize;
        crate::bar::draw_tray_sample(p.c, p.cx, b.x + 10.0, b.y + 36.0, visible, edge);
    }

    /// Sample bar groups at the chosen spacing, drawn by the bar's own group code. Follows the style and group settings above it.
    fn spacing_preview(&mut self, p: &mut Pen) {
        const H: f32 = 64.0;
        let Some(r) = p.row(H + 12.0) else { return };
        let b = R::new(r.x + 12.0, r.y + 6.0, r.w - 24.0, H);
        p.c.rect(b, 12.0, p.col("surfaceContainerHigh"));
        p.c.text("Bar", R::new(b.x + 10.0, b.y + 6.0, b.w - 20.0, 14.0), txt(11.0, 400), p.col("onSurfaceVariant"), Align::Left);
        let spacing = self.gs(p.cx, "bar.spacing");
        let (style, groups) = (self.gs(p.cx, "bar.style"), self.gs(p.cx, "bar.groups"));
        crate::bar::draw_spacing_sample(p.c, p.cx, b.x + 10.0, b.y + 40.0, &spacing, &style, &groups);
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
            self.num(p, "bar.trayVisible", "Tray icons shown (0 = all)", 0, 12, "keyboard_arrow_up");
            self.tray_preview(p);
            self.sw(p, "visualizer.bar", "Visualizer in the bar", "Small cava-style bars next to the media", "graphic_eq");
            if self.gb(p.cx, "visualizer.bar") {
                self.color(p, "visualizer.barColor", "Bar visualizer color", "Empty follows the visualizer color", "palette");
                self.viz_preview(p, &[Viz::Bar]);
            }
        });
        p.h2("dashboard_customize", "Bar layout", "");
        p.card(|p| {
            self.seg(p, "bar.position", &[("top", "Top"), ("bottom", "Bottom"), ("left", "Left"), ("right", "Right")], "Bar position", "vertical_align_top");
            self.sw(p, "bar.hideFullscreen", "Hide in full screen", "The bar hides on a monitor while an app (a game, a video) covers it", "fullscreen");
            p.note("Side bars show icons and short text only (no window title). Moving the bar frees or takes screen space for windows after zorui restarts.", 12.0);
            self.seg(p, "bar.style", &[("float", "Float"), ("flat", "Flat"), ("islands", "Islands"), ("clear", "Clear")], "Bar style", "style");
            self.seg(p, "bar.groups", &[("filled", "Filled"), ("outlined", "Outlined"), ("none", "None")], "Group backgrounds", "rounded_corner");
            self.seg(p, "bar.spacing", &[("tight", "Tight"), ("cozy", "Cozy"), ("roomy", "Roomy")], "Space between groups", "space_bar");
            self.spacing_preview(p);
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
            self.sw(p, "desktop.glass.enable", "Glass cards", "Cards show the wallpaper behind them, blurred and bent at the rim. A preset with glass turns this on", "blur_on");
            if self.get(p.cx, "desktop.glass.enable") == json!(true) {
                self.slider(p, "desktop.glass.haze", "Glass blur", px(0.0, 20.0, 1.0), "blur_on");
                self.slider(p, "desktop.glass.pull", "Glass bend at the edge", Sl { min: 0.0, max: 100.0, step: 5.0, fmt: Fmt::PctInt }, "lens_blur");
                self.slider(p, "desktop.glass.rimWidth", "Glass edge width", px(4.0, 40.0, 1.0), "border_outer");
                self.slider(p, "desktop.glass.sheen", "Glass highlight", Sl { min: 0.0, max: 1.0, step: 0.05, fmt: Fmt::Pct }, "light_mode");
                self.slider(p, "desktop.glass.split", "Glass colour fringe", Sl { min: 0.0, max: 0.3, step: 0.01, fmt: Fmt::Pct }, "gradient");
                self.slider(p, "desktop.glass.frost", "Glass frosting", Sl { min: 0.0, max: 0.2, step: 0.01, fmt: Fmt::Pct }, "ac_unit");
                self.slider(p, "desktop.glass.vividness", "Glass colour strength", Sl { min: 0.5, max: 2.0, step: 0.05, fmt: Fmt::Pct }, "palette");
                self.seg(p, "desktop.glass.tint", &[("transparent", "No tint"), ("black", "Dark"), ("white", "Light")], "Glass tint", "contrast");
                self.slider(p, "desktop.glass.tintStrength", "Glass tint strength", Sl { min: 0.0, max: 1.0, step: 0.05, fmt: Fmt::Pct }, "opacity");
                self.glass_preview(p);
            }
            self.sw(p, "desktop.snap", "Snap cards to each other", "Dragged cards line up with other cards' edges and centres. Hold Alt to drag freely", "grid_on");
            self.sw(p, "desktop.snapLines", "Snapping guide lines", "Show a line where a dragged card lines up with another. Snapping still works with this off", "straighten");
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
            self.color(p, "visualizer.color", "Visualizer color", "Every visualizer, unless one has its own. Empty follows the theme", "palette");
            self.color(p, "visualizer.desktopColor", "Desktop visualizer color", "Empty follows the visualizer color", "palette");
            self.viz_preview(p, &[Viz::Island, Viz::Bar, Viz::Desktop]);
            self.text(p, "userName", "Your name", "Shown in the greeting and sidebar", "person");
        });
        p.h2("aspect_ratio", "Card sizes", "");
        p.card(|p| {
            for id in crate::desk::SIZED {
                let name = DESKTOP_WIDGETS.iter().find(|w| w.0 == id).map_or(id, |w| w.1);
                if let Some(r) = p.row(40.0) {
                    p.left(r, "aspect_ratio", name, "", 130.0);
                    self.select_box(p, &format!("desktop.sizes.{id}"), opts(&CARD_SIZES), R::new(r.x + r.w - 130.0, r.y, 130.0, 40.0));
                }
            }
            p.note("Size in grid cells, rows by columns. The other cards fit their content. Media only takes the size while something plays.", 12.0);
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

    /// Buddy `k` ("a" or "b") as the bar draws him, standing, walking and sitting, with the options as they are set right now
    /// (before config.json catches up). Drawn by `buddy::draw_preview`, the bar's own code, so it can't drift from the real thing.
    fn buddy_preview(&mut self, p: &mut Pen, k: &str) {
        if p.dry { p.row(stances_h(64.0) + 12.0); return; }
        let (custom, list) = (self.gb(p.cx, "buddies.customEnabled"), self.get(p.cx, "buddies.custom"));
        let ap = crate::buddy::appearance(&self.gs(p.cx, &format!("buddies.{k}.character")), custom, &list, &|f| self.gs(p.cx, &format!("buddies.{k}.{f}")), None);
        let shown = buddy_label(&self.gs(p.cx, &format!("buddies.{k}.name")), ap.cast.label).to_string();
        stances_box(p, &format!("On the bar: {shown}"), Some((&ap, &mut self.buddy_sprites[usize::from(k == "b")])), "", 64.0);
    }

    /// Create a buddy: one picture in, a pack folder out (see creator.rs). The preview comes first and follows every option.
    fn creator_page(&mut self, p: &mut Pen) {
        use crate::creator;
        p.h2("person_add", "Create a buddy", "");
        let image = self.gs(p.cx, "buddies.make.image");
        let clean = self.creator_opts(p.cx);
        // the clean-up runs only when the picture or an option changed, and only when the page is drawn for real
        if !p.dry && !image.is_empty() {
            creator::refresh(&mut self.creator_prev, p.c.g, &conf::pack_dir().join("images").join(&image), &clean.key(&image), &clean, &creator::preview_dir());
        }
        p.card(|p| {
            const MAX_H: f32 = 100.0;
            if p.dry { p.row(stances_h(MAX_H) + 12.0); return; }
            let prev = self.creator_prev.as_ref().filter(|v| !image.is_empty() && v.key == clean.key(&image));
            match prev.and_then(|v| v.pack.clone()) {
                Some(mut pk) => {
                    pk.height = (self.gn(p.cx, "buddies.make.height") as f32).clamp(crate::pack::MIN_H, crate::pack::MAX_H);
                    let ap = crate::buddy::Appearance { cast: crate::cast::from_pack(&pk), style: crate::cast::Hair::default(), trim: crate::cast::Trim::default(), pack: Some(std::sync::Arc::new(pk)) };
                    stances_box(p, "How he will look on the bar", Some((&ap, &mut self.creator_sprites)), "", MAX_H);
                }
                None => {
                    let msg = if image.is_empty() { "Choose a picture to see your buddy here".to_string() } else { prev.map_or("Working on it...".to_string(), |v| v.error.clone()) };
                    stances_box(p, "How he will look on the bar", None, &msg, MAX_H);
                }
            }
        });
        p.h2("image", "1. Picture", "");
        p.card(|p| {
            if let Some(r) = p.row(44.0) {
                let (w1, w2) = (chip_w(p.c, "Choose", true), chip_w(p.c, "Clear", true));
                p.left(r, "image", "Your picture", if image.is_empty() { "A PNG, JPG or WebP of the character. A plain background is fine, step 2 removes it" } else { "Picture chosen" }, w1 + w2 + 24.0);
                let mut x = r.x + r.w;
                if !image.is_empty() { x -= w2; let a = p.act(Act::Do("clear-buddy-image", "make".into())); ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("close"), "Clear", false); x -= 12.0; }
                x -= w1; let a = p.act(Act::Do("pick-image", format!("buddy-make-{}", crate::hub::now_ms() as i64)));
                ui::chip(p.c, p.cx, x, r.y + 4.0, a, Some("folder_open"), "Choose", false);
            }
        });
        p.h2("auto_fix_high", "2. Clean up", "");
        p.card(|p| {
            self.num(p, "buddies.make.clean", "Remove background", 0, creator::MAX_CLEAN, "ink_eraser");
            p.note("0 leaves the picture as it is. Raise it until the background is gone: it removes every pixel joined to a corner by a similar color, and keeps anything inside the character. A picture that is already transparent needs 0.", 12.0);
            self.sw(p, "buddies.make.trim", "Cut off the empty edges", "So his feet stand right on the bar", "crop");
            self.sw(p, "buddies.make.flip", "Mirror the picture", "Buddies face right. Switch this on if your picture faces left", "flip");
        });
        p.h2("height", "3. Size and name", "");
        p.card(|p| {
            self.num(p, "buddies.make.height", "Height on the bar", crate::pack::MIN_H as i64, crate::pack::MAX_H as i64, "height");
            if self.gn(p.cx, "buddies.make.height") > 54.0 { p.note("On a top bar a buddy is at most 54 px tall, because that is the room above the bar. On a bottom or side bar the full height shows.", 12.0); }
            self.text_ph(p, "buddies.make.name", "Name", "Shown in the Character list", "badge", "My buddy");
        });
        p.h2("record_voice_over", "4. Lines and moves", "");
        p.card(|p| {
            let mut bases = vec![(json!(""), "Neutral words".to_string())];
            bases.extend(crate::cast::CASTS.iter().map(|c| (json!(c.id), c.label.to_string())));
            self.pick(p, "buddies.make.base", bases, "Speaks like", "Whose lines and moves it borrows. Edit its buddy.json afterwards to write your own", "face", 190.0);
            self.pick(p, "buddies.make.special", opts(&[("orbs", "Orbs (Hollow Purple)"), ("beam", "Beam"), ("aura", "Aura"), ("none", "None")]), "Double-click move", "", "bolt", 190.0);
        });
        p.h2("save", "Save", "");
        p.card(|p| {
            if let Some(r) = p.row(44.0) {
                let w = chip_w(p.c, "Save buddy", true);
                p.left(r, "save", "Save as a buddy", "Makes a folder in the packs folder. It then shows up in Buddies > Character", w + 12.0);
                p.chip_right(r, Some("save"), "Save buddy", Act::Do("make-buddy", String::new()));
            }
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
                self.buddy_preview(p, k);
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
                let (w1, w2) = (chip_w(p.c, "Choose", true), chip_w(p.c, "Clear", true));
                let sub = if img.is_empty() { "A PNG with a transparent background replaces the drawn figure" } else { "Picture chosen. It replaces the drawn figure" };
                if let Some(r) = p.row_sub(40.0, "image", sub, 130.0 + w1 + w2 + 24.0) {
                    p.left(r, "image", "Use a picture", sub, 130.0 + w1 + w2 + 24.0);
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
        p.h2("folder", "Buddy packs", "");
        p.card(|p| {
            let w = chip_w(p.c, "Open folder", true);
            let sub = "A folder with a buddy.json and your pictures makes a character. See the README";
            if let Some(r) = p.row_sub(44.0, "folder_open", sub, w + 12.0) {
                p.left(r, "folder_open", "Packs folder", sub, w + 12.0);
                p.chip_right(r, Some("folder_open"), "Open folder", Act::Do("open-buddies", String::new()));
            }
            let found = crate::pack::scan();
            for (id, res) in &found {
                if let Some(r) = p.row(44.0) {
                    match res { Ok(pk) => p.left(r, "face", &pk.name, &pk.summary(), 0.0), Err(e) => p.left(r, "error", &format!("{id}: not used"), e, 0.0) }
                }
            }
            if found.is_empty() { p.note("No packs yet. Each folder in the packs folder shows up in the Character list above once its buddy.json is valid.", 12.0); }
        });
        p.h2("favorite", "Partners", "");
        p.card(|p| {
            self.sw(p, "buddies.pair", "Make them partners", "With both on, they walk to each other, face each other and trade lines", "group");
            self.seg(p, "buddies.primary", &[("a", "Buddy 1"), ("b", "Buddy 2")], "Primary buddy", "star");
            self.sw(p, "buddies.breaks", "Remind me to take breaks", "The primary buddy speaks up after a long stretch of work", "coffee");
            self.num(p, "buddies.breakMins", "Minutes of work between breaks", 1, 240, "timer");
            self.sw(p, "buddies.watch", "Comment on what I'm doing", "The primary buddy reacts to the app in front, like an editor, a video or a game", "visibility");
            p.note("Click a buddy to hear them speak, double-click to see their move, and drag to reposition them. When two buddies are enabled, only the primary buddy comments on the desktop and reminds you to take breaks. Turn on Quiet to mute a buddy's comments and chat.", 12.0);
        });
    }

    /// The speech model dropdown (Automatic, Windows' own, each Parakeet with its size, any other model folder found), with its
    /// description under it. Picking a Parakeet that isn't downloaded starts the download. When the chosen one is a Parakeet,
    /// a row below shows the download's progress with Cancel, or offers Delete (or Download again after a failure).
    fn speech_model(&mut self, p: &mut Pen) {
        use crate::bridge::listen::{progress, MODELS};
        let cur = self.gs(p.cx, "buddies.model");
        let paths: Vec<String> = MODELS.iter().map(|m| m.path().to_string_lossy().into_owned()).collect();
        let sel = MODELS.iter().zip(&paths).find(|(_, path)| **path == cur).map(|(m, _)| m);
        let mut opts = vec![(json!(""), "Automatic".to_string()), (json!("windows"), "Windows (built in)".to_string())];
        opts.extend(MODELS.iter().zip(&paths).map(|(m, path)| (json!(path), speech_option_label(m.name, m.mb, m.installed()))));
        opts.extend(crate::assist::speech_models().into_iter().filter(|(d, _)| !paths.contains(d)).map(|(d, n)| (json!(d), n)));
        let note = match (sel, cur.as_str()) {
            (Some(m), _) => m.note,
            (None, "windows") => "Needs no download. Less accurate than Parakeet",
            (None, "") => "Uses the first Parakeet model you have, otherwise Windows' own",
            _ => "A model folder you added",
        };
        self.pick(p, "buddies.model", opts, "Speech model", note, "model_training", 360.0);
        let Some(m) = sel else { return };
        let busy = self.busy == format!("speech-dl:{}", m.id);
        let (frac, status) = progress();
        let (sub, ic, label, act) = if busy { (status, "close", "Cancel", Act::Do("cancel-speech", String::new())) }
            else if m.installed() { (format!("On disk, {} MB. Deleting it switches back to Automatic", m.mb), "delete", "Delete", Act::Do("delete-speech", m.id.to_string())) }
            else { (format!("Not downloaded yet, {} MB", m.mb), "download", "Download", Act::Do("download-speech", m.id.to_string())) };
        let w = chip_w(p.c, "Download", true) + 12.0;
        if let Some(r) = p.row_sub(44.0, "storage", &sub, w) {
            p.left(r, "storage", "Model files", &sub, w);
            p.chip_right(r, Some(ic), label, act);
        }
        if busy {
            if let Some(r) = p.row(8.0) {
                p.c.rect(R::new(r.x, r.y + 2.0, r.w, 4.0), 2.0, p.col("secondaryContainer"));
                p.c.rect(R::new(r.x, r.y + 2.0, r.w * frac, 4.0), 2.0, p.col("primary"));
            }
        }
    }

    fn voice(&mut self, p: &mut Pen) {
        p.h2("mic", "Voice commands", "");
        p.card(|p| {
            self.sw(p, "buddies.listen", "Talk to the primary buddy", "The hotkey (default Alt+B) records you and the buddy acts on what you say", "mic");
            self.seg(p, "buddies.listenMode", &[("auto", "Auto"), ("toggle", "Toggle"), ("hold", "Hold")], "Dictation mode", "keyboard_voice");
            p.note("Auto stops when you stop talking. Toggle records until you press the hotkey again. Hold records while you hold the hotkey.", 12.0);
            self.sw(p, "buddies.allow.search", "Search the web", "\"search for ...\" opens a Google search in your browser", "search");
            self.sw(p, "buddies.allow.open", "Open apps and games", "\"open <name>\" starts a Start Menu app or a Steam game", "rocket_launch");
            self.sw(p, "buddies.allow.message", "Start messages", "\"message <name> saying ...\" opens the chat with the text ready. You press Enter to send", "chat");
            self.sw(p, "buddies.allow.media", "Control the player", "The AI may pause, resume or skip the current song. \"play ...\" opens a music search and is covered by Open apps and games", "play_arrow");
            self.sw(p, "buddies.allow.system", "Control the PC", "The AI may change volume, brightness and the wallpaper, switch Wi-Fi or Bluetooth, copy text to the clipboard, and lock the PC. Sleep, restart, shut down and sign out always ask first", "tune");
            self.sw(p, "buddies.listenAi", "Ask the AI for other requests", "Sends what you said to the sidebar's AI model. Off: only the phrases above work", "auto_awesome");
            self.sw(p, "buddies.screen", "Let the buddy see my screen", "\"where is ...\" or \"what's on my screen\" sends a picture of each monitor to the AI. Off: nothing is captured", "screenshot_monitor");
            if self.gb(p.cx, "buddies.screen") {
                // only models that can read images are offered; blank follows the sidebar's AI model
                let current = self.gs(p.cx, "buddies.screenModel");
                let vision = screen_model_choices(match &self.models { Load::Ready(m) => m.as_slice(), _ => &[] }, &current);
                self.pick(p, "buddies.screenModel", vision, "Screen help model", "Models that can't read images are hidden", "model_training", 260.0);
            }
            self.sw(p, "buddies.drive", "Let the buddy take over", "\"take over and ...\" or \"click ...\": the AI moves the mouse and types for one task", "smart_toy");
            if self.gb(p.cx, "buddies.drive") {
                p.note("After you say yes, the AI uses the screen help model. Esc, your mouse or your voice stops it. Needs screen help.", 12.0);
                self.num(p, "buddies.driveRounds", "Most steps per take-over", 1, 100, "format_list_numbered");
                self.num(p, "buddies.driveSecs", "Most seconds per take-over", 10, 900, "timer");
            }
            self.sw(p, "buddies.browser", "Let the buddy use a browser", "For tasks on websites: the AI clicks and types in its own Edge window, and the pages' text goes to the AI. Needs Node.js", "language");
            if self.gb(p.cx, "buddies.browser") {
                p.note("It asks before the first step of each request, and before anything that sends, buys or deletes. It has its own logins: log in to a site once in its window. Passwords are yours to type.", 12.0);
            }
            self.sw(p, "native.gojoVoice", "Speak replies aloud", "Uses Windows' own voice", "volume_up");
            let mut mics = vec![(json!(""), "Windows default".to_string())];
            mics.extend(crate::assist::mics().into_iter().map(|m| (json!(m), m)));
            self.pick(p, "buddies.mic", mics, "Microphone", "The one the buddy listens with", "mic", 260.0);
            self.speech_model(p);
            let w = chip_w(p.c, "Open folder", true);
            let sub = "Put another model folder (the one with tokens.txt) here and it shows up in the list above";
            if let Some(r) = p.row_sub(44.0, "folder_open", sub, w + 12.0) {
                p.left(r, "folder_open", "Speech models folder", sub, w + 12.0);
                p.chip_right(r, Some("folder_open"), "Open folder", Act::Do("open-speech", String::new()));
            }
            p.note("The people you can message are on the Contacts page. Voice needs the hotkey in GlazeWM's config; re-run install.ps1 to add it.", 12.0);
            p.note("Talk to the primary buddy with the hotkey (default Alt+B). Say \"search for ...\", \"open ...\" with an app or game name, or \"message ... saying ...\" with a contact name. Contact names come from buddies.contacts in config.json. The message opens in the app, and you press Enter to send it.", 12.0);
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
        // saved Claude Code logins; Idle means Claude Code isn't installed. No look setting here, so no preview.
        if !matches!(self.claude, Load::Idle) {
            let rows = match &self.claude { Load::Ready(v) => v["accounts"].as_array().cloned().unwrap_or_default(), _ => vec![] };
            p.h2("manage_accounts", "Claude accounts", "");
            p.card(|p| {
                for a in &rows {
                    let Some(r) = p.row(44.0) else { continue };
                    let n = a["n"].as_u64().unwrap_or(0).to_string();
                    let wsw = chip_w(p.c, "Switch", false);
                    p.left(r, "person", a["email"].as_str().unwrap_or(""), &crate::bridge::claude_acct::summary(a), wsw + 48.0);
                    let act = p.act(Act::Do("claude-remove", n.clone()));
                    ui::icon_btn(p.c, p.cx, R::new(r.x + r.w - 36.0, r.y + 4.0, 36.0, 36.0), act, "delete", IconKind::Plain, 20.0, false);
                    let active = a["active"] == true;
                    let act = p.act(if active { Act::Do("noop", String::new()) } else { Act::Do("claude-switch", n) });
                    ui::chip(p.c, p.cx, r.x + r.w - 48.0 - wsw, r.y + 8.0, act, None, if active { "Active" } else { "Switch" }, active);
                }
                self.sw(p, "claude.autoSwitch", "Switch automatically", "When the active login nears its limit, the next one with room takes over. A banner gives you 20 seconds to click it and cancel", "sync_alt");
                if self.get(p.cx, "claude.autoSwitch") == true {
                    self.slider(p, "claude.threshold", "Switch at", Sl { min: 50.0, max: 100.0, step: 5.0, fmt: Fmt::PctInt }, "speed");
                }
                if let Some(r) = p.row(44.0) {
                    let wsave = chip_w(p.c, "Save current login", false);
                    p.left(r, "save", "Save the login Claude Code is using", "Run `claude` and sign in first. Don't use /logout, it can end the saved login", wsave);
                    let act = p.act(Act::Do("claude-save", String::new()));
                    ui::chip(p.c, p.cx, r.x + r.w - wsave, r.y + 8.0, act, None, "Save current login", false);
                }
            });
        }
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
        let cfg = self.keys_cfg(p.cx);
        let rec = self.rec.clone();
        let m = keymap::mod_name(&cfg);
        for (n, (ic, group)) in keymap::GROUPS.iter().enumerate() {
            p.h2(ic, group, if n == 0 { "Click a key, then press the new combo. Esc cancels" } else { "" });
            p.card(|p| {
                for b in keymap::BINDS.iter().filter(|b| b.group == *group) {
                    let labels: Vec<String> = keymap::keys_of(&cfg, b).into_iter().enumerate()
                        .map(|(i, k)| if rec.as_ref().is_some_and(|r| r.0 == b.id && r.1 == i) { "Press a key…".to_string() } else { k }).collect();
                    let reset = keymap::changed(&cfg, b);
                    let right: f32 = labels.iter().map(|l| chip_w(p.c, l, false) + 6.0).sum::<f32>() + if reset { chip_w(p.c, "", true) + 6.0 } else { 0.0 };
                    if let Some(mut r) = p.row(40.0) {
                        p.left(r, "", b.label, "", right);
                        if reset { let w = p.chip_right(r, Some("restart_alt"), "", Act::Do("key-reset", b.id.to_string())); r.w -= w + 6.0; }
                        for (i, l) in labels.iter().enumerate().rev() {
                            let w = p.chip_right(r, None, l, Act::Do("key-rec", format!("{}:{i}", b.id)));
                            r.w -= w + 6.0;
                        }
                    }
                }
                if *group == "Workspaces" {
                    for (k, d) in [(format!("{m}+1…0"), "Go to workspace 1–10"), (format!("{m}+Shift+1…0"), "Send the window to workspace 1–10")] {
                        if let Some(r) = p.row(40.0) {
                            let w = p.c.g.measure(&k, txt(13.0, 400)).0.ceil() + 16.0;
                            p.left(r, "", d, "", w);
                            let kr = R::new(r.x + r.w - w, r.y + (r.h - 24.0) / 2.0, w, 24.0);
                            p.c.rect(kr, 6.0, p.col("surfaceContainer"));
                            p.c.text(&k, R::new(kr.x + 8.0, kr.y, kr.w, kr.h), txt(13.0, 400), p.col("onSurface"), Align::Left);
                        }
                    }
                }
            });
        }
        p.lead_gap(12.0);
        let note = "Fixed: Ctrl+PgUp / PgDn switch tabs in a sidebar or here, Esc closes a sidebar or this window. Games are left alone by GlazeWM (window rule in %USERPROFILE%\\.glzr\\glazewm\\config.yaml).";
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
            let w = chip_w(p.c, "zor-ui", true);
            let sub = "A native shell for GlazeWM on Windows: bar, Dynamic Island, sidebars, buddies, dock. Some parts are ported from end4-pC by pctrade (GPL-3.0)";
            if let Some(r) = p.row_sub(40.0, "", sub, w) {
                p.left(r, "", "Zor UI", sub, w);
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
                p.left(r, "", "Config file", if p.cx.cfg.str("scriptsDir").is_empty() { "" } else { "%USERPROFILE%\\.glzr\\zor-ui\\config.json" }, w);
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
            // a failed decode is not remembered: one bad read must not blank the picture until restart
            let img = g.load_image(path, pw, ph)?;
            self.imgs.insert(key.clone(), Some(img));
        }
        self.imgs.get(&key)?.as_ref()
    }

    /// `pic` drawn into `r` with rounded corners. False when the file is missing or does not decode (nothing was drawn).
    pub fn picture(&mut self, p: &mut Pen, path: &std::path::Path, r: R, radius: f32) -> bool {
        let scale = p.c.scale;
        let g = p.c.g;
        let Some(img) = self.pic(g, scale, path, r.w, r.h) else { return false };
        p.c.clip_round(r, radius, |c| c.image(img, r, 1.0));
        true
    }

    /// `.grid`: auto-fill columns of at least 190 px, 10 px apart.
    pub fn grid(&mut self, p: &mut Pen, tiles: Vec<Tile>) {
        p.lead_gap(ROW_GAP);
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
                if let Some(u) = t.link {
                    let id = p.act(Act::Do("url", u));
                    ui::icon_btn(p.c, p.cx, R::new(b.x - 36.0 - 4.0, b.y, 36.0, 36.0), id, "open_in_new", IconKind::Plain, 20.0, false);
                }
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

const PREVIEW_W: f32 = 200.0;
const PREVIEW_H: f32 = 124.0;

/// The monitor's preview box: its shape scaled to fit the fixed preview area.
fn monitor_box(w: f32, h: f32) -> (f32, f32) {
    let k = (PREVIEW_W / w.max(1.0)).min(PREVIEW_H / h.max(1.0));
    ((w * k).round().max(8.0), (h * k).round().max(8.0))
}

/// Title and second line of a monitor's row: "DELL U2723QE", "Portrait, 1080 x 1920, its own picture".
/// An empty name falls back to "Display 2".
fn monitor_caption(name: &str, i: usize, w: i32, h: i32, custom: bool) -> (String, String) {
    let shape = if h > w { "Portrait" } else if w > h { "Landscape" } else { "Square" };
    (if name.is_empty() { format!("Display {}", i + 1) } else { name.to_string() }, format!("{shape}, {w} x {h}, {}", if custom { "its own picture" } else { "main wallpaper" }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_picture_decode_is_retried_not_cached() {
        use crate::gfx::Gfx;
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assetsonts"), &std::env::temp_dir()).unwrap();
        let mut st = crate::settings::Settings::new((1100.0, 700.0));
        let f = std::env::temp_dir().join("zor-retry-probe.png");
        std::fs::write(&f, b"not a picture").unwrap();
        assert!(st.pic(&g, 1.0, &f, 8.0, 8.0).is_none());
        let ok = std::fs::read(crate::conf::pack_dir().join(r"images\greet-1793799724.webp"));
        if let Ok(b) = ok { std::fs::write(&f, b).unwrap(); assert!(st.pic(&g, 1.0, &f, 8.0, 8.0).is_some(), "second try decodes"); }
        let _ = std::fs::remove_file(&f);
    }

    use super::*;

    fn model_list() -> Vec<(String, String)> {
        [("claude-cli:haiku", "Claude subscription: Haiku"), ("gemini-cli:default", "Gemini subscription"), ("codex-cli:default", "ChatGPT subscription"),
         ("gemini-2.5-flash", "Gemini 2.5 Flash"), ("gpt-4o-mini", "GPT-4o mini")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn contacts_are_listed_under_their_own_app() {
        assert_eq!(contact_app_label("whatsapp"), ("chat", "WhatsApp"));
        assert_eq!(contact_app_label("discord"), ("forum", "Discord"));
        for a in ["whatsappweb", "whatsapp-web", "whatsapp web"] { assert_eq!(contact_app_label(a), ("language", "WhatsApp Web"), "{a}"); }
        assert_eq!(contact_app_label("signal"), ("chat", "WhatsApp"), "an unknown app keeps the old label");
    }

    #[test]
    fn the_screen_model_picker_offers_only_models_that_can_read_images() {
        let ids = |v: &[(Value, String)]| v.iter().map(|(i, _)| i.as_str().unwrap().to_string()).collect::<Vec<_>>();
        let v = screen_model_choices(&model_list(), "");
        assert_eq!(ids(&v), ["", "claude-cli:haiku", "gemini-2.5-flash", "gpt-4o-mini"], "the sidebar's model first, text-only ones dropped, order kept");
        assert_eq!(v[0].1, "Same as the sidebar's AI model");
        assert_eq!(v[1].1, "Claude subscription: Haiku", "names come through");
    }

    #[test]
    fn a_text_only_model_that_is_already_chosen_stays_so_the_picker_is_never_blank() {
        let v = screen_model_choices(&model_list(), "gemini-cli:default");
        assert!(v.iter().any(|(i, _)| i == "gemini-cli:default"), "kept, so the user can see and change it");
        assert!(!v.iter().any(|(i, _)| i == "codex-cli:default"), "the other text-only one is still hidden");
    }

    #[test]
    fn the_screen_model_picker_works_before_the_list_has_loaded() {
        let v = screen_model_choices(&[], "");
        assert_eq!(v.len(), 1, "just the sidebar's model while the list loads");
        assert_eq!(screen_model_choices(&[], "claude-cli:sonnet").len(), 1, "a chosen model that isn't listed yet is not invented");
    }

    #[test]
    fn monitor_captions() {
        assert_eq!(monitor_caption("DELL U2723QE", 1, 1080, 1920, true), ("DELL U2723QE".into(), "Portrait, 1080 x 1920, its own picture".into()));
        assert_eq!(monitor_caption("", 1, 2560, 1440, false), ("Display 2".into(), "Landscape, 2560 x 1440, main wallpaper".into()));
        assert_eq!(monitor_caption("x", 0, 800, 800, false).1, "Square, 800 x 800, main wallpaper");
    }

    #[test]
    fn monitor_boxes_keep_their_shape_inside_the_preview_area() {
        assert_eq!(monitor_box(1920.0, 1080.0), (200.0, 113.0));
        assert_eq!(monitor_box(1080.0, 1920.0), (70.0, 124.0));
        for (w, h) in [(3440.0, 1440.0), (800.0, 800.0), (0.0, 0.0)] { let (bw, bh) = monitor_box(w, h); assert!(bw <= PREVIEW_W && bh <= PREVIEW_H && bw >= 8.0 && bh >= 8.0, "{w}x{h}"); }
    }

    #[test]
    fn captions() {
        assert_eq!(cap("all"), "All");
        assert_eq!(cap("video"), "Video");
        assert_eq!(cap(""), "");
    }

    #[test]
    fn every_page_has_its_id() {
        assert_eq!(PAGES.iter().map(|p| p.0).collect::<Vec<_>>(), ["quick", "keys", "wallpapers", "presets", "bar", "island", "buddies", "contacts", "creator", "voice", "services", "usage", "about"]);
        assert!(crate::settings::layout::BAR_MODULES.iter().all(|m| !m.2.is_empty()));
        assert_eq!(LANGS.len(), 49);
    }

    #[test]
    fn card_size_choices_are_real_grids() {
        for (v, _) in CARD_SIZES { assert!(v.is_empty() || crate::desk::grid_size(v).is_some(), "{v}"); }
        for id in crate::desk::SIZED { assert!(DESKTOP_WIDGETS.iter().any(|w| w.0 == id), "{id}"); }
    }
}

/// Height of a box of three poses whose pictures are at most `max_h` tall: room for them above the feet, the title above
/// that and the labels below.
fn stances_h(max_h: f32) -> f32 { max_h + 50.0 }

/// A box with a buddy standing, walking and sitting, drawn by the bar's own code (`buddy::draw_preview`); `what` is how he looks
/// and where his pictures are kept, or None to show `empty` in the box instead.
fn stances_box(p: &mut Pen, title: &str, what: Option<(&crate::buddy::Appearance, &mut crate::buddy::Sprites)>, empty: &str, max_h: f32) {
    let h = stances_h(max_h);
    let Some(r) = p.row(h + 12.0) else { return };
    let b = R::new(r.x + 12.0, r.y + 6.0, r.w - 24.0, h);
    p.c.rect(b, 12.0, p.col("surfaceContainer"));
    p.c.text(title, R::new(b.x + 10.0, b.y + 6.0, b.w - 20.0, 14.0), txt(11.0, 400), p.col("onSurfaceVariant"), Align::Left);
    let Some((ap, sprites)) = what else {
        p.c.text(empty, R::new(b.x + 10.0, b.y + 24.0, b.w - 20.0, h - 30.0), txt(13.0, 400), p.col("onSurfaceVariant"), Align::Center);
        return;
    };
    let feet = b.y + h - 30.0;
    for (i, (stance, label)) in [("stand", "Standing"), ("walk", "Walking"), ("sit", "Sitting")].into_iter().enumerate() {
        let x = b.x + b.w * (i as f32 * 2.0 + 1.0) / 6.0;
        crate::buddy::draw_preview(p.c, ap, sprites, x, feet, stance, p.t, max_h);
        p.c.text(label, R::new(x - 40.0, b.y + h - 17.0, 80.0, 14.0), txt(11.0, 400), p.col("onSurfaceVariant"), Align::Center);
    }
}

/// What the buddy preview calls a buddy: the name typed for the slot, else the character's own.
fn buddy_label<'a>(name: &'a str, character: &'a str) -> &'a str { let n = name.trim(); if n.is_empty() { character } else { n } }

#[cfg(test)]
mod page_header_tests {
    use super::*;

    /// Height of the creator page laid out with this config.
    fn creator_height(cfg: &str) -> f32 {
        use crate::{app::Ctx, gfx::{Canvas, Gfx}};
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 800 * 800 * 4];
        let mut c = Canvas::new(&g, &mut px, 800, 800, 1.0, 0).unwrap();
        let mut cx = Ctx::new(false);
        cx.cfg = conf::Config::parse(cfg);
        let mut st = Settings::new((800.0, 800.0)).on_page("creator");
        let mut p = Pen::new(&mut c, &cx, 0.0, 0.0, 600.0, 0.0, 0.0);
        p.dry = true;
        st.creator_page(&mut p);
        p.y
    }

    #[test]
    fn the_creator_page_lays_out_the_same_with_or_without_a_picture() {
        let empty = creator_height("{}");
        assert!(empty > 600.0, "preview and five sections: {empty}");
        assert_eq!(creator_height(r#"{"buddies":{"make":{"image":"buddy-make-1.png"}}}"#), empty, "a picture changes what the preview shows, not how tall it is");
        // the default height, 64, is already taller than a top bar allows, so it carries the note about that
        assert_eq!(creator_height(r#"{"buddies":{"make":{"height":120}}}"#), empty, "a tall buddy has the top-bar note too");
        assert!(creator_height(r#"{"buddies":{"make":{"height":54}}}"#) < empty, "one that fits a top bar needs no note");
    }

    #[test]
    fn the_monitor_card_is_measured_with_its_brightness_sliders() {
        use crate::{app::Ctx, gfx::{Canvas, Gfx}};
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 800 * 800 * 4];
        let mut c = Canvas::new(&g, &mut px, 800, 800, 1.0, 0).unwrap();
        let cx = Ctx::new(false);
        let mut st = Settings::new((800.0, 800.0));
        let mon = |i: u32, n: &str| json!({ "i": i, "w": 1920, "h": 1080, "name": n, "file": "" });
        st.wmons = Load::Ready(vec![mon(0, "A"), mon(1, "B")]);
        st.mons = Load::Ready(vec!["0|A".into(), "1|B".into()]);
        let mut p = Pen::new(&mut c, &cx, 0.0, 0.0, 600.0, 0.0, 0.0);
        p.dry = true; // nothing is on screen in the measuring pass, yet the sliders still take their room
        assert!(st.monitor_wallpapers(&mut p));
        assert!(p.measured[0] >= 2.0 * (PREVIEW_H + 24.0 + 40.0), "two pictures and two sliders: {}", p.measured[0]);
    }

    #[test]
    fn the_boxes_leave_room_for_the_tallest_picture() {
        assert!(stances_h(64.0) >= 64.0 + 30.0 + 14.0 + 6.0, "feet 30 above the bottom for the sitting legs and labels, title above the head");
        assert!(stances_h(100.0) > stances_h(64.0));
    }

    #[test]
    fn the_preview_names_the_buddy_by_its_own_name_else_the_characters() {
        assert_eq!(buddy_label("  Kuro ", "Gojo"), "Kuro");
        assert_eq!((buddy_label("   ", "Gojo"), buddy_label("", "Gojo")), ("Gojo", "Gojo"));
    }

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

    /// Height of the Contacts page (`page`) or of the Buddies page, laid out with these contacts and, if given, the entry being edited.
    fn contacts_height(list: &[&str], editing: &str, page: bool) -> f32 {
        use crate::{app::Ctx, gfx::{Canvas, Gfx}};
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 800 * 800 * 4];
        let mut c = Canvas::new(&g, &mut px, 800, 800, 1.0, 0).unwrap();
        let mut cx = Ctx::new(false);
        cx.cfg = conf::Config::parse(&json!({ "buddies": { "contacts": list } }).to_string());
        let mut st = Settings::new((800.0, 800.0)).on_page(if page { "contacts" } else { "buddies" });
        if !editing.is_empty() { st.fields.entry("contact-edit".into()).or_insert_with(|| crate::settings::Field::new(false)).tb.set(editing); }
        let mut p = Pen::new(&mut c, &cx, 0.0, 0.0, 600.0, 0.0, 0.0);
        p.dry = true;
        if page { st.contacts_page(&mut p) } else { st.buddies(&mut p) }
        p.y
    }

    #[test]
    fn the_contacts_are_no_longer_on_the_buddies_page() {
        let three = ["A=whatsapp:1", "B=discord:2", "C=whatsapp:3"];
        assert_eq!(contacts_height(&[], "", false), contacts_height(&three, "", false), "the list and its form moved out of Voice commands");
    }

    #[test]
    fn the_contacts_page_grows_with_the_list_and_while_one_is_being_edited() {
        let two = ["A=whatsapp:1", "B=discord:2"];
        assert!(contacts_height(&two, "", true) > contacts_height(&[], "", true), "a row per contact");
        assert!(contacts_height(&two, "A=whatsapp:1", true) > contacts_height(&two, "", true) + 30.0, "an 'Editing A' row with a Cancel button appears");
    }

    #[test]
    fn the_speech_dropdown_line_shows_size_and_whether_a_download_is_needed() {
        assert_eq!(speech_option_label("Parakeet TDT v3", 670, true), "Parakeet TDT v3 · 670 MB");
        assert_eq!(speech_option_label("Parakeet TDT v2", 661, false), "Parakeet TDT v2 · 661 MB · download");
    }

    #[test]
    fn every_page_has_a_blurb() {
        for (key, _, _) in PAGES.iter() { assert!(!page_blurb(key).is_empty(), "{key}"); }
        assert_ne!(page_blurb("quick"), page_blurb("about"));
        assert_ne!(page_blurb("contacts"), page_blurb("buddies"), "the new page has its own line");
    }
}
