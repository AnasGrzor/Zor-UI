//! Drawing on the CPU: tiny-skia for shapes and pictures, DirectWrite for text (layout, variable
//! font axes, ligature icons) rasterised through glyph-run analysis. No Direct2D: its software mode
//! runs on WARP (a software Direct3D) and costs ~10 MB.
//!
//! Pixels are premultiplied BGRA, the layout UpdateLayeredWindow wants, so colours handed to
//! tiny-skia (which thinks in RGBA) are passed with red and blue swapped.

use std::{cell::RefCell, collections::HashMap, ffi::c_void, path::Path};
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, PixmapMut, PixmapPaint, Stroke, Transform};
use windows::{
    core::{implement, Interface, IUnknown, HSTRING},
    Win32::{
        Foundation::{BOOL, FALSE, GENERIC_READ, RECT},
        Graphics::{DirectWrite::*, Imaging::*},
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
    },
};

pub use tiny_skia::Pixmap;
pub type Geom = tiny_skia::Path;

pub type Color = u32; // 0xAARRGGBB, straight alpha

/// `color-mix(in srgb, c p%, transparent)`: the same colour at `a` times its alpha.
pub fn alpha(c: Color, a: f32) -> Color { ((((c >> 24) as f32 * a.clamp(0.0, 1.0)).round() as u32) << 24) | (c & 0xFF_FFFF) }

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8((c & 255) as u8, (c >> 8 & 255) as u8, (c >> 16 & 255) as u8, (c >> 24) as u8); // BGRA, see top
    p.anti_alias = true;
    p
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }
impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> R { R { x, y, w, h } }
    pub fn contains(self, x: f32, y: f32) -> bool { x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Font { pub size: f32, pub weight: u16, pub icon: bool, pub fill: bool }
/// Text in the pack font (Google Sans Flex), CSS font-size / font-weight.
pub const fn txt(size: f32, weight: u16) -> Font { Font { size, weight, icon: false, fill: false } }
/// Material Symbols Rounded, by ligature name ("sunny"), like `icon()` in core.js.
pub const fn ico(size: f32, fill: bool) -> Font { Font { size, weight: 400, icon: true, fill } }

#[derive(Clone, Copy, PartialEq)]
pub enum Align { Left, Center, Right }

/// Clip shapes for pictures (shapeStyle in desktop.js).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape { Round(f32), Circle, Cookie(u32, f32), Squircle }
impl Shape {
    pub fn parse(s: &str) -> Shape {
        match s {
            "cookie" => Shape::Cookie(9, 0.06),
            "flower" => Shape::Cookie(8, 0.12),
            "clover" => Shape::Cookie(4, 0.2),
            "squircle" => Shape::Squircle,
            "circle" => Shape::Circle,
            "square" => Shape::Round(0.0),
            _ => Shape::Round(32.0),
        }
    }
    pub fn path(self, r: R) -> Option<Geom> {
        match self {
            Shape::Cookie(lobes, depth) => polygon(&cookie_points(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, lobes, depth, 360)),
            Shape::Squircle => polygon(&squircle_points(r.x, r.y, r.w)),
            Shape::Circle => PathBuilder::from_circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0),
            Shape::Round(rad) => round_rect(r, rad),
        }
    }
}

/// Points of end4's "cookie" outline: a circle with `lobes` soft scallops (cookiePath in desktop.js).
pub fn cookie_points(cx: f32, cy: f32, r: f32, lobes: u32, depth: f32, steps: u32) -> Vec<(f32, f32)> {
    (0..steps).map(|i| {
        let a = i as f32 / steps as f32 * std::f32::consts::TAU;
        let rr = r * (1.0 - depth + depth * (lobes as f32 * a).cos());
        (cx + rr * a.cos(), cy + rr * a.sin())
    }).collect()
}

/// Superellipse of exponent 4 (squirclePath in desktop.js).
pub fn squircle_points(x: f32, y: f32, size: f32) -> Vec<(f32, f32)> {
    let r = size / 2.0;
    let f = |v: f32| v.signum() * v.abs().powf(0.5);
    (0..160).map(|i| {
        let a = i as f32 / 160.0 * std::f32::consts::TAU;
        (x + r + r * f(a.cos()), y + r + r * f(a.sin()))
    }).collect()
}

pub fn polygon(pts: &[(f32, f32)]) -> Option<Geom> {
    let mut b = PathBuilder::new();
    let (x, y) = *pts.first()?;
    b.move_to(x, y);
    for &(x, y) in &pts[1..] { b.line_to(x, y); }
    b.close();
    b.finish()
}

/// Cubic Bézier arc of up to 90°, centre (cx, cy), from angle a0 to a1 (radians, clockwise on screen).
fn arc_to(b: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    let k = 4.0 / 3.0 * ((a1 - a0) / 4.0).tan();
    let (s0, c0, s1, c1) = (a0.sin(), a0.cos(), a1.sin(), a1.cos());
    b.cubic_to(cx + r * (c0 - k * s0), cy + r * (s0 + k * c0), cx + r * (c1 + k * s1), cy + r * (s1 - k * c1), cx + r * c1, cy + r * s1);
}

pub fn round_rect(r: R, rad: f32) -> Option<Geom> {
    let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    if rad == 0.0 { return tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h).map(PathBuilder::from_rect); }
    use std::f32::consts::FRAC_PI_2 as Q;
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let mut b = PathBuilder::new();
    b.move_to(x0 + rad, y0);
    b.line_to(x1 - rad, y0);
    arc_to(&mut b, x1 - rad, y0 + rad, rad, -Q, 0.0);
    b.line_to(x1, y1 - rad);
    arc_to(&mut b, x1 - rad, y1 - rad, rad, 0.0, Q);
    b.line_to(x0 + rad, y1);
    arc_to(&mut b, x0 + rad, y1 - rad, rad, Q, 2.0 * Q);
    b.line_to(x0, y0 + rad);
    arc_to(&mut b, x0 + rad, y0 + rad, rad, 2.0 * Q, 3.0 * Q);
    b.close();
    b.finish()
}

const fn tag(t: &[u8; 4]) -> DWRITE_FONT_AXIS_TAG {
    DWRITE_FONT_AXIS_TAG(t[0] as u32 | (t[1] as u32) << 8 | (t[2] as u32) << 16 | (t[3] as u32) << 24)
}

pub fn wide(s: &str) -> Vec<u16> { s.encode_utf16().collect() }

pub struct Gfx {
    pub dw: IDWriteFactory5,
    pub wic: IWICImagingFactory,
    coll: IDWriteFontCollection1,
    /// The text font: config font.family when it's installed, else the pack font.
    text_family: RefCell<HSTRING>,
    default_family: HSTRING,
    /// config font.size: text (not icons) is drawn this many times its CSS size.
    text_scale: std::cell::Cell<f32>,
    /// config font.weight: added to every text weight (CSS 400 / 500 / 600...), so headings stay bolder.
    weight_shift: std::cell::Cell<i32>,
    icon_family: HSTRING,
    formats: RefCell<HashMap<(bool, bool, u32, u16), IDWriteTextFormat>>,
    /// Shaped text kept between frames (a label is drawn identically every frame); cleared on a font change or when big.
    layouts: RefCell<HashMap<(String, [u32; 3], [bool; 3], u16, u8), IDWriteTextLayout>>,
    sink: IDWriteTextRenderer,
}

impl Gfx {
    /// `fonts`: the pack's assets/fonts folder; `cache`: where the unpacked icon font is kept.
    pub fn new(fonts: &Path, cache: &Path) -> windows::core::Result<Gfx> {
        unsafe {
            let dw: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;

            // System fonts + the pack's two fonts. DirectWrite reads the variable axes (wght, FILL),
            // so weights like 500/600 and filled icons come out like the CSS.
            let builder: IDWriteFontSetBuilder1 = dw.CreateFontSetBuilder()?.cast()?;
            builder.AddFontSet(&dw.GetSystemFontSet()?)?;
            let add = |p: &Path| -> bool {
                let Ok(file) = dw.CreateFontFileReference(&HSTRING::from(p), None) else { return false };
                builder.AddFontFile(&file).is_ok()
            };
            let has_text = add(&fonts.join("GoogleSansFlex.ttf"));
            let icons = unpack_woff2(&dw, &fonts.join("MaterialSymbolsRounded.woff2"), &cache.join("MaterialSymbolsRounded.ttf"));
            let has_icons = icons.map(|p| add(&p)).unwrap_or(false);
            let coll = dw.CreateFontCollectionFromFontSet(&builder.CreateFontSet()?)?;
            let sink: IDWriteTextRenderer = Sink { f: dw.cast()? }.into();
            Ok(Gfx {
                dw, wic, coll, sink,
                // base.css: --font falls back to Segoe UI Variable Text
                text_family: RefCell::new(HSTRING::from(if has_text { "Google Sans Flex" } else { "Segoe UI Variable Text" })),
                default_family: HSTRING::from(if has_text { "Google Sans Flex" } else { "Segoe UI Variable Text" }),
                text_scale: std::cell::Cell::new(1.0),
                weight_shift: std::cell::Cell::new(0),
                icon_family: HSTRING::from(if has_icons { "Material Symbols Rounded" } else { "Segoe Fluent Icons" }),
                formats: RefCell::new(HashMap::new()),
                layouts: RefCell::new(HashMap::new()),
            })
        }
    }

    /// Every installed font family, for the Font setting's list.
    pub fn families(&self) -> Vec<String> {
        unsafe {
            let mut v: Vec<String> = (0..self.coll.GetFontFamilyCount()).filter_map(|i| {
                let names = self.coll.GetFontFamily(i).ok()?.GetFamilyNames().ok()?;
                let (mut at, mut found) = (0u32, BOOL(0));
                let _ = names.FindLocaleName(windows::core::w!("en-us"), &mut at, &mut found);
                if !found.as_bool() { at = 0; }
                let len = names.GetStringLength(at).ok()? as usize;
                let mut buf = vec![0u16; len + 1];
                names.GetString(at, &mut buf).ok()?;
                Some(String::from_utf16_lossy(&buf[..len]))
            }).collect();
            v.sort_by_key(|s| s.to_lowercase());
            v.dedup();
            v
        }
    }

    /// Settings > Appearance > Font. Returns true if anything changed (windows must repaint).
    pub fn set_font(&self, family: &str, scale: f32, shift: i32) -> bool {
        let installed = |name: &str| unsafe {
            let (mut i, mut ok) = (0u32, BOOL(0));
            self.coll.FindFamilyName(&HSTRING::from(name), &mut i, &mut ok).is_ok() && ok.as_bool()
        };
        let fam = if !family.trim().is_empty() && installed(family.trim()) { HSTRING::from(family.trim()) } else { self.default_family.clone() };
        let scale = if scale.is_finite() { scale.clamp(0.8, 1.3) } else { 1.0 };
        let shift = shift.clamp(-300, 300);
        if *self.text_family.borrow() == fam && self.text_scale.get() == scale && self.weight_shift.get() == shift { return false; }
        self.weight_shift.set(shift);
        *self.text_family.borrow_mut() = fam;
        self.text_scale.set(scale);
        self.formats.borrow_mut().clear();
        self.layouts.borrow_mut().clear();
        true
    }

    fn format(&self, f: Font) -> windows::core::Result<IDWriteTextFormat> {
        let key = (f.icon, f.fill, (f.size * 100.0) as u32, f.weight);
        if let Some(x) = self.formats.borrow().get(&key) { return Ok(x.clone()); }
        unsafe {
            let fam = if f.icon { self.icon_family.clone() } else { self.text_family.borrow().clone() };
            let size = if f.icon { f.size } else { f.size * self.text_scale.get() };
            let weight = if f.icon { f.weight } else { (f.weight as i32 + self.weight_shift.get()).clamp(100, 950) as u16 };
            let fmt = self.dw.CreateTextFormat(&fam, &self.coll, DWRITE_FONT_WEIGHT(weight as i32), DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL, size, &HSTRING::from("en-us"))?;
            if let Ok(f3) = fmt.cast::<IDWriteTextFormat3>() {
                let mut axes = vec![DWRITE_FONT_AXIS_VALUE { axisTag: tag(b"wght"), value: weight as f32 }];
                if f.icon {
                    axes.push(DWRITE_FONT_AXIS_VALUE { axisTag: tag(b"FILL"), value: if f.fill { 1.0 } else { 0.0 } });
                    axes.push(DWRITE_FONT_AXIS_VALUE { axisTag: tag(b"opsz"), value: f.size.clamp(20.0, 48.0) });
                }
                let _ = f3.SetFontAxisValues(&axes);
            }
            self.formats.borrow_mut().insert(key, fmt.clone());
            Ok(fmt)
        }
    }

    fn layout(&self, s: &str, f: Font, w: f32, h: f32, align: Align, wrap: bool) -> Option<IDWriteTextLayout> {
        let key = (s.to_string(), [f.size.to_bits(), w.to_bits(), h.to_bits()], [f.icon, f.fill, wrap], f.weight, align as u8);
        if let Some(l) = self.layouts.borrow().get(&key) { return Some(l.clone()); }
        let l = self.layout_new(s, f, w, h, align, wrap)?;
        let mut all = self.layouts.borrow_mut();
        if all.len() > LAYOUTS_MAX { all.clear(); }
        all.insert(key, l.clone());
        Some(l)
    }

    fn layout_new(&self, s: &str, f: Font, w: f32, h: f32, align: Align, wrap: bool) -> Option<IDWriteTextLayout> {
        unsafe {
            let l = self.dw.CreateTextLayout(&wide(s), &self.format(f).ok()?, w.max(1.0), h.max(1.0)).ok()?;
            let _ = l.SetTextAlignment(match align { Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING, Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER, Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING });
            let _ = l.SetParagraphAlignment(if wrap { DWRITE_PARAGRAPH_ALIGNMENT_NEAR } else { DWRITE_PARAGRAPH_ALIGNMENT_CENTER });
            if !wrap {
                let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                // text-overflow: ellipsis (the .ellipsis class)
                if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&l) {
                    let t = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, ..Default::default() };
                    let _ = l.SetTrimming(&t, &sign);
                }
            }
            Some(l)
        }
    }

    /// Width and height of one line of text.
    pub fn measure(&self, s: &str, f: Font) -> (f32, f32) {
        let Some(l) = self.layout(s, f, 10_000.0, 10_000.0, Align::Left, true) else { return (0.0, 0.0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { let _ = l.GetMetrics(&mut m); }
        (m.widthIncludingTrailingWhitespace, m.height)
    }

    /// Height wrapped text takes at width `w`.
    pub fn measure_wrapped(&self, s: &str, f: Font, w: f32) -> f32 {
        let Some(l) = self.layout(s, f, w, 10_000.0, Align::Left, true) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { let _ = l.GetMetrics(&mut m); }
        m.height
    }

    /// A layout for caret maths: top-left aligned, wrapped at `w` (or one endless line), no ellipsis.
    fn plain_layout(&self, s: &str, f: Font, w: f32, wrap: bool) -> Option<IDWriteTextLayout> {
        unsafe {
            let l = self.dw.CreateTextLayout(&wide(s), &self.format(f).ok()?, if wrap { w.max(1.0) } else { 100_000.0 }, 100_000.0).ok()?;
            if !wrap { let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP); }
            Some(l)
        }
    }

    /// Where the caret before byte `idx` of `s` sits, as (x, y, height) from the text's top-left,
    /// for text drawn with `Canvas::para` (wrap) or `Canvas::text` left-aligned (no wrap) at width `w`.
    pub fn caret(&self, s: &str, f: Font, w: f32, wrap: bool, idx: usize) -> (f32, f32, f32) {
        let fallback = (0.0, 0.0, f.size * self.text_scale.get() * 1.25);
        let Some(l) = self.plain_layout(s, f, w, wrap) else { return fallback };
        let idx = idx.min(s.len());
        let u = s.get(..idx).map_or(0, |p| p.encode_utf16().count());
        let (mut x, mut y, mut m) = (0f32, 0f32, DWRITE_HIT_TEST_METRICS::default());
        unsafe { if l.HitTestTextPosition(u as u32, FALSE, &mut x, &mut y, &mut m).is_err() { return fallback; } }
        (x, y, if m.height > 0.0 { m.height } else { fallback.2 })
    }

    /// The byte index of `s` nearest to the point (x, y) (relative to the text's top-left): the inverse of `caret`.
    pub fn hit_index(&self, s: &str, f: Font, w: f32, wrap: bool, x: f32, y: f32) -> usize {
        let Some(l) = self.plain_layout(s, f, w, wrap) else { return s.len() };
        let (mut trailing, mut inside, mut m) = (FALSE, FALSE, DWRITE_HIT_TEST_METRICS::default());
        unsafe { if l.HitTestPoint(x, y, &mut trailing, &mut inside, &mut m).is_err() { return s.len(); } }
        let u = (m.textPosition + if trailing.as_bool() { m.length } else { 0 }) as usize;
        let (mut units, mut at) = (0, s.len());
        for (b, ch) in s.char_indices() {
            if units >= u { at = b; break; }
            units += ch.len_utf16();
        }
        at
    }

    /// Decodes a picture at `w`x`h` physical px, centre-cropped to that aspect (object-fit: cover).
    pub fn load_image(&self, path: &Path, w: u32, h: u32) -> Option<Pixmap> {
        let dec = unsafe { self.wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()? };
        self.decode(&dec, w, h)
    }

    /// Width and height of a picture file in px.
    pub fn image_size(&self, path: &Path) -> Option<(u32, u32)> {
        unsafe {
            let dec = self.wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
            let (mut w, mut h) = (0, 0);
            dec.GetFrame(0).ok()?.GetSize(&mut w, &mut h).ok()?;
            Some((w, h))
        }
    }

    /// The same from file bytes in memory (media thumbnails).
    pub fn load_image_bytes(&self, bytes: &[u8], w: u32, h: u32) -> Option<Pixmap> {
        unsafe {
            let s = self.wic.CreateStream().ok()?;
            s.InitializeFromMemory(bytes).ok()?;
            let dec = self.wic.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnLoad).ok()?;
            self.decode(&dec, w, h)
        }
    }

    fn decode(&self, dec: &IWICBitmapDecoder, w: u32, h: u32) -> Option<Pixmap> {
        let (w, h) = (w.max(1), h.max(1));
        unsafe {
            let frame = dec.GetFrame(0).ok()?;
            let (mut iw, mut ih) = (0, 0);
            frame.GetSize(&mut iw, &mut ih).ok()?;
            let (sw, sh) = cover_crop(iw, ih, w, h);
            let clip = self.wic.CreateBitmapClipper().ok()?;
            clip.Initialize(&frame, &WICRect { X: ((iw - sw) / 2) as i32, Y: ((ih - sh) / 2) as i32, Width: sw as i32, Height: sh as i32 }).ok()?;
            let scale = self.wic.CreateBitmapScaler().ok()?;
            scale.Initialize(&clip, w, h, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            let conv = self.wic.CreateFormatConverter().ok()?;
            conv.Initialize(&scale, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut).ok()?;
            let mut px = vec![0u8; (w * h * 4) as usize];
            conv.CopyPixels(std::ptr::null(), w * 4, &mut px).ok()?;
            Pixmap::from_vec(px, tiny_skia::IntSize::from_wh(w, h)?)
        }
    }
}

/// Largest centred source rect with the target's aspect ratio.
pub fn cover_crop(iw: u32, ih: u32, w: u32, h: u32) -> (u32, u32) {
    if iw == 0 || ih == 0 || w == 0 || h == 0 { return (iw.max(1), ih.max(1)); }
    if iw as u64 * h as u64 > ih as u64 * w as u64 { ((ih as u64 * w as u64 / h as u64) as u32, ih) } else { (iw, (iw as u64 * h as u64 / w as u64) as u32) }
}

/// Material Symbols ships as WOFF2; DirectWrite unpacks it once to a TTF it can memory-map.
fn unpack_woff2(dw: &IDWriteFactory5, src: &Path, dst: &Path) -> Option<std::path::PathBuf> {
    let fresh = |d: &Path| Some(std::fs::metadata(d).ok()?.modified().ok()? >= std::fs::metadata(src).ok()?.modified().ok()?);
    if fresh(dst) == Some(true) { return Some(dst.into()); }
    let data = std::fs::read(src).ok()?;
    unsafe {
        let stream = dw.UnpackFontFile(DWRITE_CONTAINER_TYPE_WOFF2, data.as_ptr() as *const c_void, data.len() as u32).ok()?;
        let size = stream.GetFileSize().ok()?;
        let (mut ptr, mut ctx) = (std::ptr::null_mut::<c_void>(), std::ptr::null_mut());
        stream.ReadFileFragment(&mut ptr, 0, size, &mut ctx).ok()?;
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size as usize).to_vec();
        stream.ReleaseFileFragment(ctx);
        std::fs::create_dir_all(dst.parent()?).ok()?;
        std::fs::write(dst, bytes).ok()?;
    }
    Some(dst.into())
}

// ------------------------------------------------------------------ text rasterising

/// Receives DirectWrite's glyph runs and turns each into an 8-bit coverage texture.
#[implement(IDWriteTextRenderer)]
struct Sink { f: IDWriteFactory2 }

/// What `Sink` draws into, passed through DirectWrite as the client drawing context.
struct TextJob<'c, 'a> { c: &'c mut Canvas<'a>, color: Color, me: IDWriteTextRenderer }

impl IDWritePixelSnapping_Impl for Sink_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> windows::core::Result<BOOL> { Ok(FALSE) }
    fn GetCurrentTransform(&self, _: *const c_void, t: *mut DWRITE_MATRIX) -> windows::core::Result<()> {
        unsafe { *t = DWRITE_MATRIX { m11: 1.0, m22: 1.0, ..Default::default() }; }
        Ok(())
    }
    fn GetPixelsPerDip(&self, ctx: *const c_void) -> windows::core::Result<f32> { Ok(unsafe { (*(ctx as *const TextJob)).c.scale }) }
}

impl IDWriteTextRenderer_Impl for Sink_Impl {
    fn DrawGlyphRun(&self, ctx: *const c_void, x: f32, y: f32, mode: DWRITE_MEASURING_MODE, run: *const DWRITE_GLYPH_RUN,
        _: *const DWRITE_GLYPH_RUN_DESCRIPTION, _: Option<&IUnknown>) -> windows::core::Result<()> {
        unsafe {
            let job = &mut *(ctx as *mut TextJob);
            let s = job.c.scale;
            // shortcut: colour (emoji) glyphs draw as plain coverage; use TranslateColorGlyphRun if emoji show up
            let m = DWRITE_MATRIX { m11: s, m22: s, ..Default::default() };
            let a = self.f.CreateGlyphRunAnalysis(run, Some(&m), DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, mode,
                DWRITE_GRID_FIT_MODE_DEFAULT, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, x, y + job.c.dy)?;
            let b = a.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
            let (w, h) = (b.right - b.left, b.bottom - b.top);
            if w <= 0 || h <= 0 { return Ok(()); }
            let mut cov = vec![0u8; (w * h) as usize];
            a.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &b, &mut cov)?;
            job.c.blend(&b, &cov, job.color);
            Ok(())
        }
    }
    fn DrawUnderline(&self, ctx: *const c_void, x: f32, y: f32, u: *const DWRITE_UNDERLINE, _: Option<&IUnknown>) -> windows::core::Result<()> {
        unsafe { let j = &mut *(ctx as *mut TextJob); let u = &*u; j.c.rect(R::new(x, y + u.offset, u.width, u.thickness), 0.0, j.color); }
        Ok(())
    }
    fn DrawStrikethrough(&self, ctx: *const c_void, x: f32, y: f32, st: *const DWRITE_STRIKETHROUGH, _: Option<&IUnknown>) -> windows::core::Result<()> {
        unsafe { let j = &mut *(ctx as *mut TextJob); let st = &*st; j.c.rect(R::new(x, y + st.offset, st.width, st.thickness), 0.0, j.color); }
        Ok(())
    }
    fn DrawInlineObject(&self, ctx: *const c_void, x: f32, y: f32, obj: Option<&IDWriteInlineObject>, side: BOOL, rtl: BOOL, fx: Option<&IUnknown>) -> windows::core::Result<()> {
        // the "…" of an ellipsis-trimmed line
        let me = unsafe { (*(ctx as *const TextJob)).me.clone() };
        match obj { Some(o) => unsafe { o.Draw(Some(ctx), &me, x, y, side, rtl, fx) }, None => Ok(()) }
    }
}

// ------------------------------------------------------------------ canvas

enum Surface<'a> { Pix(PixmapMut<'a>), Mask(&'a mut Mask) }

/// One paint pass. Records clickable spots (`hit`) so the window can route clicks and hover.
pub struct Canvas<'a> {
    pub g: &'a Gfx,
    surf: Surface<'a>,
    clip: Option<Mask>,
    /// Physical px per CSS px.
    pub scale: f32,
    /// Vertical offset (CSS px) of everything drawn; the shadow pass uses it.
    dy: f32,
    pub hits: Vec<(R, u32)>,
    pub hover: u32,
    pub w: f32,
    pub h: f32,
}

impl<'a> Canvas<'a> {
    /// `px`: premultiplied BGRA pixels, `pw` x `ph`. Cleared to transparent.
    pub fn new(g: &'a Gfx, px: &'a mut [u8], pw: u32, ph: u32, scale: f32, hover: u32) -> Option<Canvas<'a>> {
        px.fill(0);
        Some(Canvas { g, surf: Surface::Pix(PixmapMut::from_bytes(px, pw, ph)?), clip: None, scale, dy: 0.0, hits: vec![], hover,
            w: pw as f32 / scale, h: ph as f32 / scale })
    }

    fn size_px(&self) -> (u32, u32) {
        match &self.surf { Surface::Pix(p) => (p.width(), p.height()), Surface::Mask(m) => (m.width(), m.height()) }
    }
    fn ts(&self) -> Transform { Transform::from_row(self.scale, 0.0, 0.0, self.scale, 0.0, self.dy * self.scale) }

    /// Something static drawn first (a window's panel, with its blurred shadow): `draw` renders it once into
    /// a bitmap kept under `key` (one entry; put everything it depends on in the key), later frames blit that.
    pub fn backdrop(&mut self, key: &[u32], draw: impl FnOnce(&mut Canvas)) {
        // a few entries, so two open windows (settings + overview) don't evict each other every frame
        let (pw, ph) = self.size_px();
        if !KEPT.with(|k| k.borrow().iter().any(|e| e.0 == key)) {
            let Some(mut pm) = Pixmap::new(pw, ph) else { return draw(self) };
            if let Some(mut sc) = Canvas::new(self.g, pm.data_mut(), pw, ph, self.scale, 0) { draw(&mut sc); }
            KEPT.with(|k| { let mut k = k.borrow_mut(); if k.len() >= KEPT_MAX { k.remove(0); } k.push((key.to_vec(), pm)); });
        }
        KEPT.with(|k| if let (Some((_, pm)), Surface::Pix(px)) = (k.borrow().iter().find(|e| e.0 == key), &mut self.surf) {
            // a backdrop goes on a fresh (cleared) canvas, so a plain copy equals source-over and skips the blend pipeline
            if self.clip.is_none() && px.data_mut().len() == pm.data().len() { px.data_mut().copy_from_slice(pm.data()); }
            else { px.draw_pixmap(0, 0, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), self.clip.as_ref()); }
        });
    }

    /// A clickable spot; returns true while the pointer is over it (for hover backgrounds).
    pub fn hit(&mut self, r: R, id: u32) -> bool { self.hits.push((r, id)); self.hover == id }

    pub fn fill_path(&mut self, p: &Geom, c: Color) {
        let ts = self.ts();
        match &mut self.surf {
            Surface::Pix(px) => px.fill_path(p, &paint(c), FillRule::Winding, ts, self.clip.as_ref()),
            Surface::Mask(m) => m.fill_path(p, FillRule::Winding, true, ts),
        }
    }

    fn stroke_path(&mut self, p: &Geom, width: f32, c: Color) {
        let st = Stroke { width, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
        if let Some(outline) = p.stroke(&st, self.scale) { self.fill_path(&outline, c); }
    }

    pub fn rect(&mut self, r: R, radius: f32, c: Color) { if let Some(p) = round_rect(r, radius) { self.fill_path(&p, c); } }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) { if let Some(p) = PathBuilder::from_circle(cx, cy, r) { self.fill_path(&p, c); } }

    pub fn line(&mut self, (x1, y1): (f32, f32), (x2, y2): (f32, f32), width: f32, c: Color) {
        let mut b = PathBuilder::new();
        b.move_to(x1, y1);
        b.line_to(x2, y2);
        if let Some(p) = b.finish() { self.stroke_path(&p, width, c); }
    }

    /// A round-capped stroke through `pts` (the wave progress bar).
    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Color) {
        let mut b = PathBuilder::new();
        let Some(&(x, y)) = pts.first() else { return };
        b.move_to(x, y);
        for &(x, y) in &pts[1..] { b.line_to(x, y); }
        if let Some(p) = b.finish() { self.stroke_path(&p, width, c); }
    }

    /// A ring arc from 12 o'clock, `frac` of a full turn (ring() in core.js).
    pub fn arc(&mut self, cx: f32, cy: f32, r: f32, frac: f32, width: f32, c: Color) {
        let frac = frac.clamp(0.0, 1.0);
        if frac <= 0.0 { return; }
        let mut b = PathBuilder::new();
        let (start, end) = (-std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU);
        b.move_to(cx + r * start.cos(), cy + r * start.sin());
        let steps = (frac * 4.0).ceil().max(1.0) as usize;
        for i in 0..steps {
            let a0 = start + (end - start) * i as f32 / steps as f32;
            arc_to(&mut b, cx, cy, r, a0, start + (end - start) * (i + 1) as f32 / steps as f32);
        }
        if let Some(p) = b.finish() { self.stroke_path(&p, width, c); }
    }

    fn draw_layout(&mut self, l: &IDWriteTextLayout, x: f32, y: f32, color: Color) {
        let me = self.g.sink.clone();
        let mut job = TextJob { c: self, color, me: me.clone() };
        unsafe { let _ = l.Draw(Some(&mut job as *mut TextJob as *const c_void), &me, x, y); }
    }

    /// One line of text, vertically centred in `r`, cut with an ellipsis if it doesn't fit.
    pub fn text(&mut self, s: &str, r: R, f: Font, c: Color, align: Align) {
        if let Some(l) = self.g.layout(s, f, r.w, r.h, align, false) { self.draw_layout(&l, r.x, r.y, c); }
    }

    /// Wrapped paragraph from the top of `r`.
    pub fn para(&mut self, s: &str, r: R, f: Font, c: Color, align: Align) {
        if let Some(l) = self.g.layout(s, f, r.w, r.h, align, true) { self.draw_layout(&l, r.x, r.y, c); }
    }

    pub fn icon(&mut self, name: &str, r: R, f: Font, c: Color) { self.text(name, r, f, c, Align::Center); }

    /// Glyph coverage (physical px rect `b`) in colour `c`, through the current clip.
    fn blend(&mut self, b: &RECT, cov: &[u8], c: Color) {
        let (pw, ph) = self.size_px();
        let (w, ca) = ((b.right - b.left) as usize, c >> 24);
        let (cb, cg, cr) = (c & 255, c >> 8 & 255, c >> 16 & 255);
        let clip = self.clip.as_ref().map(|m| m.data());
        let (data, bpp): (&mut [u8], usize) = match &mut self.surf { Surface::Pix(p) => (p.data_mut(), 4), Surface::Mask(m) => (m.data_mut(), 1) };
        for y in b.top.max(0)..b.bottom.min(ph as i32) {
            for x in b.left.max(0)..b.right.min(pw as i32) {
                let k = cov[(y - b.top) as usize * w + (x - b.left) as usize] as u32;
                if k == 0 { continue; }
                let i = y as usize * pw as usize + x as usize;
                let k = clip.map_or(k, |m| k * m[i] as u32 / 255);
                let a = k * ca / 255;
                if bpp == 1 { data[i] = data[i].max(a as u8); continue; }
                let d = &mut data[i * 4..i * 4 + 4];
                let inv = 255 - a;
                d[0] = (cb * a / 255 + d[0] as u32 * inv / 255) as u8;
                d[1] = (cg * a / 255 + d[1] as u32 * inv / 255) as u8;
                d[2] = (cr * a / 255 + d[2] as u32 * inv / 255) as u8;
                d[3] = (a + d[3] as u32 * inv / 255) as u8;
            }
        }
    }

    /// Runs `draw` clipped to a path (overflow: hidden with border-radius, clip-path).
    pub fn clip(&mut self, path: Option<Geom>, draw: impl FnOnce(&mut Self)) {
        let (Some(p), Surface::Pix(_)) = (path, &self.surf) else { return draw(self) };
        let (pw, ph) = self.size_px();
        let ts = self.ts();
        let m = match &self.clip {
            Some(old) => Mask::new(pw, ph).map(|mut m| {
                // the new path alone, then times the old clip, only inside the path's bounds (intersect_path walks the whole mask)
                m.fill_path(&p, FillRule::Winding, true, ts);
                let b = p.bounds();
                let (x0, x1) = (((b.left() * self.scale).floor() as i32 - 1).max(0) as usize, ((b.right() * self.scale).ceil() as i32 + 1).clamp(0, pw as i32) as usize);
                let (y0, y1) = ((((b.top() + self.dy) * self.scale).floor() as i32 - 1).max(0) as usize, (((b.bottom() + self.dy) * self.scale).ceil() as i32 + 1).clamp(0, ph as i32) as usize);
                let (o, d) = (old.data(), m.data_mut());
                for y in y0..y1.max(y0) { for i in y * pw as usize + x0..y * pw as usize + x1.max(x0) { d[i] = (d[i] as u32 * o[i] as u32 / 255) as u8; } }
                m
            }),
            None => Mask::new(pw, ph).map(|mut m| { m.fill_path(&p, FillRule::Winding, true, ts); m }),
        };
        let old = std::mem::replace(&mut self.clip, m);
        draw(self);
        self.clip = old;
    }

    pub fn clip_round(&mut self, r: R, radius: f32, draw: impl FnOnce(&mut Self)) { self.clip(round_rect(r, radius), draw) }

    /// A square-cornered clip that costs almost nothing: draws unclipped, then puts back what was outside `r`
    /// (a mask clip slows every fill and glyph in the closure). Only the strips around `r` are saved: the full
    /// height above and below, but sideways just 24 px. shortcut: drawing more than 24 px past the left or
    /// right edge leaks, use clip_round for that. Falls back to a mask inside another clip.
    pub fn scissor(&mut self, r: R, draw: impl FnOnce(&mut Self)) {
        let (pw, ph) = self.size_px();
        let (pw, ph) = (pw as usize, ph as usize);
        if self.clip.is_some() || !matches!(self.surf, Surface::Pix(_)) { return self.clip_round(r, 0.0, draw); }
        let side = (24.0 * self.scale) as usize;
        let (x0, x1) = (((r.x * self.scale).floor().max(0.0) as usize).min(pw), (((r.x + r.w) * self.scale).ceil().max(0.0) as usize).min(pw));
        let (y0, y1) = (((r.y * self.scale).floor().max(0.0) as usize).min(ph), (((r.y + r.h) * self.scale).ceil().max(0.0) as usize).min(ph));
        let (x1, y1) = (x1.max(x0), y1.max(y0));
        let (xa, xb) = (x0.saturating_sub(side), (x1 + side).min(pw));
        // x, y, w, h in device px
        let strips = [(xa, 0, xb - xa, y0), (xa, y1, xb - xa, ph - y1), (xa, y0, x0 - xa, y1 - y0), (x1, y0, xb - x1, y1 - y0)];
        let copy = |d: &[u8], (x, y, w, h): (usize, usize, usize, usize)| -> Vec<u8> {
            (y..y + h).flat_map(|row| d[(row * pw + x) * 4..(row * pw + x + w) * 4].iter().copied()).collect()
        };
        let saved: Vec<Vec<u8>> = match &mut self.surf { Surface::Pix(px) => strips.iter().map(|&st| copy(px.data_mut(), st)).collect(), _ => vec![] };
        draw(self);
        let Surface::Pix(px) = &mut self.surf else { return };
        let d = px.data_mut();
        for (&(x, y, w, h), bytes) in strips.iter().zip(&saved) {
            for (i, row) in (y..y + h).enumerate() { d[(row * pw + x) * 4..(row * pw + x + w) * 4].copy_from_slice(&bytes[i * w * 4..(i + 1) * w * 4]); }
        }
    }

    /// A picture decoded at its drawn size (Gfx::load_image), top-left at `r`.
    pub fn image(&mut self, img: &Pixmap, r: R, opacity: f32) {
        let (x, y) = ((r.x * self.scale).round() as i32, ((r.y + self.dy) * self.scale).round() as i32);
        let p = PixmapPaint { opacity, ..Default::default() };
        if let Surface::Pix(px) = &mut self.surf { px.draw_pixmap(x, y, img.as_ref(), &p, Transform::identity(), self.clip.as_ref()); }
    }

    /// A picture decoded at its drawn size, standing with its bottom centre on (x, y): mirrored when `dir` is
    /// negative and tilted `rot` radians around that point.
    pub fn sprite(&mut self, img: &Pixmap, x: f32, y: f32, dir: f32, rot: f32) {
        let (w, h) = (img.width() as f32, img.height() as f32);
        let t = Transform::from_translate(x * self.scale, (y + self.dy) * self.scale).pre_rotate(rot.to_degrees()).pre_scale(dir.signum(), 1.0).pre_translate(-w / 2.0, -h);
        if let Surface::Pix(px) = &mut self.surf { px.draw_pixmap(0, 0, img.as_ref(), &PixmapPaint::default(), t, self.clip.as_ref()); }
    }

    /// Soft shadow under what `draw` paints (CSS drop-shadow / text-shadow `0 dy blur`): `draw`
    /// runs into a coverage mask (it gets Some(colour) then), which is blurred and filled black;
    /// then `draw` runs again for real (None).
    pub fn shadow(&mut self, dy: f32, blur: f32, opacity: f32, draw: impl Fn(&mut Canvas, Option<Color>)) {
        let (pw, ph) = self.size_px();
        if let Some(mut m) = Mask::new(pw, ph) {
            {
                let mut sc = Canvas { g: self.g, surf: Surface::Mask(&mut m), clip: None, scale: self.scale, dy: self.dy + dy,
                    hits: vec![], hover: 0, w: self.w, h: self.h };
                draw(&mut sc, Some(0xFF00_0000));
            }
            box_blur(m.data_mut(), pw as usize, ph as usize, (blur / 2.0 * self.scale).round() as usize); // CSS blur = 2 sigma
            let full = tiny_skia::Rect::from_xywh(0.0, 0.0, pw as f32, ph as f32);
            if let (Surface::Pix(px), Some(full)) = (&mut self.surf, full) {
                px.fill_rect(full, &paint(alpha(0xFF00_0000, opacity)), Transform::identity(), Some(&m));
            }
        }
        draw(self, None);
    }
}

/// Most window-sized bitmaps `Canvas::backdrop` keeps, and most text layouts `Gfx::layout` keeps before it starts over.
const KEPT_MAX: usize = 2;
const LAYOUTS_MAX: usize = 500;
thread_local! { static KEPT: std::cell::RefCell<Vec<(Vec<u32>, Pixmap)>> = Default::default(); }

/// Frees the kept backdrops (a closed window's panel is a full-window bitmap that nothing will blit again).
pub fn drop_backdrops() { KEPT.with(|k| k.borrow_mut().clear()); }

/// Three box-blur passes each way, about a Gaussian of std. dev. `r`.
fn box_blur(a: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 { return; }
    let mut tmp = vec![0u8; a.len()];
    let pass = |src: &[u8], dst: &mut [u8], len: usize, n: usize, step: usize, stride: usize| {
        for line in 0..n {
            let at = |i: isize| src[line * stride + (i.clamp(0, len as isize - 1) as usize) * step] as u32;
            let mut sum: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for i in 0..len {
                dst[line * stride + i * step] = (sum / (2 * r as u32 + 1)) as u8;
                sum += at(i as isize + r as isize + 1);
                sum -= at(i as isize - r as isize);
            }
        }
    };
    for _ in 0..3 {
        pass(a, &mut tmp, w, h, 1, w);
        pass(&tmp, a, h, w, w, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_backdrops_empties_the_cache() {
        KEPT.with(|k| k.borrow_mut().push((vec![1], Pixmap::new(2, 2).unwrap())));
        drop_backdrops();
        assert!(KEPT.with(|k| k.borrow().is_empty()));
        assert!(KEPT_MAX >= 2, "settings + overview must not evict each other");
    }

    #[test]
    fn colours() { assert_eq!(alpha(0xFF11_2233, 0.88), 0xE011_2233); }
    #[test]
    fn crop() {
        assert_eq!(cover_crop(1920, 1080, 100, 100), (1080, 1080));
        assert_eq!(cover_crop(1000, 2000, 200, 100), (1000, 500));
    }
    #[test]
    fn shapes() {
        assert_eq!(Shape::parse("flower"), Shape::Cookie(8, 0.12));
        assert_eq!(Shape::parse("whatever"), Shape::Round(32.0));
        let p = cookie_points(0.0, 0.0, 10.0, 9, 0.06, 360);
        assert!((p[0].0 - 10.0).abs() < 1e-4); // a lobe peak sits at angle 0
        let b = round_rect(R::new(0.0, 0.0, 100.0, 40.0), 20.0).unwrap().bounds();
        assert!((b.width() - 100.0).abs() < 0.01 && (b.height() - 40.0).abs() < 0.01);
    }
    #[test]
    fn blur_spreads() {
        let mut a = vec![0u8; 21 * 21];
        a[10 * 21 + 10] = 255;
        a[10 * 21 + 9] = 255;
        box_blur(&mut a, 21, 21, 2);
        assert!(a[10 * 21 + 10] > 0 && a[10 * 21 + 10] < 255 && a[10 * 21 + 14] > 0);
    }

    /// caret and hit_index agree, across a multibyte character and a wrapped line.
    #[test]
    fn caret_round_trip() {
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        let f = txt(16.0, 400);
        let s = "héllo wörld again";
        let (x0, _, h) = g.caret(s, f, 1000.0, false, 0);
        let (x1, _, _) = g.caret(s, f, 1000.0, false, 3); // after "hé"
        assert!(x0 == 0.0 && x1 > 10.0 && h > 10.0);
        assert_eq!(g.hit_index(s, f, 1000.0, false, x1 + 1.0, 0.0), 3);
        assert_eq!(g.hit_index(s, f, 1000.0, false, 5000.0, 0.0), s.len());
        let (_, y, _) = g.caret(s, f, 60.0, true, s.len()); // narrow: wraps to a later row
        assert!(y > h);
        let i = g.hit_index(s, f, 60.0, true, 1.0, y + 2.0);
        assert!(s.is_char_boundary(i) && i > 0);
    }

    /// Text lands where it's asked to at 1x and 2x (the glyph-run origin is in DIPs, scaled by the transform).
    #[test]
    fn text_position_scales() {
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED); }
        let g = Gfx::new(&crate::conf::pack_dir().join(r"assets\fonts"), &std::env::temp_dir()).unwrap();
        for scale in [1.0f32, 2.0] {
            let (pw, ph) = ((200.0 * scale) as u32, (60.0 * scale) as u32);
            let mut px = vec![0u8; (pw * ph * 4) as usize];
            let mut c = Canvas::new(&g, &mut px, pw, ph, scale, 0).unwrap();
            c.text("I", R::new(100.0, 0.0, 50.0, 60.0), txt(40.0, 400), 0xFFFF_FFFF, Align::Left);
            drop(c);
            let first = (0..pw).find(|&x| (0..ph).any(|y| px[((y * pw + x) * 4 + 3) as usize] > 128)).unwrap();
            assert!((first as f32 / scale - 100.0).abs() < 8.0, "scale {scale}: text starts at {}", first as f32 / scale);
        }
    }
}
