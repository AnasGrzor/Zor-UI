//! The listening pill: while the primary buddy records you (Alt+B), a small pill floats at the bottom
//! of the screen with a waveform that follows your voice, then a calm wave while it works out what you
//! said. The recorder (scripts/buddy_listen.py) sends the loudness; assist.rs passes it here.

use crate::{
    app::{App, Ctx},
    gfx::{alpha, ico, Canvas, Gfx, R},
    win::{View, Win, Z},
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
static HWND_PILL: AtomicIsize = AtomicIsize::new(0);
static SINCE: Mutex<Option<Instant>> = Mutex::new(None);

pub const LISTENING: u8 = 1;
pub const THINKING: u8 = 2;

pub fn set(state: u8) {
    STATE.store(state, Ordering::Relaxed);
    if let Ok(mut s) = SINCE.lock() { *s = (state != 0).then(Instant::now); }
}
pub fn active() -> bool { STATE.load(Ordering::Relaxed) != 0 }
pub fn level(rms: f32) { LEVEL.store(rms.to_bits(), Ordering::Relaxed); }

/// Window size (CSS px): the pill plus room for its shadow.
const W: f32 = 254.0;
const H: f32 = 76.0;
const PILL: R = R { x: 12.0, y: 12.0, w: 230.0, h: 52.0 };
const BARS: usize = 26;

/// Bar height 0..1 for a voice level: speech sits around 0.02 to 0.2, the noise floor near 0.003.
pub fn bar(rms: f32) -> f32 { (rms.max(0.0) * 9.0).sqrt().min(1.0) }

/// Shows the pill at the bottom centre of the screen the pointer is on, listening.
pub fn open(app: &mut App) {
    set(LISTENING);
    level(0.0);
    let at = HWND_PILL.load(Ordering::Relaxed);
    if app.wins.iter().any(|w| w.hwnd.0 as isize == at) { return; }
    let (mut cur, mut mi) = (POINT::default(), MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() });
    unsafe {
        let _ = GetCursorPos(&mut cur);
        let _ = GetMonitorInfoW(MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST), &mut mi);
    }
    let m = mi.rcMonitor;
    let Ok(mut w) = Win::new(Box::new(Pill::new()), "Zor UI / listen", m.left + (m.right - m.left) / 2, m.bottom - 120, Z::Top, true) else { return };
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
    if STATE.load(Ordering::Relaxed) != 0 { return; }
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
}

impl Pill {
    pub fn new() -> Pill { Pill { bars: [0.0; BARS], t0: Instant::now(), shifted: 0.0, smooth: 0.0 } }

    /// A made-up conversation for `--render pill`.
    pub fn demo() -> Pill {
        set(LISTENING);
        let mut p = Pill::new();
        p.t0 -= std::time::Duration::from_secs(1); // past the fade-in
        for (i, b) in p.bars.iter_mut().enumerate() { *b = (0.12 + 0.8 * ((i as f32 * 0.55).sin().abs() * (1.0 - i as f32 / 40.0))).min(1.0); }
        p
    }
}

impl View for Pill {
    fn size(&self, _g: &Gfx, _cx: &Ctx, _s: f32) -> (f32, f32) { (W, H) }
    fn frame_ms(&self) -> u32 { if STATE.load(Ordering::Relaxed) == 0 { 500 } else { 33 } }
    fn key(&self, _cx: &Ctx) -> String { format!("{}", self.t0.elapsed().as_millis() / 33) }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let state = STATE.load(Ordering::Relaxed);
        if state == 0 { return; }
        let t = self.t0.elapsed().as_secs_f32();
        if state == LISTENING {
            // a new bar about every 60 ms, from the smoothed loudness
            let target = bar(f32::from_bits(LEVEL.load(Ordering::Relaxed)));
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
        let fade = (t / 0.18).min(1.0);
        let (bg, fg, dim) = (alpha(cx.theme.c("surfaceContainerHigh"), fade), cx.theme.c("primary"), cx.theme.c("onSurfaceVariant"));
        c.shadow(1.0, 5.0, 0.4 * fade, |c, over| {
            c.rect(PILL, 26.0, over.unwrap_or(bg));
        });
        c.icon("mic", R::new(PILL.x + 14.0, PILL.y, 24.0, PILL.h), ico(22.0, state == LISTENING), alpha(if state == LISTENING { fg } else { dim }, fade));
        let (x0, mid) = (PILL.x + 50.0, PILL.y + PILL.h / 2.0);
        for (i, v) in self.bars.iter().enumerate() {
            let h = 4.0 + v.clamp(0.0, 1.0) * (PILL.h - 22.0);
            c.rect(R::new(x0 + i as f32 * 6.5, mid - h / 2.0, 3.5, h), 1.75, alpha(if state == LISTENING { fg } else { dim }, fade));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_follow_the_voice_but_stay_in_range() {
        assert!(bar(0.0) == 0.0);
        assert!(bar(0.003) < 0.2, "the noise floor stays low: {}", bar(0.003));
        assert!(bar(0.05) > 0.5 && bar(0.05) < 0.8, "speech is clearly up: {}", bar(0.05));
        assert_eq!(bar(5.0), 1.0, "loud is capped");
        assert_eq!(bar(-1.0), 0.0);
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
