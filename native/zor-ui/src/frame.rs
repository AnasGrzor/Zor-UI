//! The screen frame (end4's `bar.showFrame` + `fakeScreenRounding`): a thin click-through ring around each monitor
//! whose inner corners are rounded, so windows look like they sit in a rounded screen.

use crate::{
    app::{dpi_scale, App, Ctx},
    gfx::{polygon, Canvas, Gfx, R},
    ui, util,
    win::{View, Win, Z},
};
use std::sync::Mutex;
use windows::Win32::Foundation::RECT;

/// The windows' (handle, monitor rect, strip rect, thickness, radius) so a config or monitor change can rebuild them.
static CUR: Mutex<Vec<(isize, [i32; 4], [i32; 4], u32, u32)>> = Mutex::new(Vec::new());

/// (thickness, radius) in CSS px from config `frame`, or None when the frame is off.
pub fn settings(cx: &Ctx) -> Option<(u32, u32)> {
    let f = cx.cfg.get("frame");
    f["enabled"].as_bool().unwrap_or(false).then(|| (f["thickness"].as_u64().unwrap_or(5).clamp(1, 40) as u32, f["radius"].as_u64().unwrap_or(20).clamp(0, 80) as u32))
}

/// The four spandrels between the frame's inner rectangle and its rounded corners, each as polygon points.
pub fn corners(inner: R, radius: f32, steps: u32) -> [Vec<(f32, f32)>; 4] {
    let r = radius.min(inner.w / 2.0).min(inner.h / 2.0);
    let (l, t, rt, b) = (inner.x, inner.y, inner.x + inner.w, inner.y + inner.h);
    // corner point, arc centre and the angle (degrees) the arc starts at; each arc turns 90 degrees
    let spec = [((l, t), (l + r, t + r), 180.0), ((rt, t), (rt - r, t + r), 270.0), ((rt, b), (rt - r, b - r), 0.0), ((l, b), (l + r, b - r), 90.0)];
    spec.map(|(corner, (cx, cy), start): ((f32, f32), (f32, f32), f32)| {
        let mut pts = vec![corner];
        for i in 0..=steps {
            let a = (start + 90.0 * i as f32 / steps as f32).to_radians();
            pts.push((cx + r * a.cos(), cy + r * a.sin()));
        }
        pts
    })
}

/// One strip of a monitor's frame. `mon` is the monitor rect and `part` the strip's rect relative to it, both in
/// physical px. Only the strip is a bitmap, not the whole monitor: a full-screen buffer for a thin ring was 8 MB per screen.
struct Frame { mon: [i32; 4], part: [i32; 4], thick: f32, radius: f32 }

/// The four strips (x, y, w, h in physical px relative to the monitor) that tile the ring of depth `e`: top and
/// bottom full width, left and right between them. `e` is thickness plus radius, so every rounded corner fits in its strip.
fn strips(mw: i32, mh: i32, e: i32) -> [[i32; 4]; 4] {
    let e = e.min(mw / 2).min(mh / 2).max(0);
    [[0, 0, mw, e], [0, mh - e, mw, e], [0, e, e, mh - 2 * e], [mw - e, e, e, mh - 2 * e]]
}

impl View for Frame {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) { (self.part[2] as f32 / scale, self.part[3] as f32 / scale) }

    fn key(&self, cx: &Ctx) -> String { ui::layer0(cx).to_string() }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        // Drawn in the whole monitor's logical coordinates, then shifted so this strip's origin is (0, 0).
        let col = ui::layer0(cx);
        let s = c.scale;
        let (w, h, t) = ((self.mon[2] - self.mon[0]) as f32 / s, (self.mon[3] - self.mon[1]) as f32 / s, self.thick);
        let (ox, oy) = (self.part[0] as f32 / s, self.part[1] as f32 / s);
        let at = |r: R| R::new(r.x - ox, r.y - oy, r.w, r.h);
        for r in [R::new(0.0, 0.0, w, t), R::new(0.0, h - t, w, t), R::new(0.0, t, t, h - 2.0 * t), R::new(w - t, t, t, h - 2.0 * t)] { c.rect(at(r), 0.0, col); }
        for pts in corners(R::new(t, t, w - 2.0 * t, h - 2.0 * t), self.radius, 12) {
            let pts: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (x - ox, y - oy)).collect();
            if let Some(p) = polygon(&pts) { c.fill_path(&p, col); }
        }
    }
}

/// Opens / closes the frames to match config.json (called where `dock::sync` is).
pub fn sync(app: &mut App) {
    let want = settings(&app.cx);
    let (mons, _) = util::monitors();
    let (thick, radius) = want.unwrap_or_default();
    // Each monitor gets four strips. The ring is thickness + radius deep, in that monitor's own DPI.
    let spec: Vec<([i32; 4], [i32; 4])> = if want.is_some() {
        mons.iter().flat_map(|m| {
            let mon = [m.left, m.top, m.right, m.bottom];
            let e = ((thick + radius) as f32 * dpi_scale(RECT { left: m.left, top: m.top, right: m.right, bottom: m.bottom })).ceil() as i32;
            strips(mon[2] - mon[0], mon[3] - mon[1], e).map(|p| (mon, p))
        }).collect()
    } else { vec![] };
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if !cur.is_empty() && cur.iter().map(|c| (c.1, c.2, c.3, c.4)).eq(spec.iter().map(|(m, p)| (*m, *p, thick, radius))) { return; }
    for (h, ..) in cur.drain(..) { app.wins.retain(|w| w.hwnd.0 as isize != h); }
    for (m, p) in spec {
        let (x, y) = (m[0] + p[0], m[1] + p[1]);
        let Ok(mut w) = Win::new(Box::new(Frame { mon: m, part: p, thick: thick as f32, radius: radius as f32 }), "Zor UI / frame", x, y, Z::Top, true) else { continue };
        w.update(&app.g, &app.cx, true);
        w.move_to(x, y);
        w.show();
        w.pin(); // no app bar: nothing is reserved
        cur.push((w.hwnd.0 as isize, m, p, thick, radius));
        app.wins.push(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corner_arcs_stay_inside_their_square() {
        let inner = R::new(5.0, 5.0, 1910.0, 1070.0);
        let c = corners(inner, 20.0, 12);
        // top-left spandrel: starts at the corner, arc runs from (5, 25) to (25, 5)
        assert_eq!(c[0][0], (5.0, 5.0));
        let (first, last) = (c[0][1], *c[0].last().unwrap());
        assert!((first.0 - 5.0).abs() < 1e-3 && (first.1 - 25.0).abs() < 1e-3 && (last.0 - 25.0).abs() < 1e-3 && (last.1 - 5.0).abs() < 1e-3);
        for (i, pts) in c.iter().enumerate() {
            assert_eq!(pts.len(), 14, "corner {i}");
            for &(x, y) in pts { assert!(x >= 5.0 - 1e-3 && x <= 1915.0 + 1e-3 && y >= 5.0 - 1e-3 && y <= 1075.0 + 1e-3); }
        }
        // a huge radius is limited to half the shorter side
        assert!(corners(R::new(0.0, 0.0, 100.0, 40.0), 500.0, 4)[0][1].1 <= 20.0 + 1e-3);
    }

    #[test]
    fn strips_tile_the_ring_once() {
        let (w, h, e) = (1920, 1080, 25);
        let s = strips(w, h, e);
        for r in s { assert!(r[0] >= 0 && r[1] >= 0 && r[0] + r[2] <= w && r[1] + r[3] <= h, "{r:?}"); }
        // The strips lie in the ring of depth e. Their total area equals the ring's only if none overlap and none is missing.
        let area: i32 = s.iter().map(|r| r[2] * r[3]).sum();
        assert_eq!(area, w * h - (w - 2 * e) * (h - 2 * e));
        // A monitor smaller than twice the depth is clamped, never a negative strip.
        assert!(strips(40, 40, 50).iter().all(|r| r[2] >= 0 && r[3] >= 0));
    }
}
