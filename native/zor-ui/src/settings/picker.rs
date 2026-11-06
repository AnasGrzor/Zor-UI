//! The colour picker popover of "Theme from a color": a saturation/value square, a hue slider, a hex
//! field and preset colours. Picking only changes the popover's colour and the live preview (the whole
//! Settings window is painted with the palette the colour would give); Apply is what re-themes.

use crate::{
    app::Ctx,
    gfx::{alpha, txt, Align, Canvas, Color, R},
    ui::TextBox,
};
use tiny_skia::Pixmap;

pub const BG: u32 = 3;
pub const SQ: u32 = 4;
pub const HUE: u32 = 5;
pub const OK: u32 = 6;
pub const CANCEL: u32 = 7;
pub const HEX: u32 = 8;
pub const PRESET0: u32 = 20;

pub const PRESETS: [Color; 14] = [
    0xFFEF4444, 0xFFF97316, 0xFFFACC15, 0xFF4ADE80, 0xFF2DD4BF, 0xFF3B82F6, 0xFF6366F1,
    0xFFEC4899, 0xFFF43F5E, 0xFFD946EF, 0xFF8B5CF6, 0xFF0EA5E9, 0xFF10B981, 0xFF84CC16,
];

const W: f32 = 300.0;
const PAD: f32 = 14.0;

pub fn hsv_argb(h: f32, s: f32, v: f32) -> Color {
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let f = |n: f32| {
        let k = (n + h.rem_euclid(360.0) / 60.0) % 6.0;
        ((v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)) * 255.0).round() as u32
    };
    0xFF00_0000 | f(5.0) << 16 | f(3.0) << 8 | f(1.0)
}

pub fn argb_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let d = mx - mn;
    let h = if d == 0.0 { 0.0 } else if mx == r { ((g - b) / d).rem_euclid(6.0) } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 } * 60.0;
    (h, if mx == 0.0 { 0.0 } else { d / mx }, mx)
}

/// Where everything sits for a window of `w` x `h` CSS px: the popover hugs the right edge so the page stays visible.
#[derive(Clone)]
pub struct Geo { pub bx: R, pub sq: R, pub hue: R, pub hex: R, pub sw: R, pub label: R, pub presets: [(f32, f32); 14], pub cancel: R, pub ok: R }

pub fn geo(w: f32, h: f32) -> Geo {
    let inner = W - PAD * 2.0;
    let total = 434.0;
    let bx = R::new((w - W - 20.0).max(8.0), ((h - total) / 2.0).max(8.0), W, total);
    let (x, y) = (bx.x + PAD, bx.y + PAD);
    let sq = R::new(x, y, inner, 168.0);
    let hue = R::new(x, sq.y + sq.h + 14.0, inner, 14.0);
    let hex = R::new(x, hue.y + hue.h + 18.0, 160.0, 36.0);
    let sw = R::new(x + 170.0, hex.y, 36.0, 36.0);
    let label = R::new(x, hex.y + 36.0 + 16.0, inner, 16.0);
    let row1 = label.y + 16.0 + 8.0;
    let pitch = inner / 7.0;
    let mut presets = [(0.0, 0.0); 14];
    for (i, p) in presets.iter_mut().enumerate() { *p = (x + pitch * ((i % 7) as f32 + 0.5), row1 + (i / 7) as f32 * 38.0 + 13.0); }
    let by = row1 + 38.0 + 26.0 + 16.0;
    let half = (inner - 8.0) / 2.0;
    Geo { bx, sq, hue, hex, sw, label, presets, cancel: R::new(x, by, half, 36.0), ok: R::new(x + half + 8.0, by, half, 36.0) }
}

/// Hue 0..360, saturation and value 0..1. `grab`: 0 nothing, 1 the square, 2 the hue slider.
pub struct Picker {
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub grab: u8,
    pub g: Geo,
    pub tb: TextBox,
    sq_img: Option<((u32, u32), Pixmap)>,
    hue_img: Option<(u32, Pixmap)>,
}

impl Picker {
    pub fn new(c: Color, area: (f32, f32)) -> Picker {
        let (h, s, v) = argb_hsv(c);
        let mut p = Picker { h, s, v, grab: 0, g: geo(area.0, area.1), tb: TextBox::new(false), sq_img: None, hue_img: None };
        p.tb.set(&p.hex());
        p
    }

    pub fn color(&self) -> Color { hsv_argb(self.h, self.s, self.v) }
    pub fn hex(&self) -> String { format!("#{:06X}", self.color() & 0xFF_FFFF) }

    pub fn set_color(&mut self, c: Color) {
        (self.h, self.s, self.v) = argb_hsv(c);
        self.tb.set(&self.hex());
    }

    /// After typing in the hex field: a complete `#RRGGBB` moves the picker (the text itself is left alone).
    pub fn typed(&mut self) {
        let t = self.tb.text.trim();
        if t.len() == 7 { if let Some(c) = crate::conf::parse_hex(t) { (self.h, self.s, self.v) = argb_hsv(c); } }
    }

    pub fn start(&mut self, id: u32, x: f32, y: f32) {
        self.grab = match id { SQ => 1, HUE => 2, _ => 0 };
        self.drag(x, y);
    }

    pub fn drag(&mut self, x: f32, y: f32) {
        let g = &self.g;
        match self.grab {
            1 => { self.s = ((x - g.sq.x) / g.sq.w).clamp(0.0, 1.0); self.v = 1.0 - ((y - g.sq.y) / g.sq.h).clamp(0.0, 1.0); }
            2 => self.h = ((x - g.hue.x) / g.hue.w).clamp(0.0, 1.0) * 360.0,
            _ => return,
        }
        self.tb.set(&self.hex());
    }
}

/// Coverage (0..1) of a rounded rectangle at the pixel centre (x, y).
fn corner_alpha(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (px, py) = (x.min(w - x), y.min(h - y));
    if px >= r || py >= r { 1.0 } else { (r - (r - px).hypot(r - py) + 0.5).clamp(0.0, 1.0) }
}

/// Premultiplied BGRA picture of `w` x `h` px (the layout Canvas::image wants), each pixel from `color(u, v)` with u, v in 0..1.
fn bake(w: u32, h: u32, radius: f32, color: impl Fn(f32, f32) -> Color) -> Option<Pixmap> {
    let mut pm = Pixmap::new(w, h)?;
    for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        let (xi, yi) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
        let (c, a) = (color(xi / w as f32, yi / h as f32), corner_alpha(xi, yi, w as f32, h as f32, radius));
        let f = |sh: u32| (((c >> sh) & 255) as f32 * a).round() as u8;
        px.copy_from_slice(&[f(0), f(8), f(16), (a * 255.0).round() as u8]);
    }
    Some(pm)
}

pub fn paint(c: &mut Canvas, cx: &Ctx, p: &mut Picker) {
    let g = geo(c.w, c.h);
    p.g = g.clone();
    c.hit(R::new(0.0, 0.0, c.w, c.h), BG);
    c.shadow(8.0, 28.0, 0.4, |c, col| if let Some(col) = col { c.rect(g.bx, 22.0, col) });
    c.rect(g.bx, 22.0, alpha(cx.theme.c("outlineVariant"), 0.5));
    c.rect(R::new(g.bx.x + 1.0, g.bx.y + 1.0, g.bx.w - 2.0, g.bx.h - 2.0), 21.0, cx.theme.c("surfaceContainerHigh"));

    // the saturation/value square and the hue slider: baked per physical pixel, the square again when the hue moves
    let sc = c.scale;
    let key = ((p.h * 4.0) as u32, (sc * 100.0) as u32);
    if p.sq_img.as_ref().map(|i| i.0) != Some(key) {
        let h = p.h;
        p.sq_img = bake((g.sq.w * sc).round() as u32, (g.sq.h * sc).round() as u32, 14.0 * sc, |u, v| hsv_argb(h, u, 1.0 - v)).map(|i| (key, i));
    }
    if p.hue_img.as_ref().map(|i| i.0) != Some(key.1) {
        p.hue_img = bake((g.hue.w * sc).round() as u32, (g.hue.h * sc).round() as u32, g.hue.h / 2.0 * sc, |u, _| hsv_argb(u * 360.0, 1.0, 1.0)).map(|i| (key.1, i));
    }
    if let Some((_, img)) = &p.sq_img { c.image(img, g.sq, 1.0); }
    if let Some((_, img)) = &p.hue_img { c.image(img, g.hue, 1.0); }
    c.hit(g.sq, SQ);
    c.hit(R::new(g.hue.x, g.hue.y - 6.0, g.hue.w, g.hue.h + 12.0), HUE);

    let knob = |c: &mut Canvas, x: f32, y: f32, r: f32, col: Color| {
        c.circle(x, y, r + 1.5, 0x55000000);
        c.circle(x, y, r, 0xFFFF_FFFF);
        c.circle(x, y, r - 2.5, col);
    };
    knob(c, g.sq.x + p.s * g.sq.w, g.sq.y + (1.0 - p.v) * g.sq.h, 9.0, p.color());
    knob(c, g.hue.x + g.hue.w * p.h / 360.0, g.hue.y + g.hue.h / 2.0, 10.0, hsv_argb(p.h, 1.0, 1.0));

    // hex field and the colour it makes
    c.rect(g.hex, 12.0, alpha(cx.theme.c("outline"), 0.6));
    c.rect(R::new(g.hex.x + 1.0, g.hex.y + 1.0, g.hex.w - 2.0, g.hex.h - 2.0), 11.0, cx.theme.c("surfaceContainer"));
    c.hit(g.hex, HEX);
    p.tb.draw(c, cx, R::new(g.hex.x + 12.0, g.hex.y + 1.0, g.hex.w - 24.0, g.hex.h - 2.0), txt(14.0, 400), "#6750a4", true);
    c.rect(g.sw, 12.0, p.color());

    c.text("Presets", g.label, txt(12.0, 400), cx.theme.c("onSurfaceVariant"), Align::Left);
    for (i, &(x, y)) in g.presets.iter().enumerate() {
        let hov = c.hit(R::new(x - 15.0, y - 15.0, 30.0, 30.0), PRESET0 + i as u32);
        if PRESETS[i] == p.color() | 0xFF00_0000 {
            c.circle(x, y, 16.0, cx.theme.c("onSurface"));
            c.circle(x, y, 14.0, cx.theme.c("surfaceContainerHigh"));
        }
        c.circle(x, y, if hov { 13.5 } else { 12.5 }, PRESETS[i]);
    }

    for (r, id, label, bg, fg) in [(g.cancel, CANCEL, "Cancel", "secondaryContainer", "onSecondaryContainer"), (g.ok, OK, "Apply", "primary", "onPrimary")] {
        let hov = c.hit(r, id);
        c.rect(r, 18.0, cx.theme.c(bg));
        if hov { c.rect(r, 18.0, alpha(cx.theme.c(fg), 0.12)); }
        c.text(label, r, txt(14.0, 500), cx.theme.c(fg), Align::Center);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * pm.width() + x) * 4) as usize;
        pm.data()[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn hsv_known_values_and_round_trip() {
        assert_eq!((hsv_argb(0.0, 1.0, 1.0), hsv_argb(120.0, 1.0, 1.0), hsv_argb(240.0, 1.0, 1.0)), (0xFFFF0000, 0xFF00FF00, 0xFF0000FF));
        assert_eq!((hsv_argb(0.0, 0.0, 1.0), hsv_argb(77.0, 0.5, 0.0), hsv_argb(360.0, 1.0, 1.0)), (0xFFFFFFFF, 0xFF000000, 0xFFFF0000));
        for r in (0..=255u32).step_by(15) {
            for g in (0..=255u32).step_by(17) {
                for b in (0..=255u32).step_by(23) {
                    let c = 0xFF00_0000 | r << 16 | g << 8 | b;
                    let (h, s, v) = argb_hsv(c);
                    assert_eq!(hsv_argb(h, s, v), c, "{c:08X}");
                }
            }
        }
    }

    #[test]
    fn layout_fits_the_popover_and_stays_on_screen() {
        let g = geo(900.0, 700.0);
        assert!(g.bx.x + g.bx.w <= 900.0 && g.bx.y >= 0.0 && g.bx.y + g.bx.h <= 700.0);
        for r in [g.sq, g.hue, g.hex, g.sw, g.cancel, g.ok] {
            assert!(r.x >= g.bx.x && r.x + r.w <= g.bx.x + g.bx.w && r.y >= g.bx.y && r.y + r.h <= g.bx.y + g.bx.h, "{r:?}");
        }
        assert!(g.presets.iter().all(|&(x, y)| g.bx.contains(x - 13.0, y - 13.0) && g.bx.contains(x + 13.0, y + 13.0)));
        assert!(g.cancel.x + g.cancel.w < g.ok.x);
    }

    #[test]
    fn square_drag_sets_saturation_value_and_hex_and_clamps() {
        let mut p = Picker::new(0xFF00_00FF, (900.0, 700.0));
        let g = p.g.clone();
        p.start(SQ, g.sq.x, g.sq.y);
        assert_eq!((p.s, p.v, p.hex().as_str(), p.tb.text.as_str()), (0.0, 1.0, "#FFFFFF", "#FFFFFF"));
        p.drag(g.sq.x + g.sq.w + 50.0, g.sq.y + g.sq.h + 50.0);
        assert_eq!((p.s, p.v, p.tb.text.as_str()), (1.0, 0.0, "#000000"));
        assert!((p.h - 240.0).abs() < 0.01);
    }

    #[test]
    fn hue_drag_maps_the_slider_to_degrees() {
        let mut p = Picker::new(0xFFFF_0000, (900.0, 700.0));
        let g = p.g.clone();
        p.start(HUE, g.hue.x + g.hue.w / 3.0, g.hue.y);
        assert!((p.h - 120.0).abs() < 0.01 && p.tb.text == "#00FF00");
        p.drag(g.hue.x - 40.0, g.hue.y);
        assert_eq!(p.h, 0.0);
        p.drag(g.hue.x + g.hue.w + 40.0, g.hue.y);
        assert_eq!(p.h, 360.0);
    }

    #[test]
    fn a_press_elsewhere_grabs_nothing() {
        let mut p = Picker::new(0xFF12_3456, (900.0, 700.0));
        let before = (p.h, p.s, p.v, p.tb.text.clone());
        p.start(HEX, 10.0, 10.0);
        assert_eq!(p.grab, 0);
        assert_eq!(before, (p.h, p.s, p.v, p.tb.text.clone()));
    }

    #[test]
    fn typing_a_complete_hex_moves_the_picker_and_a_partial_one_does_not() {
        let mut p = Picker::new(0xFF00_0000, (900.0, 700.0));
        p.tb.set("#4F46E5");
        p.typed();
        assert_eq!(p.hex(), "#4F46E5");
        p.tb.set("#4F46");
        p.typed();
        assert_eq!(p.hex(), "#4F46E5");
        p.tb.set("#zzzzzz");
        p.typed();
        assert_eq!(p.hex(), "#4F46E5");
    }

    #[test]
    fn a_preset_sets_the_colour_and_the_field() {
        let mut p = Picker::new(0xFF00_0000, (900.0, 700.0));
        p.set_color(PRESETS[5]);
        assert_eq!((p.hex().as_str(), p.tb.text.as_str()), ("#3B82F6", "#3B82F6"));
    }

    #[test]
    fn baked_square_has_white_black_and_hue_corners_and_cut_edges() {
        let pm = bake(200, 120, 14.0, |u, v| hsv_argb(0.0, u, 1.0 - v)).unwrap();
        assert_eq!(bgra(&pm, 0, 0)[3], 0); // the rounded corner is cut away
        let black = bgra(&pm, 100, 118);
        assert!(black[0] < 4 && black[1] < 4 && black[2] < 4 && black[3] == 255, "{black:?}");
        let (white, red) = (bgra(&pm, 14, 2), bgra(&pm, 190, 1));
        assert!(white[0] > 230 && white[1] > 230 && white[2] > 230 && white[3] == 255, "{white:?}");
        assert!(red[2] > 245 && red[1] < 20 && red[0] < 20 && red[3] == 255, "{red:?}");
    }

    #[test]
    fn baked_hue_bar_runs_red_green_blue_in_bgra_order() {
        let pm = bake(360, 14, 7.0, |u, _| hsv_argb(u * 360.0, 1.0, 1.0)).unwrap();
        let (green, blue) = (bgra(&pm, 120, 7), bgra(&pm, 240, 7)); // B G R A
        assert!(green[1] == 255 && green[0] < 6 && green[2] < 6 && green[3] == 255, "{green:?}");
        assert!(blue[0] > 250 && blue[2] < 6 && blue[1] < 6, "{blue:?}");
        assert_eq!(bgra(&pm, 0, 0)[3], 0);
    }
}
