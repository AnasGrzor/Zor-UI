//! The listening pill: while the primary buddy records you (Alt+B), a small pill floats at the bottom
//! of the screen with a waveform that follows your voice, then a calm wave while it works out what you
//! said. The recorder (bridge/listen.rs) sends the loudness; assist.rs passes it here.

use crate::{
    app::{App, Ctx},
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    win::{Btn, Input, View, Win, Z},
};
use std::{
    sync::{atomic::{AtomicIsize, AtomicU32, AtomicU8, Ordering}, Mutex},
    time::Instant,
};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    UI::WindowsAndMessaging::*,
};

/// 0 hidden, 1 listening, 2 working out what you said.
static STATE: AtomicU8 = AtomicU8::new(0);
/// The latest voice level (an f32's bits), 0 to about 0.3 for speech.
static LEVEL: AtomicU32 = AtomicU32::new(0);
/// The latest peak sample (0 to 1) and the recorder's own speech threshold (f32 bits).
static PEAK: AtomicU32 = AtomicU32::new(0);
static FLOOR: AtomicU32 = AtomicU32::new(0);
/// The listen mode: 0 auto, 1 toggle, 2 hold.
static MODE: AtomicU8 = AtomicU8::new(0);
/// Counts recordings, so a pill window that outlives one knows a new one began.
static EPOCH: AtomicU32 = AtomicU32::new(0);
/// What the recognizer has heard so far (the live line above the pill).
static TEXT: Mutex<String> = Mutex::new(String::new());
static HWND_PILL: AtomicIsize = AtomicIsize::new(0);
static SINCE: Mutex<Option<Instant>> = Mutex::new(None);

pub const LISTENING: u8 = 1;
pub const THINKING: u8 = 2;
/// The microphone is still opening.
pub const STARTING: u8 = 3;

/// When the pill was told to hide; it slides away for `CLOSE_S` before the window goes.
static CLOSING: Mutex<Option<Instant>> = Mutex::new(None);
const CLOSE_S: f32 = 0.15;

pub fn set(state: u8) {
    let was = STATE.swap(state, Ordering::Relaxed);
    if state == STARTING { EPOCH.fetch_add(1, Ordering::Relaxed); partial(""); }
    if let Ok(mut s) = SINCE.lock() { *s = (state != 0).then(Instant::now); }
    if let Ok(mut c) = CLOSING.lock() {
        if state != 0 { *c = None } else if was != 0 { *c = Some(Instant::now()) }
    }
}
/// How far the hide animation is, 0 to 1 and beyond; None when the pill isn't hiding.
fn closing() -> Option<f32> { CLOSING.lock().ok().and_then(|c| *c).map(|t| t.elapsed().as_secs_f32() / CLOSE_S) }
pub fn active() -> bool { STATE.load(Ordering::Relaxed) != 0 }
/// The microphone is recording (not still opening, not transcribing).
pub fn listening() -> bool { STATE.load(Ordering::Relaxed) == LISTENING }
pub fn level(rms: f32) { LEVEL.store(rms.to_bits(), Ordering::Relaxed); }
/// One reading from the recorder: loudness, loudest sample, and the loudness it counts as speech (0 if not sent).
pub fn meter(rms: f32, peak: f32, floor: f32) {
    level(rms);
    PEAK.store(peak.to_bits(), Ordering::Relaxed);
    FLOOR.store(floor.to_bits(), Ordering::Relaxed);
}
/// The text heard so far; "" clears it.
pub fn partial(t: &str) { if let Ok(mut s) = TEXT.lock() { *s = t.trim().to_string(); } }
fn heard() -> String { TEXT.lock().map(|s| s.clone()).unwrap_or_default() }

/// The words shown in the live line: the whole text if it fits in `max` characters, else its latest words after an ellipsis.
pub fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max { return text.to_string(); }
    let cut: String = text.chars().skip(n - (max - 1)).collect();
    let cut = match cut.find(' ') { Some(i) if i + 1 < cut.len() => cut[i + 1..].to_string(), _ => cut };
    format!("\u{2026}{cut}")
}

/// A made-up sentence for `--render pill`.
pub fn sample_partial() -> &'static str { "open the settings and turn the bar blur up a little" }

/// Remembers how this recording ends, for the hint under the label.
pub fn set_mode(mode: &str) { MODE.store(mode_code(mode), Ordering::Relaxed); }
pub fn mode_code(mode: &str) -> u8 { match mode { "toggle" => 1, "hold" => 2, _ => 0 } }

/// Window size (CSS px): the pill plus room for its shadow.
const W: f32 = 430.0;
/// Room above the pill for the live text card.
const TX: f32 = 44.0;
const H: f32 = 76.0 + TX;
const PILL: R = R { x: 12.0, y: 12.0 + TX, w: 406.0, h: 52.0 };
/// The stop button's spot id, and its size.
const STOP: u32 = 1;
const STOP_D: f32 = 30.0;
/// Characters of live text that fit the card.
const TEXT_CHARS: usize = 58;
const BARS: usize = 26;
/// How far the pill rises from (CSS px) when it opens.
const RISE: f32 = 12.0;
/// A voice level below this is silence (the noise floor is near 0.003).
const SILENT: f32 = 0.006;
/// A peak at or above this is clipping; the pill keeps saying so for `CLIP_HOLD_S`.
const CLIP_PEAK: f32 = 0.98;
const CLIP_HOLD_S: f32 = 0.5;
/// Seconds of silence before the pill says it hears nothing.
const NO_SOUND_S: f32 = 3.0;

/// Bar height 0..1 for a voice level: speech sits around 0.02 to 0.2, the noise floor near 0.003.
pub fn bar(rms: f32) -> f32 { (rms.max(0.0) * 9.0).sqrt().min(1.0) }

/// Bars in the middle stand taller than the ones at the edges (0.55 to 1).
pub fn envelope(i: usize) -> f32 { 0.55 + 0.45 * (std::f32::consts::PI * (i as f32 + 0.5) / BARS as f32).sin() }

/// Starts fast and settles: 0 to 1 for an input of 0 to 1.
pub fn ease_out(x: f32) -> f32 { 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3) }

/// Seconds as m:ss.
pub fn clock(secs: f32) -> String { let s = secs.max(0.0) as u32; format!("{}:{:02}", s / 60, s % 60) }

/// Whether a loudness is silence: below the recorder's own speech threshold, or `SILENT` when it sent none.
pub fn silent(rms: f32, floor: f32) -> bool { rms < if floor > 0.0 { floor } else { SILENT } }

/// The word next to the waveform.
pub fn label(state: u8) -> &'static str {
    match state { STARTING => "Starting mic", LISTENING => "Listening", THINKING => "Processing", _ => "" }
}

/// The word, with what the voice is doing while recording.
pub fn status(state: u8, speaking: bool, clipping: bool) -> &'static str {
    match (state, clipping, speaking) { (LISTENING, true, _) => "Too loud", (LISTENING, _, true) => "Speaking", _ => label(state) }
}

/// How the recording ends in a listen mode (0 auto, 1 toggle, 2 hold).
pub fn press_hint(mode: u8) -> &'static str {
    match mode { 1 => "press again to send", 2 => "release to send", _ => "pause to send" }
}

/// The small line under the word: the timer and how to finish, or what is going on.
pub fn sub(state: u8, secs: f32, quiet: f32, mode: u8, clipping: bool) -> String {
    match state {
        STARTING => "Opening the microphone".into(),
        LISTENING if clipping => "Too loud, speak softer or move back".into(),
        LISTENING if quiet >= NO_SOUND_S => "No sound heard, check your mic".into(),
        LISTENING => format!("{} · {}", clock(secs), press_hint(mode)),
        THINKING => "Working out what you said".into(),
        _ => String::new(),
    }
}

/// Shows the pill at the bottom centre of the screen the pointer is on, starting the mic.
pub fn open(app: &mut App) {
    set(STARTING);
    meter(0.0, 0.0, 0.0);
    let at = HWND_PILL.load(Ordering::Relaxed);
    if app.wins.iter().any(|w| w.hwnd.0 as isize == at) { return; }
    let (mut cur, mut mi) = (POINT::default(), MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() });
    unsafe {
        let _ = GetCursorPos(&mut cur);
        let _ = GetMonitorInfoW(MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST), &mut mi);
    }
    let m = mi.rcMonitor;
    let Ok(mut w) = Win::new(Box::new(Pill::new()), "Zor UI / listen", m.left + (m.right - m.left) / 2, m.bottom - 120, Z::Top, false) else { set(0); return };
    HWND_PILL.store(w.hwnd.0 as isize, Ordering::Relaxed);
    w.update(&app.g, &app.cx, true);
    let s = w.scale();
    unsafe {
        let _ = SetWindowPos(w.hwnd, None, m.left + (m.right - m.left) / 2 - (W * s / 2.0) as i32, m.bottom - (H * s) as i32 - (56.0 * s) as i32, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    w.show();
    app.wins.push(w);
}

/// Closes the window once the pill is hidden (every second), or if it was never told to hide.
pub fn sync(app: &mut App) {
    let stuck = SINCE.lock().ok().and_then(|s| *s).is_some_and(|t| t.elapsed().as_secs() > 45);
    if stuck { set(0); }
    if STATE.load(Ordering::Relaxed) != 0 || closing().is_some_and(|k| k < 1.1) { return; }
    let at = HWND_PILL.load(Ordering::Relaxed);
    if let Some(i) = app.wins.iter().position(|w| at != 0 && w.hwnd.0 as isize == at) {
        app.wins.remove(i);
        HWND_PILL.store(0, Ordering::Relaxed);
    }
}

pub struct Pill {
    bars: [f32; BARS],
    t0: Instant,
    shifted: f32,
    smooth: f32,
    /// When recording began (for the timer).
    listen_since: Option<Instant>,
    /// Seconds the voice has been silent.
    quiet: f32,
    /// Until when (in seconds since `t0`) the pill reports clipping.
    clip_until: f32,
    last_t: f32,
    /// The state to keep drawing while the pill slides away.
    last_state: u8,
    epoch: u32,
}

impl Pill {
    pub fn new() -> Pill { Pill { bars: [0.0; BARS], t0: Instant::now(), shifted: 0.0, smooth: 0.0, listen_since: None, quiet: 0.0, clip_until: 0.0, last_t: 0.0, last_state: 0, epoch: EPOCH.load(Ordering::Relaxed) } }

    /// Forgets the last recording: the timer, the silence count, the bars and the open animation.
    fn reset(&mut self) {
        *self = Pill { epoch: EPOCH.load(Ordering::Relaxed), ..Pill::new() };
    }

    /// A made-up conversation for `--render pill`.
    pub fn demo() -> Pill {
        set(LISTENING);
        partial(sample_partial());
        let mut p = Pill::new();
        p.t0 -= std::time::Duration::from_secs(1); // past the fade-in
        p.listen_since = Some(Instant::now() - std::time::Duration::from_secs(7));
        for (i, b) in p.bars.iter_mut().enumerate() { *b = (0.12 + 0.8 * ((i as f32 * 0.55).sin().abs() * (1.0 - i as f32 / 40.0))).min(1.0); }
        p
    }
}

impl View for Pill {
    fn size(&self, _g: &Gfx, _cx: &Ctx, _s: f32) -> (f32, f32) { (W, H) }
    fn frame_ms(&self) -> u32 { if STATE.load(Ordering::Relaxed) == 0 && closing().is_none() { 500 } else { 33 } }
    fn key(&self, _cx: &Ctx) -> String { format!("{}", self.t0.elapsed().as_millis() / 33) }

    fn input(&mut self, ev: Input, _cx: &mut Ctx) {
        // the stop button ends the recording and sends what was said, like the hotkey in Toggle mode
        if let Input::Click(STOP, Btn::Left) = ev { if listening() { crate::bridge::listen::request_finish(); } }
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let live = STATE.load(Ordering::Relaxed);
        if self.epoch != EPOCH.load(Ordering::Relaxed) { self.reset(); }
        // hidden: keep drawing the last state while the pill slides away
        let out = if live == 0 { match closing() { Some(k) if k < 1.0 => k, _ => return } } else { 0.0 };
        if live != 0 { self.last_state = live; }
        let state = self.last_state;
        let t = self.t0.elapsed().as_secs_f32();
        let dt = (t - self.last_t).max(0.0);
        self.last_t = t;
        let rms = f32::from_bits(LEVEL.load(Ordering::Relaxed));
        let floor = f32::from_bits(FLOOR.load(Ordering::Relaxed));
        if f32::from_bits(PEAK.load(Ordering::Relaxed)) >= CLIP_PEAK && live == LISTENING { self.clip_until = t + CLIP_HOLD_S; }
        if live == 0 {
            // sliding away: the bars stay as they are
        } else if state == STARTING {
            // mic still opening: a flat line with a dot travelling along it
            for (i, b) in self.bars.iter_mut().enumerate() { *b = if ((t * 14.0) as usize % BARS) == i { 0.3 } else { 0.0 }; }
        } else if state == LISTENING {
            self.listen_since.get_or_insert_with(Instant::now);
            self.quiet = if silent(rms, floor) { self.quiet + dt } else { 0.0 };
            // a new bar about every 60 ms, from the smoothed loudness
            let target = bar(rms);
            self.smooth += (target - self.smooth) * 0.6;
            if t - self.shifted > 0.06 && self.t0.elapsed().as_millis() > 0 {
                self.shifted = t;
                self.bars.rotate_left(1);
                self.bars[BARS - 1] = self.smooth;
            }
        } else {
            // working it out: a slow wave, so it's clear it heard you
            for (i, b) in self.bars.iter_mut().enumerate() { *b = 0.22 + 0.16 * (t * 5.0 - i as f32 * 0.45).sin(); }
        }
        let listening = state == LISTENING;
        let clipping = listening && t < self.clip_until;
        let speaking = listening && !silent(rms, floor);
        let fade = (t / 0.18).min(1.0) * (1.0 - out);
        // rises into place on open, drops a little on close
        let p = R::new(PILL.x, PILL.y + RISE * (1.0 - ease_out(t / 0.25)) + 8.0 * out, PILL.w, PILL.h);
        let (bg, fg, dim) = (alpha(cx.theme.c("surfaceContainerHigh"), fade), cx.theme.c("primary"), cx.theme.c("onSurfaceVariant"));
        let col = if clipping { cx.theme.c("error") } else if listening { fg } else { dim };
        c.shadow(1.0, 5.0, 0.4 * fade, |c, over| {
            c.rect(p, 26.0, over.unwrap_or(bg));
        });
        let mid = p.y + p.h / 2.0;
        if listening { c.circle(p.x + 26.0, mid, 13.0 + self.smooth * 7.0, alpha(col, fade * (0.10 + 0.25 * self.smooth))); } // glow that follows the voice
        c.icon("mic", R::new(p.x + 14.0, p.y, 24.0, p.h), ico(22.0, listening), alpha(col, fade));
        let x0 = p.x + 50.0;
        // the bars glide left between shifts; the oldest fades out, the newest grows in
        let phase = if listening { ((t - self.shifted) / 0.06).clamp(0.0, 1.0) } else { 0.0 };
        for (i, v) in self.bars.iter().enumerate() {
            let grow = if i == BARS - 1 { phase } else { 1.0 };
            let edge = if i == 0 { 1.0 - phase } else { 1.0 };
            let v = (v * envelope(i) * grow).clamp(0.0, 1.0);
            let h = 4.0 + v * (p.h - 22.0);
            // quiet bars dim; while processing a bright sweep travels along them
            let lit = if state == THINKING { 0.25 + 0.75 * (t * 4.0 - i as f32 * 0.35).sin().max(0.0) } else { 0.45 + 0.55 * v };
            c.rect(R::new(x0 + (i as f32 - phase) * 6.5, mid - h / 2.0, 3.5, h), 1.75, alpha(col, fade * edge * lit));
        }
        let lx = x0 + BARS as f32 * 6.5 + 4.0;
        let stop = listening && out == 0.0;
        let w = p.x + p.w - lx - 12.0 - if stop { STOP_D + 6.0 } else { 0.0 };
        let secs = self.listen_since.map_or(0.0, |s| s.elapsed().as_secs_f32());
        c.text(status(state, speaking, clipping), R::new(lx, p.y + 6.0, w, 22.0), txt(13.0, 500), alpha(col, fade), Align::Left);
        if stop {
            let b = R::new(p.x + p.w - 12.0 - STOP_D, mid - STOP_D / 2.0, STOP_D, STOP_D);
            let hot = c.hit(b, STOP);
            c.rect(b, STOP_D / 2.0, alpha(col, fade * if hot { 0.28 } else { 0.14 }));
            c.rect(R::new(b.x + 10.0, b.y + 10.0, 10.0, 10.0), 2.5, alpha(col, fade)); // the stop square
        }
        let heard = heard();
        if !heard.is_empty() && state != STARTING {
            let card = R::new(p.x, p.y - 8.0 - 34.0, p.w, 34.0);
            c.shadow(1.0, 5.0, 0.4 * fade, |c, over| { c.rect(card, 17.0, over.unwrap_or(bg)); });
            c.text(&tail(&heard, TEXT_CHARS), R::new(card.x + 16.0, card.y, card.w - 32.0, card.h), txt(12.5, 400), alpha(cx.theme.c("onSurface"), fade), Align::Left);
        }
        c.text(&sub(state, secs, self.quiet, MODE.load(Ordering::Relaxed), clipping), R::new(lx, p.y + 26.0, w, 18.0), txt(10.5, 400), alpha(dim, fade), Align::Left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_bars_stand_taller_than_the_edges() {
        assert!(envelope(BARS / 2) > envelope(0) && envelope(BARS / 2) > envelope(BARS - 1));
        assert!((0..BARS).all(|i| (0.55..=1.0).contains(&envelope(i))));
    }

    #[test]
    fn a_new_recording_restarts_the_timer_of_a_pill_that_is_still_open() {
        let mut p = Pill::demo();
        assert!(p.listen_since.is_some());
        p.quiet = 5.0;
        p.last_state = LISTENING;
        set(STARTING); // the hotkey pressed again before the old window closed
        assert_ne!(p.epoch, EPOCH.load(Ordering::Relaxed), "the pill notices a new recording");
        p.reset();
        assert!(p.listen_since.is_none() && p.quiet == 0.0 && p.last_state == 0);
        assert_eq!(p.epoch, EPOCH.load(Ordering::Relaxed));
        set(0);
    }

    #[test]
    fn the_live_line_keeps_the_latest_words() {
        assert_eq!(tail("short", 58), "short");
        let t = tail("open the settings and turn the bar blur up a little please now", 30);
        assert!(t.starts_with('\u{2026}') && t.chars().count() <= 30, "{t}");
        assert!(t.ends_with("please now") && !t.contains("settings"), "{t}");
        assert_eq!(tail("", 5), "");
        assert!(sample_partial().chars().count() <= TEXT_CHARS, "the demo sentence fits the card");
        partial("  spaced out ");
        assert_eq!(heard(), "spaced out");
        partial("");
    }

    #[test]
    fn easing_runs_from_zero_to_one_and_clamps() {
        assert_eq!((ease_out(0.0), ease_out(1.0), ease_out(2.0), ease_out(-1.0)), (0.0, 1.0, 1.0, 0.0));
        assert!(ease_out(0.5) > 0.5, "fast start");
    }

    #[test]
    fn the_timer_reads_minutes_and_seconds() {
        assert_eq!((clock(0.0).as_str(), clock(7.9).as_str(), clock(65.0).as_str()), ("0:00", "0:07", "1:05"));
        assert_eq!(clock(-3.0), "0:00");
    }

    #[test]
    fn the_hint_says_how_the_recording_ends_in_each_mode() {
        assert_eq!(sub(LISTENING, 7.0, 0.0, 1, false), "0:07 · press again to send");
        assert_eq!(sub(LISTENING, 7.0, 0.0, 2, false), "0:07 · release to send");
        assert_eq!(sub(LISTENING, 7.0, 0.0, 0, false), "0:07 · pause to send");
        assert_eq!((mode_code("toggle"), mode_code("hold"), mode_code("auto"), mode_code("")), (1, 2, 0, 0));
    }

    #[test]
    fn the_hint_warns_about_silence_and_clipping_and_otherwise_shows_the_timer() {
        assert!(sub(LISTENING, 7.0, NO_SOUND_S, 1, false).starts_with("No sound"));
        assert!(sub(LISTENING, 7.0, NO_SOUND_S, 1, true).starts_with("Too loud"), "clipping wins over silence");
        assert_eq!(sub(THINKING, 7.0, 9.0, 1, false), "Working out what you said");
        assert_eq!(sub(0, 1.0, 0.0, 1, false), "");
    }

    #[test]
    fn silence_follows_the_recorders_threshold_with_a_fallback() {
        assert!(silent(0.01, 0.03) && !silent(0.05, 0.03), "the recorder's threshold decides");
        assert!(silent(0.003, 0.0) && !silent(0.01, 0.0), "no threshold sent: the fixed one");
    }

    #[test]
    fn the_word_tracks_what_the_voice_is_doing() {
        assert_eq!(status(LISTENING, false, false), "Listening");
        assert_eq!(status(LISTENING, true, false), "Speaking");
        assert_eq!(status(LISTENING, true, true), "Too loud");
        assert_eq!(status(THINKING, true, true), "Processing", "only recording reports the voice");
    }

    #[test]
    fn bars_follow_the_voice_but_stay_in_range() {
        assert!(bar(0.0) == 0.0);
        assert!(bar(0.003) < 0.2, "the noise floor stays low: {}", bar(0.003));
        assert!(bar(0.05) > 0.5 && bar(0.05) < 0.8, "speech is clearly up: {}", bar(0.05));
        assert_eq!(bar(5.0), 1.0, "loud is capped");
        assert_eq!(bar(-1.0), 0.0);
    }

    #[test]
    fn every_stage_has_a_label() {
        assert_eq!(label(STARTING), "Starting mic");
        assert_eq!(label(LISTENING), "Listening");
        assert_eq!(label(THINKING), "Processing");
        assert_eq!(label(0), "");
    }

    #[test]
    fn the_pill_hides_when_set_to_zero() {
        set(LISTENING);
        assert_eq!(STATE.load(Ordering::Relaxed), LISTENING);
        assert!(active());
        set(0);
        assert_eq!(STATE.load(Ordering::Relaxed), 0);
        assert!(SINCE.lock().unwrap().is_none());
        // the hotkey pressed again cancels a recording
        set(LISTENING);
        crate::assist::cancel();
        assert!(!active());
    }
}
