//! The colour science of end4_theme.py: a Material You palette from an image or a hex colour,
//! plus the terminal colours harmonised toward it. Pure functions, no I/O.

use material_colors::{
    color::Rgb,
    dynamic_color::{DynamicScheme, MaterialDynamicColors, Platform, SpecVersion},
    hct::Hct,
    scheme::variant::{SchemeContent, SchemeExpressive, SchemeFidelity, SchemeFruitSalad, SchemeMonochrome, SchemeNeutral, SchemeRainbow, SchemeTonalSpot, SchemeVibrant},
};
use serde_json::Value;
use std::collections::BTreeMap;

/// The scheme names end4_theme.py accepts besides "auto".
pub const SCHEMES: &[&str] = &["content", "expressive", "fidelity", "fruit-salad", "monochrome", "neutral", "rainbow", "tonal-spot", "vibrant"];

/// end4's terminal base colours (scripts/terminal/scheme-base.json), compiled in.
const TERM_BASE: &str = include_str!("../../../../../scripts/terminal/scheme-base.json");

pub fn rgb_hex(c: Rgb) -> String { format!("#{:02X}{:02X}{:02X}", c.red, c.green, c.blue) }

/// "#RRGGBB" (the # is optional). None if it isn't six hex digits.
pub fn hex_rgb(h: &str) -> Option<Rgb> {
    let h = h.trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() { return None; }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Rgb::new(p(0)?, p(2)?, p(4)?))
}

fn sanitize(d: f64) -> f64 { let r = d % 360.0; if r < 0.0 { r + 360.0 } else { r } }
fn difference(a: f64, b: f64) -> f64 { 180.0 - ((a - b).abs() - 180.0).abs() }
fn direction(from: f64, to: f64) -> f64 { if sanitize(to - from) <= 180.0 { 1.0 } else { -1.0 } }

/// Shifts `design`'s hue toward `source` (by `harmony` of the gap, at most `threshold` degrees).
pub fn harmonize(design: Rgb, source: Rgb, threshold: f64, harmony: f64) -> Rgb {
    let (a, b) = (Hct::new(design), Hct::new(source));
    let rot = (difference(a.get_hue(), b.get_hue()) * harmony).min(threshold);
    Hct::from(sanitize(a.get_hue() + rot * direction(a.get_hue(), b.get_hue())), a.get_chroma(), a.get_tone()).into()
}

/// Scales chroma and tone.
pub fn boost(c: Rgb, chroma: f64, tone: f64) -> Rgb {
    let h = Hct::new(c);
    Hct::from(h.get_hue(), h.get_chroma() * chroma, h.get_tone() * tone).into()
}

/// Pillow's `Image.resize(.., BICUBIC)` on RGB bytes (antialiased, fixed-point like Pillow), so
/// the seed colour matches the Python script's.
pub fn pil_resize(src: &[u8], (w, h): (usize, usize), (nw, nh): (usize, usize)) -> Vec<u8> {
    let pass = |src: &[u8], (w, h): (usize, usize), n: usize, horizontal: bool| -> Vec<u8> {
        let (inn, scale) = if horizontal { (w, w as f64 / n as f64) } else { (h, h as f64 / n as f64) };
        let fs = scale.max(1.0);
        let support = 2.0 * fs;
        let cubic = |x: f64| {
            let x = x.abs();
            if x < 1.0 { ((1.5 * x - 2.5) * x) * x + 1.0 } else if x < 2.0 { (((x - 5.0) * x + 8.0) * x - 4.0) * -0.5 } else { 0.0 }
        };
        // (first source index, fixed-point coefficients) per output index
        let coefs: Vec<(usize, Vec<i32>)> = (0..n)
            .map(|o| {
                let center = (o as f64 + 0.5) * scale;
                let lo = ((center - support + 0.5) as i64).max(0) as usize;
                let hi = (((center + support + 0.5) as i64).max(0) as usize).min(inn);
                let mut k: Vec<f64> = (lo..hi).map(|x| cubic((x as f64 - center + 0.5) / fs)).collect();
                let sum: f64 = k.iter().sum();
                if sum != 0.0 { k.iter_mut().for_each(|v| *v /= sum); }
                (lo, k.iter().map(|v| (v * 4194304.0 + if *v < 0.0 { -0.5 } else { 0.5 }) as i32).collect())
            })
            .collect();
        let (ow, oh) = if horizontal { (n, h) } else { (w, n) };
        let mut out = vec![0u8; ow * oh * 3];
        for y in 0..oh {
            for x in 0..ow {
                let (o, line) = if horizontal { (x, y) } else { (y, x) };
                let (lo, k) = &coefs[o];
                for ch in 0..3 {
                    let mut acc = 1i64 << 21;
                    for (i, c) in k.iter().enumerate() {
                        let idx = if horizontal { (line * w + lo + i) * 3 + ch } else { ((lo + i) * w + line) * 3 + ch };
                        acc += src.get(idx).copied().unwrap_or(0) as i64 * *c as i64;
                    }
                    out[(y * ow + x) * 3 + ch] = (acc >> 22).clamp(0, 255) as u8;
                }
            }
        }
        out
    };
    let mut cur = src.to_vec();
    let mut size = (w, h);
    if nw != w { cur = pass(&cur, size, nw, true); size.0 = nw; }
    if nh != h { cur = pass(&cur, size, nh, false); }
    cur
}

/// The wallpaper's seed colour: shrink to ~`size`x`size` pixels, quantise (Celebi), score. `rgb` is w*h*3 bytes.
pub fn source_from_rgb(rgb: &[u8], w: usize, h: usize, size: usize) -> Rgb {
    let (mut w, mut h, mut data) = (w, h, rgb.to_vec());
    if w * h > size * size {
        let s = ((size * size) as f64 / (w * h) as f64).sqrt();
        let (nw, nh) = (((w as f64 * s).round() as usize).max(1), ((h as f64 * s).round() as usize).max(1));
        data = pil_resize(&data, (w, h), (nw, nh));
        (w, h) = (nw, nh);
    }
    let px: Vec<Rgb> = data.chunks_exact(3).take(w * h).map(|p| Rgb::new(p[0], p[1], p[2])).collect();
    material_colors::image::extract_color(&px)
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    for (i, part) in snake.split('_').enumerate() {
        let mut ch = part.chars();
        if let Some(c) = ch.next() {
            if i > 0 { out.extend(c.to_uppercase()); } else { out.push(c); }
            out.push_str(ch.as_str());
        }
    }
    out
}

/// materialyoucolor 3.x (what the venv has) defaults to the 2025 spec, so this does too.
fn build(name: &str, hct: Hct, dark: bool) -> DynamicScheme {
    let c = Some(0.0);
    match name {
        "content" => SchemeContent::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "expressive" => SchemeExpressive::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "fidelity" => SchemeFidelity::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "fruit-salad" => SchemeFruitSalad::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "monochrome" => SchemeMonochrome::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "neutral" => SchemeNeutral::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "rainbow" => SchemeRainbow::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        "vibrant" => SchemeVibrant::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
        _ => SchemeTonalSpot::with_spec(hct, dark, c, SpecVersion::Spec2025, Platform::Phone).scheme,
    }
}

/// (colours, terminal colours, scheme used). `smart` swaps a low-chroma seed to the neutral scheme.
pub fn generate(seed: Rgb, dark: bool, scheme_name: &str, smart: bool) -> (BTreeMap<String, String>, BTreeMap<String, String>, String) {
    let hct = Hct::new(seed);
    let name = if smart && hct.get_chroma() < 20.0 { "neutral" } else if SCHEMES.contains(&scheme_name) { scheme_name } else { "tonal-spot" };
    let scheme = build(name, hct, dark);
    let mut colors: BTreeMap<String, String> = MaterialDynamicColors::ALL.iter().map(|r| (camel(r.name()), rgb_hex(r.color().get_rgb(&scheme)))).collect();
    // end4's extended "success" palette.
    let success: [(&str, &str); 4] = if dark {
        [("success", "#B5CCBA"), ("onSuccess", "#213528"), ("successContainer", "#374B3E"), ("onSuccessContainer", "#D1E9D6")]
    } else {
        [("success", "#4F6354"), ("onSuccess", "#FFFFFF"), ("successContainer", "#D1E8D5"), ("onSuccessContainer", "#0C1F13")]
    };
    colors.extend(success.iter().map(|(k, v)| (k.to_string(), v.to_string())));

    // Terminal colours, harmonised toward the primary key colour (end4 defaults).
    let base: Value = serde_json::from_str(TERM_BASE).unwrap_or(Value::Null);
    let key = colors.get("primaryPaletteKeyColor").or(colors.get("primary")).and_then(|h| hex_rgb(h)).unwrap_or(seed);
    let mut term = BTreeMap::new();
    for (k, v) in base[if dark { "dark" } else { "light" }].as_object().into_iter().flatten() {
        let Some(c) = v.as_str().and_then(hex_rgb) else { continue };
        let out = if name == "monochrome" { c } else { boost(harmonize(c, key, 100.0, 0.8), 1.0, 1.0 + 0.35 * if dark { 1.0 } else { -1.0 }) };
        term.insert(k.clone(), rgb_hex(out));
    }
    (colors, term, name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_rejects_junk() {
        assert_eq!(rgb_hex(hex_rgb("#6750A4").unwrap()), "#6750A4");
        assert!(hex_rgb("#67").is_none() && hex_rgb("zzzzzz").is_none() && hex_rgb("é12345").is_none());
    }

    #[test]
    fn camel_names() {
        assert_eq!(camel("on_primary_container"), "onPrimaryContainer");
        assert_eq!(camel("primary_palette_key_color"), "primaryPaletteKeyColor");
    }

    #[test]
    fn resize_flat_colour_stays_flat() {
        let src = vec![200u8; 40 * 30 * 3];
        let out = pil_resize(&src, (40, 30), (10, 8));
        assert_eq!(out.len(), 10 * 8 * 3);
        assert!(out.iter().all(|v| *v == 200));
    }

    #[test]
    fn generate_has_the_keys_the_widgets_read() {
        let (c, t, used) = generate(hex_rgb("#6750A4").unwrap(), true, "tonal-spot", true);
        assert_eq!(used, "tonal-spot");
        for k in ["primary", "onSurface", "outlineVariant", "surfaceContainerLow", "primaryPaletteKeyColor", "success"] { assert!(c.contains_key(k), "{k}"); }
        assert_eq!(t.len(), 16);
        // a grey seed falls back to neutral when smart
        assert_eq!(generate(hex_rgb("#808080").unwrap(), false, "tonal-spot", true).2, "neutral");
    }
}
