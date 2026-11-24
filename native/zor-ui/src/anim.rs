//! Motion for the island: a spring that keeps its speed when the target moves, and the colour fade
//! between two activities.

use crate::gfx::{Color, R};
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
    /// Only the tests use it: the island goes through `Motion`.
    #[cfg_attr(not(test), allow(dead_code))]
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

/// Seconds the island panel takes to grow out of the island pill.
pub const GROW_SECS: f32 = 0.26;
/// Pill size (CSS px) a panel opened without a pill to grow from (a shortcut, the buddy) starts at.
pub const PILL_FALLBACK: (f32, f32) = (160.0, 32.0);

/// The panel `elapsed` seconds after it opened: the card's shape (rect and corner radius, in window px), the
/// window's opacity, and whether it is done. The shape eases out from the pill `from` (a capsule) to the card
/// `to` (corners `r_to`). The opacity rises fast (a square root over the first 30%), so the pill-sized shape
/// shows by the second frame and a shrink still ends fully transparent.
pub fn morph(elapsed: f32, from: R, to: R, r_to: f32) -> (R, f32, u8, bool) {
    let t = (elapsed / GROW_SECS).clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    let l = |a: f32, b: f32| a + (b - a) * e;
    let r = R::new(l(from.x, to.x), l(from.y, to.y), l(from.w, to.w).max(1.0), l(from.h, to.h).max(1.0));
    let rad = l(from.w.min(from.h) / 2.0, r_to).min(r.w.min(r.h) / 2.0);
    (r, rad, ((t / 0.3).sqrt().min(1.0) * 255.0).round() as u8, t >= 1.0)
}

/// Clears the premultiplied BGRA pixels `px` (`w` px a row) outside the rounded rect `r` with corners `rad`,
/// with a 1 px soft edge: the panel shows only the morph's current shape.
pub fn clip_round(px: &mut [u8], w: usize, r: R, rad: f32) {
    let (cx, cy, hx, hy) = (r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, r.h / 2.0);
    // the rounded box's signed distance at a pixel centre, as 0..=1 coverage
    let cover = |x: f32, y: f32| {
        let (qx, qy) = ((x - cx).abs() - hx + rad, (y - cy).abs() - hy + rad);
        let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad;
        (0.5 - d).clamp(0.0, 1.0)
    };
    for (y, row) in px.chunks_exact_mut(w.max(1) * 4).enumerate() {
        let (top, bot) = (y as f32, y as f32 + 1.0);
        if bot <= r.y || top >= r.y + r.h { row.fill(0); continue; }
        let solid = top >= r.y + rad && bot <= r.y + r.h - rad; // no corner on this row: only the sides need work
        for (x, p) in row.chunks_exact_mut(4).enumerate() {
            if solid && x as f32 >= r.x && x as f32 + 1.0 <= r.x + r.w { continue; }
            let k = cover(x as f32 + 0.5, y as f32 + 0.5);
            if k < 1.0 { for b in p.iter_mut() { *b = (*b as f32 * k) as u8; } }
        }
    }
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
    /// The value as of the last `tick`. Read by the tests; the island draws what `tick` returns.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn value(&self) -> f32 { self.s.x }

    /// Sets the value at once, with no motion.
    pub fn jump(&mut self, v: f32) { self.curve = None; self.s = Spring { x: v, v: 0.0, to: v, last: None }; }

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

    /// Moves to `to` at `now`: a grow springs, a shrink eases on the close curve. The first target, and every
    /// target when `animate` is false (reduced motion), is placed at once.
    pub fn set(&mut self, to: f32, now: Instant, animate: bool) {
        if !animate || (self.curve.is_none() && self.s.to == 0.0) { return self.jump(to); }
        self.tick(now); // a new move starts from the value as of now
        if (to - self.s.to).abs() < 0.5 { return; }
        if to > self.s.to { self.spring_to_at(to, now) } else { self.curve_to(to, now) }
    }

    pub fn moving(&self) -> bool { self.curve.is_some() || self.s.moving() }
}

/// Whether views animate: false when Windows' Animation effects are off or the reducedMotion setting is on, and they snap.
pub fn animate(reduced: bool) -> bool { !reduced }

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
    fn morph_starts_as_the_pill_and_ends_as_the_card() {
        let (pill, card) = (R::new(122.0, 0.0, 200.0, 32.0), R::new(12.0, 12.0, 420.0, 693.0));
        let (r, rad, a, done) = morph(0.0, pill, card, 28.0);
        assert_eq!((r, rad, a, done), (pill, 16.0, 0, false), "a capsule the pill's size, not yet visible");
        assert_eq!(morph(GROW_SECS, pill, card, 28.0), (card, 28.0, 255, true));
        let (mid, rad, a, _) = morph(GROW_SECS / 2.0, pill, card, 28.0);
        assert!(mid.w > 200.0 && mid.w < 420.0 && mid.h > 32.0 && mid.h < 693.0 && a == 255, "{mid:?} alpha {a}");
        assert!(rad > 16.0 && rad < 28.0);
        assert!(morph(GROW_SECS * 0.05, pill, card, 28.0).2 > 80, "the pill shape shows by the second frame");
        assert_eq!(morph(1.0, R::new(0.0, 0.0, 0.0, 0.0), R::new(0.0, 0.0, 0.0, 0.0), 28.0).0.w, 1.0); // never an empty size
    }

    #[test]
    fn a_shrink_is_the_grow_played_backwards() {
        // the close feeds morph the time left: the card at the close, the pill (transparent) at the end
        let (pill, card) = (R::new(122.0, 0.0, 200.0, 32.0), R::new(12.0, 12.0, 420.0, 693.0));
        let w = |t: f32| morph((GROW_SECS - t).max(0.0), pill, card, 28.0).0.w;
        assert_eq!(w(0.0), 420.0);
        assert_eq!(w(GROW_SECS), 200.0);
        assert!(w(GROW_SECS / 2.0) < 420.0 && w(GROW_SECS / 2.0) > 200.0);
    }

    #[test]
    fn clip_round_keeps_only_the_shape() {
        let px = |rad: f32| { let mut p = vec![255u8; 10 * 10 * 4]; clip_round(&mut p, 10, R::new(2.0, 2.0, 6.0, 6.0), rad); p };
        let at = |p: &[u8], x: usize, y: usize| p[(y * 10 + x) * 4 + 3];
        let square = px(0.0);
        assert_eq!((at(&square, 0, 0), at(&square, 1, 5), at(&square, 9, 9)), (0, 0, 0), "outside is cleared");
        assert_eq!((at(&square, 2, 2), at(&square, 5, 5), at(&square, 7, 7)), (255, 255, 255), "inside is kept");
        let mut round = vec![255u8; 20 * 20 * 4];
        clip_round(&mut round, 20, R::new(2.0, 2.0, 16.0, 16.0), 4.0);
        let at = |x: usize, y: usize| round[(y * 20 + x) * 4 + 3];
        assert_eq!(at(2, 2), 0, "the corner is cut");
        assert!(at(3, 3) > 0 && at(3, 3) < 255, "the curve has a soft edge");
        assert_eq!((at(10, 2), at(2, 10), at(10, 10)), (255, 255, 255), "edges and middle stay");
        assert!(round.chunks(4).all(|c| c.iter().all(|&b| b == c[3])), "every channel scales with alpha (premultiplied)");
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

    #[test]
    fn set_jumps_the_first_target_and_grows_or_shrinks_after_it() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.set(300.0, t0, true);
        assert_eq!((m.value(), m.moving()), (300.0, false), "first target at once");
        m.set(100.0, t0, true);
        assert!(m.moving() && m.value() == 300.0, "a shrink starts from 300 and eases");
        assert!(m.tick(after(t0, 170)) < 300.0);
        assert_eq!(m.tick(after(t0, 340)), 100.0);
    }

    #[test]
    fn a_shrink_ticks_down_to_its_target_without_a_jump() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.set(300.0, t0, true);
        m.set(100.0, t0, true);
        let mut prev = 300.0;
        for ms in (16..=352).step_by(16) {
            let x = m.tick(after(t0, ms));
            assert!(x <= prev + 1e-3 && prev - x < 40.0, "step at {ms} ms: {prev} to {x}");
            prev = x;
        }
        assert_eq!(prev, 100.0);
    }

    #[test]
    fn a_grow_retargeted_mid_shrink_does_not_jump() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.set(300.0, t0, true);
        m.set(100.0, t0, true);
        let t1 = after(t0, 120);
        let x = m.tick(t1);
        m.set(300.0, t1, true);
        assert_eq!(m.value(), x, "same position right after the retarget");
        let after_one_frame = m.tick(after(t1, 1));
        assert!((after_one_frame - x).abs() < 10.0, "moved {} in 1 ms", after_one_frame - x);
    }

    #[test]
    fn reduced_motion_snaps_every_target() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        m.set(300.0, t0, false);
        m.set(100.0, t0, false);
        assert_eq!((m.value(), m.moving()), (100.0, false));
        m.set(200.0, t0, false);
        assert_eq!((m.tick(after(t0, 16)), m.moving()), (200.0, false));
    }

    #[test]
    fn animate_is_off_only_when_reduced() {
        assert!(animate(false));
        assert!(!animate(true));
    }
}
