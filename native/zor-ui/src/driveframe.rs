//! While the buddy drives (drive.rs), a coloured border runs around every monitor and a hint sits at the bottom: the screen itself
//! says the mouse is not yours right now and how to stop it. One click-through overlay per monitor, made when the drive starts and
//! removed when it ends. The overlays are hidden from the model's screenshots like every other window of this app.

use crate::{
    app::{App, Ctx},
    gfx::{alpha, txt, Align, Canvas, Gfx, R},
    util,
    win::{View, Win, Z},
};
use std::sync::Mutex;

/// What the hint says. Esc is the one way out that always works.
pub const HINT: &str = "The buddy is driving. Press Esc to stop.";
/// Border thickness in CSS px.
pub const THICK: f32 = 4.0;

/// The overlays' (handle, monitor rect), to tell when a monitor change needs them rebuilt.
static CUR: Mutex<Vec<(isize, [i32; 4])>> = Mutex::new(Vec::new());

/// The monitors that get an overlay: all of them while driving, none otherwise.
pub fn spec(driving: bool, mons: &[[i32; 4]]) -> Vec<[i32; 4]> { if driving { mons.to_vec() } else { vec![] } }

/// The four border strips of a `w` x `h` overlay (top, bottom, left, right); they meet at the corners without overlapping.
pub fn strips(w: f32, h: f32, t: f32) -> [R; 4] {
    [R::new(0.0, 0.0, w, t), R::new(0.0, h - t, w, t), R::new(0.0, t, t, h - 2.0 * t), R::new(w - t, t, t, h - 2.0 * t)]
}

/// The hint's pill for text `tw` wide and `th` high: centred, a little above the bottom edge (the bar is at the top and the
/// border runs along the edge).
pub fn hint_rect(w: f32, h: f32, tw: f32, th: f32) -> R {
    let (pw, ph) = (tw + 36.0, th + 18.0);
    R::new(((w - pw) / 2.0).round(), (h - THICK - 28.0 - ph).round(), pw, ph)
}

struct Overlay { mon: [i32; 4] }

impl View for Overlay {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) { ((self.mon[2] - self.mon[0]) as f32 / scale, (self.mon[3] - self.mon[1]) as f32 / scale) }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let accent = alpha(cx.theme.c("primary"), 0.9);
        for r in strips(c.w, c.h, THICK) { c.rect(r, 0.0, accent); }
        let font = txt(14.0, 600);
        let (tw, th) = c.g.measure(HINT, font);
        let pill = hint_rect(c.w, c.h, tw, th);
        c.rect(pill, pill.h / 2.0, alpha(cx.theme.c("surfaceContainer"), 0.94));
        c.text(HINT, R::new(pill.x, pill.y + 9.0, pill.w, th), font, cx.theme.c("onSurface"), Align::Center);
    }
}

/// Opens or closes the overlays to match whether a drive is running. Cheap when nothing changed, so the app can call it after every job.
pub fn sync(app: &mut App) {
    let (mons, _) = util::monitors();
    let spec = spec(crate::drive::driving(), &mons.iter().map(|m| [m.left, m.top, m.right, m.bottom]).collect::<Vec<_>>());
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if cur.iter().map(|c| c.1).collect::<Vec<_>>() == spec { return; }
    for (h, _) in cur.drain(..) { app.wins.retain(|w| w.hwnd.0 as isize != h); }
    for m in spec {
        let Ok(mut w) = Win::new(Box::new(Overlay { mon: m }), "Zor UI / driving", m[0], m[1], Z::Top, true) else { continue };
        crate::drive::hide_from_capture(w.hwnd); // before it is shown, so not even the first picture has it
        w.update(&app.g, &app.cx, true);
        w.move_to(m[0], m[1]);
        w.show();
        w.pin();
        cur.push((w.hwnd.0 as isize, m));
        app.wins.push(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_monitor_gets_a_border_only_while_driving() {
        let mons = [[0, 0, 1920, 1080], [-1920, 0, 0, 1080]];
        assert_eq!(spec(true, &mons), mons.to_vec());
        assert!(spec(false, &mons).is_empty(), "no drive, no overlay windows");
        assert!(spec(true, &[]).is_empty());
    }

    #[test]
    fn the_border_strips_cover_the_edge_once() {
        let s = strips(1280.0, 720.0, 4.0);
        let area: f32 = s.iter().map(|r| r.w * r.h).sum();
        assert_eq!(area, 1280.0 * 720.0 - (1280.0 - 8.0) * (720.0 - 8.0), "the ring and nothing else, no corner counted twice");
        assert_eq!((s[0].y, s[1].y + s[1].h), (0.0, 720.0));
        assert_eq!((s[2].x, s[3].x + s[3].w), (0.0, 1280.0));
    }

    #[test]
    fn the_hint_sits_centred_inside_the_border_near_the_bottom() {
        let r = hint_rect(1280.0, 720.0, 300.0, 18.0);
        assert!((r.x + r.w / 2.0 - 640.0).abs() <= 0.5, "centred");
        assert!(r.y > 720.0 / 2.0 && r.y + r.h < 720.0 - THICK, "in the lower half and clear of the border: {r:?}");
        assert!(r.w > 300.0 && r.h > 18.0, "padding around the text");
    }

    #[test]
    fn the_hint_names_the_way_out() {
        assert!(HINT.contains("Esc") && HINT.to_lowercase().contains("driving"));
    }
}
