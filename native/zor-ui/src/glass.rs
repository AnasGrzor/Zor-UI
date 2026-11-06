//! The "glass" look of a preset's desktop cards: the wallpaper behind a card, blurred, bent near the card's rim
//! with a colour fringe, tinted and given a highlight. Cards are bottom-most windows, so what is behind them is
//! the wallpaper; `backdrop` cuts that part out and `render` turns it into the card's fill.

use crate::{conf::Config, gfx::{Gfx, Pixmap}};
use std::{path::{Path, PathBuf}, sync::{Arc, Mutex}, time::SystemTime};

/// Brightness of the 1px line along a card's edge.
const EDGE_LINE: f32 = 0.14;

/// The value of a `desktop.glass.<key>` number when config.json has none (Settings shows the same).
pub fn default_of(key: &str) -> f64 {
    match key { "haze" => 6.0, "pull" => 50.0, "rimWidth" => 16.0, "sheen" => 0.3, "split" => 0.06, "tintStrength" => 0.3, "vividness" => 1.0, "frost" => 0.05, _ => 0.0 }
}

/// A preset's `background.widgets.glass`, in the same units (haze and rimWidth in CSS px, pull as a percent of the rim).
#[derive(Clone, Debug, PartialEq)]
pub struct Glass { pub frost: f32, pub haze: f32, pub pull: f32, pub rim: f32, pub sheen: f32, pub split: f32, pub tint: (f32, f32, f32), pub tint_strength: f32, pub vivid: f32 }

impl Glass {
    /// `desktop.glass` from config.json; None unless it is switched on.
    pub fn from_cfg(cfg: &Config) -> Option<Glass> {
        cfg.bool("desktop.glass.enable").then(|| {
            let n = |k: &str| cfg.get(&format!("desktop.glass.{k}")).as_f64().unwrap_or(default_of(k)) as f32;
            let tint = tint_rgb(cfg.str("desktop.glass.tint"));
            Glass { frost: n("frost").clamp(0.0, 0.3), haze: n("haze").clamp(0.0, 30.0), pull: n("pull").clamp(0.0, 100.0), rim: n("rimWidth").clamp(1.0, 60.0), sheen: n("sheen").clamp(0.0, 1.0),
                split: n("split").clamp(0.0, 0.5), tint: tint.unwrap_or((0.0, 0.0, 0.0)), tint_strength: if tint.is_some() { n("tintStrength").clamp(0.0, 1.0) } else { 0.0 },
                vivid: n("vividness").clamp(0.0, 3.0) }
        })
    }
}

/// "black", "white", "#rrggbb" as 0..1 channels; None for "transparent" or anything else (no tint).
pub fn tint_rgb(s: &str) -> Option<(f32, f32, f32)> {
    let v = match s {
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        _ => u32::from_str_radix(s.strip_prefix('#').filter(|h| h.len() == 6)?, 16).ok()?,
    };
    Some((((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0))
}

/// Signed distance (px, negative inside) from (px, py) to the rounded rectangle (x, y, w, h, radius).
fn sd(r: (f32, f32, f32, f32, f32), px: f32, py: f32) -> f32 {
    let (x, y, w, h, rad) = r;
    let rad = rad.min(w / 2.0).min(h / 2.0);
    let (qx, qy) = ((px - (x + w / 2.0)).abs() - (w / 2.0 - rad), (py - (y + h / 2.0)).abs() - (h / 2.0 - rad));
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rad
}

/// Three box blurs of radius `r` (about a Gaussian of that std. dev.) over each of the 4 channels.
fn blur(src: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    if r == 0 { return src.to_vec(); }
    let mut a = src.to_vec();
    let mut tmp = vec![0u8; a.len()];
    // one direction: `n` lines of `len` samples, `step` bytes apart, lines `stride` apart
    let pass = |from: &[u8], to: &mut [u8], len: usize, n: usize, step: usize, stride: usize| {
        for line in 0..n {
            for ch in 0..4 {
                let at = |i: isize| from[line * stride + ch + (i.clamp(0, len as isize - 1) as usize) * step] as u32;
                let mut sum: u32 = (-(r as isize)..=r as isize).map(at).sum();
                for i in 0..len {
                    to[line * stride + ch + i * step] = (sum / (2 * r as u32 + 1)) as u8;
                    sum += at(i as isize + r as isize + 1);
                    sum -= at(i as isize - r as isize);
                }
            }
        }
    };
    for _ in 0..3 {
        pass(&a, &mut tmp, w, h, 4, w * 4);
        pass(&tmp, &mut a, h, w, w * 4, 4);
    }
    a
}

/// Bilinear sample of an RGBA buffer at (x, y) (px), edge pixels repeated; channel `ch` as 0..1.
fn sample(a: &[u8], w: usize, h: usize, x: f32, y: f32, ch: usize) -> f32 {
    let (x, y) = (x.clamp(0.0, w as f32 - 1.0), y.clamp(0.0, h as f32 - 1.0));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |xx: usize, yy: usize| a[(yy * w + xx) * 4 + ch] as f32 / 255.0;
    (px(x0, y0) * (1.0 - fx) + px(x1, y0) * fx) * (1.0 - fy) + (px(x0, y1) * (1.0 - fx) + px(x1, y1) * fx) * fy
}

/// The glass fill for rounded rectangles `rects` (x, y, w, h, radius in physical px) laid over `back`, a picture of
/// what is behind the window. Same size as `back`; transparent outside the rectangles.
pub fn render(back: &Pixmap, rects: &[(f32, f32, f32, f32, f32)], g: &Glass, scale: f32) -> Option<Pixmap> {
    let (w, h) = (back.width() as usize, back.height() as usize);
    let mut out = Pixmap::new(w as u32, h as u32)?;
    let soft = blur(back.data(), w, h, (g.haze * scale).round() as usize);
    let (rim, pull) = ((g.rim * scale).max(1.0), g.rim * scale * g.pull / 100.0);
    let data = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let Some((r, d)) = rects.iter().map(|r| (*r, sd(*r, px, py))).filter(|(_, d)| *d < 0.5).min_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
            let cover = (0.5 - d).clamp(0.0, 1.0);
            // the outward normal of the edge nearest this pixel
            let (gx, gy) = (sd(r, px + 1.0, py) - sd(r, px - 1.0, py), sd(r, px, py + 1.0) - sd(r, px, py - 1.0));
            let len = (gx * gx + gy * gy).sqrt().max(1e-4);
            let (nx, ny) = (gx / len, gy / len);
            let edge = (1.0 - (-d) / rim).clamp(0.0, 1.0); // 1 at the rim, 0 once deeper than rimWidth
            let bend = pull * edge * edge;
            let mut rgb = [0f32; 3];
            for (i, k) in [1.0 + g.split, 1.0, 1.0 - g.split].into_iter().enumerate() {
                rgb[i] = sample(&soft, w, h, px - nx * bend * k, py - ny * bend * k, i);
            }
            let lum = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            let (tr, tg, tb) = g.tint;
            for (i, t) in [tr, tg, tb].into_iter().enumerate() {
                let v = lum + (rgb[i] - lum) * g.vivid;
                rgb[i] = v * (1.0 - g.tint_strength) + t * g.tint_strength;
            }
            // a highlight on the edges that face the top-left light
            let lit = (-(nx * 0.7 + ny * 0.7)).max(0.0) * edge * edge * g.sheen + g.frost + EDGE_LINE * (1.0 - (-d / 1.5).clamp(0.0, 1.0)) * scale.min(1.0).max(0.5);
            let o = (y * w + x) * 4;
            for i in 0..3 { data[o + i] = (((rgb[i] + lit).clamp(0.0, 1.0)) * cover * 255.0).round() as u8; }
            data[o + 3] = (cover * 255.0).round() as u8;
        }
    }
    Some(out)
}

/// The wallpaper decoded at a monitor's size, kept between cards.
struct Wall { path: PathBuf, stamp: SystemTime, size: (u32, u32), pix: Arc<Pixmap> }
static WALL: Mutex<Option<Wall>> = Mutex::new(None);

/// The wallpaper `path` under the screen rectangle `win` (x, y, w, h physical px) of the monitor `mon` (left, top,
/// right, bottom), fitted to the monitor the way the wallpaper is set (cover).
pub fn backdrop(g: &Gfx, path: &Path, mon: (i32, i32, i32, i32), win: (i32, i32, u32, u32)) -> Option<Pixmap> {
    let size = ((mon.2 - mon.0).max(1) as u32, (mon.3 - mon.1).max(1) as u32);
    let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let pix = {
        let mut cache = WALL.lock().ok()?;
        match cache.as_ref().filter(|w| w.path == path && w.stamp == stamp && w.size == size) {
            Some(w) => w.pix.clone(),
            None => {
                let pix = Arc::new(g.load_image(path, size.0, size.1)?);
                *cache = Some(Wall { path: path.to_path_buf(), stamp, size, pix: pix.clone() });
                pix
            }
        }
    };
    let mut out = Pixmap::new(win.2.max(1), win.3.max(1))?;
    let (ow, oh) = (out.width() as i32, out.height() as i32);
    let (sw, sh) = (pix.width() as i32, pix.height() as i32);
    for y in 0..oh {
        let sy = (win.1 - mon.1 + y).clamp(0, sh - 1);
        for x in 0..ow {
            let sx = (win.0 - mon.0 + x).clamp(0, sw - 1);
            let (s, d) = (((sy * sw + sx) * 4) as usize, ((y * ow + x) * 4) as usize);
            out.data_mut()[d..d + 4].copy_from_slice(&pix.data()[s..s + 4]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, rgb: [u8; 3]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for px in p.data_mut().chunks_mut(4) { px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]); }
        p
    }

    fn glass() -> Glass { Glass { frost: 0.05, haze: 2.0, pull: 80.0, rim: 12.0, sheen: 0.3, split: 0.1, tint: (0.0, 0.0, 0.0), tint_strength: 0.35, vivid: 1.0 } }

    #[test]
    fn tints() {
        assert_eq!(tint_rgb("black"), Some((0.0, 0.0, 0.0)));
        assert_eq!(tint_rgb("white"), Some((1.0, 1.0, 1.0)));
        assert_eq!(tint_rgb("#ff0000"), Some((1.0, 0.0, 0.0)));
        assert_eq!(tint_rgb("transparent"), None);
        assert_eq!(tint_rgb("#12"), None);
    }

    #[test]
    fn config_needs_enable() {
        assert!(Glass::from_cfg(&Config::parse(r#"{"desktop":{"glass":{"enable":false}}}"#)).is_none());
        let g = Glass::from_cfg(&Config::parse(r#"{"desktop":{"glass":{"enable":true,"haze":6,"pull":26,"tint":"transparent","tintStrength":0.3}}}"#)).unwrap();
        assert_eq!((g.haze, g.pull, g.tint_strength), (6.0, 26.0, 0.0)); // transparent tint = no tint
    }

    #[test]
    fn settings_are_clamped_and_default() {
        let g = Glass::from_cfg(&Config::parse(r#"{"desktop":{"glass":{"enable":true,"haze":999,"sheen":-4,"tint":"black"}}}"#)).unwrap();
        assert_eq!((g.haze, g.sheen), (30.0, 0.0));
        assert_eq!((g.pull, g.frost, g.vivid, g.tint_strength), (50.0, 0.05, 1.0, 0.3)); // not set: the defaults
        assert_eq!(default_of("nonsense"), 0.0);
    }

    #[test]
    fn rounded_rect_distance() {
        let r = (10.0, 10.0, 100.0, 50.0, 10.0);
        assert!(sd(r, 60.0, 35.0) < -20.0); // inside
        assert!((sd(r, 10.0, 35.0)).abs() < 1e-3); // on the left edge
        assert!(sd(r, 0.0, 0.0) > 5.0); // outside the corner
    }

    #[test]
    fn glass_fills_only_inside_and_tints() {
        let back = flat(120, 80, [200, 200, 200]);
        let out = render(&back, &[(10.0, 10.0, 100.0, 60.0, 12.0)], &glass(), 1.0).unwrap();
        let at = |x: usize, y: usize| { let o = (y * 120 + x) * 4; [out.data()[o], out.data()[o + 1], out.data()[o + 2], out.data()[o + 3]] };
        assert_eq!(at(2, 2)[3], 0, "outside the card stays transparent");
        let mid = at(60, 40);
        assert_eq!(mid[3], 255);
        // a 35% black tint over 200 grey (130) plus the frost lift (+13), no vividness change
        assert!((mid[0] as i32 - 143).abs() <= 2, "{mid:?}");
        // the 1px edge line is brighter than the middle
        assert!(at(60, 11)[0] > mid[0] || at(10, 40)[0] > mid[0], "edge line");
    }

    #[test]
    fn the_rim_differs_from_the_middle() {
        // dark left of x=16, light right of it: the seam is inside the left rim (x 10..22), so bending changes colours
        // there but not in the middle
        let mut back = flat(120, 80, [20, 20, 20]);
        for y in 0..80 { for x in 16..120 { let o = (y * 120 + x) * 4; back.data_mut()[o..o + 3].copy_from_slice(&[240, 240, 240]); } }
        let out = render(&back, &[(10.0, 10.0, 100.0, 60.0, 12.0)], &Glass { sheen: 0.0, haze: 0.0, ..glass() }, 1.0).unwrap();
        let plain = render(&back, &[(10.0, 10.0, 100.0, 60.0, 12.0)], &Glass { sheen: 0.0, haze: 0.0, pull: 0.0, split: 0.0, ..glass() }, 1.0).unwrap();
        assert_ne!(out.data(), plain.data());
        let o = (40 * 120 + 30) * 4; // far from the rim they agree
        assert_eq!(out.data()[o..o + 3], plain.data()[o..o + 3]);
    }
}
