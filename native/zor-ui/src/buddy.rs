//! A small Gojo who lives on the bar: walks along it, sits on its edge with his legs hanging, falls
//! asleep when you're away, and turns to look at the pointer when it comes near the bar.
//! Hover: he lifts the blindfold. Click: he talks. Double-click and right-click play the character's moves: Gojo's are
//! Hollow Purple and Infinity (for 10 s the pointer can't reach him); see `cast::Move`. Drag him anywhere; let go and he falls back onto the bar,
//! or teleports back if you dropped him below it.
//! He also watches the desktop and comments on what happens: a new track, the volume, the battery, the
//! network, the weather, notifications, a busy CPU.
//! Turned on by config.json "native": {"gojo": true}. Drawn from shapes, no image files.

use crate::{
    app::{App, Ctx},
    assist,
    bar::{pretty_app, Edge},
    cast::{self, Cast, Hair, Move, Say, Trim},
    desk::greeting,
    gfx::{alpha, polygon, txt, Align, Canvas, Color, Gfx, R},
    pack::{self, Pack},
    util,
    win::{Btn, Input, View, Win, Z},
};
use std::{
    collections::HashMap,
    sync::{atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicUsize, Ordering}, Arc},
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

/// Says `s` as Buddy `slot`'s voice through the Voice API (src/voice): the provider, its voice and the queue
/// are chosen there, so this file names no provider. Off unless "native": {"gojoVoice": true}.
fn speak(slot: usize, s: &str) {
    if !TALK.load(Ordering::Relaxed) { return; }
    crate::voice::speak(slot, s, crate::voice::queue::Priority::Reply);
}

/// True while Gojo is turned on; the Claude usage poll only runs then.
pub fn on() -> bool { ON.load(Ordering::Relaxed) }

/// Window size and where his feet sit in it (CSS px). The window's top lines up with the monitor
/// top while he stands on the bar; the rest is transparent, so clicks pass through it.
const W: f32 = 340.0;
const H: f32 = 100.0;
const FX: f32 = W / 2.0;
/// The feet's height in the window for the drawn figure; a taller picture raises it (`Gojo::fy`) and the window grows
/// upward with it, keeping `H - FY` below the feet.
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

/// What he'd say about a change since `old`, in the words of `s`; None when nothing worth a line happened. A reaction
/// whose line is empty is skipped, so a pack can silence one.
fn notice(old: &Around, new: &Around, s: &Say) -> Option<String> {
    let say = |t: &str, pairs: &[(&str, &str)]| -> Option<String> {
        (!t.is_empty()).then(|| pairs.iter().fold(t.to_string(), |l, (k, v)| l.replace(k, v)))
    };
    let low = |a: &Around| a.battery.is_some_and(|p| p <= 15) && a.charging != Some(true);
    if low(new) && !low(old) { if let Some(l) = say(s.low_battery, &[]) { return Some(l); } }
    // only on the way up: a window resetting back to 0 % needs no remark
    if claude_tier(new.claude) > claude_tier(old.claude) {
        let p = new.claude.unwrap_or(0);
        if let Some(l) = say(if p >= 95 { s.claude_high } else { s.claude_near }, &[("{pct}", &p.to_string())]) { return Some(l); }
    }
    if !new.note.is_empty() && new.note != old.note {
        if let Some(l) = say(s.note, &[("{app}", &pretty_app(new.note.split('\t').next().unwrap_or("")))]) { return Some(l); }
    }
    if !new.track.is_empty() && new.track != old.track {
        if let Some(l) = say(s.track, &[("{track}", &cut(new.track.split('\t').next().unwrap_or(""), 28))]) { return Some(l); }
    }
    if new.playing != old.playing && !new.track.is_empty() {
        if let Some(l) = say(if new.playing { s.music_on } else { s.music_off }, &[]) { return Some(l); }
    }
    if let (Some(a), Some(b)) = (old.muted, new.muted) {
        if a != b { if let Some(l) = say(if b { s.muted } else { s.unmuted }, &[]) { return Some(l); } }
    }
    if let (Some(a), Some(b)) = (old.volume, new.volume) {
        if !new.muted.unwrap_or(false) && (a as i16 - b as i16).abs() >= 20 {
            if let Some(l) = say(if b > a { s.loud } else { s.quiet }, &[]) { return Some(l); }
        }
    }
    if new.online != old.online {
        if let Some(l) = say(if new.online { s.online } else { s.offline }, &[]) { return Some(l); }
    }
    if let (Some(a), Some(b)) = (old.charging, new.charging) {
        if a != b { if let Some(l) = say(if b { s.charging } else { s.unplugged }, &[]) { return Some(l); } }
    }
    if new.weather != old.weather {
        let line = match new.weather {
            w if w.starts_with("thunder") => s.thunder,
            w if w.starts_with("snow") => s.snow,
            w if w.contains("rain") => s.rain,
            _ => "",
        };
        if let Some(l) = say(line, &[]) { return Some(l); }
    }
    if new.busy && !old.busy { if let Some(l) = say(s.busy, &[]) { return Some(l); } }
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
    crate::voice::configure(app.cx.cfg.get("voice"), app.cx.cfg.str("buddies.a.character"), app.cx.cfg.str("buddies.b.character"));
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
enum Mode { Stand, Walk, Sit, Sleep, Held, Fall, Point }

/// Seconds a move plays. A shield isn't timed here: it lasts as long as `infinity` says.
fn move_secs(m: Move) -> f32 { match m { Move::Orbs => 2.6, Move::Beam => 2.4, Move::Aura => 2.0, Move::Shield | Move::None => 0.0 } }

/// The beam's state `p` seconds into its move of `dur`: how charged the ball is (0 to 1), how long the beam is (CSS px,
/// nothing until the ball is charged, then out to `BEAM_MAX` in a quarter second) and how visible it is (fading out
/// over the last half second). It stays inside his 340 px window.
struct Beam { charge: f32, len: f32, alpha: f32 }
const BEAM_CHARGE: f32 = 0.9;
const BEAM_MAX: f32 = 140.0;
fn beam(p: f32, dur: f32) -> Beam {
    Beam { charge: (p / BEAM_CHARGE).clamp(0.0, 1.0), len: BEAM_MAX * smooth((p - BEAM_CHARGE) / 0.25), alpha: ((dur - p) / 0.5).clamp(0.0, 1.0) }
}

/// How strong the aura is `p` seconds into its move of `dur`: it swells in over 0.3 s and fades over the last 0.5 s.
fn aura(p: f32, dur: f32) -> f32 { smooth(p / 0.3) * smooth((dur - p) / 0.5) }

/// The decoded frames of one pack at one height: (pack id, manifest stamp), height in px, and each file's pixmap
/// (None if it can't be read, so it isn't retried every frame). Dropped when any of the three changes.
#[derive(Default)]
pub struct Sprites { key: (String, u64), h: u32, map: HashMap<String, Option<tiny_skia::Pixmap>> }

impl Sprites {
    /// The picture of `p` to draw for the state `want` ("stand", "walk", ...) at `t` seconds, `h_css` CSS px tall, loaded
    /// and kept: its file name and whether it is a real animation (more than one frame of its own state), which switches
    /// off the breathing squash. None when the file can't be read, so the drawn figure stands in and the failure isn't
    /// retried every frame.
    pub fn pick(&mut self, g: &Gfx, scale: f32, p: &Pack, want: &str, t: f32, h_css: f32) -> Option<(String, bool)> {
        let h = (h_css * scale).round() as u32;
        if self.key.0 != p.id || self.key.1 != p.stamp || self.h != h { *self = Sprites { key: (p.id.clone(), p.stamp), h, map: HashMap::new() }; }
        let (m, exact) = p.resolve(want)?;
        let frames = &p.frames[m];
        let file = &frames[pack::frame_index(frames.len(), p.fps, t)];
        let img = self.map.entry(file.clone()).or_insert_with(|| {
            let path = p.dir.join(file);
            // never wider than his window (a very wide picture is cropped at the sides, not squeezed)
            let max_w = (4 * h).min(((W - 40.0) * scale) as u32).max(1);
            g.image_size(&path).filter(|&(w, h)| w > 0 && h > 0)
                .and_then(|(iw, ih)| g.load_image(&path, ((h as f32 * iw as f32 / ih as f32).round() as u32).clamp(1, max_w), h))
        });
        img.is_some().then(|| (file.clone(), exact && frames.len() > 1))
    }

    /// The decoded picture of a file `pick` returned.
    pub fn image(&self, file: &str) -> Option<&tiny_skia::Pixmap> { self.map.get(file)?.as_ref() }
}

/// (picture height, feet height in the window), CSS px, for a picture `height` tall with `room` above his feet on the
/// screen: a tall one is cut down to fit under the screen's top (on a top bar there are only 54 px), and the window
/// grows upward to hold what is left, never shorter than the drawn figure's.
fn fit(height: f32, room: f32) -> (f32, f32) {
    let h = height.min(room).max(pack::MIN_H);
    (h, (h + 4.0).max(FY))
}

/// The pack a buddy looks like: the folder named by a "pack:" character, else the single picture chosen in Settings.
/// `cur` is the pack already in use, kept when it is still the right one so its frames aren't dropped every second.
fn pack_for(character: &str, image: &str, cur: Option<Arc<Pack>>) -> Option<Arc<Pack>> {
    if let Some(id) = character.strip_prefix("pack:") { return pack::load(id); }
    if image.is_empty() { return None; }
    cur.filter(|p| p.id == format!("image:{image}")).or_else(|| Some(Arc::new(pack::single(image))))
}

/// A flight to a spot on the screen he was asked to point at (screen help), and the stay there.
#[derive(Clone, Debug)]
struct PointJob {
    /// Feet where he took off and where he lands, and the spot itself (screen px).
    from: (f32, f32),
    feet: (f32, f32),
    to: (f32, f32),
    label: String,
    t0: f32,
    fly: f32,
    /// Which side of the spot he stands on (-1 left, 1 right); his bubble opens away from the spot.
    side: f32,
    /// Set on the frame he lands, so the landing puff and hop happen once.
    landed: bool,
}

/// Seconds he stays at the spot after landing.
const POINT_HOLD: f32 = 8.0;

/// Flight time for `dist` px: 800 px/s, but never snappier than 0.6 s or lazier than 1.4 s.
fn flight_secs(dist: f32) -> f32 { (dist / 800.0).clamp(0.6, 1.4) }

/// Ease in and out: 0 to 1 with no jerk at either end.
fn smooth(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

/// The flight's pace: a gentle takeoff, a fast middle and a soft arrival (ease in-out cubic, steeper than `smooth`).
fn fly_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// How far he leans into the flight (radians): head toward where he is going, more the faster he goes, at most 0.4.
fn lean(vx: f32, scale: f32) -> f32 { (vx / (900.0 * scale.max(0.1))).clamp(-1.0, 1.0) * 0.4 }

/// Where the spot's name is drawn, in his window (css px): x, offset below the ring's centre, width, alignment, and whether
/// it wraps onto two lines.
#[derive(Debug, PartialEq)]
struct Label { x: f32, dy: f32, w: f32, left: bool, wrap: bool }

/// The rounded backing of the name: as wide as its text plus 4 px each side but never wider than its box, pushed to the box's
/// outer end (the screen edge side), and tall enough for its lines. `ry` is the ring's centre.
fn pill_rect(l: &Label, ry: f32, text_w: f32, text_h: f32) -> R {
    let w = (text_w + 8.0).min(l.w);
    R::new(if l.left { l.x } else { l.x + l.w - w }, ry + l.dy - 3.0, w, text_h + 6.0)
}

/// Where the spot's name goes for a ring at x = `rx`. `room` is the space (css px) between the ring and the screen edge on
/// the side away from him, `text_w` the name's width. If it fits it sits beside the ring, level with it, on that side, so it
/// never lands on his legs. If not (the spot is at the screen edge) it goes below the ring, wrapped, ending 4 px inside the
/// screen and starting no nearer to him than 20 px from the ring.
fn label_box(rx: f32, side: f32, room: f32, text_w: f32) -> Label {
    if room >= text_w + 24.0 {
        let w = text_w + 8.0;
        return if side < 0.0 { Label { x: rx + 16.0, dy: -8.0, w, left: true, wrap: false } }
               else { Label { x: rx - 16.0 - w, dy: -8.0, w, left: false, wrap: false } };
    }
    let w = (room + 16.0).max(44.0);
    if side < 0.0 { Label { x: rx + room - 4.0 - w, dy: 20.0, w, left: false, wrap: true } }
    else { Label { x: rx - room + 4.0, dy: 20.0, w, left: true, wrap: true } }
}

/// The spot at progress `t` (0 to 1) on a curve from `a` to `b` that bows upward by a quarter of the distance (at most 120 px).
fn arc(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let c = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0 - (d * 0.25).min(120.0));
    let u = 1.0 - t;
    (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1)
}

/// Where his feet go to point at `to` on a monitor whose left edge is `mon_left` and bottom edge `mon_bottom`: a little
/// below the spot and to one side, on the side with room for his bubble. His bubble reaches 12 px below his feet, so
/// near the bottom edge he stands level with the spot instead of below it, rather than being cut off by the screen.
/// Returns (x, y, side) with side -1 for left of the spot, 1 for right.
fn point_feet(to: (f32, f32), s: f32, mon_left: f32, mon_bottom: f32) -> (f32, f32, f32) {
    let off = 34.0 * s;
    let side = if to.0 - off - 180.0 * s < mon_left { 1.0 } else { -1.0 };
    (to.0 + side * off, (to.1 + 40.0 * s).min(mon_bottom - 14.0 * s), side)
}

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
    /// When the move he is playing started, and which move it is.
    move_t: Option<f32>,
    playing: Move,
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
    /// What he looks like when he is a picture: the pack's frames (None: the drawn figure), or the single picture
    /// chosen in Settings.
    pack: Option<Arc<Pack>>,
    /// The decoded frames of that pack.
    sprites: Sprites,
    /// Height of his picture in CSS px (the pack's, cut down so he fits under the screen top), and where his feet sit
    /// in his window: the window grows upward for a tall picture.
    sprite_h: f32,
    fy: f32,
    /// Half the width and the height (CSS px) of what you can grab: the figure's, or the picture's.
    body: (f32, f32),
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
    /// The spot he is flying to or standing at, while pointing.
    job: Option<PointJob>,
    /// Horizontal speed in flight (px/s, smoothed), for the lean, and his last positions (x, y, time) for the sparkle trail.
    fly_v: f32,
    trail: Vec<(f32, f32, f32)>,
}

impl Gojo {
    pub(crate) fn new() -> Gojo { Gojo::slot(0) }

    fn slot(slot: usize) -> Gojo {
        let seed = unsafe { GetTickCount() } as u64 | 1;
        let g = Gojo {
            x: f32::NAN, y: f32::NAN, vy: 0.0, mode: Mode::Stand, to: 0.0, wait: 4.0, dir: -1.0, t: 0.0, last: Instant::now(),
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), peek: 0.0, hovered: false, peek_said: -99.0, bubble: None, infinity: 0.0,
            move_t: None, playing: Move::None, poof: Some(0.0), hop: None, press: None, grab: (0.0, 0.0), dragged: false, last_click: -9.0, was_down: false,
            chat_at: 200.0, around: Around::default(), primed: false, quiet: 0.0, queued: None, hidden: false, check_at: 0.0, perch: None, sit_after: false, sit_k: 0.0, k_t: 0.0,
            mon: RECT::default(), scale: 1.0,
            slot, cast: cast::CASTS[slot.min(cast::CASTS.len() - 1)].clone(), style: Hair::default(), trim: Trim::default(), pack: None, sprites: Sprites::default(), sprite_h: pack::DEFAULT_H, fy: FY, body: (11.0, 48.0), name: String::new(), walk: true, chatty: 1.0, pair: false, banter_at: clock() + 30.0, greeted: false,
            work: 0.0, break_mins: 50.0, breaks: true, focus: "", focus_since: 0.0, away_from: None, ground_at: 0.0, focus_said: ("", -9999.0), watch: true, job: None, fly_v: 0.0, trail: vec![],        };
        g
    }

    /// Reads this buddy's settings from config (once a second, so Settings changes show at once).
    fn tune(&mut self, cx: &Ctx) {
        let k = ["a", "b"][self.slot];
        let a = appearance(cx.cfg.str(&format!("buddies.{k}.character")), cast::custom_on(&cx.cfg), cx.cfg.get("buddies.custom"),
            &|f| cx.cfg.str(&format!("buddies.{k}.{f}")).to_string(), self.pack.take());
        (self.cast, self.style, self.trim, self.pack) = (a.cast, a.style, a.trim, a.pack);
        let room = (self.line() - self.mon.top as f32) / self.scale.max(0.1) - 2.0; // above his feet on the screen, so he isn't cut off
        (self.sprite_h, self.fy) = self.pack.as_ref().map_or((pack::DEFAULT_H, FY), |p| fit(p.height, room));
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
    /// The picture to draw now (see `Sprites::pick`); None when he is the drawn figure.
    fn pick_sprite(&mut self, c: &Canvas) -> Option<(String, bool)> {
        let p = self.pack.clone()?;
        let mode = match self.mode {
            Mode::Walk => "walk", Mode::Sit => "sit", Mode::Sleep => "sleep", Mode::Fall => "fall", Mode::Held => "held", Mode::Point => "point",
            Mode::Stand if self.bubble.as_ref().is_some_and(|b| b.1 > self.t) && p.frames.contains_key("talk") => "talk",
            Mode::Stand => "stand",
        };
        self.sprites.pick(c.g, c.scale, &p, mode, self.t, self.sprite_h)
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
    fn say(&mut self, s: &str, secs: f32) {
        if s.is_empty() { return; } // a pack can silence a line
        self.bubble = Some((s.to_string(), self.t + secs));
        // each buddy is its own window: lift the speaker above the other one so his bubble isn't hidden behind it
        let h = HWNDS[self.slot].load(Ordering::Relaxed);
        if h != 0 {
            unsafe { let _ = SetWindowPos(HWND(h as *mut _), HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
        }
        speak(self.slot, s);
    }
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
        if let Some(l) = notice(&self.around, &now, &self.cast.say) { self.queued = Some(l); }
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
        let s = self.cast.say;
        let lines: &[&str] = match kind {
            "code" => s.focus_code,
            "video" => s.focus_video,
            "social" => s.focus_social,
            "chat" => s.focus_chat,
            "game" => s.focus_game,
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
            if self.leads() { let l = self.pick(self.cast.say.game_start); self.say(&l, 5.0); }
        } else if let Some((x, y, game_win)) = self.away_from {
            // he stays until the game's window is gone, so clicking the other monitor doesn't call him back
            if unsafe { IsWindow(HWND(game_win as *mut _)) }.as_bool() { return; }
            self.away_from = None;
            self.x = x; self.y = y; self.perch = None;
            if self.leads() && !self.cast.say.game_end.is_empty() { self.say(self.cast.say.game_end, 4.0); }
        }
    }

    /// Counts how long you have worked without a 5 minute pause and says when it's time for a break.
    fn watch_break(&mut self, idle_s: f32, dt: f32) {
        if idle_s > 300.0 { self.work = 0.0; } else if idle_s < 60.0 { self.work += dt; }
        if !self.breaks || self.break_mins <= 0.0 || self.work < self.break_mins * 60.0 || !self.leads() || self.hidden || self.talking() || self.mode == Mode::Held { return; }
        let mins = (self.work / 60.0).round() as u32;
        self.work = 0.0;
        if self.mode == Mode::Sleep { self.mode = Mode::Stand; self.wait = 3.0; }
        let l = self.pick(self.cast.say.break_tips);
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
        // while he points he is wherever the spot is, on whatever monitor
        if m != self.mon { self.mon = m; self.perch = None; if !matches!(self.mode, Mode::Held | Mode::Point) { self.y = self.line(); } }

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
        let (bl, br, bt, bb) = (self.x - self.body.0 * s, self.x + self.body.0 * s, self.y - self.body.1 * s, self.y + if self.sitting() { 11.0 } else { 1.0 } * s);
        self.hovered = !self.hidden && cx >= bl && cx < br && cy >= bt && cy < bb;

        // left button: press over him, then a drag past 6 px picks him up
        let down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
        if down && !self.was_down && self.hovered { self.press = Some((cur.x, cur.y)); }
        if !down { self.press = None; }
        if let Some((px, py)) = self.press {
            if self.mode != Mode::Held && (cur.x - px).pow(2) + (cur.y - py).pow(2) > 36 {
                self.mode = Mode::Held;
                self.dragged = true;
                self.move_t = None;
                self.job = None; // picking him up ends pointing
                self.grab = (self.x - px as f32, self.y - py as f32);
                let l = self.pick(self.cast.say.held);
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
                self.hold_bubble(); // the answer can arrive after he took off
            }
            // screen help found something to show you (assist.rs): he flies there
            if self.mode != Mode::Held {
                if let Some((px, py, label)) = assist::next_point() { self.start_point((px as f32, py as f32), label); }
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
                    self.say(self.cast.say.teleport, 1.5);
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
                    else { self.mode = Mode::Stand; self.wait = 2.0; self.say(self.cast.say.landed, 2.0); }
                }
            }
            Mode::Point => match self.job.clone() {
                None => { self.mode = Mode::Fall; self.vy = 0.0; self.fly_v = 0.0; self.trail.clear(); }
                Some(j) => {
                    let el = self.t - j.t0;
                    if el < j.fly {
                        let before = self.x;
                        let (x, y) = arc(j.from, j.feet, fly_ease(el / j.fly));
                        if (x - self.x).abs() > 0.5 { self.dir = (x - self.x).signum(); }
                        (self.x, self.y) = (x, y);
                        self.fly_v += ((self.x - before) / dt.max(0.001) - self.fly_v) * 0.35;
                        self.trail.push((self.x, self.y, self.t));
                    } else {
                        (self.x, self.y) = j.feet;
                        self.fly_v = 0.0;
                        self.dir = -j.side; // facing the spot
                        if !j.landed {
                            // touchdown: one puff and one hop
                            if let Some(job) = &mut self.job { job.landed = true; }
                            self.hop = Some(self.t);
                            self.poof = Some(self.t);
                        }
                        if el > j.fly + POINT_HOLD {
                            // done: back to the bar, with the same puff as any teleport
                            self.job = None;
                            self.perch = None;
                            self.poof = Some(self.t);
                            self.mode = Mode::Fall;
                            self.vy = 0.0;
                            self.trail.clear();
                        }
                    }
                    let t = self.t;
                    self.trail.retain(|p| t - p.2 < 0.3);
                }
            },
            Mode::Sleep => if idle_s < 2.0 { self.mode = Mode::Stand; self.wait = 3.0; self.say(self.cast.say.back, 3.0); },
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
                if self.wait <= 0.0 && self.move_t.is_none() {
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
        if self.move_t.is_some_and(|p| self.t - p > move_secs(self.playing)) { self.move_t = None; }
        if self.t > self.chat_at {
            self.chat_at = self.t + (150.0 + self.rand() * 150.0) / self.chatty.max(0.1);
            if self.chatty > 0.0 && !self.talking() && matches!(self.mode, Mode::Stand | Mode::Sit | Mode::Walk) {
                let l = if util::now().wHour < 5 && !self.cast.say.late.is_empty() { self.cast.say.late.to_string() } else { self.pick(self.cast.chat) };
                self.say(&l, 4.0);
            }
        }

        let hwnd = HWND(HWNDS[self.slot].load(Ordering::Relaxed) as *mut _);
        unsafe {
            let _ = SetWindowPos(hwnd, None, (self.x - FX * s).round() as i32, (self.y - self.fy * s).round() as i32, 0, 0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn sitting(&self) -> bool { matches!(self.mode, Mode::Sit | Mode::Sleep) }

    /// Plays move `m` (his double-click's or right-click's); false for `Move::None`, so the caller can say a line instead.
    /// A shield turns on or off; the others start unless one is already playing.
    fn start_move(&mut self, m: Move) -> bool {
        match m {
            Move::None => return false,
            Move::Shield => {
                if self.infinity > self.t { self.infinity = 0.0; self.say(self.cast.say.shield_off, 2.0); }
                else { self.infinity = self.t + 10.0; self.say(self.cast.say.shield_on, 3.0); }
            }
            _ if self.move_t.is_some() => {}
            _ => {
                if self.mode == Mode::Walk || self.sitting() { self.mode = Mode::Stand; }
                self.wait = 4.0;
                self.playing = m;
                self.move_t = Some(self.t);
                self.say(self.cast.shout, move_secs(m));
            }
        }
        true
    }

    /// Which side of him the speech bubble goes: right unless that runs off the monitor. While he points it opens away
    /// from the spot, which is on his other side.
    fn bubble_side(&self) -> f32 {
        if let (Mode::Point, Some(j)) = (self.mode, &self.job) { return j.side; }
        if self.x + 170.0 * self.scale > self.mon.right as f32 { -1.0 } else { 1.0 }
    }

    /// Takes off for the spot `to` (screen px) and points at it until `POINT_HOLD` after landing.
    fn start_point(&mut self, to: (f32, f32), label: String) {
        let m = monitor(POINT { x: to.0 as i32, y: to.1 as i32 }, false);
        let (fx, fy, side) = point_feet(to, self.scale, m.left as f32, m.bottom as f32);
        let from = (self.x, self.y);
        let dist = ((fx - from.0).powi(2) + (fy - from.1).powi(2)).sqrt();
        self.job = Some(PointJob { from, feet: (fx, fy), to, label, t0: self.t, fly: flight_secs(dist), side, landed: false });
        self.mode = Mode::Point;
        self.perch = None;
        self.move_t = None;
        self.sit_after = false;
        self.poof = Some(self.t);
        self.fly_v = 0.0;
        self.trail.clear();
        self.hold_bubble();
    }

    /// Keeps what he is saying up for as long as he points, so the answer doesn't vanish while he is still there.
    fn hold_bubble(&mut self) {
        if let (Some(j), Some(b)) = (&self.job, &mut self.bubble) { b.1 = b.1.max(j.t0 + j.fly + POINT_HOLD - 0.3); }
    }
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

/// What `Gojo::tune` and the Settings preview both work out from a buddy's options: `character` is the picked id, `custom` the
/// saved buddies (a JSON list, used when `custom_on`), `slot` reads the slot's own fields ("hair", "shirt", "pants", "eyes",
/// "bun", "style", "skin", "glasses", "ears", "image") and `cur` is the pack already in use, kept while it is still the right one.
pub struct Appearance { pub cast: Cast, pub style: Hair, pub trim: Trim, pub pack: Option<Arc<Pack>> }

pub fn appearance(character: &str, custom_on: bool, custom: &serde_json::Value, slot: &dyn Fn(&str) -> String, cur: Option<Arc<Pack>>) -> Appearance {
    let (mut cast, mut style) = cast::pick_with(custom_on, custom, character);
    cast.dress(&slot("hair"), &slot("shirt"), &slot("pants"), &slot("eyes"), &slot("bun"));
    if let Some(h) = Hair::parse(&slot("style")) { style = h; }
    let trim = if custom_on { Trim::read(&cast::trim_with(custom_on, custom, character), |f| slot(f)) } else { Trim::default() };
    let pack = pack_for(character, &trim.image, cur);
    Appearance { cast, style, trim, pack }
}

/// Draws the body: the picture when there is one (`animated`: it has frames of its own for this moment, so it doesn't breathe),
/// else the drawn figure, with its soft shadow when `shadow` (the preview leaves it out: it needs a window-sized mask).
/// Returns half the width and the height (CSS px) of what you can grab. The bar and the Settings preview both draw through this.
fn draw_body(c: &mut Canvas, look: &Look<'_>, pose: Pose, sprite: Option<(&tiny_skia::Pixmap, bool)>, sway: f32, talking: bool, t: f32, shadow: bool) -> (f32, f32) {
    match sprite {
        // a picture stands where the figure's feet would be and sits a little lower on a ledge; one with no animation of its
        // own for this moment breathes, squashes when it sits and pulses while it talks
        Some((img, animated)) => {
            let (sx, sy) = if animated { (1.0, 1.0) } else { pack::squash(t, look.sit, talking) };
            c.sprite_scaled(img, pose.ox, pose.oy + 4.0 * look.sit, pose.dir, pose.rot + sway, sx, sy);
            ((img.width() as f32 / c.scale / 2.0 * 0.8).max(8.0), img.height() as f32 / c.scale)
        }
        None => {
            if shadow { c.shadow(0.6, 2.4, 0.7, |c, over| figure(&mut Ink { c, pose, over }, look)); }
            else { figure(&mut Ink { c, pose, over: None }, look); }
            (11.0, 48.0)
        }
    }
}

/// One buddy for the Settings preview with his feet at (`x`, `y`) CSS px, `stance` "stand", "walk" or "sit", `t` seconds in; a
/// picture is at most `max_h` tall. The same drawing as on the bar.
pub fn draw_preview(c: &mut Canvas, ap: &Appearance, sprites: &mut Sprites, x: f32, y: f32, stance: &str, t: f32, max_h: f32) {
    let (walk, sit) = match stance { "walk" => (Some(t * 6.0), 0.0), "sit" => (None, 1.0), _ => (None, 0.0) };
    let drawn = ap.pack.as_ref().and_then(|p| sprites.pick(c.g, c.scale, p, stance, t, p.height.min(max_h)));
    let img = drawn.as_ref().and_then(|(f, animated)| sprites.image(f).map(|i| (i, *animated)));
    let look = Look { walk, sit, swing: t * 1.5, peek: 0.0, arm: 0.0, talk: false, hair: 0.0, style: ap.style, c: &ap.cast, trim: &ap.trim };
    draw_body(c, &look, Pose { ox: x, oy: y, dir: 1.0, rot: 0.0 }, img, 0.0, false, t, false);
}

impl View for Gojo {
    fn size(&self, g: &Gfx, _cx: &Ctx, _s: f32) -> (f32, f32) {
        let bubble = self.bubble.as_ref().filter(|b| b.1 > self.t).map(|b| bubble_dims(g, &b.0).3);
        (W, window_h(self.fy + (H - FY), bubble))
    }

    fn frame_ms(&self) -> u32 {
        if self.hidden { return 500; }
        if matches!(self.mode, Mode::Held | Mode::Fall | Mode::Point) || self.move_t.is_some() || self.infinity > self.t { 16 }
        else if self.mode == Mode::Walk || self.hop.is_some() || (self.peek > 0.0 && self.peek < 1.0) || self.poof.is_some() { 33 }
        else { 100 }
    }

    fn input(&mut self, ev: Input, _cx: &mut Ctx) {
        let Input::Click(_, b) = ev else { return };
        if std::mem::take(&mut self.dragged) && b == Btn::Left { return; } // the end of a drag isn't a click
        match b {
            Btn::Left if self.t - std::mem::replace(&mut self.last_click, self.t) < 0.45 => self.input(Input::Click(1, Btn::Double), _cx),
            Btn::Left => {
                if self.mode == Mode::Sleep { self.mode = Mode::Stand; self.wait = 3.0; self.say(self.cast.say.wake_click, 2.5); return; }
                self.hop = Some(self.t);
                let l = self.pick(self.cast.click);
                self.say(&l, 3.5);
            }
            Btn::Double => { if !self.start_move(self.cast.special) { self.say(self.cast.shout, 2.0); } }
            Btn::Right => {
                if !self.start_move(self.cast.alt) && !self.cast.right.is_empty() { let l = self.pick(self.cast.right); self.say(&l, 2.5); }
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
        let flying = self.mode == Mode::Point && self.job.as_ref().is_some_and(|j| t - j.t0 < j.fly);
        let rot = if held { (t * 4.0).sin() * 0.18 } else if flying { lean(self.fly_v, c.scale) } else if self.mode == Mode::Sleep { 0.07 } else { 0.0 };
        let pt = self.move_t.map(|p| t - p);
        // the front arm points forward while he fires orbs or a beam
        let arm = pt.filter(|_| matches!(self.playing, Move::Orbs | Move::Beam)).map_or(0.0, |p| { let dur = move_secs(self.playing); (p / 0.3).min(1.0) * if p > dur - 0.6 { (dur - p) / 0.6 } else { 1.0 } });
        // where his feet are in the window, and how much taller than the drawn figure he is (the effects scale with it)
        let fy = self.fy;
        let k = self.sprite_h.max(pack::DEFAULT_H) / pack::DEFAULT_H;
        let pose = Pose { ox: FX, oy: fy - hop.max(0.0) - bob, dir: self.dir, rot };
        self.sit_k = ease_toward(self.sit_k, if self.sitting() { 1.0 } else { 0.0 }, (t - self.k_t).clamp(0.0, 0.1));
        self.k_t = t;
        let sprite = self.pick_sprite(c);
        let talking = self.talking();
        let look = Look {
            walk, sit: self.sit_k, swing: t * if self.mode == Mode::Sleep { 1.2 } else { 1.5 }, peek: self.peek.clamp(0.0, 1.0), arm,
            talk: self.talking() && (t * 7.0).sin() > 0.0 && self.mode != Mode::Sleep, hair: if held { (t * 9.0).sin() * 1.2 } else if flying { (t * 12.0).sin() * 0.9 } else { -bob }, style: self.style, c: &self.cast, trim: &self.trim,
        };

        if self.infinity > t {
            for i in 0..3 {
                let r = 24.0 + i as f32 * 7.0 + (t * 10.0) % 7.0;
                c.arc(FX, fy - 24.0 * k, r, 1.0, 1.0, alpha(self.cast.eye, 0.45 * (1.0 - (r - 24.0) / 24.0)));
            }
        }
        // a streak behind him in flight: a ribbon through his last positions that thins and fades over 0.3 s. Segments, not
        // dots: at flying speed the positions are 30-70 px apart and only a couple fall inside his window. A glow in the
        // theme's colour with a white core, so it shows on dark and light desktops.
        if self.mode == Mode::Point && !self.trail.is_empty() {
            let glow = cx.theme.c("primary");
            let at = |&(hx, hy, _): &(f32, f32, f32)| (FX + (hx - self.x) / c.scale, fy - 22.0 * k + (hy - self.y) / c.scale);
            let mut pts: Vec<((f32, f32), f32)> = self.trail.iter().map(|p| (at(p), 1.0 - ((t - p.2) / 0.3).clamp(0.0, 1.0))).collect();
            pts.push(((FX, fy - 22.0 * k), 1.0));
            for w in pts.windows(2) {
                let k = w[1].1;
                c.line(w[0].0, w[1].0, 8.0 * k, alpha(glow, 0.4 * k));
                c.line(w[0].0, w[1].0, 2.6 * k, alpha(0xFFFFFFFF, 0.85 * k));
            }
        }
        let drawn = sprite.as_ref().and_then(|(f, animated)| self.sprites.image(f).map(|img| (img, *animated)));
        let sway = walk.map_or(0.0, |w| (w * 2.0).sin() * 0.05);
        let body = draw_body(c, &look, pose, drawn, sway, talking, t, true);
        self.body = body;
        c.hit(R::new(FX - body.0, fy - body.1, body.0 * 2.0, body.1 + if self.sitting() { 11.0 } else { 1.0 }), 1);

        // the move: orbs gather and fire (Hollow Purple), a ball charges and a beam shoots, or an aura glows around him
        if let Some(p) = pt {
            let (d, orbs, dur) = (self.dir, self.cast.orbs, move_secs(self.playing));
            let orb = |c: &mut Canvas, x: f32, y: f32, r: f32, col: Color, a: f32| {
                c.circle(x, y, r * 2.2, alpha(col, 0.22 * a));
                c.circle(x, y, r, alpha(col, a));
                c.circle(x, y, r * 0.5, alpha(0xFFFFFFFF, 0.85 * a));
            };
            let y = fy - 26.0 * k;
            match self.playing {
                Move::Orbs => {
                    if p < 1.4 {
                        let g = (p / 0.9).min(1.0);
                        let m = ((p - 0.9) / 0.5).clamp(0.0, 1.0);
                        let r = 6.0 * g;
                        let at = |side: f32| FX + d * (14.0 + (side * 40.0 - 14.0) * (1.0 - m * m));
                        orb(c, at(-1.0), y, r, orbs.0, 1.0);
                        orb(c, at(1.0), y, r, orbs.1, 1.0);
                    } else {
                        let q = p - 1.4;
                        let a = if q > 0.7 { (1.2 - q).max(0.0) / 0.5 } else { 1.0 };
                        orb(c, FX + d * (14.0 + q * q * 300.0), y, 7.0 + q * 12.0, orbs.2, a);
                    }
                }
                Move::Beam => {
                    let b = beam(p, dur);
                    let x0 = FX + d * 14.0;
                    if b.len <= 0.0 { orb(c, x0, y, 2.0 + 5.0 * b.charge, orbs.0, 1.0); } else {
                        let (x1, pulse) = (x0 + d * b.len, 1.0 + 0.12 * (t * 40.0).sin());
                        c.line((x0, y), (x1, y), 16.0 * pulse, alpha(orbs.0, 0.25 * b.alpha));
                        c.line((x0, y), (x1, y), 8.0 * pulse, alpha(orbs.2, 0.8 * b.alpha));
                        c.line((x0, y), (x1, y), 3.0, alpha(0xFFFFFFFF, 0.95 * b.alpha));
                        orb(c, x0, y, 7.0, orbs.0, b.alpha);
                    }
                }
                Move::Aura => {
                    let (env, cy) = (aura(p, dur), fy - 22.0 * k);
                    c.circle(FX, cy, 30.0 * k, alpha(orbs.2, 0.16 * env));
                    for i in 0..3 {
                        let q = (p * 1.1 + i as f32 / 3.0) % 1.0;
                        c.arc(FX, cy, (10.0 + q * 30.0) * k, 1.0, 0.4 + 2.0 * (1.0 - q), alpha(orbs.0, 0.8 * env * (1.0 - q)));
                    }
                    for i in 0..6 {
                        let q = (p * 0.9 + i as f32 / 6.0) % 1.0;
                        c.circle(FX + (i as f32 * 2.3).sin() * 14.0, fy - (6.0 + q * 44.0) * k, 1.4 * (1.0 - q), alpha(0xFFFFFFFF, env * (1.0 - q)));
                    }
                }
                Move::Shield | Move::None => {}
            }
        }
        if let Some(p0) = self.poof {
            let p = (t - p0) / 0.5;
            if p >= 1.0 { self.poof = None; } else {
                c.arc(FX, fy - 20.0 * k, 8.0 + p * 22.0, 1.0, 2.0 * (1.0 - p), alpha(self.cast.eye, 1.0 - p));
                for i in 0..6 {
                    let a = i as f32 * 1.047 + p;
                    c.circle(FX + a.cos() * (10.0 + p * 20.0), fy - 20.0 * k + a.sin() * (10.0 + p * 20.0), 1.4 * (1.0 - p), alpha(self.cast.hair, 1.0 - p));
                }
            }
        }
        // the spot he points at: a ring that fades in as he arrives and pulses, a ripple at touchdown, and its name beside it
        if let (Mode::Point, Some(j)) = (self.mode, &self.job) {
            let el = t - j.t0 - j.fly; // seconds since touchdown, negative while he is still flying
            let a = smooth((el + 0.25 * j.fly) / (0.25 * j.fly + 0.25)) * if el > POINT_HOLD - 0.4 { ((POINT_HOLD - el) / 0.4).max(0.0) } else { 1.0 };
            if a > 0.0 {
                let (rx, ry) = (FX + (j.to.0 - self.x) / c.scale, fy + (j.to.1 - self.y) / c.scale);
                let (col, pulse) = (cx.theme.c("primary"), (t * 5.0).sin() * 0.5 + 0.5);
                c.circle(rx, ry, 16.0 + 4.0 * pulse, alpha(col, 0.18 * a)); // soft glow
                // Three strokes, so the ring shows on any background: dark outside (white pages), white in the middle (dark
                // pages), the theme's colour inside. The theme colour alone is as pale as 1.7:1 on white in the dark theme.
                let (r, shade) = (11.0 + 2.0 * pulse, 0xFF101018u32);
                c.arc(rx, ry, r, 1.0, 5.0, alpha(shade, 0.55 * a));
                c.arc(rx, ry, r, 1.0, 3.4, alpha(0xFFFFFFFF, 0.95 * a));
                c.arc(rx, ry, r, 1.0, 1.8, alpha(col, a));
                c.circle(rx, ry, 6.0, alpha(shade, 0.55 * a));
                c.circle(rx, ry, 5.0, alpha(0xFFFFFFFF, 0.95 * a));
                c.circle(rx, ry, 3.4, alpha(col, a));
                if (0.0..0.6).contains(&el) {
                    let p = el / 0.6;
                    c.arc(rx, ry, 6.0 + 44.0 * p, 1.0, 3.0 * (1.0 - p), alpha(0xFFFFFFFF, 0.7 * (1.0 - p)));
                    c.arc(rx, ry, 6.0 + 44.0 * p, 1.0, 1.6 * (1.0 - p), alpha(col, 0.8 * (1.0 - p)));
                }
                if !j.label.is_empty() {
                    // the name on a small rounded pill in the bubble's colours, which read on any background
                    let f = txt(11.0, 600);
                    let room = if j.side < 0.0 { (self.mon.right as f32 - j.to.0) / c.scale } else { (j.to.0 - self.mon.left as f32) / c.scale };
                    let (tw, th) = c.g.measure(&j.label, f);
                    let l = label_box(rx, j.side, room, tw);
                    let text_h = if l.wrap { c.g.measure_wrapped(&j.label, f, (l.w - 8.0).max(10.0)) } else { th };
                    let pill = pill_rect(&l, ry, tw, text_h);
                    let (bg, fg) = (alpha(cx.theme.c("surfaceContainerHigh"), 0.96 * a), alpha(cx.theme.c("onSurface"), a));
                    c.shadow(1.0, 4.0, 0.35 * a, |c, over| c.rect(pill, 8.0, over.unwrap_or(bg)));
                    let (inner, al) = (R::new(pill.x + 4.0, pill.y + 3.0, pill.w - 8.0, pill.h - 6.0), if l.left { Align::Left } else { Align::Right });
                    if l.wrap { c.para(&j.label, inner, f, fg, al); } else { c.text(&j.label, inner, f, fg, al); }
                }
            }
        }
        if self.mode == Mode::Sleep {
            for i in 0..3 {
                let p = (t * 0.6 + i as f32 / 3.0) % 1.0;
                let r = R::new(FX + 9.0 + p * 10.0, fy - 22.0 * k - p * 26.0, 20.0, 16.0);
                c.text("z", r, txt(8.0 + p * 6.0, 700), alpha(cx.theme.c("onSurface"), 1.0 - p), Align::Left);
            }
        }

        // speech bubble beside his head
        if let Some((text, until)) = self.bubble.clone() {
            if until <= t { self.bubble = None; return; }
            let f = txt(11.0, 500);
            let (tw, th, bw, bh) = bubble_dims(c.g, &text);
            let side = self.bubble_side();
            let bx = if side > 0.0 { FX + 17.0 } else { FX - 17.0 - bw };
            let by = bubble_top(fy - 30.0 * k - bh / 2.0 - hop.max(0.0), bh, c.h);
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

/// The widest a bubble's text gets: the bubble must fit beside him in the window.
const BUBBLE_MAX_W: f32 = W - FX - 17.0 - 16.0 - 4.0;

/// (text width, text height, bubble width, bubble height) for `text`: one line if it fits, else wrapped at `BUBBLE_MAX_W`.
fn bubble_dims(g: &Gfx, text: &str) -> (f32, f32, f32, f32) {
    let f = txt(11.0, 500);
    let tw = g.measure(text, f).0.min(BUBBLE_MAX_W);
    let th = g.measure_wrapped(text, f, tw + 1.0);
    (tw, th, tw + 16.0, th + 10.0)
}

/// Window height: room for the feet, and for a bubble of height `bubble` (a long line wraps tall) with 2 px to spare.
fn window_h(base: f32, bubble: Option<f32>) -> f32 { base.max(bubble.map_or(0.0, |b| b + 4.0)) }

/// The bubble's top: where `want` puts it, kept inside a window `win_h` tall (2 px margins).
fn bubble_top(want: f32, bh: f32, win_h: f32) -> f32 { want.min(win_h - bh - 2.0).max(2.0) }

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

    /// `notice` in Gojo's words, which these tests were written against.
    fn notice(old: &Around, new: &Around) -> Option<String> { super::notice(old, new, &cast::GOJO_SAY) }

    #[test]
    fn a_pack_speaks_in_its_own_words_and_can_be_silenced() {
        let a = calm();
        let mut b = a.clone();
        b.volume = Some(75);
        let mut say = cast::NEUTRAL_SAY;
        assert_eq!(super::notice(&a, &b, &say).as_deref(), Some("That's loud."), "neutral words, not Gojo's");
        say.loud = "OVER 9000!";
        assert_eq!(super::notice(&a, &b, &say).as_deref(), Some("OVER 9000!"));
        say.loud = "";
        assert_eq!(super::notice(&a, &b, &say), None, "an empty line says nothing");
        // the rule is skipped, not the ones after it: going offline in the same look still gets its line
        b.online = false;
        assert_eq!(super::notice(&a, &b, &say).as_deref(), Some("The internet is gone."));
        let mut c = a.clone();
        c.claude = Some(96);
        say.claude_high = "Ki at {pct}%.";
        assert_eq!(super::notice(&a, &c, &say).as_deref(), Some("Ki at 96%."), "the percentage is filled in");
        let mut d = a.clone();
        d.track = "Cha-La\tHead".into();
        say.track = "{track}? {track}!";
        assert_eq!(super::notice(&a, &d, &say).as_deref(), Some("Cha-La? Cha-La!"));
    }

    #[test]
    fn a_tall_picture_grows_the_window_but_is_cut_to_fit_under_the_screen_top() {
        assert_eq!(fit(52.0, 54.0), (52.0, FY), "the default picture changes nothing");
        assert_eq!(fit(80.0, 54.0), (54.0, 58.0), "a top bar leaves 54 px above the feet; the 4 px margin sits above the screen's edge");
        assert_eq!(fit(80.0, 900.0), (80.0, 84.0), "elsewhere it is whole and the window grows to hold it");
        assert_eq!(fit(100.0, -5.0).0, pack::MIN_H, "no room at all still gives a visible one");
        assert!(fit(pack::MAX_H, 1e6).1 + (H - FY) <= 220.0, "the largest window stays modest");
    }

    #[test]
    fn a_chosen_picture_is_one_pack_kept_while_it_stays_chosen() {
        assert!(pack_for("gojo", "", None).is_none(), "no picture, no pack");
        let p = pack_for("gojo", "a.png", None).unwrap();
        assert_eq!(p.id, "image:a.png");
        let again = pack_for("gojo", "a.png", Some(p.clone())).unwrap();
        assert!(Arc::ptr_eq(&p, &again), "the same picture keeps its decoded frames");
        let other = pack_for("gojo", "b.png", Some(p.clone())).unwrap();
        assert_eq!(other.id, "image:b.png");
        assert!(pack_for("gojo", "", Some(p)).is_none(), "clearing the picture drops it");
        assert!(pack_for("pack:no-such-folder-zorui", "a.png", None).is_none(), "a missing pack is the drawn figure, not the old picture");
    }

    #[test]
    fn a_tall_bubble_gets_a_window_that_holds_it() {
        // DirectWrite needs COM on this thread; the test threads do not get it from anywhere else
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let base = FY + (H - FY);
        assert_eq!(window_h(base, None), base, "no bubble: the usual window");
        assert_eq!(window_h(base, Some(30.0)), base, "a short bubble fits already");
        assert_eq!(window_h(base, Some(130.0)), 134.0, "a tall one grows the window, with 2 px spare each side");
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let long = "Opening the message box for Shahmeer and typing the message, without sending it yet. Then I will wait for you to check it.";
        let (_, _, bw, bh) = bubble_dims(&g, long);
        assert!(bw <= BUBBLE_MAX_W + 16.0 && bh > 60.0, "the long line wraps to several lines: {bw} x {bh}");
        let (_, _, _, one) = bubble_dims(&g, "Hi.");
        assert!(one < bh / 2.0, "a short line is a single line");
        let h = window_h(base, Some(bh));
        let top = bubble_top(-50.0, bh, h);
        assert!(top >= 2.0 && top + bh <= h - 2.0, "inside the window: {top} + {bh} in {h}");
        assert_eq!(bubble_top(1000.0, bh, h), h - bh - 2.0, "never below the window");
    }

    #[test]
    fn a_silenced_line_makes_no_bubble() {
        let mut g = Gojo::new();
        g.say("", 3.0);
        assert!(!g.talking() && g.bubble.is_none());
        g.say("Hi.", 3.0);
        assert!(g.talking());
    }

    #[test]
    fn every_say_line_is_filled_in_for_both_defaults() {
        for s in [cast::GOJO_SAY, cast::NEUTRAL_SAY] {
            for list in [s.held, s.break_tips, s.game_start, s.focus_code, s.focus_video, s.focus_social, s.focus_chat, s.focus_game] {
                assert!(!list.is_empty() && list.iter().all(|l| !l.is_empty()), "a list that can be picked from is never empty");
            }
            assert!(s.claude_high.contains("{pct}") && s.claude_near.contains("{pct}") && s.note.contains("{app}") && s.track.contains("{track}"));
        }
    }

    fn slot<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> String + 'a {
        move |f| pairs.iter().find(|(k, _)| *k == f).map_or(String::new(), |(_, v)| v.to_string())
    }

    #[test]
    fn the_bar_and_the_preview_work_out_a_look_the_same_way() {
        let none = serde_json::json!([]);
        // a slot's colours and hairstyle change the built-in character
        let ap = appearance("geto", false, &none, &slot(&[("hair", "#102030"), ("style", "mohawk"), ("bun", "no")]), None);
        assert_eq!((ap.cast.id, ap.cast.hair, ap.style, ap.cast.bun), ("geto", 0xFF10_2030, Hair::Mohawk, false));
        assert!(ap.pack.is_none());
        // saved buddies, skin and the picture need the flag: off, a saved id is Gojo and the picture is ignored
        let saved = serde_json::json!([{"id": "c1", "name": "X", "base": "yuji", "skin": "#102030"}]);
        let off = appearance("c1", false, &saved, &slot(&[("image", "a.png"), ("skin", "#FFFFFF")]), None);
        assert_eq!((off.cast.id, off.trim.clone(), off.pack.is_none()), ("gojo", Trim::default(), true));
        let on = appearance("c1", true, &saved, &slot(&[("image", "a.png")]), None);
        assert_eq!((on.cast.label, on.trim.skin, on.trim.image.as_str()), ("X", Some(0xFF10_2030), "a.png"));
        assert_eq!(on.pack.as_ref().unwrap().id, "image:a.png", "a chosen picture becomes his pack");
        let again = appearance("c1", true, &saved, &slot(&[("image", "a.png")]), on.pack.clone());
        assert!(Arc::ptr_eq(on.pack.as_ref().unwrap(), again.pack.as_ref().unwrap()), "the pack in use is kept");
        let gone = appearance("pack:no-such-folder-zorui", false, &none, &slot(&[]), None);
        assert!(gone.pack.is_none() && gone.cast.id == "gojo", "a pack that isn't there is the drawn Gojo");
    }

    #[test]
    fn tune_reads_the_slots_options_through_that_same_look() {
        let mut cx = Ctx::new(false);
        cx.cfg = crate::conf::Config::parse(r##"{"buddies":{"a":{"character":"yuji","hair":"#102030","style":"long"}}}"##);
        let mut g = Gojo::new();
        g.tune(&cx);
        assert_eq!((g.cast.id, g.cast.hair, g.style, g.pack.is_none()), ("yuji", 0xFF10_2030, Hair::Long, true));
    }

    /// A PNG of `w` x `h` solid opaque red, by hand (stored deflate blocks), since the PNG encoder isn't built in.
    fn red_png(w: u32, h: u32) -> Vec<u8> {
        fn crc(d: &[u8]) -> u32 { let mut c = !0u32; for b in d { c ^= *b as u32; for _ in 0..8 { c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 }; } } !c }
        let chunk = |kind: &[u8; 4], data: &[u8]| { let mut v = (data.len() as u32).to_be_bytes().to_vec(); let body = [&kind[..], data].concat(); v.extend(&body); v.extend(crc(&body).to_be_bytes()); v };
        let raw: Vec<u8> = (0..h).flat_map(|_| std::iter::once(0u8).chain((0..w).flat_map(|_| [255u8, 0, 0, 255]))).collect();
        let (mut a, mut b) = (1u32, 0u32);
        for x in &raw { a = (a + *x as u32) % 65521; b = (b + a) % 65521; }
        let mut z = vec![0x78, 0x01, 1];
        z.extend((raw.len() as u16).to_le_bytes());
        z.extend((!(raw.len() as u16)).to_le_bytes());
        z.extend(&raw);
        z.extend(((b << 16) | a).to_be_bytes());
        let mut ihdr = [w.to_be_bytes(), h.to_be_bytes()].concat();
        ihdr.extend([8, 6, 0, 0, 0]);
        [b"\x89PNG\r\n\x1a\n".to_vec(), chunk(b"IHDR", &ihdr), chunk(b"IDAT", &z), chunk(b"IEND", &[])].concat()
    }

    /// How many pixels `draw_preview` paints for `ap` in `stance` on a blank 120 x 120 canvas.
    fn painted(ap: &Appearance, stance: &str) -> usize {
        use crate::gfx::{Canvas, Gfx};
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let mut px = vec![0u8; 120 * 120 * 4];
        let mut c = Canvas::new(&g, &mut px, 120, 120, 1.0, 0).unwrap();
        draw_preview(&mut c, ap, &mut Sprites::default(), 60.0, 100.0, stance, 0.0, 64.0);
        drop(c);
        px.chunks(4).filter(|p| p[3] > 0).count()
    }

    #[test]
    fn the_preview_draws_the_figure_in_every_stance() {
        let ap = appearance("gojo", false, &serde_json::json!([]), &slot(&[]), None);
        let (stand, walk, sit) = (painted(&ap, "stand"), painted(&ap, "walk"), painted(&ap, "sit"));
        assert!(stand > 300 && walk > 300 && sit > 300, "{stand} {walk} {sit}");
        assert_ne!(stand, sit, "sitting is not the standing pose");
        let tinted = appearance("gojo", false, &serde_json::json!([]), &slot(&[("style", "long")]), None);
        assert_ne!(painted(&tinted, "stand"), stand, "a Settings change shows in the preview");
    }

    #[test]
    fn the_preview_draws_a_packs_picture_at_the_packs_height() {
        let dir = std::env::temp_dir().join(format!("zorui-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stand.png"), red_png(16, 32)).unwrap();
        let pk = |height: u32| Arc::new(pack::parse("t", &dir, &format!(r#"{{"height":{height},"frames":{{"stand":["stand.png"]}}}}"#), 1).unwrap());
        let with = |p: Arc<Pack>| Appearance { cast: cast::get("gojo").clone(), style: Hair::default(), trim: Trim::default(), pack: Some(p) };
        let tall = painted(&with(pk(40)), "stand");
        let short = painted(&with(pk(20)), "stand");
        // the picture is 1:2, so 40 px tall is 20 x 40 = 800 px (breathing makes it a few more or fewer) and 20 px tall is 200
        assert!((700..=900).contains(&tall) && (150..=260).contains(&short), "tall {tall}, short {short}");
        assert_eq!(painted(&with(pk(100)), "stand"), painted(&with(pk(64)), "stand"), "the preview caps a tall picture");
        // the same picture stands in for the states it has none for
        assert!(painted(&with(pk(40)), "sit") > 600, "sitting uses the standing picture");
        let missing = Appearance { pack: Some(Arc::new(pack::parse("t", &dir, r#"{"frames":{"stand":["nope.png"]}}"#, 1).unwrap())), ..with(pk(40)) };
        assert!(painted(&missing, "stand") > 300, "a picture that can't be read leaves the drawn figure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_beam_charges_then_shoots_then_fades() {
        let dur = move_secs(Move::Beam);
        let b = beam(0.0, dur);
        assert_eq!((b.charge, b.len, b.alpha), (0.0, 0.0, 1.0));
        assert_eq!((beam(0.45, dur).charge, beam(0.9, dur).charge, beam(5.0, dur).charge), (0.5, 1.0, 1.0));
        assert_eq!((beam(0.5, dur).len, beam(BEAM_CHARGE, dur).len), (0.0, 0.0), "nothing shoots while the ball is charging");
        assert_eq!(beam(BEAM_CHARGE + 0.25, dur).len, BEAM_MAX);
        let lens: Vec<f32> = (0..=240).map(|i| beam(i as f32 * 0.01, dur).len).collect();
        assert!(lens.windows(2).all(|w| w[0] <= w[1]) && lens.iter().all(|l| (0.0..=BEAM_MAX).contains(l)), "it only grows, and never past its limit");
        assert_eq!((beam(dur - 0.5, dur).alpha, beam(dur, dur).alpha, beam(dur + 1.0, dur).alpha), (1.0, 0.0, 0.0));
        assert!(14.0 + BEAM_MAX < FX, "the beam ends inside his window on either side");
    }

    #[test]
    fn the_aura_swells_in_and_fades_out() {
        let dur = move_secs(Move::Aura);
        assert_eq!((aura(0.0, dur), aura(dur, dur)), (0.0, 0.0));
        assert_eq!(aura(dur / 2.0, dur), 1.0);
        assert!(aura(0.1, dur) > 0.0 && aura(0.1, dur) < 1.0 && aura(dur - 0.2, dur) < 1.0);
        assert!((0..=200).all(|i| (0.0..=1.0).contains(&aura(i as f32 * 0.01, dur))));
    }

    #[test]
    fn each_timed_move_has_a_length_and_a_shield_or_none_has_none() {
        assert_eq!(move_secs(Move::Orbs), 2.6, "Hollow Purple plays as long as it always did");
        assert!(move_secs(Move::Beam) > 1.5 && move_secs(Move::Aura) > 1.0);
        assert_eq!((move_secs(Move::Shield), move_secs(Move::None)), (0.0, 0.0));
    }

    #[test]
    fn a_move_starts_once_and_a_shield_toggles() {
        let _l = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = Gojo::new();
        g.mode = Mode::Walk;
        assert!(!g.start_move(Move::None) && g.move_t.is_none() && g.mode == Mode::Walk, "no move does nothing and says it did nothing");
        assert!(g.start_move(Move::Beam));
        assert_eq!((g.playing, g.mode, g.move_t.is_some(), g.talking()), (Move::Beam, Mode::Stand, true, true), "he stops walking, plays it and shouts");
        assert!(g.start_move(Move::Aura));
        assert_eq!(g.playing, Move::Beam, "a second move waits until the first is over");
        g.t = 10.0;
        assert!(g.start_move(Move::Shield) && g.infinity == 20.0, "the shield lasts 10 s");
        assert_eq!(g.bubble.as_ref().unwrap().0, cast::GOJO_SAY.shield_on);
        assert!(g.start_move(Move::Shield) && g.infinity == 0.0);
        assert_eq!(g.bubble.as_ref().unwrap().0, cast::GOJO_SAY.shield_off);
        assert_eq!(g.playing, Move::Beam, "the shield doesn't touch the move that is playing");
    }

    #[test]
    fn the_buttons_play_the_characters_own_moves() {
        let _l = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        let mut cx = Ctx::new(false);
        // Gojo: double-click is Hollow Purple, right-click is Infinity, as before
        let mut g = Gojo::new();
        g.input(Input::Click(1, Btn::Double), &mut cx);
        assert_eq!((g.playing, g.move_t.is_some(), g.talking()), (Move::Orbs, true, true));
        g.input(Input::Click(1, Btn::Right), &mut cx);
        assert!(g.infinity > g.t, "right-click turned the shield on");
        // Geto has no right-click move: he says one of his lines and nothing else happens
        let mut geto = Gojo::new();
        geto.cast = cast::get("geto").clone();
        geto.input(Input::Click(1, Btn::Right), &mut cx);
        assert!(geto.infinity <= geto.t && geto.move_t.is_none());
        assert!(cast::get("geto").right.contains(&geto.bubble.as_ref().unwrap().0.as_str()));
        // a character whose double-click does nothing still shouts, with no effect
        let mut still = Gojo::new();
        still.cast.special = Move::None;
        still.input(Input::Click(1, Btn::Double), &mut cx);
        assert!(still.move_t.is_none() && still.bubble.as_ref().unwrap().0 == still.cast.shout);
        // no right-click move and no lines: silence, not a crash
        let mut quiet = Gojo::new();
        quiet.cast.alt = Move::None;
        assert!(quiet.cast.right.is_empty());
        quiet.input(Input::Click(1, Btn::Right), &mut cx);
        assert!(quiet.bubble.is_none());
    }

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
    fn flights_take_between_a_half_and_one_and_a_half_seconds() {
        assert_eq!(flight_secs(0.0), 0.6, "a short hop is not instant");
        assert_eq!(flight_secs(400.0), 0.6);
        assert!((flight_secs(800.0) - 1.0).abs() < 1e-6);
        assert_eq!(flight_secs(10_000.0), 1.4, "across three monitors is still quick");
    }

    #[test]
    fn the_flight_starts_and_ends_exactly_where_asked() {
        let (a, b) = ((100.0, 50.0), (1500.0, 700.0));
        assert_eq!(arc(a, b, 0.0), a);
        let e = arc(a, b, 1.0);
        assert!((e.0 - b.0).abs() < 1e-3 && (e.1 - b.1).abs() < 1e-3);
        let mid = arc(a, b, 0.5);
        assert!(mid.1 < (a.1 + b.1) / 2.0, "the path bows upward, so he swoops instead of sliding");
        assert_eq!(arc((5.0, 5.0), (5.0, 5.0), 0.5), (5.0, 5.0), "pointing where he stands doesn't move him");
    }

    #[test]
    fn easing_is_flat_at_both_ends_and_never_leaves_0_to_1() {
        assert_eq!((smooth(0.0), smooth(1.0), smooth(0.5)), (0.0, 1.0, 0.5));
        assert_eq!((smooth(-3.0), smooth(9.0)), (0.0, 1.0));
        assert!(smooth(0.1) < 0.1 && smooth(0.9) > 0.9);
        assert!((0..=100).map(|i| smooth(i as f32 / 100.0)).collect::<Vec<_>>().windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn he_stands_below_the_spot_on_the_side_with_room() {
        let (x, y, side) = point_feet((1000.0, 500.0), 1.0, 0.0, 1080.0);
        assert_eq!((x, y, side), (966.0, 540.0, -1.0), "left of the spot, so the ring is on his right");
        let (x, _, side) = point_feet((100.0, 500.0), 1.0, 0.0, 1080.0);
        assert_eq!((x, side), (134.0, 1.0), "near the left edge he stands on the right");
        let (x, y, _) = point_feet((1000.0, 500.0), 2.0, 0.0, 1080.0);
        assert_eq!((x, y), (932.0, 580.0), "the offsets grow with the screen scale");
        let (_, _, side) = point_feet((-1000.0, 10.0), 1.0, -1920.0, 1080.0);
        assert_eq!(side, -1.0, "a monitor to the left of the primary has negative coordinates");
    }

    #[test]
    fn near_the_bottom_edge_he_stands_level_instead_of_off_screen() {
        assert_eq!(point_feet((1000.0, 1040.0), 1.0, 0.0, 1080.0).1, 1066.0, "his bubble stays on the monitor");
        assert_eq!(point_feet((1000.0, 1040.0), 2.0, 0.0, 1080.0).1, 1052.0, "the margin scales");
        assert_eq!(point_feet((1000.0, 860.0), 1.0, 0.0, 880.0).1, 866.0, "a monitor that starts above zero counts from its own bottom");
        assert_eq!(point_feet((1000.0, 10.0), 1.0, 0.0, 1080.0).1, 50.0, "the top edge needs no change");
        // wherever the spot is, the ring stays inside his 100 px window
        for s in [1.0f32, 1.5, 2.0] {
            for to_y in (0..=1080).step_by(20) {
                let y = point_feet((900.0, to_y as f32), s, 0.0, 1080.0).1;
                let ring = 56.0 + (to_y as f32 - y) / s;
                assert!((0.0..=90.0).contains(&ring), "ring at {ring} for a spot at {to_y}, scale {s}");
            }
        }
    }

    #[test]
    fn the_flight_has_a_gentle_takeoff_a_fast_middle_and_a_soft_landing() {
        assert_eq!((fly_ease(0.0), fly_ease(0.5), fly_ease(1.0)), (0.0, 0.5, 1.0));
        assert_eq!((fly_ease(-1.0), fly_ease(2.0)), (0.0, 1.0));
        assert!(fly_ease(0.1) < smooth(0.1) && fly_ease(0.9) > smooth(0.9), "slower at the ends than the plain ease");
        assert!(fly_ease(0.6) - fly_ease(0.4) > smooth(0.6) - smooth(0.4), "faster through the middle");
        assert!((0..=100).map(|i| fly_ease(i as f32 / 100.0)).collect::<Vec<_>>().windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn he_leans_into_the_flight_and_never_flips_over() {
        assert_eq!(lean(0.0, 1.0), 0.0);
        assert!(lean(500.0, 1.0) > 0.0 && lean(-500.0, 1.0) < 0.0, "head toward where he is going");
        assert!(lean(900.0, 1.0) > lean(300.0, 1.0), "more the faster he goes");
        assert!((lean(1e9, 1.0) - 0.4).abs() < 1e-6 && (lean(-1e9, 1.0) + 0.4).abs() < 1e-6, "at most 0.4 radians");
        assert!((lean(900.0, 2.0) - lean(450.0, 1.0)).abs() < 1e-6, "the same flight on a scaled screen leans the same");
        assert!(lean(500.0, 0.0).is_finite(), "a zero scale does not divide by zero");
    }

    #[test]
    fn the_name_goes_beside_the_ring_when_it_fits() {
        let l = label_box(204.0, -1.0, 600.0, 60.0);
        assert_eq!(l, Label { x: 220.0, dy: -8.0, w: 68.0, left: true, wrap: false }, "he is left of the spot, so the name is to its right");
        let l = label_box(136.0, 1.0, 600.0, 60.0);
        assert_eq!(l, Label { x: 52.0, dy: -8.0, w: 68.0, left: false, wrap: false }, "he is right of the spot, so the name ends before the ring");
        assert!(label_box(204.0, -1.0, 84.0, 60.0).wrap == false, "exactly enough room still fits beside the ring");
        assert!(label_box(204.0, -1.0, 83.9, 60.0).wrap, "a hair less does not");
    }

    #[test]
    fn at_the_screen_edge_the_name_drops_below_the_ring_and_stays_on_screen() {
        // right edge: the gear is 30 px from it, and he stands 34 px left of the ring
        let (rx, room) = (204.0, 30.0);
        let l = label_box(rx, -1.0, room, 70.0);
        assert!(l.wrap && l.dy > 0.0 && !l.left, "below the ring, wrapped, pushed against the edge");
        assert!(l.x + l.w <= rx + room - 4.0 + 1e-3, "ends inside the screen: {l:?}");
        let buddy_right = rx - 34.0 + 11.0;
        assert!(l.x >= buddy_right, "and starts clear of him: {l:?} vs his edge {buddy_right}");
        // left edge, mirrored
        let l = label_box(136.0, 1.0, room, 70.0);
        assert!(l.wrap && l.left && l.x >= 136.0 - room + 4.0 - 1e-3, "starts inside the screen: {l:?}");
        assert!(l.x + l.w <= 136.0 + 34.0 - 11.0, "and ends clear of him on the right: {l:?}");
        // a target right at the edge still gets a readable width, however little room there is
        assert!(label_box(204.0, -1.0, 0.0, 70.0).w >= 44.0 && label_box(204.0, -1.0, -10.0, 70.0).w >= 44.0);
        // either way the box stays inside his 340 px window for any ring a fixed 34 px from the middle
        for room in [0.0, 10.0, 30.0, 60.0, 90.0, 300.0] {
            let l = label_box(204.0, -1.0, room, 70.0);
            assert!(l.x + l.w <= 340.0 && l.x >= 0.0, "room {room}: {l:?}");
        }
    }

    #[test]
    fn the_names_pill_hugs_its_text_and_stays_in_its_box() {
        // beside the ring: the box is exactly text + 8, so the pill fills it
        let l = label_box(204.0, -1.0, 600.0, 60.0);
        let p = pill_rect(&l, 16.0, 60.0, 14.0);
        assert_eq!((p.x, p.w, p.h), (l.x, 68.0, 20.0));
        assert_eq!(p.y, 16.0 - 8.0 - 3.0, "centred on the ring's height");
        // at the right edge the box is wider than a short name: the pill sits at the screen end, not stretched
        let l = label_box(204.0, -1.0, 30.0, 70.0);
        let p = pill_rect(&l, 16.0, 30.0, 14.0);
        assert_eq!(p.w, 38.0);
        assert_eq!(p.x + p.w, l.x + l.w, "its right end is the box's right end, which is inside the screen");
        // a long name is held to the box
        let p = pill_rect(&l, 16.0, 200.0, 28.0);
        assert_eq!((p.x, p.w), (l.x, l.w), "never wider than the room there is");
        // left edge, mirrored: it starts at the screen end
        let l = label_box(136.0, 1.0, 30.0, 70.0);
        let p = pill_rect(&l, 16.0, 30.0, 14.0);
        assert_eq!((p.x, p.w), (l.x, 38.0));
        // taller for two lines
        assert!(pill_rect(&l, 16.0, 70.0, 28.0).h > pill_rect(&l, 16.0, 70.0, 14.0).h);
        // always inside his window at any room
        for room in [0.0, 10.0, 30.0, 60.0, 90.0, 300.0] {
            for side in [-1.0f32, 1.0] {
                let rx = if side < 0.0 { 204.0 } else { 136.0 };
                let (l, p) = (label_box(rx, side, room, 70.0), pill_rect(&label_box(rx, side, room, 70.0), 16.0, 70.0, 14.0));
                assert!(p.x >= l.x - 1e-3 && p.x + p.w <= l.x + l.w + 1e-3, "room {room} side {side}: {p:?} in {l:?}");
            }
        }
    }

    fn job(t0: f32, fly: f32) -> PointJob { PointJob { from: (0.0, 0.0), feet: (10.0, 10.0), to: (20.0, 10.0), label: "x".into(), t0, fly, side: -1.0, landed: false } }

    #[test]
    fn saying_a_line_without_a_window_still_sets_the_bubble() {
        let mut g = Gojo::new();
        g.say("Hi.", 3.0);
        assert!(g.talking(), "the raise is skipped for a missing window and the bubble is still made");
    }

    #[test]
    fn the_answer_stays_up_for_as_long_as_he_points() {
        let mut g = Gojo::new();
        g.job = Some(job(2.0, 1.0));
        g.bubble = Some(("The File menu is up there.".into(), 9.0));
        g.hold_bubble();
        assert_eq!(g.bubble.as_ref().unwrap().1, 2.0 + 1.0 + POINT_HOLD - 0.3, "a 7 second line is stretched to cover the stay");
        g.bubble = Some(("A long one.".into(), 99.0));
        g.hold_bubble();
        assert_eq!(g.bubble.as_ref().unwrap().1, 99.0, "a longer line is left alone");
        g.bubble = None;
        g.hold_bubble();
        assert!(g.bubble.is_none(), "no line means no bubble is made up");
        g.job = None;
        g.bubble = Some(("Hi.".into(), 5.0));
        g.hold_bubble();
        assert_eq!(g.bubble.as_ref().unwrap().1, 5.0, "ordinary chatter is untouched when he isn't pointing");
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
