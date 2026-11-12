//! Motion for the island: a spring that keeps its speed when the target moves, and the colour fade
//! between two activities.

use crate::gfx::Color;
use std::time::Instant;

/// Stiffness and damping of the island's width spring. Damping ratio 0.68 overshoots by about 5%.
const K: f32 = 420.0;
const C: f32 = 2.0 * 0.68 * 20.493902; // 2 * zeta * sqrt(K)
/// Integration step in seconds; K = 420 stays stable at this step.
const H: f32 = 1.0 / 240.0;

/// Seconds an activity change takes to fade in.
pub const FADE_SECS: f32 = 0.22;

#[derive(Default)]
pub struct Spring { x: f32, v: f32, to: f32, last: Option<Instant> }

impl Spring {
    /// Moves the target. The first target (from 0) is taken at once, and a move in flight keeps its speed.
    pub fn set(&mut self, to: f32) {
        if (to - self.to).abs() < 0.5 { return; }
        if self.to == 0.0 { self.x = to; self.v = 0.0; }
        self.to = to;
        if !self.moving() { self.last = None; }
    }

    /// Steps by `dt` seconds (at most 50 ms, so a stalled frame doesn't jump).
    pub fn advance(&mut self, dt: f32) {
        let mut left = dt.clamp(0.0, 0.05);
        while left > 0.0 {
            let h = left.min(H);
            self.v += (-K * (self.x - self.to) - C * self.v) * h;
            self.x += self.v * h;
            left -= h;
        }
        if !self.moving() { self.x = self.to; self.v = 0.0; }
    }

    /// Steps to `now` and returns the width.
    pub fn tick(&mut self, now: Instant) -> f32 {
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        self.last = Some(now);
        self.advance(dt);
        self.x
    }

    pub fn moving(&self) -> bool { (self.x - self.to).abs() > 0.3 || self.v.abs() > 1.0 }
}

/// 0 to 1 over `FADE_SECS`, fast at first and slow at the end.
pub fn fade(elapsed: f32) -> f32 {
    let t = (elapsed / FADE_SECS).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Seconds the island panel takes to grow out of the island.
pub const GROW_SECS: f32 = 0.26;
/// Share of the panel's height that shows on the first frame.
const GROW_FROM: f32 = 0.12;

/// The panel `elapsed` seconds after it opened, for a window `h` px tall: (rows showing, opacity 0..=255, finished).
/// It grows from the island's edge with an ease-out and fades in over the first 40% of the time.
pub fn grow(elapsed: f32, h: i32) -> (i32, u8, bool) {
    let t = (elapsed / GROW_SECS).clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    let rows = ((GROW_FROM + (1.0 - GROW_FROM) * e) * h as f32).round() as i32;
    (rows.clamp(1, h.max(1)), ((t / 0.4).min(1.0) * 255.0).round() as u8, t >= 1.0)
}

/// Control points of `cubic-bezier(.45,0,.2,1)`, the island's shrink curve (the Coucou close curve).
pub const CLOSE_BEZIER: [f32; 4] = [0.45, 0.0, 0.2, 1.0];
/// Seconds the island's shrink takes.
pub const CLOSE_SECS: f32 = 0.34;

/// CSS-style `cubic-bezier(x1, y1, x2, y2)` at progress `x` (0..=1): the eased value. Bisection on x, 24 steps.
pub fn bezier(p: [f32; 4], x: f32) -> f32 {
    let [x1, y1, x2, y2] = p;
    let at = |a: f32, b: f32, t: f32| 3.0 * (1.0 - t) * (1.0 - t) * t * a + 3.0 * (1.0 - t) * t * t * b + t * t * t;
    let x = x.clamp(0.0, 1.0);
    let (mut lo, mut hi, mut t) = (0.0f32, 1.0f32, x);
    for _ in 0..24 {
        if at(x1, x2, t) < x { lo = t } else { hi = t }
        t = (lo + hi) / 2.0;
    }
    at(y1, y2, t)
}

/// A value that moves by a spring (grow) or by a timed curve (shrink). A retarget never snaps: a spring that
/// takes over a curve keeps the curve's speed, and a curve that takes over a spring starts where the spring is.
#[derive(Default)]
pub struct Motion { s: Spring, curve: Option<Curve> }

#[derive(Clone, Copy)]
struct Curve { from: f32, to: f32, t0: Instant, dur: f32, p: [f32; 4] }

impl Curve {
    fn at(&self, now: Instant) -> f32 {
        let x = (now.duration_since(self.t0).as_secs_f32() / self.dur).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * bezier(self.p, x)
    }
}

impl Motion {
    /// The value as of the last `tick`.
    pub fn value(&self) -> f32 { self.s.x }

    /// Sets the value at once, with no motion.
    pub fn jump(&mut self, v: f32) { self.curve = None; self.s = Spring { x: v, v: 0.0, to: v, last: None }; }

    /// Springs to `to` (grow). A move in flight keeps its speed.
    pub fn spring_to(&mut self, to: f32) { self.spring_to_at(to, Instant::now()) }

    fn spring_to_at(&mut self, to: f32, now: Instant) {
        if let Some(c) = self.curve.take() {
            // Hand the curve's speed (units per second, a 1 ms difference) to the spring.
            let (b, a) = (c.at(now), c.at(now + std::time::Duration::from_millis(1)));
            self.s = Spring { x: b, v: (a - b) * 1000.0, to, last: None };
            return;
        }
        // Unlike Spring::set, never snaps: only `jump` places the value.
        if (to - self.s.to).abs() < 0.5 { return; }
        self.s.to = to;
        if !self.s.moving() { self.s.last = None; }
    }

    /// Eases to `to` on the shrink curve, from the current value.
    pub fn curve_to(&mut self, to: f32, now: Instant) {
        self.curve = Some(Curve { from: self.s.x, to, t0: now, dur: CLOSE_SECS, p: CLOSE_BEZIER });
        self.s.to = to;
    }

    /// Steps to `now` and returns the value.
    pub fn tick(&mut self, now: Instant) -> f32 {
        let Some(c) = self.curve else { return self.s.tick(now) };
        self.s.x = c.at(now);
        if now.duration_since(c.t0).as_secs_f32() >= c.dur {
            self.curve = None;
            self.s = Spring { x: c.to, v: 0.0, to: c.to, last: None };
        }
        self.s.x
    }

    pub fn moving(&self) -> bool { self.curve.is_some() || self.s.moving() }
}

/// `a` to `b` (ARGB), channel by channel.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * t.clamp(0.0, 1.0)).round() as u32) << s
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Spring, secs: f32) -> f32 {
        let mut peak = s.x;
        for _ in 0..(secs * 60.0) as usize { s.advance(1.0 / 60.0); peak = peak.max(s.x); }
        peak
    }

    #[test]
    fn first_target_is_taken_at_once() {
        let mut s = Spring::default();
        s.set(120.0);
        assert_eq!((s.x, s.moving()), (120.0, false));
    }

    #[test]
    fn settles_on_the_target_with_a_small_overshoot() {
        let mut s = Spring::default();
        s.set(100.0);
        s.set(200.0);
        assert!(s.moving());
        let peak = run(&mut s, 2.0);
        assert!(peak > 200.0 && peak < 215.0, "peak {peak}");
        assert_eq!((s.x, s.moving()), (200.0, false));
    }

    #[test]
    fn a_new_target_keeps_the_speed() {
        let mut s = Spring::default();
        s.set(100.0);
        s.set(200.0);
        for _ in 0..6 { s.advance(1.0 / 60.0); }
        let (x, v) = (s.x, s.v);
        assert!(v > 100.0);
        s.set(150.0); // retarget mid-flight
        assert_eq!((s.x, s.v), (x, v));
        s.advance(1.0 / 60.0);
        assert!(s.x > x, "keeps moving the way it was going");
    }

    #[test]
    fn a_stalled_frame_does_not_jump() {
        let mut s = Spring::default();
        s.set(100.0);
        s.set(200.0);
        s.advance(5.0);
        assert!(s.x < 200.0 + 15.0 && s.moving());
    }

    #[test]
    fn small_changes_are_ignored() {
        let mut s = Spring::default();
        s.set(100.0);
        s.set(100.3);
        assert_eq!(s.to, 100.0);
    }

    #[test]
    fn fade_runs_zero_to_one() {
        assert_eq!(fade(0.0), 0.0);
        assert_eq!(fade(FADE_SECS), 1.0);
        assert_eq!(fade(5.0), 1.0);
        assert!(fade(FADE_SECS / 2.0) > 0.5);
    }

    #[test]
    fn grow_starts_small_and_transparent_and_ends_whole() {
        assert_eq!(grow(0.0, 500), (60, 0, false));
        assert_eq!(grow(GROW_SECS, 500), (500, 255, true));
        let (mid, a, _) = grow(GROW_SECS / 2.0, 500);
        assert!(mid > 250 && mid < 500 && a == 255, "mid {mid} alpha {a}");
        assert_eq!(grow(1.0, 0).0, 1); // never an empty size
    }

    #[test]
    fn mix_blends_each_channel() {
        assert_eq!(mix(0xFF000000, 0xFFFFFFFF, 0.0), 0xFF000000);
        assert_eq!(mix(0xFF000000, 0xFFFFFFFF, 1.0), 0xFFFFFFFF);
        assert_eq!(mix(0xFF102030, 0xFF305070, 0.5), 0xFF203850);
        assert_eq!(mix(0x00FF0000, 0xFF0000FF, 0.5), 0x80800080);
    }

    fn after(t: Instant, ms: u64) -> Instant { t + std::time::Duration::from_millis(ms) }

    #[test]
    fn bezier_hits_both_ends_and_never_goes_back() {
        assert!(bezier(CLOSE_BEZIER, 0.0).abs() < 1e-4);
        assert!((bezier(CLOSE_BEZIER, 1.0) - 1.0).abs() < 1e-4);
        let mut prev = 0.0;
        for i in 0..=100 {
            let y = bezier(CLOSE_BEZIER, i as f32 / 100.0);
            assert!(y >= prev - 1e-5, "dipped at {i}");
            prev = y;
        }
    }

    #[test]
    fn curve_eases_out_and_ends_on_time() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.jump(0.0);
        m.curve_to(100.0, t0);
        let mid = m.tick(after(t0, 170));
        assert!(mid > 50.0 && mid < 100.0, "mid {mid}");
        assert!(m.moving());
        assert_eq!(m.tick(after(t0, 340)), 100.0);
        assert!(!m.moving());
    }

    #[test]
    fn spring_takes_over_a_curve_without_a_jump_or_a_stop() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.jump(0.0);
        m.curve_to(100.0, t0);
        let now = after(t0, 100);
        let x = m.tick(now);
        m.spring_to_at(0.0, now); // reverse mid-curve
        assert_eq!(m.value(), x, "no snap");
        assert!(m.s.v > 0.0, "keeps moving the way the curve was going");
        let mut t = now;
        for _ in 0..240 { t = after(t, 16); m.tick(t); }
        assert!(!m.moving() && m.value().abs() < 0.5, "settles on the new target: {}", m.value());
    }

    #[test]
    fn curve_takes_over_a_spring_from_where_it_is() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.jump(0.0);
        m.spring_to_at(100.0, t0);
        m.s.last = Some(t0);
        let mut t = t0;
        for _ in 0..10 { t = after(t, 16); m.tick(t); }
        let x = m.value();
        assert!(x > 0.0 && m.moving());
        m.curve_to(0.0, t);
        assert_eq!(m.tick(t), x, "starts from where the spring is");
        assert!(m.tick(after(t, 340)).abs() < 1e-3);
    }

    #[test]
    fn a_stalled_spring_frame_is_clamped_after_a_takeover() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.jump(0.0);
        m.spring_to_at(100.0, t0);
        m.s.last = Some(t0);
        let x = m.tick(after(t0, 5000));
        assert!(x < 150.0, "a 5 s stall moved it to {x}");
    }
}
