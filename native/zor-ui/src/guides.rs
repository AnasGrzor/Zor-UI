//! Snapping guides: while a desktop card is dragged, thin lines show which edges and centres it lines up with, like
//! Figma's. One click-through overlay per monitor draws whatever `set` last stored.

use crate::{
    app::{App, Ctx},
    gfx::{Canvas, Gfx, R},
    util,
    win::{View, Win, Z},
};
use std::sync::Mutex;

/// A line on screen (physical px): `vertical` lines sit at x = `at`, horizontal ones at y = `at`, from `from` to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide { pub vertical: bool, pub at: i32, pub from: i32, pub to: i32 }

static LINES: Mutex<Vec<Guide>> = Mutex::new(Vec::new());
/// The overlay windows' (handle, monitor rect) so a monitor change can rebuild them.
static CUR: Mutex<Vec<(isize, [i32; 4])>> = Mutex::new(Vec::new());

fn lines() -> Vec<Guide> { LINES.lock().unwrap_or_else(|e| e.into_inner()).clone() }
pub fn set(l: Vec<Guide>) { *LINES.lock().unwrap_or_else(|e| e.into_inner()) = l; }
pub fn clear() { set(vec![]); }

/// Guides for a card at `mine` (left, top, right, bottom) among `others`: wherever one of its left / centre / right
/// (or top / centre / bottom) is within a pixel of the same line on another rectangle, a line spans both.
pub fn align_lines(mine: (i32, i32, i32, i32), others: &[(i32, i32, i32, i32)]) -> Vec<Guide> {
    let mut out: Vec<Guide> = vec![];
    let mut add = |g: Guide| if !out.contains(&g) { out.push(g); };
    for o in others {
        for (a, b) in [(mine.0, o.0), (mine.0, o.2), (mine.2, o.0), (mine.2, o.2), ((mine.0 + mine.2) / 2, (o.0 + o.2) / 2)] {
            if (a - b).abs() <= 1 { add(Guide { vertical: true, at: b, from: mine.1.min(o.1), to: mine.3.max(o.3) }); }
        }
        for (a, b) in [(mine.1, o.1), (mine.1, o.3), (mine.3, o.1), (mine.3, o.3), ((mine.1 + mine.3) / 2, (o.1 + o.3) / 2)] {
            if (a - b).abs() <= 1 { add(Guide { vertical: false, at: b, from: mine.0.min(o.0), to: mine.2.max(o.2) }); }
        }
    }
    out
}

/// Whether guides should be drawn: snapping and its lines are both on (each defaults to on) and the cards are shown.
pub fn wanted(cx: &Ctx) -> bool {
    let on = |k: &str| cx.cfg.get(k).as_bool() != Some(false);
    on("desktop.snap") && on("desktop.snapLines") && cx.cfg.get("desktopWidgets").as_bool() != Some(false)
}

struct Overlay { mon: [i32; 4] }

/// The overlay's size in CSS px: the whole monitor while lines show, 1 px otherwise, so the idle window
/// holds a 4-byte buffer instead of a full-screen one (8 MB at 1080p).
fn overlay_size(mon: [i32; 4], scale: f32, active: bool) -> (f32, f32) {
    if !active { return (1.0, 1.0); }
    ((mon[2] - mon[0]) as f32 / scale, (mon[3] - mon[1]) as f32 / scale)
}

impl View for Overlay {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) { overlay_size(self.mon, scale, !lines().is_empty()) }

    fn key(&self, _: &Ctx) -> String { format!("{:?}", lines()) }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let col = cx.theme.c("primary");
        let k = c.scale;
        for g in lines() {
            let (x0, y0) = (self.mon[0], self.mon[1]);
            let r = if g.vertical { R::new((g.at - x0) as f32 / k - 0.5, (g.from - y0) as f32 / k, 1.0, (g.to - g.from) as f32 / k) }
                else { R::new((g.from - x0) as f32 / k, (g.at - y0) as f32 / k - 0.5, (g.to - g.from) as f32 / k, 1.0) };
            c.rect(r, 0.0, col);
        }
    }
}

/// Opens / closes the overlays to match config.json (called where `frame::sync` is).
pub fn sync(app: &mut App) {
    let (mons, _) = util::monitors();
    let spec: Vec<[i32; 4]> = if wanted(&app.cx) { mons.iter().map(|m| [m.left, m.top, m.right, m.bottom]).collect() } else { vec![] };
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if cur.iter().map(|c| c.1).collect::<Vec<_>>() == spec { return; }
    for (h, _) in cur.drain(..) { app.wins.retain(|w| w.hwnd.0 as isize != h); }
    for m in spec {
        let Ok(mut w) = Win::new(Box::new(Overlay { mon: m }), "Zor UI / guides", m[0], m[1], Z::Top, true) else { continue };
        w.update(&app.g, &app.cx, true);
        w.move_to(m[0], m[1]);
        w.show();
        w.pin();
        cur.push((w.hwnd.0 as isize, m));
        app.wins.push(w);
    }
}

/// Repaints the overlays if the lines changed.
pub fn refresh(app: &mut App) {
    let handles: Vec<isize> = CUR.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|c| c.0).collect();
    for w in app.wins.iter_mut().filter(|w| handles.contains(&(w.hwnd.0 as isize))) { w.update(&app.g, &app.cx, false); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_span_both_cards() {
        // a card whose left edge sits on another's left edge (x=100) gets a vertical line covering both
        let g = align_lines((100, 300, 400, 500), &[(100, 100, 300, 250)]);
        assert!(g.contains(&Guide { vertical: true, at: 100, from: 100, to: 500 }), "{g:?}");
    }

    #[test]
    fn centres_and_one_pixel_slack() {
        let g = align_lines((150, 0, 250, 50), &[(100, 100, 301, 150)]); // centres 200 and 200.5 -> 200 / 200
        assert!(g.iter().any(|l| l.vertical && l.at == 200), "{g:?}");
        assert!(align_lines((150, 0, 250, 50), &[(400, 100, 500, 150)]).is_empty());
    }

    #[test]
    fn horizontal_alignment() {
        let g = align_lines((0, 100, 50, 200), &[(300, 100, 400, 180)]);
        assert!(g.contains(&Guide { vertical: false, at: 100, from: 0, to: 400 }), "{g:?}");
    }

    #[test]
    fn idle_overlay_is_one_pixel() {
        let mon = [0, 0, 1920, 1080];
        assert_eq!(overlay_size(mon, 1.0, false), (1.0, 1.0));
        assert_eq!(overlay_size(mon, 1.5, true), (1280.0, 720.0));
    }

    #[test]
    fn set_and_clear() {
        set(vec![Guide { vertical: true, at: 5, from: 0, to: 9 }]);
        assert_eq!(lines().len(), 1);
        clear();
        assert!(lines().is_empty());
    }
}
