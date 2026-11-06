//! A small Gojo who lives on the bar: walks along it, sits on its edge with his legs hanging, falls
//! asleep when you're away, and turns to look at the pointer when it comes near the bar.
//! Hover: he lifts the blindfold. Click: he talks. Double-click: Hollow Purple. Right-click: Infinity
//! (for 10 s the pointer can't reach him). Drag him anywhere; let go and he falls back onto the bar,
//! or teleports back if you dropped him below it.
//! He also watches the desktop and comments on what happens: a new track, the volume, the battery, the
//! network, the weather, notifications, a busy CPU.
//! Turned on by config.json "native": {"gojo": true}. Drawn from shapes, no image files.

use crate::{
    app::{App, Ctx},
    assist,
    bar::{pretty_app, Edge},
    cast::{self, Cast, Hair, Trim},
    desk::greeting,
    gfx::{alpha, polygon, txt, Align, Canvas, Color, Gfx, R},
    util,
    win::{Btn, Input, View, Win, Z},
};
use std::{
    sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicUsize, Ordering},
    time::Instant,
};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY},
    System::SystemInformation::GetTickCount,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

static HWNDS: [AtomicIsize; 2] = [AtomicIsize::new(0), AtomicIsize::new(0)];
/// Slot of the primary buddy, from config buddies.primary.
static PRIMARY: AtomicUsize = AtomicUsize::new(0);
static ON: AtomicBool = AtomicBool::new(false);
/// config.json "native": {"gojoVoice": true} turns his spoken voice on; off by default.
static TALK: AtomicBool = AtomicBool::new(false);

/// The voice speaking right now; a new line cuts it off.
static VOICE: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

/// Says `s` aloud with Windows' built-in voice (System.Speech, preferring a male one). The text goes in
/// through an environment variable, not the command line, so a track title can't inject script.
/// shortcut: PowerShell start-up delays the voice ~0.5 s, move to SAPI through the windows crate if it bothers.
fn speak(s: &str) {
    use std::os::windows::process::CommandExt;
    if !TALK.load(Ordering::Relaxed) { return; }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = "Add-Type -AssemblyName System.Speech; $v = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                  $v.SelectVoiceByHints('Male'); $v.Speak($env:GOJO_SAY)";
    let child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("GOJO_SAY", s)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let Ok(mut slot) = VOICE.lock() else { return };
    if let Some(mut old) = slot.take() { let _ = old.kill(); }
    *slot = child.ok();
}

/// True while Gojo is turned on; the Claude usage poll only runs then.
pub fn on() -> bool { ON.load(Ordering::Relaxed) }

/// Window size and where his feet sit in it (CSS px). The window's top lines up with the monitor
/// top while he stands on the bar; the rest is transparent, so clicks pass through it.
const W: f32 = 340.0;
const H: f32 = 100.0;
const FX: f32 = W / 2.0;
const FY: f32 = 56.0;
/// The bar's height (bar.rs docks 56 px).
const BAR: f32 = 56.0;
/// The bar's edge (`Edge as u8`): the buddies stand on its inner side, and on the monitor's bottom for a side bar.
static BAR_EDGE: AtomicU8 = AtomicU8::new(Edge::Top as u8);
/// Where the buddies' feet are (screen px): the bar's bottom edge for a top bar, the bar's top edge for a bottom bar,
/// and the monitor's bottom for a left or right bar.
fn feet_line(edge: Edge, top: i32, bottom: i32, scale: f32) -> f32 {
    let bar = (BAR * scale).round();
    match edge { Edge::Top => top as f32 + bar, Edge::Bottom => bottom as f32 - bar, Edge::Left | Edge::Right => bottom as f32 }
}
const SPEED: f32 = 28.0;

/// One look at the desktop around him: the bar's data sources, reduced to what he comments on.
#[derive(Clone, Default, PartialEq)]
struct Around {
    /// The current media session's title and artist.
    track: String,
    playing: bool,
    /// None until the audio thread has answered once.
    volume: Option<u8>,
    muted: Option<bool>,
    battery: Option<u8>,
    charging: Option<bool>,
    online: bool,
    /// Weather status ("clear_day", "light_rain_night", ...), "" while unknown.
    weather: &'static str,
    /// The last notification's app and text.
    note: String,
    busy: bool,
    /// Claude plan usage in percent (the busier of the 5-hour and 7-day windows); None while unknown.
    claude: Option<u8>,
}

/// 0 below 80 %, 1 from 80 %, 2 from 95 %.
fn claude_tier(p: Option<u8>) -> u8 { match p { Some(95..) => 2, Some(80..) => 1, _ => 0 } }

impl Around {
    fn of(cx: &Ctx) -> Around {
        let m = cx.media.as_ref().filter(|m| !m.title.is_empty());
        Around {
            track: m.map(|m| format!("{}\t{}", m.title, m.artist)).unwrap_or_default(),
            playing: m.is_some_and(|m| m.playing),
            volume: cx.audio.map(|a| a.volume.clamp(0.0, 100.0).round() as u8),
            muted: cx.audio.map(|a| a.muted),
            battery: cx.battery.map(|b| b.percent.clamp(0.0, 100.0).round() as u8),
            charging: cx.battery.map(|b| b.charging),
            online: cx.net.is_some(),
            weather: cx.weather.as_ref().map_or("", |w| w.status),
            note: cx.notif.as_ref().filter(|n| !n.text.is_empty()).map(|n| format!("{}\t{}", n.app, n.text)).unwrap_or_default(),
            busy: cx.cpu >= 0.85,
            claude: cx.claude.map(|u| u.worst()),
        }
    }
}

/// What he'd say about a change since `old`; None when nothing worth a line happened.
fn notice(old: &Around, new: &Around) -> Option<String> {
    let low = |a: &Around| a.battery.is_some_and(|p| p <= 15) && a.charging != Some(true);
    if low(new) && !low(old) { return Some("Battery's low. Find a plug, would you?".into()); }
    // only on the way up: a window resetting back to 0 % needs no remark
    if claude_tier(new.claude) > claude_tier(old.claude) {
        let p = new.claude.unwrap_or(0);
        return Some(if p >= 95 { format!("Claude's at {p}%. Even I can't beat a rate limit.") } else { format!("Claude's at {p}% of the limit. Pace yourself.") });
    }
    if !new.note.is_empty() && new.note != old.note {
        return Some(format!("Word from {}.", pretty_app(new.note.split('\t').next().unwrap_or(""))));
    }
    if !new.track.is_empty() && new.track != old.track {
        return Some(format!("{}, huh? Good taste.", cut(new.track.split('\t').next().unwrap_or(""), 28)));
    }
    if new.playing != old.playing && !new.track.is_empty() {
        return Some(if new.playing { "Music's on. Turn it up~".into() } else { "Aw, the music stopped.".to_string() });
    }
    if let (Some(a), Some(b)) = (old.muted, new.muted) {
        if a != b { return Some(if b { "Muted? I can't hear a thing.".into() } else { "Sound's back.".to_string() }); }
    }
    if let (Some(a), Some(b)) = (old.volume, new.volume) {
        if !new.muted.unwrap_or(false) && (a as i16 - b as i16).abs() >= 20 {
            return Some(if b > a { "That's loud. I approve.".into() } else { "Too quiet in here.".to_string() });
        }
    }
    if new.online != old.online {
        return Some(if new.online { "We're back online.".into() } else { "The internet's gone. Not my doing.".to_string() });
    }
    if let (Some(a), Some(b)) = (old.charging, new.charging) {
        if a != b { return Some(if b { "Charging. Good.".into() } else { "On battery? Better be quick.".to_string() }); }
    }
    if new.weather != old.weather {
        let line = match new.weather {
            s if s.starts_with("thunder") => "Thunder outside? Even I'd stay in.",
            s if s.starts_with("snow") => "It's snowing. Cute.",
            s if s.contains("rain") => "It's raining out there. Stay in.",
            _ => "",
        };
        if !line.is_empty() { return Some(line.into()); }
    }
    if new.busy && !old.busy { return Some("Whatever's running is working hard.".into()); }
    None
}

/// What kind of app is in front: "code", "video", "social", "chat", "game", or "" for anything else.
fn focus_kind(title: &str, process: &str) -> &'static str {
    let p = process.to_lowercase();
    let p = p.trim_end_matches(".exe");
    let t = title.to_lowercase();
    let has = |xs: &[&str], s: &str| xs.iter().any(|x| s.contains(x));
    if has(&["discord", "whatsapp", "telegram", "slack", "teams"], p) { return "chat"; }
    if has(&["code", "cursor", "devenv", "idea64", "pycharm64", "rustrover", "zed", "sublime_text", "nvim"], p) { return "code"; }
    if has(&["steam", "valorant", "javaw", "fortnite", "leagueclient", "epicgameslauncher", "cs2", "gta5"], p) { return "game"; }
    if has(&["chrome", "msedge", "firefox", "brave", "opera", "vlc", "mpv"], p) {
        if has(&["youtube", "netflix", "twitch", "prime video", "crunchyroll", "disney+"], &t) || p == "vlc" || p == "mpv" { return "video"; }
        if has(&["instagram", "reddit", "twitter", "facebook", "tiktok"], &t) { return "social"; }
    }
    ""
}

/// `s` shortened to `n` characters, so a long track title fits in a bubble line.
fn cut(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) { Some((i, _)) => format!("{}…", &s[..i]), None => s.to_string() }
}

/// Opens or closes each buddy when config changes: slot 0 is "native": {"gojo": true} (the original
/// flag), slot 1 is "buddies": {"b": {"on": true}}.
pub fn sync(app: &mut App) {
    let wants = [app.cx.native("gojo"), app.cx.native("buddyB") || app.cx.cfg.bool("buddies.b.on")];
    ON.store(wants.iter().any(|&w| w), Ordering::Relaxed);
    TALK.store(app.cx.cfg.bool("native.gojoVoice"), Ordering::Relaxed);
    BAR_EDGE.store(app.cx.bar_edge() as u8, Ordering::Relaxed);
    PRIMARY.store((app.cx.cfg.str("buddies.primary") == "b") as usize, Ordering::Relaxed);
    for (slot, want) in wants.into_iter().enumerate() {
        let at = app.wins.iter().position(|w| w.hwnd.0 as isize == HWNDS[slot].load(Ordering::Relaxed));
        match (want, at) {
            (true, None) => {
                let Ok(mut w) = Win::new(Box::new(Gojo::slot(slot)), "Zor UI / gojo", -32000, -32000, Z::Top, false) else { continue };
                HWNDS[slot].store(w.hwnd.0 as isize, Ordering::Relaxed);
                w.update(&app.g, &app.cx, true);
                w.show();
                app.wins.push(w);
            }
            (false, Some(i)) => {
                app.wins.remove(i);
                HWNDS[slot].store(0, Ordering::Relaxed);
                if let Ok(mut p) = PEERS.lock() { p[slot] = NO_PEER; }
            }
            _ => {}
        }
    }
}

/// What one buddy shares with the other so they can act as partners.
#[derive(Clone)]
struct Peer {
    on: bool,
    x: f32,
    mon_left: i32,
    /// Standing, sitting or walking, so free to talk.
    calm: bool,
    name: String,
    /// A line waiting for this buddy's answer: (when to answer, who spoke).
    inbox: Option<(f32, String, cast::Tone)>,
}
const NO_PEER: Peer = Peer { on: false, x: 0.0, mon_left: 0, calm: false, name: String::new(), inbox: None };
static PEERS: std::sync::Mutex<[Peer; 2]> = std::sync::Mutex::new([NO_PEER, NO_PEER]);

/// Seconds on one clock both buddies share (each has its own `t`).
fn clock() -> f32 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f32()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Stand, Walk, Sit, Sleep, Held, Fall }

pub struct Gojo {
    /// Feet, in physical screen px. NaN until the first frame places him.
    x: f32,
    y: f32,
    vy: f32,
    mode: Mode,
    to: f32,
    /// Seconds left before he decides what to do next.
    wait: f32,
    dir: f32,
    t: f32,
    last: Instant,
    rng: u64,
    peek: f32,
    hovered: bool,
    peek_said: f32,
    bubble: Option<(String, f32)>,
    infinity: f32,
    purple: Option<f32>,
    poof: Option<f32>,
    hop: Option<f32>,
    /// Left button: where it went down over him, and the grab offset once it's a drag.
    press: Option<(i32, i32)>,
    grab: (f32, f32),
    dragged: bool,
    /// When the last left click landed: a second one soon after is a double-click (the hop moves the
    /// window, so Windows' own double-click detection misses it).
    last_click: f32,
    was_down: bool,
    chat_at: f32,
    /// The desktop as he last saw it, and whether he has seen enough to compare against.
    around: Around,
    primed: bool,
    /// No ambient remark before this time, and the newest one waiting for its turn to speak.
    quiet: f32,
    queued: Option<String>,
    hidden: bool,
    check_at: f32,
    /// Top edge (screen px) of the open window he sits on, instead of the bar.
    perch: Option<f32>,
    /// Fell to the perch to sit there: sit when he lands.
    sit_after: bool,
    /// How far into the sitting pose he is drawn (1 sitting, 0 standing), eased so he rises instead of snapping.
    sit_k: f32,
    k_t: f32,
    mon: RECT,
    scale: f32,
    /// 0 or 1: which window and which "buddies" config slot ("a" or "b") this is.
    slot: usize,
    cast: Cast,
    style: Hair,
    /// Skin, glasses, ears and the picture that replaces the drawn figure.
    trim: Trim,
    /// The decoded picture: (file, height in px, pixmap; None if it can't be read, so it isn't retried every frame). Reloaded when the file or the screen scale changes.
    sprite: Option<(String, u32, Option<tiny_skia::Pixmap>)>,
    /// Empty: the cast's own name.
    name: String,
    walk: bool,
    /// How talkative: 0 silent, 1 normal, 2 chatty.
    chatty: f32,
    pair: bool,
    /// Shared-clock time of the next line to the partner.
    banter_at: f32,
    greeted: bool,
    /// Seconds of continuous work (reset by a 5 minute pause), and the minutes between break reminders (0 = off).
    work: f32,
    break_mins: f32,
    breaks: bool,
    /// What kind of app is in front, since when, and the last kind he remarked on and when.
    focus: &'static str,
    focus_since: f32,
    /// Where he stood before a game sent him to the other monitor: (x, y).
    away_from: Option<(f32, f32, isize)>,
    /// Next time (s) a walking buddy looks for the window top under his feet.
    ground_at: f32,
    focus_said: (&'static str, f32),
    watch: bool,
}

impl Gojo {
    pub(crate) fn new() -> Gojo { Gojo::slot(0) }

    fn slot(slot: usize) -> Gojo {
        let seed = unsafe { GetTickCount() } as u64 | 1;
        let g = Gojo {
            x: f32::NAN, y: f32::NAN, vy: 0.0, mode: Mode::Stand, to: 0.0, wait: 4.0, dir: -1.0, t: 0.0, last: Instant::now(),
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), peek: 0.0, hovered: false, peek_said: -99.0, bubble: None, infinity: 0.0,
            purple: None, poof: Some(0.0), hop: None, press: None, grab: (0.0, 0.0), dragged: false, last_click: -9.0, was_down: false,
            chat_at: 200.0, around: Around::default(), primed: false, quiet: 0.0, queued: None, hidden: false, check_at: 0.0, perch: None, sit_after: false, sit_k: 0.0, k_t: 0.0,
            mon: RECT::default(), scale: 1.0,
            slot, cast: cast::CASTS[slot.min(cast::CASTS.len() - 1)].clone(), style: Hair::default(), trim: Trim::default(), sprite: None, name: String::new(), walk: true, chatty: 1.0, pair: false, banter_at: clock() + 30.0, greeted: false,
            work: 0.0, break_mins: 50.0, breaks: true, focus: "", focus_since: 0.0, away_from: None, ground_at: 0.0, focus_said: ("", -9999.0), watch: true,        };
        g
    }

    /// Reads this buddy's settings from config (once a second, so Settings changes show at once).
    fn tune(&mut self, cx: &Ctx) {
        let k = ["a", "b"][self.slot];
        (self.cast, self.style) = cast::pick(&cx.cfg, cx.cfg.str(&format!("buddies.{k}.character")));
        let s = |f: &str| cx.cfg.str(&format!("buddies.{k}.{f}")).to_string();
        self.cast.dress(&s("hair"), &s("shirt"), &s("pants"), &s("eyes"), &s("bun"));
        if let Some(h) = Hair::parse(&s("style")) { self.style = h; }
        self.trim = if cast::custom_on(&cx.cfg) { Trim::read(&cast::trim(&cx.cfg, cx.cfg.str(&format!("buddies.{k}.character"))), |f| s(f)) } else { Trim::default() };
        self.name = cx.cfg.str(&format!("buddies.{k}.name")).trim().to_string();
        self.walk = cx.cfg.get(&format!("buddies.{k}.walk")).as_bool().unwrap_or(true);
        self.chatty = match cx.cfg.str(&format!("buddies.{k}.chatty")) { "quiet" => 0.0, "chatty" => 2.0, _ => 1.0 };
        self.pair = cx.cfg.bool("buddies.pair");
        self.break_mins = cx.cfg.get("buddies.breakMins").as_f64().unwrap_or(50.0) as f32;
        self.breaks = cx.cfg.get("buddies.breaks").as_bool().unwrap_or(true);
        self.watch = cx.cfg.get("buddies.watch").as_bool().unwrap_or(true);
        // the first look at the config is also when they greet you, in the chosen character's words
        if !std::mem::replace(&mut self.greeted, true) { self.say(&format!("{} {}!", self.cast.greet, greeting(util::now().wHour)), 4.0); }
    }
    /// Loads (or keeps) the picture named by the look options; false when there is none or it can't be read.
    fn load_sprite(&mut self, c: &Canvas) -> bool {
        let h = (SPRITE_H * c.scale).round() as u32;
        if self.trim.image.is_empty() { self.sprite = None; return false; }
        if let Some((f, ph, img)) = &self.sprite { if *f == self.trim.image && *ph == h { return img.is_some(); } }
        let path = crate::conf::pack_dir().join("images").join(&self.trim.image);
        let loaded = c.g.image_size(&path).filter(|&(w, h)| w > 0 && h > 0)
            .and_then(|(iw, ih)| c.g.load_image(&path, ((h as f32 * iw as f32 / ih as f32).round() as u32).clamp(1, 4 * h), h));
        let ok = loaded.is_some();
        self.sprite = Some((self.trim.image.clone(), h, loaded));
        ok
    }
    fn shown_name(&self) -> String { if self.name.is_empty() { self.cast.label.to_string() } else { self.name.clone() } }

    /// Only the primary buddy (config buddies.primary) comments on the desktop, so a pair doesn't say
    /// everything twice. The other one leads when the primary is off.
    fn leads(&self) -> bool {
        let pri = PRIMARY.load(Ordering::Relaxed);
        self.slot == pri || PEERS.lock().map_or(true, |p| !p[pri].on)
    }

    /// Publishes where this buddy is, answers a line from the partner, and starts a chat now and then.
    /// Returns the partner's x when they are partnered, both on, and on the same monitor.
    fn partner(&mut self) -> Option<f32> {
        let now = clock();
        let calm = matches!(self.mode, Mode::Stand | Mode::Sit | Mode::Walk) && !self.hidden;
        let name = self.shown_name();
        let (me, other) = (self.slot, 1 - self.slot);
        let Ok(mut p) = PEERS.lock() else { return None };
        p[me].name.clone_from(&name);
        p[me].on = true;
        p[me].x = self.x;
        p[me].mon_left = self.mon.left;
        p[me].calm = calm;
        if !(self.pair && p[other].on) { p[me].inbox = None; return None; }
        let near = p[other].mon_left == self.mon.left;
        if let Some((due, from, tone)) = p[me].inbox.clone() {
            if now > due + 20.0 { p[me].inbox = None; } // missed it
            else if now >= due && calm && !self.talking() {
                p[me].inbox = None;
                let l = self.pick(self.cast.reply[tone as usize]).replace("{p}", &from); // answers the tone it heard
                self.say(&l, 3.5);
            }
        }
        let close = near && (p[other].x - self.x).abs() < 160.0 * self.scale;
        if now >= self.banter_at && self.chatty > 0.0 && calm && p[other].calm && !self.talking() && p[me].inbox.is_none() {
            if close {
                self.banter_at = now + (50.0 + self.rand() * 70.0) / self.chatty;
                let (tone, line) = self.cast.to[(self.rand() * self.cast.to.len() as f32) as usize % self.cast.to.len()];
                self.say(&line.replace("{p}", &p[other].name), 4.0);
                p[other].inbox = Some((now + 4.2, name, tone));
            } else { self.banter_at = now + 6.0; } // wait until they have walked together
        }
        near.then_some(p[other].x)
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }
    fn pick(&mut self, lines: &[&str]) -> String { lines[(self.rand() * lines.len() as f32) as usize % lines.len()].to_string() }
    fn say(&mut self, s: &str, secs: f32) { self.bubble = Some((s.to_string(), self.t + secs)); speak(s); }
    fn talking(&self) -> bool { self.bubble.as_ref().is_some_and(|b| b.1 > self.t) }
    fn line(&self) -> f32 {
        let edge = match BAR_EDGE.load(Ordering::Relaxed) { x if x == Edge::Bottom as u8 => Edge::Bottom, x if x == Edge::Left as u8 => Edge::Left, x if x == Edge::Right as u8 => Edge::Right, _ => Edge::Top };
        feet_line(edge, self.mon.top, self.mon.bottom, self.scale)
    }

    /// Looks at what's going on around him and comments on a change: at most one line every ~20-35 s, and
    /// never over one he's already saying (the newest change waits for its turn).
    fn react(&mut self, cx: &Ctx) {
        let now = Around::of(cx);
        if self.chatty == 0.0 || !self.leads() { return; }
        if !self.primed {
            // the first look, once the data sources have answered at least once, only records the state
            if cx.net.is_some() || self.t > 15.0 { self.primed = true; self.around = now; }
            return;
        }
        if let Some(l) = notice(&self.around, &now) { self.queued = Some(l); }
        self.watch_focus(cx);
        self.around = now;
        if self.t < self.quiet || self.talking() { return; }
        let Some(line) = self.queued.take() else { return };
        // a change worth a line wakes him: nothing is sadder than missing a good song asleep
        if self.mode == Mode::Sleep { self.mode = Mode::Stand; self.wait = 3.0; }
        self.quiet = self.t + (20.0 + self.rand() * 15.0) / self.chatty;
        self.say(&line, 4.0);
    }

    /// Remarks on the kind of app in front once it has been there 20 s, and at most one remark per
    /// kind every 15 minutes, so alt-tabbing doesn't make him chatty.
    fn watch_focus(&mut self, cx: &Ctx) {
        if !self.watch || self.queued.is_some() { return; }
        let kind = cx.wm.window.as_ref().map_or("", |(title, proc)| focus_kind(title, proc));
        if kind != self.focus { self.focus = kind; self.focus_since = self.t; return; }
        if kind.is_empty() || self.t - self.focus_since < 20.0 || (self.focus_said.0 == kind && self.t - self.focus_said.1 < 900.0) { return; }
        let lines: &[&str] = match kind {
            "code" => &["Coding? Don't let a bug beat you.", "Writing code again. I'll be here if it breaks."],
            "video" => &["Watching something? Don't forget to blink.", "Is it any good? Move over."],
            "social" => &["Scrolling again? Just a little, okay?", "Social media time. Don't lose an hour."],
            "chat" => &["Chatting with someone? Say hi from me.", "Busy talking, I see."],
            "game" => &["A game! Show them what you've got.", "Game time. Go win."],
            _ => return,
        };
        self.focus_said = (kind, self.t);
        let l = self.pick(lines);
        self.queued = Some(l);
    }

    /// A full-screen game on his monitor sends him to the other monitor (if there is one) to cheer from
    /// there; when the game ends he walks back to where he was.
    fn follow_game(&mut self, cx: &Ctx) {
        let fs = fullscreen_monitor();
        let game = fs.is_some() && cx.wm.window.as_ref().map_or(true, |(t, p)| focus_kind(t, p) != "video");
        if self.mode == Mode::Held { return; }
        if game && self.away_from.is_none() {
            let Some(fs) = fs.filter(|f| *f == self.mon) else { return };
            let other = [fs.right + 10, fs.left - 10].map(|x| monitor(POINT { x, y: fs.top }, false)).into_iter().find(|m| *m != fs);
            let Some(o) = other else { return };
            self.away_from = Some((self.x, self.y, unsafe { GetForegroundWindow() }.0 as isize));
            self.mon = o;
            self.perch = None;
            // each slot gets its own spot, like on first load, so a pair doesn't stack
            let from_right = if self.slot == 0 { 0.3 } else { 0.55 };
            self.x = o.right as f32 - (o.right - o.left) as f32 * from_right;
            self.y = self.line();
            self.mode = Mode::Stand;
            self.wait = 2.0;
            self.hidden = false;
            if self.leads() { let l = self.pick(&["A game! I'll watch from here. Go win.", "Game time. I'm on the other screen, cheering."]); self.say(&l, 5.0); }
        } else if let Some((x, y, game_win)) = self.away_from {
            // he stays until the game's window is gone, so clicking the other monitor doesn't call him back
            if unsafe { IsWindow(HWND(game_win as *mut _)) }.as_bool() { return; }
            self.away_from = None;
            self.x = x; self.y = y; self.perch = None;
            if self.leads() { self.say("Good game? Welcome back.", 4.0); }
        }
    }

    /// Counts how long you have worked without a 5 minute pause and says when it's time for a break.
    fn watch_break(&mut self, idle_s: f32, dt: f32) {
        if idle_s > 300.0 { self.work = 0.0; } else if idle_s < 60.0 { self.work += dt; }
        if !self.breaks || self.break_mins <= 0.0 || self.work < self.break_mins * 60.0 || !self.leads() || self.hidden || self.talking() || self.mode == Mode::Held { return; }
        let mins = (self.work / 60.0).round() as u32;
        self.work = 0.0;
        if self.mode == Mode::Sleep { self.mode = Mode::Stand; self.wait = 3.0; }
        let l = self.pick(&["Stand up, stretch, drink some water.", "Look away from the screen for a minute.", "Take a walk. The code will wait."]);
        self.say(&format!("{mins} minute{} straight. Break time! {l}", if mins == 1 { "" } else { "s" }), 7.0);
    }

    /// One simulation step: pointer, buttons, behaviour, then moves the window to his feet.
    fn step(&mut self, s: f32, ctx: &Ctx) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.t += dt;
        self.scale = s;
        let mut cur = POINT::default();
        unsafe { let _ = GetCursorPos(&mut cur); }
        let (cx, cy) = (cur.x as f32, cur.y as f32);

        if self.x.is_nan() {
            self.mon = monitor(POINT { x: 0, y: 0 }, true);
            // each slot starts at its own spot so a pair doesn't stack on first load
            let from_right = if self.slot == 0 { 0.3 } else { 0.55 };
            self.x = self.mon.right as f32 - (self.mon.right - self.mon.left) as f32 * from_right;
            self.y = self.line();
        }
        let m = monitor(POINT { x: self.x as i32, y: self.y as i32 - 1 }, false);
        if m != self.mon { self.mon = m; self.perch = None; if self.mode != Mode::Held { self.y = self.line(); } }

        if self.t >= self.check_at {
            self.check_at = self.t + 1.0;
            self.tune(ctx);
            self.follow_game(ctx);
            self.hidden = fullscreen_on(self.mon);
            // the window he sits on moved or closed
            if self.perch.is_some() && matches!(self.mode, Mode::Sit | Mode::Stand) {
                match window_top(self.mon, self.x, self.line()) {
                    Some(t) => { self.perch = Some(t); self.y = t; }
                    None => { self.perch = None; self.y = self.line(); self.poof = Some(self.t); self.mode = Mode::Stand; self.wait = 2.0; }
                }
            }
        }

        let buddy_x = self.partner();

        // his body, screen px
        let (bl, br, bt, bb) = (self.x - 11.0 * s, self.x + 11.0 * s, self.y - 48.0 * s, self.y + if self.sitting() { 11.0 } else { 1.0 } * s);
        self.hovered = !self.hidden && cx >= bl && cx < br && cy >= bt && cy < bb;

        // left button: press over him, then a drag past 6 px picks him up
        let down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
        if down && !self.was_down && self.hovered { self.press = Some((cur.x, cur.y)); }
        if !down { self.press = None; }
        if let Some((px, py)) = self.press {
            if self.mode != Mode::Held && (cur.x - px).pow(2) + (cur.y - py).pow(2) > 36 {
                self.mode = Mode::Held;
                self.dragged = true;
                self.purple = None;
                self.grab = (self.x - px as f32, self.y - py as f32);
                let l = self.pick(&["Hey! Put me down!", "Whoa, whoa!", "Careful, I'm precious."]);
                self.say(&l, 2.5);
            }
        }
        self.was_down = down;

        let line = self.line();
        let idle_s = unsafe {
            let mut li = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
            if GetLastInputInfo(&mut li).as_bool() { GetTickCount().wrapping_sub(li.dwTime) as f32 / 1000.0 } else { 0.0 }
        };

        self.watch_break(idle_s, dt);
        // what you asked the primary buddy to do (assist.rs) answers in his bubble
        if self.leads() {
            if let Some((l, secs)) = assist::next_line() {
                if self.mode == Mode::Sleep { self.mode = Mode::Stand; self.wait = 3.0; }
                self.say(&l, secs);
            }
        }

        match self.mode {
            Mode::Held => {
                if down { self.x = cx + self.grab.0; self.y = cy + self.grab.1; }
                else if self.y > line + 4.0 * s {
                    // dropped below the bar: teleport back up
                    self.perch = None;
                    self.y = line;
                    self.poof = Some(self.t);
                    self.mode = Mode::Stand;
                    self.wait = 2.0;
                    self.say("Teleport~", 1.5);
                } else { self.mode = Mode::Fall; self.vy = 0.0; }
            }
            Mode::Fall => {
                let ground = self.perch.unwrap_or(line);
                self.vy += 1800.0 * s * dt;
                self.y += self.vy * dt;
                if self.y >= ground {
                    self.y = ground;
                    self.hop = Some(self.t);
                    if std::mem::take(&mut self.sit_after) { self.mode = Mode::Sit; self.wait = 8.0 + self.rand() * 14.0; }
                    else { self.mode = Mode::Stand; self.wait = 2.0; self.say("Perfect landing.", 2.0); }
                }
            }
            Mode::Sleep => if idle_s < 2.0 { self.mode = Mode::Stand; self.wait = 3.0; self.say("Hm? Oh, you're back.", 3.0); },
            // he only sleeps when nothing is playing: a video is company, and his bubble stays up
            _ if idle_s > 180.0 && !ctx.media.as_ref().is_some_and(|m| m.playing) => { self.mode = Mode::Sleep; self.queued = None; }
            Mode::Walk => {
                let d = self.to - self.x;
                self.dir = d.signum();
                let stepx = SPEED * s * dt;
                if d.abs() <= stepx { self.x = self.to; self.mode = Mode::Stand; self.wait = 3.0 + self.rand() * 5.0; } else { self.x += self.dir * stepx; }
                // he walks on the top edge of the window under him, stepping up onto taller ones and dropping off edges
                if self.t >= self.ground_at {
                    self.ground_at = self.t + 0.1;
                    self.perch = window_top(self.mon, self.x, line).filter(|t| *t > line + 6.0 * s);
                }
                (self.y, self.vy) = settle(self.y, self.perch.unwrap_or(line), self.vy, 1800.0 * s, dt);
            }
            Mode::Stand | Mode::Sit => {
                self.wait -= dt;
                if self.wait <= 0.0 && self.purple.is_none() {
                    let r = self.rand();
                    if self.mode == Mode::Sit {
                        self.mode = Mode::Stand; self.wait = 1.0 + self.rand() * 2.0;
                        // he keeps his footing on the window: no teleport up to the bar
                    }
                    else if r < 0.5 && self.walk {
                        let (l, rt) = (self.mon.left as f32 + 40.0 * s, self.mon.right as f32 - 40.0 * s);
                        // a partner draws them together: most walks end beside the other buddy
                        self.to = match buddy_x {
                            Some(px) if self.rand() < 0.7 => (px + if self.rand() < 0.5 { -42.0 } else { 42.0 } * s).clamp(l, rt),
                            _ => l + self.rand() * (rt - l),
                        };
                        self.mode = Mode::Walk;
                    } else if r < 0.8 {
                        // on an open window's top edge when there is one below him, else on the bar
                        match window_top(self.mon, self.x, line) {
                            Some(t) if t > line + 6.0 * s => { self.perch = Some(t); self.sit_after = true; self.mode = Mode::Fall; self.vy = 0.0; }
                            _ => { self.mode = Mode::Sit; self.wait = 8.0 + self.rand() * 14.0; }
                        }
                    }
                    else { self.wait = 3.0 + self.rand() * 5.0; }
                }
                // looks at the pointer while it's near the bar
                let near = cy < line + 40.0 * s && cx >= self.mon.left as f32 && cx < self.mon.right as f32;
                if near && (cx - self.x).abs() > 4.0 * s { self.dir = (cx - self.x).signum(); }
                else if let Some(px) = buddy_x.filter(|px| !near && (px - self.x).abs() < 150.0 * s && (px - self.x).abs() > 4.0 * s) { self.dir = (px - self.x).signum(); }
            }
        }

        // Infinity: the pointer can't get closer than 60 px
        if self.infinity > self.t && !down && !self.hidden {
            let (ox, oy) = (self.x, self.y - 24.0 * s);
            let (dx, dy) = (cx - ox, cy - oy);
            let (d, rad) = ((dx * dx + dy * dy).sqrt(), 60.0 * s);
            if d < rad {
                let (ux, uy) = if d < 1.0 { (1.0, 0.0) } else { (dx / d, dy / d) };
                unsafe { let _ = SetCursorPos((ox + ux * rad) as i32, (oy + uy * rad) as i32); }
            }
        }

        let want = if self.hovered && self.mode != Mode::Held && self.mode != Mode::Sleep { 1.0 } else { 0.0 };
        self.peek += (want - self.peek).clamp(-dt * 5.0, dt * 5.0);
        if self.peek >= 1.0 && self.t - self.peek_said > 25.0 && !self.talking() {
            self.peek_said = self.t;
            let l = self.pick(self.cast.peek);
            self.say(&l, 3.0);
        }
        if self.purple.is_some_and(|p| self.t - p > 2.6) { self.purple = None; }
        if self.t > self.chat_at {
            self.chat_at = self.t + (150.0 + self.rand() * 150.0) / self.chatty.max(0.1);
            if self.chatty > 0.0 && !self.talking() && matches!(self.mode, Mode::Stand | Mode::Sit | Mode::Walk) {
                let l = if util::now().wHour < 5 { "It's late. Even I sleep sometimes.".to_string() } else { self.pick(self.cast.chat) };
                self.say(&l, 4.0);
            }
        }

        let hwnd = HWND(HWNDS[self.slot].load(Ordering::Relaxed) as *mut _);
        unsafe {
            let _ = SetWindowPos(hwnd, None, (self.x - FX * s).round() as i32, (self.y - FY * s).round() as i32, 0, 0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn sitting(&self) -> bool { matches!(self.mode, Mode::Sit | Mode::Sleep) }

    /// Which side of him the speech bubble goes: right unless that runs off the monitor.
    fn bubble_side(&self) -> f32 { if self.x + 170.0 * self.scale > self.mon.right as f32 { -1.0 } else { 1.0 } }
}

/// Where he's drawn: feet at (ox, oy), facing `dir`, tilted `rot` radians around the feet.
#[derive(Clone, Copy)]
struct Pose { ox: f32, oy: f32, dir: f32, rot: f32 }
impl Pose {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        let x = x * self.dir;
        let (sn, cs) = self.rot.sin_cos();
        (self.ox + x * cs - y * sn, self.oy + x * sn + y * cs)
    }
}

fn ell(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<(f32, f32)> {
    (0..28).map(|i| { let a = i as f32 / 28.0 * std::f32::consts::TAU; (cx + rx * a.cos(), cy + ry * a.sin()) }).collect()
}

fn rr(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Vec<(f32, f32)> {
    let mut v = vec![];
    for (cx, cy, a0) in [(x1 - r, y0 + r, -90.0f32), (x1 - r, y1 - r, 0.0), (x0 + r, y1 - r, 90.0), (x0 + r, y0 + r, 180.0)] {
        for k in 0..=4 { let a = (a0 + k as f32 * 22.5).to_radians(); v.push((cx + r * a.cos(), cy + r * a.sin())); }
    }
    v
}

/// A quad from (x0, y0) to (x1, y1), `w` wide: a limb.
fn limb(x0: f32, y0: f32, x1: f32, y1: f32, w: f32) -> Vec<(f32, f32)> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let l = (dx * dx + dy * dy).sqrt().max(0.01);
    let (nx, ny) = (-dy / l * w / 2.0, dx / l * w / 2.0);
    vec![(x0 + nx, y0 + ny), (x1 + nx, y1 + ny), (x1 - nx, y1 - ny), (x0 - nx, y0 - ny)]
}

struct Ink<'c, 'a> { c: &'c mut Canvas<'a>, pose: Pose, over: Option<Color> }
impl Ink<'_, '_> {
    fn fill(&mut self, pts: &[(f32, f32)], col: Color) {
        let v: Vec<_> = pts.iter().map(|&(x, y)| self.pose.p(x, y)).collect();
        if let Some(g) = polygon(&v) { self.c.fill_path(&g, self.over.unwrap_or(col)); }
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), w: f32, col: Color) {
        if self.over.is_none() { self.c.line(self.pose.p(a.0, a.1), self.pose.p(b.0, b.1), w, col); }
    }
}

const SKIN: Color = 0xFFF7E0D3;
/// Height of a picture buddy in CSS px, about the drawn figure's.
const SPRITE_H: f32 = 52.0;
const BAND: Color = 0xFF16161D;
const EYE_DARK: Color = 0xFF20202A;
const SHOE: Color = 0xFF09090D;
const MOUTH: Color = 0xFF9A5050;

/// What the drawing needs from the state for one frame.
struct Look<'c> { walk: Option<f32>, sit: f32, swing: f32, peek: f32, arm: f32, talk: bool, hair: f32, style: Hair, c: &'c Cast, trim: &'c Trim }

/// The chibi, feet at the pose origin, y up is negative. Colours are replaced by `over` in the
/// shadow pass.
fn figure(k: &mut Ink, l: &Look<'_>) {
    // legs and shoes
    // `e` blends standing (0) into sitting (1): the body sinks onto the ledge, the legs bend out to the knee
    let e = l.sit * l.sit * (3.0 - 2.0 * l.sit);
    let u = 10.0 * e;
    let mix = |a: f32, b: f32| a + (b - a) * e;
    for side in [-1.0f32, 1.0] {
        let hx = side * 2.6;
        let stand = if let Some(ph) = l.walk {
            let sw = (ph * side).sin();
            (hx + sw * 2.4, -2.0 - (ph * side).cos().max(0.0) * 1.3)
        } else { (hx, -2.0) };
        // thigh out along the ledge, shin hanging from the knee and swinging
        let knee = (if side > 0.0 { 10.0 } else { 8.0 }, 1.5);
        let a = (l.swing + if side > 0.0 { 0.0 } else { 2.2 }).sin() * 0.4;
        let sit = (knee.0 + a.sin() * 9.0, knee.1 + a.cos() * 9.0);
        let hip = (mix(hx, hx * 0.5), mix(-10.0, 1.0));
        let foot = (mix(stand.0, sit.0), mix(stand.1, sit.1));
        // standing, the knee is halfway down a straight leg
        let kn = (mix((hip.0 + foot.0) / 2.0, knee.0), mix((hip.1 + foot.1) / 2.0, knee.1));
        k.fill(&limb(hip.0, hip.1, kn.0, kn.1, mix(3.6, 3.8)), l.c.pants);
        k.fill(&limb(kn.0, kn.1, foot.0, foot.1, 3.6), l.c.pants);
        k.fill(&ell(foot.0 + 0.7, foot.1 + 0.7, 2.7, 1.6), SHOE);
    }
    // coat, collar, zip
    k.fill(&rr(-7.2, -21.0 + u, 7.2, -8.5 + u, 3.0), l.c.coat);
    k.fill(&rr(-4.4, -22.6 + u, 4.4, -19.2 + u, 1.5), l.c.coat_hi);
    k.line((0.0, -19.2 + u), (0.0, -9.5 + u), 0.6, l.c.coat_hi);
    // arms: hands in pockets; the front one points forward for Hollow Purple
    k.fill(&limb(-6.4, -19.5 + u, -8.0, -11.5 + u, 3.0), l.c.coat);
    let (hx, hy) = (8.0 + l.arm * 6.0, -11.5 + u - l.arm * 11.0);
    k.fill(&limb(6.4, -19.5 + u, hx, hy, 3.0), l.c.coat);
    if l.arm > 0.05 { k.fill(&ell(hx + 0.8, hy - 0.4, 1.8, 1.8), l.trim.skin.unwrap_or(SKIN)); }
    // hair behind the head: the spikes the blindfold pushes up
    let b = l.hair;
    let behind: Vec<(f32, f32)> = match l.style {
        Hair::Spiky => vec![
            (-10.4, -29.0), (-13.4, -35.5), (-10.2, -35.0), (-12.0, -41.5 + b), (-7.2, -38.6), (-6.4, -45.5 + b), (-2.6, -40.4),
            (0.6, -48.0 + b), (3.2, -40.8), (7.2, -45.8 + b), (7.8, -39.4), (12.4, -41.8 + b), (10.6, -35.4), (13.8, -34.6), (10.6, -29.0),
        ],
        Hair::Short => vec![],
        Hair::Long => rr(-11.6, -37.0 + u, 11.6, -13.0 + u, 4.5),
        Hair::Mohawk => vec![(-3.4, -38.0), (-3.0, -45.0 + b), (-1.4, -42.0), (0.6, -49.5 + b), (2.2, -42.0), (3.6, -45.0 + b), (3.8, -38.0)],
    };
    let behind: Vec<_> = behind.iter().map(|&(x, y)| (x, y + u)).collect();
    if !behind.is_empty() { k.fill(&behind, l.c.hair); }
    let streaks: &[(f32, f32, f32)] = match l.style {
        Hair::Spiky => &[(-6.0, -9.5, -39.0), (-1.0, -4.5, -42.5), (3.5, 5.5, -42.0), (7.5, 10.0, -38.5)],
        Hair::Long => &[(-8.0, -9.0, -18.0), (8.0, 9.0, -18.0)],
        _ => &[],
    };
    for &(x0, x1, y1) in streaks {
        k.line((x0, -36.0 + u), (x1, y1 + u + b * 0.6), 0.7, l.c.hair_sh);
    }
    // head, hair cap, blindfold, face (features nudged 1.2 px towards where he faces)
    k.fill(&ell(0.0, -31.0 + u, 10.0, 9.4), l.trim.skin.unwrap_or(SKIN));
    let mut cap: Vec<_> = (0..=14).map(|i| {
        let a = std::f32::consts::PI * (1.0 + i as f32 / 14.0);
        (10.5 * a.cos(), -31.0 + u + 10.1 * a.sin())
    }).collect();
    cap.extend([(10.5, -31.5 + u), (6.0, -30.0 + u), (3.0, -31.5 + u), (-1.0, -29.6 + u), (-5.0, -31.4 + u), (-10.5, -31.5 + u)]);
    k.fill(&cap, l.c.hair);
    let f = 1.2;
    if l.c.bun { k.fill(&ell(0.6, -49.5 + u + b, 4.2, 4.2), l.c.hair); }
    if l.trim.ears {
        for side in [-1.0f32, 1.0] {
            k.fill(&[(side * 9.6, -34.0 + u), (side * 8.6, -45.0 + u + b), (side * 3.2, -39.6 + u)], l.c.hair);
            k.fill(&[(side * 8.2, -36.0 + u), (side * 7.7, -42.0 + u + b), (side * 5.0, -38.6 + u)], 0xFFF0A0B0);
        }
    }
    if !l.c.blindfold {
        for ex in [-4.0f32, 4.4] { k.fill(&ell(ex + f, -28.6 + u, 1.3, 1.7), EYE_DARK); }
        if l.trim.glasses {
            for ex in [-4.0f32, 4.4] {
                let ring = ell(ex + f, -28.6 + u, 3.3, 3.0);
                for (i, &a) in ring.iter().enumerate() { k.line(a, ring[(i + 1) % ring.len()], 0.7, BAND); }
            }
            k.line((-0.7 + f, -28.8 + u), (1.1 + f, -28.8 + u), 0.7, BAND);
        }
    } else {
        if l.peek > 0.02 {
            for ex in [-4.0f32, 4.4] {
                let x = ex + f;
                k.fill(&ell(x, -28.6 + u, 3.6 * l.peek, 3.6 * l.peek), alpha(l.c.eye, 0.28));
                k.fill(&ell(x, -28.6 + u, 1.9, 2.3 * l.peek), l.c.eye);
                k.fill(&ell(x + 0.2, -28.4 + u, 1.0, 1.3 * l.peek), l.c.eye_d);
                k.fill(&ell(x - 0.6, -29.4 + u, 0.5, 0.5), 0xFFFFFFFF);
                k.line((x - 2.2, -31.0 + u), (x + 2.2, -31.3 + u), 0.7, l.c.hair);
            }
        }
        let up = l.peek * 6.5;
        k.fill(&rr(-10.8, -33.2 + u - up, 10.8, -27.2 + u - up, 1.6), BAND);
    }
    if l.talk { k.fill(&ell(0.8 + f, -24.4 + u, 1.2, 0.9), MOUTH); }
    else { k.line((-1.2 + f, -24.4 + u), (2.4 + f, -25.2 + u), 0.75, MOUTH); }
}

impl View for Gojo {
    fn size(&self, _g: &Gfx, _cx: &Ctx, _s: f32) -> (f32, f32) { (W, H) }

    fn frame_ms(&self) -> u32 {
        if self.hidden { return 500; }
        if matches!(self.mode, Mode::Held | Mode::Fall) || self.purple.is_some() || self.infinity > self.t { 16 }
        else if self.mode == Mode::Walk || self.hop.is_some() || (self.peek > 0.0 && self.peek < 1.0) || self.poof.is_some() { 33 }
        else { 100 }
    }

    fn input(&mut self, ev: Input, _cx: &mut Ctx) {
        let Input::Click(_, b) = ev else { return };
        if std::mem::take(&mut self.dragged) && b == Btn::Left { return; } // the end of a drag isn't a click
        match b {
            Btn::Left if self.t - std::mem::replace(&mut self.last_click, self.t) < 0.45 => self.input(Input::Click(1, Btn::Double), _cx),
            Btn::Left => {
                if self.mode == Mode::Sleep { self.mode = Mode::Stand; self.wait = 3.0; self.say("Five more minutes...", 2.5); return; }
                self.hop = Some(self.t);
                let l = self.pick(self.cast.click);
                self.say(&l, 3.5);
            }
            Btn::Double => {
                if self.purple.is_some() { return; }
                if self.mode == Mode::Walk || self.sitting() { self.mode = Mode::Stand; }
                self.wait = 4.0;
                self.purple = Some(self.t);
                self.say(self.cast.shout, 2.6);
            }
            Btn::Right if !self.cast.blindfold => { let l = self.pick(self.cast.right); self.say(&l, 2.5); }
            Btn::Right => {
                if self.infinity > self.t { self.infinity = 0.0; self.say("Fine, you can touch me.", 2.0); }
                else { self.infinity = self.t + 10.0; self.say("Infinity. You can't touch me~", 3.0); }
            }
            Btn::Middle => {}
        }
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        self.step(c.scale, cx);
        self.react(cx);
        if self.hidden { return; }
        let t = self.t;
        let hop = self.hop.map_or(0.0, |h0| { let p = (t - h0) / 0.45; if p >= 1.0 { -1.0 } else { 4.0 * p * (1.0 - p) * 7.0 } });
        if hop < 0.0 { self.hop = None; }
        // The gait follows the ground he covers, not the clock: his legs never skate, and at walking
        // speed they swing about once a second instead of running away from his 28 px/s crawl.
        let walk = (self.mode == Mode::Walk).then_some(self.x * 0.22);
        let bob = walk.map_or(0.0, |w| (w * 2.0).cos().abs() * 0.8);
        let held = self.mode == Mode::Held;
        let rot = if held { (t * 4.0).sin() * 0.18 } else if self.mode == Mode::Sleep { 0.07 } else { 0.0 };
        let pt = self.purple.map(|p| t - p);
        let arm = pt.map_or(0.0, |p| (p / 0.3).min(1.0) * if p > 2.0 { (2.6 - p) / 0.6 } else { 1.0 });
        let pose = Pose { ox: FX, oy: FY - hop.max(0.0) - bob, dir: self.dir, rot };
        self.sit_k = ease_toward(self.sit_k, if self.sitting() { 1.0 } else { 0.0 }, (t - self.k_t).clamp(0.0, 0.1));
        self.k_t = t;
        let sprite = self.load_sprite(c);
        let look = Look {
            walk, sit: self.sit_k, swing: t * if self.mode == Mode::Sleep { 1.2 } else { 1.5 }, peek: self.peek.clamp(0.0, 1.0), arm,
            talk: self.talking() && (t * 7.0).sin() > 0.0 && self.mode != Mode::Sleep, hair: if held { (t * 9.0).sin() * 1.2 } else { -bob }, style: self.style, c: &self.cast, trim: &self.trim,
        };

        if self.infinity > t {
            for i in 0..3 {
                let r = 24.0 + i as f32 * 7.0 + (t * 10.0) % 7.0;
                c.arc(FX, FY - 24.0, r, 1.0, 1.0, alpha(self.cast.eye, 0.45 * (1.0 - (r - 24.0) / 24.0)));
            }
        }
        match &self.sprite {
            // a picture stands where the figure's feet would be, and sits a little lower on a ledge
            Some((_, _, Some(img))) if sprite => {
                let sway = walk.map_or(0.0, |w| (w * 2.0).sin() * 0.05);
                c.sprite(img, FX, pose.oy + 4.0 * self.sit_k, self.dir, rot + sway);
            }
            _ => c.shadow(0.6, 2.4, 0.7, |c, over| figure(&mut Ink { c, pose, over }, &look)),
        }
        c.hit(R::new(FX - 11.0, FY - 48.0, 22.0, if self.sitting() { 59.0 } else { 49.0 }), 1);

        // Hollow Purple: blue and red gather, meet in front of him, and fire
        if let Some(p) = pt {
            let d = self.dir;
            let orb = |c: &mut Canvas, x: f32, y: f32, r: f32, col: Color, a: f32| {
                c.circle(x, y, r * 2.2, alpha(col, 0.22 * a));
                c.circle(x, y, r, alpha(col, a));
                c.circle(x, y, r * 0.5, alpha(0xFFFFFFFF, 0.85 * a));
            };
            let y = FY - 26.0;
            if p < 1.4 {
                let g = (p / 0.9).min(1.0);
                let m = ((p - 0.9) / 0.5).clamp(0.0, 1.0);
                let r = 6.0 * g;
                let at = |side: f32| FX + d * (14.0 + (side * 40.0 - 14.0) * (1.0 - m * m));
                orb(c, at(-1.0), y, r, self.cast.orbs.0, 1.0);
                orb(c, at(1.0), y, r, self.cast.orbs.1, 1.0);
            } else {
                let q = p - 1.4;
                let a = if q > 0.7 { (1.2 - q).max(0.0) / 0.5 } else { 1.0 };
                orb(c, FX + d * (14.0 + q * q * 300.0), y, 7.0 + q * 12.0, self.cast.orbs.2, a);
            }
        }
        if let Some(p0) = self.poof {
            let p = (t - p0) / 0.5;
            if p >= 1.0 { self.poof = None; } else {
                c.arc(FX, FY - 20.0, 8.0 + p * 22.0, 1.0, 2.0 * (1.0 - p), alpha(self.cast.eye, 1.0 - p));
                for i in 0..6 {
                    let a = i as f32 * 1.047 + p;
                    c.circle(FX + a.cos() * (10.0 + p * 20.0), FY - 20.0 + a.sin() * (10.0 + p * 20.0), 1.4 * (1.0 - p), alpha(self.cast.hair, 1.0 - p));
                }
            }
        }
        if self.mode == Mode::Sleep {
            for i in 0..3 {
                let p = (t * 0.6 + i as f32 / 3.0) % 1.0;
                let r = R::new(FX + 9.0 + p * 10.0, FY - 22.0 - p * 26.0, 20.0, 16.0);
                c.text("z", r, txt(8.0 + p * 6.0, 700), alpha(cx.theme.c("onSurface"), 1.0 - p), Align::Left);
            }
        }

        // speech bubble beside his head
        if let Some((text, until)) = self.bubble.clone() {
            if until <= t { self.bubble = None; return; }
            let f = txt(11.0, 500);
            let tw = c.g.measure(&text, f).0.min(W - FX - 17.0 - 16.0 - 4.0); // the bubble must fit in the window
            let th = c.g.measure_wrapped(&text, f, tw + 1.0);
            let (bw, bh) = (tw + 16.0, th + 10.0);
            let side = self.bubble_side();
            let bx = if side > 0.0 { FX + 17.0 } else { FX - 17.0 - bw };
            let by = (FY - 30.0 - bh / 2.0 - hop.max(0.0)).max(2.0);
            let fade = ((until - t) / 0.3).min(1.0);
            let (bg, fg) = (alpha(cx.theme.c("surfaceContainerHigh"), fade), alpha(cx.theme.c("onSurface"), fade));
            c.shadow(1.0, 4.0, 0.35 * fade, |c, over| {
                c.rect(R::new(bx, by, bw, bh), 9.0, over.unwrap_or(bg));
                let tx = if side > 0.0 { bx } else { bx + bw };
                if let Some(g) = polygon(&[(tx, by + bh / 2.0 - 4.0), (tx - side * 6.0, by + bh / 2.0 + 2.0), (tx, by + bh / 2.0 + 4.0)]) {
                    c.fill_path(&g, over.unwrap_or(bg));
                }
            });
            c.para(&text, R::new(bx + 8.0, by + 5.0, tw + 1.0, th + 2.0), f, fg, Align::Left);
        }
    }
}

/// Moves `k` towards `target` at a constant rate, so a full sit or stand takes `SIT_SECS`.
fn ease_toward(k: f32, target: f32, dt: f32) -> f32 {
    let step = dt / SIT_SECS;
    if (target - k).abs() <= step { target } else { k + step * (target - k).signum() }
}
const SIT_SECS: f32 = 0.45;

/// One step of getting to `ground` (screen y): up is instant, down falls with gravity `g`. Returns (y, vy).
fn settle(y: f32, ground: f32, vy: f32, g: f32, dt: f32) -> (f32, f32) {
    if y >= ground { return (ground, 0.0); }
    let vy = vy + g * dt;
    ((y + vy * dt).min(ground), vy)
}

/// The monitor rect under `p` (or the primary one).
fn monitor(p: POINT, primary: bool) -> RECT {
    unsafe {
        let h = MonitorFromPoint(p, if primary { MONITOR_DEFAULTTOPRIMARY } else { MONITOR_DEFAULTTONEAREST });
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(h, &mut mi);
        mi.rcMonitor
    }
}

/// Top edge (screen px) of the front-most open window that has a part under x and hangs below the bar's
/// line on `mon`; None when the windows reach the bar (nothing to drop onto).
fn window_top(mon: RECT, x: f32, line: f32) -> Option<f32> {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    struct Scan { mon: RECT, x: i32, line: i32, top: Option<i32> }
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        let sc = &mut *(lp.0 as *mut Scan);
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || GetWindowTextLengthW(h) == 0 { return BOOL(1); }
        if GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 { return BOOL(1); }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        if cloaked != 0 { return BOOL(1); }
        let mut cls = [0u16; 32];
        let n = GetClassNameW(h, &mut cls) as usize;
        if matches!(String::from_utf16_lossy(&cls[..n]).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "zor-ui-widget") { return BOOL(1); }
        let mut r = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, size_of::<RECT>() as u32).is_err() { return BOOL(1); }
        if r.left > sc.x || r.right <= sc.x || r.bottom <= sc.line + 40 || r.top >= sc.mon.bottom || r.top < sc.mon.top { return BOOL(1); }
        sc.top = Some(r.top);
        BOOL(0) // EnumWindows runs front to back: the first hit is the one in front
    }
    let mut sc = Scan { mon, x: x as i32, line: line as i32, top: None };
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut sc as *mut Scan as isize)); }
    sc.top.map(|t| t as f32)
}

/// A full-screen app (game, video) in front on `mon`: he hides so he isn't drawn over it.
fn fullscreen_on(mon: RECT) -> bool { fullscreen_monitor() == Some(mon) }

/// True when window rect `r` fills `mon`: it may overhang by the invisible resize border (7 px on Windows 11).
fn covers(r: RECT, mon: RECT) -> bool {
    const SLACK: i32 = 16;
    r.left <= mon.left && r.top <= mon.top && r.right >= mon.right && r.bottom >= mon.bottom
        && r.left >= mon.left - SLACK && r.top >= mon.top - SLACK && r.right <= mon.right + SLACK && r.bottom <= mon.bottom + SLACK
}

/// The monitor that the foreground window covers completely, if any.
pub fn fullscreen_monitor() -> Option<RECT> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() { return None; }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() { return None; }
        let mon = monitor(POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }, false);
        if !covers(r, mon) { return None; }
        let mut cls = [0u16; 32];
        let n = GetClassNameW(fg, &mut cls) as usize;
        (!matches!(String::from_utf16_lossy(&cls[..n]).as_str(), "Progman" | "WorkerW")).then_some(mon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feet_stand_on_the_bar_wherever_it_is() {
        // 1080p monitor, 1.0 scale: a top bar's bottom, a bottom bar's top, a side bar leaves the monitor's bottom
        assert_eq!(feet_line(Edge::Top, 0, 1080, 1.0), 56.0);
        assert_eq!(feet_line(Edge::Bottom, 0, 1080, 1.0), 1024.0);
        assert_eq!((feet_line(Edge::Left, 0, 1080, 1.0), feet_line(Edge::Right, 0, 1080, 1.0)), (1080.0, 1080.0));
        // a second monitor above the first and a 150 % scale
        assert_eq!(feet_line(Edge::Bottom, -1080, 0, 1.5), -84.0);
    }
    #[test]
    fn fullscreen_may_overhang_by_its_border() {
        let m = RECT { left: -1920, top: 0, right: 0, bottom: 1080 };
        assert!(covers(m, m));
        assert!(covers(RECT { left: -1927, top: 0, right: 7, bottom: 1087 }, m));
        assert!(!covers(RECT { left: -1920, top: 56, right: 0, bottom: 1080 }, m)); // tiled below the bar
        assert!(!covers(RECT { left: -3840, top: 0, right: 0, bottom: 1080 }, m)); // spans two monitors
    }
    #[test]
    fn pose_mirrors_and_tilts() {
        let p = Pose { ox: 10.0, oy: 20.0, dir: -1.0, rot: 0.0 };
        assert_eq!(p.p(3.0, -5.0), (7.0, 15.0));
        let q = Pose { ox: 0.0, oy: 0.0, dir: 1.0, rot: std::f32::consts::FRAC_PI_2 };
        let (x, y) = q.p(1.0, 0.0);
        assert!(x.abs() < 1e-5 && (y - 1.0).abs() < 1e-5);
    }
    #[test]
    fn settle_steps_up_at_once_and_falls_down() {
        assert_eq!(settle(300.0, 200.0, 5.0, 1800.0, 0.016), (200.0, 0.0), "a taller window is stepped onto");
        let (y, vy) = settle(200.0, 300.0, 0.0, 1800.0, 0.016);
        assert!(y > 200.0 && y < 300.0 && vy > 0.0, "off an edge he falls: {y} {vy}");
        assert_eq!(settle(299.9, 300.0, 400.0, 1800.0, 0.1).0, 300.0, "and stops at the ground");
    }
    #[test]
    fn standing_up_takes_a_moment_and_never_overshoots() {
        let mut k = 1.0;
        k = ease_toward(k, 0.0, 0.016);
        assert!(k > 0.9 && k < 1.0, "first frame barely moves: {k}");
        for _ in 0..40 { k = ease_toward(k, 0.0, 0.016); }
        assert_eq!(k, 0.0);
        assert_eq!(ease_toward(0.5, 1.0, 10.0), 1.0, "a long frame clamps at the target");
    }
    #[test]
    fn rand_stays_in_range() {
        let mut g = Gojo::new();
        for _ in 0..1000 { let r = g.rand(); assert!((0.0..1.0).contains(&r)); }
        let click = cast::CASTS[0].click;
        assert!(click.contains(&g.pick(click).as_str()));
    }

    /// A quiet desktop: music on, plugged in, online, clear sky.
    fn calm() -> Around {
        Around {
            track: "Blue in Green\tMiles Davis".into(), playing: true, volume: Some(40), muted: Some(false),
            battery: Some(80), charging: Some(true), online: true, weather: "clear_day", note: String::new(), busy: false, claude: Some(30),
        }
    }
    #[test]
    fn notices_claude_limit_only_on_the_way_up() {
        let a = calm();
        let at = |p| Around { claude: Some(p), ..a.clone() };
        assert_eq!(notice(&a, &at(79)), None);
        assert_eq!(notice(&a, &at(80)).as_deref(), Some("Claude's at 80% of the limit. Pace yourself."));
        assert_eq!(notice(&at(80), &at(90)), None, "same tier says nothing again");
        assert_eq!(notice(&at(90), &at(96)).as_deref(), Some("Claude's at 96%. Even I can't beat a rate limit."));
        assert_eq!(notice(&at(96), &at(5)), None, "a reset is not a remark");
        assert_eq!(notice(&a, &Around { claude: None, ..a.clone() }), None);
    }
    #[test]
    fn notice_fires_once_per_change() {
        let a = calm();
        assert_eq!(notice(&a, &a), None);
        let mut b = a.clone();
        b.track = "Kikufuku\tSatoru".into();
        assert_eq!(notice(&a, &b).as_deref(), Some("Kikufuku, huh? Good taste."));
        assert_eq!(notice(&b, &b), None);
        let mut c = b.clone();
        c.battery = Some(10);
        c.charging = Some(false);
        assert_eq!(notice(&b, &c).as_deref(), Some("Battery's low. Find a plug, would you?"));
        assert_eq!(notice(&c, &b).as_deref(), Some("Charging. Good."));
        assert_eq!(notice(&c, &c), None);
    }
    #[test]
    fn notices_audio_media_and_weather() {
        let a = calm();
        let mut b = a.clone();
        b.volume = Some(75);
        assert_eq!(notice(&a, &b).as_deref(), Some("That's loud. I approve."));
        let mut c = a.clone();
        c.muted = Some(true);
        assert_eq!(notice(&a, &c).as_deref(), Some("Muted? I can't hear a thing."));
        let mut d = a.clone();
        d.note = "spotify.exe\tSomebody".into();
        assert_eq!(notice(&a, &d).as_deref(), Some("Word from Spotify."));
        let mut e = a.clone();
        e.playing = false;
        assert_eq!(notice(&a, &e).as_deref(), Some("Aw, the music stopped."));
        let mut f = a.clone();
        f.online = false;
        assert_eq!(notice(&a, &f).as_deref(), Some("The internet's gone. Not my doing."));
        let mut g = a.clone();
        g.weather = "light_rain_night";
        assert_eq!(notice(&a, &g).as_deref(), Some("It's raining out there. Stay in."));
        assert_eq!(notice(&a, &calm()), None, "an unchanged look says nothing");
        assert_eq!(cut("Hollow Purple", 6), "Hollow…");
        assert_eq!(cut("Nah", 6), "Nah");
    }

    /// Tests that touch the shared PRIMARY/PEERS statics take this, so they don't race.
    static SHARED: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn focus_kind_sorts_apps() {
        assert_eq!(focus_kind("main.rs - zor-ui", "Code.exe"), "code");
        assert_eq!(focus_kind("Lo-fi - YouTube - Google Chrome", "chrome.exe"), "video");
        assert_eq!(focus_kind("Home / X - Reddit", "msedge.exe"), "social");
        assert_eq!(focus_kind("Docs", "chrome.exe"), "");
        assert_eq!(focus_kind("general", "Discord.exe"), "chat");
        assert_eq!(focus_kind("Steam", "steam.exe"), "game");
        assert_eq!(focus_kind("", "notepad.exe"), "");
    }

    #[test]
    fn break_reminder_fires_once_and_resets_on_pause() {
        let _l = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        PRIMARY.store(0, Ordering::Relaxed);
        let mut g = Gojo::new();
        g.break_mins = 1.0;
        for _ in 0..50 { g.watch_break(0.0, 1.0); }
        assert!(g.bubble.is_none(), "not yet: 50 s of work");
        for _ in 0..10 { g.watch_break(0.0, 1.0); }
        let b = g.bubble.clone().expect("reminder after 60 s").0;
        assert!(b.starts_with("1 minute straight. Break time!"), "{b}");
        assert!(g.work < 1.0, "work counter restarts");
        g.work = 30.0;
        g.watch_break(400.0, 1.0);
        assert_eq!(g.work, 0.0, "a 5 minute pause resets the count");
        g.work = 30.0;
        g.watch_break(120.0, 1.0);
        assert_eq!(g.work, 30.0, "idle 1-5 min neither counts nor resets");
        g.break_mins = 0.0;
        g.bubble = None;
        g.work = 99999.0;
        g.watch_break(0.0, 1.0);
        assert!(g.bubble.is_none(), "0 minutes is off too");
        g.break_mins = 1.0;
        g.breaks = false;
        g.watch_break(0.0, 1.0);
        assert!(g.bubble.is_none(), "the switch turns reminders off");
    }

    #[test]
    fn only_the_primary_leads() {
        let _l = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        let (a, mut b) = (Gojo::new(), Gojo::slot(1));
        let on = |i: usize, v: bool| PEERS.lock().unwrap()[i].on = v;
        PRIMARY.store(1, Ordering::Relaxed);
        on(0, true); on(1, true);
        assert!(!a.leads() && b.leads(), "buddy 2 is primary");
        on(1, false);
        assert!(a.leads(), "primary off: the other takes over");
        PRIMARY.store(0, Ordering::Relaxed);
        on(1, true);
        assert!(a.leads() && !b.leads(), "buddy 1 is primary");
        b.break_mins = 1.0; b.work = 999.0;
        b.watch_break(0.0, 1.0);
        assert!(b.bubble.is_none(), "a non-primary never reminds");
        on(0, false); on(1, false);
    }

    #[test]
    fn focus_remark_waits_20s_and_then_cools_down() {
        let _l = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        PRIMARY.store(0, Ordering::Relaxed);
        let mut cx = Ctx::new(false);
        cx.wm.window = Some(("main.rs".into(), "Code.exe".into()));
        let mut g = Gojo::new();
        g.watch_focus(&cx);
        assert_eq!(g.focus, "code");
        g.t = 10.0; g.watch_focus(&cx);
        assert!(g.queued.is_none(), "only 10 s in front");
        g.t = 25.0; g.watch_focus(&cx);
        assert!(g.queued.is_some(), "20 s passed");
        g.queued = None; g.t = 100.0; g.watch_focus(&cx);
        assert!(g.queued.is_none(), "same kind again within 15 min");
        g.t = 1000.0; g.watch_focus(&cx);
        assert!(g.queued.is_some(), "after the cooldown");
        g.queued = None; g.watch = false; g.focus_said.1 = -9999.0; g.t = 2000.0; g.watch_focus(&cx);
        assert!(g.queued.is_none(), "switch off");
    }
}
