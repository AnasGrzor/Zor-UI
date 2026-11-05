//! The screen frame (end4's `bar.showFrame` + `fakeScreenRounding`): a thin click-through ring around each monitor
//! whose inner corners are rounded, so windows look like they sit in a rounded screen.

use crate::{
    app::{App, Ctx},
    gfx::{polygon, Canvas, Gfx, R},
    ui, util,
    win::{View, Win, Z},
};
use std::sync::Mutex;

/// The windows' (handle, monitor rect, thickness, radius) so a config or monitor change can rebuild them.
static CUR: Mutex<Vec<(isize, [i32; 4], u32, u32)>> = Mutex::new(Vec::new());

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

struct Frame { mon: [i32; 4], thick: f32, radius: f32 }

impl View for Frame {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) { ((self.mon[2] - self.mon[0]) as f32 / scale, (self.mon[3] - self.mon[1]) as f32 / scale) }

    fn key(&self, cx: &Ctx) -> String { ui::layer0(cx).to_string() }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let col = ui::layer0(cx);
        let (w, h, t) = (c.w, c.h, self.thick);
        for r in [R::new(0.0, 0.0, w, t), R::new(0.0, h - t, w, t), R::new(0.0, t, t, h - 2.0 * t), R::new(w - t, t, t, h - 2.0 * t)] { c.rect(r, 0.0, col); }
        for pts in corners(R::new(t, t, w - 2.0 * t, h - 2.0 * t), self.radius, 12) {
            if let Some(p) = polygon(&pts) { c.fill_path(&p, col); }
        }
    }
}

/// Opens / closes the frames to match config.json (called where `dock::sync` is).
pub fn sync(app: &mut App) {
    let want = settings(&app.cx);
    let (mons, _) = util::monitors();
    let spec: Vec<[i32; 4]> = if want.is_some() { mons.iter().map(|m| [m.left, m.top, m.right, m.bottom]).collect() } else { vec![] };
    let (thick, radius) = want.unwrap_or_default();
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if !cur.is_empty() && cur.iter().map(|c| (c.1, c.2, c.3)).collect::<Vec<_>>() == spec.iter().map(|m| (*m, thick, radius)).collect::<Vec<_>>() { return; }
    for (h, ..) in cur.drain(..) { app.wins.retain(|w| w.hwnd.0 as isize != h); }
    for m in spec {
        let Ok(mut w) = Win::new(Box::new(Frame { mon: m, thick: thick as f32, radius: radius as f32 }), "Zor UI / frame", m[0], m[1], Z::Top, true) else { continue };
        w.update(&app.g, &app.cx, true);
        w.move_to(m[0], m[1]);
        w.show();
        w.pin(); // no app bar: nothing is reserved
        cur.push((w.hwnd.0 as isize, m, thick, radius));
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
}
