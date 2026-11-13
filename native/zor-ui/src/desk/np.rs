//! The now-playing desk cards (desktop.js renderMedia / renderNowPlaying): "media", "lyrics" and "cover".

use super::{LH, W};
use crate::{
    app::Ctx,
    bar,
    gfx::{alpha, cookie_points, ico, polygon, txt, Align, Canvas, Color, Font, Gfx, Pixmap, R},
    hub,
    lyrics::{self, Lyr, Lyrics, Track},
    media::{self, Audio, Media},
    mixer_app::{AppMix, Cmd},
    ui,
    win::{Input, Phase},
};
use std::sync::Arc;

/// The lyrics / cover cards are wider than the rest (`body[data-card="lyrics"] #col`).
pub const W2: f32 = 428.0;
const PREV: u32 = 1;
const PP: u32 = 2;
const NEXT: u32 = 3;
const SEEK: u32 = 4;
const MUTE: u32 = 5;
const VOL: u32 = 6;
const LYR: u32 = 7;
const NONE_H: f32 = 16.0 + 20.0 + 16.0; // .np-none: padding 16, 20 px icon
const WHITE: Color = 0xFFFF_FFFF;

pub struct Np {
    // Each cache holds the Arc it was made from: the address of a live Arc is never reused, so `Arc::ptr_eq` can't match a freed one.
    art: Option<(Arc<Vec<u8>>, u32, Pixmap)>,
    bg: Option<(Option<Arc<Vec<u8>>>, (u32, u32, Color), Pixmap)>,
    // synced lyrics slide
    lkey: Option<Arc<Lyrics>>,
    idx: Option<usize>,
    prev: Option<usize>,
    since: f64,
    from: f32,
    heights: Option<(Arc<Lyrics>, u32, Vec<f32>)>,
    scroll: ui::Scroll,
    plain_h: f32,
    seek: R,
    scrub: ui::Scrub,
    vol: R,
    mix: AppMix,
    drag_v: Option<f32>,
    /// ms between repaints the card wants right now (0: still).
    pub anim: u32,
}

impl Np {
    pub fn new() -> Np {
        Np { art: None, bg: None, lkey: None, idx: None, prev: None, since: 0.0, from: 0.0, heights: None, scroll: Default::default(), plain_h: 0.0,
            seek: R::default(), scrub: Default::default(), vol: R::default(), mix: AppMix::default(), drag_v: None, anim: 0 }
    }
}

/// Debug builds only: `ZORUI_SAMPLE_ART=<picture>` gives `zorui.exe --render` a cover to draw.
#[cfg(debug_assertions)]
fn sampled(mut m: Media) -> Media {
    if let Some(b) = std::env::var_os("ZORUI_SAMPLE_ART").and_then(|p| std::fs::read(p).ok()) { m.art = Some(Arc::new(b)); }
    m
}
#[cfg(not(debug_assertions))]
fn sampled(m: Media) -> Media { m }

fn session(cx: &Ctx) -> Option<&Media> { cx.media.as_ref().filter(|m| !m.title.is_empty()) }

/// fmtTime() in core.js.
pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let ch = |s: u32| (((a >> s & 255) as f32) * (1.0 - t) + ((b >> s & 255) as f32) * t).round() as u32;
    0xFF00_0000 | ch(16) << 16 | ch(8) << 8 | ch(0)
}

// ------------------------------------------------------------------ blurred cover background

/// Three box passes each way over premultiplied 4-channel pixels (std. dev. about `r`).
fn blur4(px: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 || h == 0 { return; }
    let mut tmp = vec![0u8; px.len()];
    let pass = |src: &[u8], dst: &mut [u8], len: usize, n: usize, step: usize, stride: usize| {
        for line in 0..n {
            for ch in 0..4 {
                let at = |i: isize| src[line * stride + (i.clamp(0, len as isize - 1) as usize) * step + ch] as u32;
                let mut sum: u32 = (-(r as isize)..=r as isize).map(at).sum();
                for i in 0..len as isize {
                    dst[line * stride + i as usize * step + ch] = (sum / (2 * r as u32 + 1)) as u8;
                    sum = sum + at(i + r as isize + 1) - at(i - r as isize);
                }
            }
        }
    };
    for _ in 0..3 {
        pass(px, &mut tmp, w, h, 4, w * 4);
        pass(&tmp, px, h, w, w * 4, 4);
    }
}

/// `#cover .bg` + its ::after, baked: the art blurred (28 px, saturate 1.3, opacity .85 over #222) with
/// `surf` at 35 % on top, `pw` x `ph` physical px. The art is blurred small and scaled up (a blur is smooth anyway).
fn cover_bg(g: &Gfx, art: Option<&[u8]>, pw: u32, ph: u32, scale: f32, surf: Color) -> Option<Pixmap> {
    let mut out = Pixmap::new(pw, ph)?;
    let (m, sc) = ((40.0 * scale) as u32, 4u32); // .bg is inset -40px
    let (bw, bh) = (pw + 2 * m, ph + 2 * m);
    let small = art.and_then(|b| g.load_image_bytes(b, (bw / sc).max(1), (bh / sc).max(1)));
    let (sw, sh) = small.as_ref().map_or((1, 1), |s| (s.width() as usize, s.height() as usize));
    let mut sm = small.map(|s| s.data().to_vec());
    if let Some(d) = sm.as_mut() { blur4(d, sw, sh, ((28.0 * scale / sc as f32).round() as usize).max(1)); }
    let (sb, sg, sr) = ((surf & 255) as f32, (surf >> 8 & 255) as f32, (surf >> 16 & 255) as f32);
    let dst = out.data_mut();
    for y in 0..ph as usize {
        for x in 0..pw as usize {
            let mut p = [0x22 as f32; 3];
            if let Some(d) = &sm {
                let fx = ((x as f32 + m as f32) * sw as f32 / bw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
                let fy = ((y as f32 + m as f32) * sh as f32 / bh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let s = |xx: usize, yy: usize, c: usize| d[(yy * sw + xx) * 4 + c] as f32;
                let mut a = [0.0f32; 3];
                for (c, ac) in a.iter_mut().enumerate() { *ac = (s(x0, y0, c) * (1.0 - tx) + s(x1, y0, c) * tx) * (1.0 - ty) + (s(x0, y1, c) * (1.0 - tx) + s(x1, y1, c) * tx) * ty; }
                let luma = 0.0722 * a[0] + 0.7152 * a[1] + 0.2126 * a[2];
                for c in 0..3 { p[c] = (luma + 1.3 * (a[c] - luma)).clamp(0.0, 255.0) * 0.85 + 0x22 as f32 * 0.15; }
            }
            let i = (y * pw as usize + x) * 4;
            dst[i] = (p[0] * 0.65 + sb * 0.35) as u8;
            dst[i + 1] = (p[1] * 0.65 + sg * 0.35) as u8;
            dst[i + 2] = (p[2] * 0.65 + sr * 0.35) as u8;
            dst[i + 3] = 255;
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ drawing helpers



struct Item { s: String, r: R, f: Font, col: Color, al: Align, wrap: bool }

/// A round control with a hover tint. Returns true while hovered.
fn round_btn(c: &mut Canvas, r: R, id: u32, icon: &str, size: f32, fg: Color, tint: Color) -> bool {
    let over = c.hit(r, id);
    if over { c.rect(r, r.h / 2.0, tint); }
    c.icon(icon, r, ico(size, true), fg);
    over
}

fn ease(t: f32) -> f32 { 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(4) }

/// The same cover (or both none): cache entries are matched by the Arc they were made from.
fn same_art(a: Option<&Arc<Vec<u8>>>, b: Option<&Arc<Vec<u8>>>) -> bool {
    match (a, b) { (Some(x), Some(y)) => Arc::ptr_eq(x, y), (None, None) => true, _ => false }
}

impl Np {
    fn art_for(&mut self, g: &Gfx, m: &Media, px: u32) -> Option<&Pixmap> {
        let bytes = m.art.as_ref()?;
        if self.art.as_ref().is_none_or(|a| !Arc::ptr_eq(&a.0, bytes) || a.1 != px) {
            self.art = g.load_image_bytes(bytes, px, px).map(|p| (bytes.clone(), px, p));
        }
        self.art.as_ref().map(|a| &a.2)
    }

    /// The playing app's volume: (level, muted, is the app's own). None: nothing to show.
    fn volume(&self, cx: &Ctx) -> Option<(f32, bool, bool)> { self.mix.level(cx) }

    fn app_name(&self, cx: &Ctx) -> Option<String> { self.mix.app(cx).map(|a| a.0) }

    pub fn size(&self, cx: &Ctx, id: &str) -> (f32, f32) {
        let pad = 8.0;
        let m = session(cx).is_some();
        match id {
            "media" => (W, if m { 92.0 + pad } else { pad }),
            "lyrics" => (W2, if m { 120.0 + 4.0 + 230.0 } else { NONE_H } + pad),
            _ => (W2, if m { 14.0 + 150.0 + 12.0 + 16.0 * LH + 14.0 * LH + 46.0 + 6.0 + 34.0 + if self.volume(cx).is_some() { 4.0 + 30.0 } else { 0.0 } + 14.0 } else { NONE_H } + pad),
        }
    }

    pub fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match ev {
            Input::Click(PREV, _) => media::control("previous"),
            Input::Click(PP, _) => media::control("togglePlayPause"),
            Input::Click(NEXT, _) => media::control("next"),
            Input::Click(MUTE, _) => self.toggle_mute(cx),
            Input::Wheel(LYR, d) => self.scroll.wheel(d, self.plain_h, 230.0),
            Input::Drag(SEEK, x, _, ph) => {
                if let Some(m) = session(cx).filter(|m| m.duration > 0.0) {
                    let v = ui::slider_value(self.seek, x);
                    self.scrub.set(v, ph == Phase::End);
                    if ph == Phase::End { media::seek(&m.id, v as f64 * m.duration); }
                }
            }
            Input::Drag(VOL, x, _, ph) => {
                let v = (ui::slider_value(self.vol, x) * 100.0).round();
                self.drag_v = if ph == Phase::End { None } else { Some(v) };
                self.set_volume(cx, v);
            }
            _ => {}
        }
    }

    /// setVolume() in desktop.js.
    fn set_volume(&mut self, cx: &Ctx, v: f32) { self.mix.set_volume(cx, v); }

    fn toggle_mute(&mut self, cx: &Ctx) {
        if let Some(app) = self.app_name(cx) {
            let now = !self.mix.muted(&app);
            self.mix.cache(&app, None, Some(now));
            self.mix.send(cx, Cmd::Mute(app, now));
        } else if let Some(a) = cx.audio { media::set_mute(!a.muted); }
    }

    /// Polls the mixer every 4 s while this card lives (apps start and stop playing).
    fn poll_mixer(&mut self, cx: &Ctx) { self.mix.poll(cx); }

    pub fn paint(&mut self, c: &mut Canvas, cx: &Ctx, id: &str, back: Option<&Pixmap>) {
        let t = &cx.theme;
        self.anim = 0;
        let h = c.h;
        let Some(m) = session(cx).cloned().map(sampled) else {
            if id == "media" { return; } // #media:empty { display: none }
            // .np-none
            let card = R::new(4.0, 4.0, c.w - 8.0, NONE_H);
            let fill = match id { "lyrics" => alpha(t.c("primaryContainer"), 0.8), "cover" => 0xFF22_2222, _ => alpha(t.c("surfaceContainerLow"), 0.88) };
            c.rect(card, 23.0, fill);
            let fg = if id == "cover" { WHITE } else { t.c("onSurfaceVariant") };
            c.icon("music_off", R::new(20.0, 20.0, 20.0, 20.0), ico(20.0, false), fg);
            c.text("Nothing playing", R::new(50.0, 20.0, c.w - 74.0, 20.0), txt(13.0, 400), fg, Align::Left);
            return;
        };
        let scale = c.scale;
        match id {
            "media" => {
                let ah = c.h - 8.0; // 92 by default, a preset size (1 row) makes it taller
                let card = R::new(4.0, 4.0, c.w - 8.0, ah);
                match back { Some(b) => c.image(b, R::new(0.0, 0.0, 0.0, 0.0), 1.0), None => c.rect(card, 23.0, alpha(t.c("surfaceContainerLow"), 0.88)) }
                // two rows or more: the picture on top, title and controls in a strip below; one cell: picture only
                let (tall, tiny) = (ah > 140.0, c.w < 160.0);
                let px = ((if tall || tiny { c.w - 8.0 } else { ah }) * scale).round() as u32;
                let art = self.art_for(c.g, &m, px).cloned();
                if tall || tiny {
                    let strip = if tall { 92.0 } else { 0.0 };
                    let region = R::new(4.0, 4.0, c.w - 8.0, ah - strip);
                    let aw = c.w - 8.0; // the decoded picture is a square this wide, centred in the region
                    c.clip_round(card, 23.0, |c| {
                        c.clip(crate::gfx::polygon(&[(region.x, region.y), (region.x + region.w, region.y), (region.x + region.w, region.y + region.h), (region.x, region.y + region.h)]), |c| match &art {
                            Some(a) => c.image(a, R::new(4.0, 4.0 - (aw - region.h) / 2.0, aw, aw), 1.0),
                            None => {
                                c.rect(region, 0.0, mix(t.c("primaryContainer"), t.c("tertiaryContainer"), 0.5));
                                c.icon("music_note", region, ico(34.0, true), alpha(t.c("onPrimaryContainer"), 0.6));
                            }
                        });
                        let (right, bottom) = (4.0 + c.w - 8.0 - 12.0, 4.0 + ah - 10.0);
                        let pp = R::new(right - 38.0, bottom - 38.0, 38.0, 38.0);
                        if tall {
                            let w = c.w - 8.0 - 24.0 - 38.0 * 3.0;
                            c.text(&m.title, R::new(18.0, 4.0 + ah - strip + 12.0, w, 14.0 * LH), txt(14.0, 500), t.c("onSurface"), Align::Left);
                            c.text(&m.artist, R::new(18.0, 4.0 + ah - strip + 12.0 + 14.0 * LH, w, 11.5 * LH), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left);
                            let tint = alpha(t.c("onSurface"), 0.08);
                            round_btn(c, R::new(pp.x - 4.0 - 30.0 - 4.0 - 30.0, pp.y + 4.0, 30.0, 30.0), PREV, "skip_previous", 18.0, t.c("onSurface"), tint);
                            round_btn(c, R::new(pp.x - 4.0 - 30.0, pp.y + 4.0, 30.0, 30.0), NEXT, "skip_next", 18.0, t.c("onSurface"), tint);
                        }
                        c.rect(pp, 12.0, t.c("primary"));
                        if c.hit(pp, PP) { c.rect(pp, 12.0, alpha(t.c("onPrimary"), 0.08)); }
                        c.icon(if m.playing { "pause" } else { "play_arrow" }, pp, ico(18.0, true), t.c("onPrimary"));
                    });
                    return;
                }
                c.clip_round(card, 23.0, |c| {
                    let r = R::new(4.0, 4.0, ah, ah);
                    match &art {
                        Some(a) => c.image(a, r, 1.0),
                        None => {
                            // linear-gradient(135deg, primary-container, tertiary-container), as a diagonal split of two blends
                            c.rect(r, 0.0, mix(t.c("primaryContainer"), t.c("tertiaryContainer"), 0.25));
                            c.clip(crate::gfx::polygon(&[(r.x + r.w, r.y), (r.x + r.w, r.y + r.h), (r.x, r.y + r.h)]), |c| c.rect(r, 0.0, mix(t.c("primaryContainer"), t.c("tertiaryContainer"), 0.75)));
                            c.icon("music_note", r, ico(34.0, true), alpha(t.c("onPrimaryContainer"), 0.6));
                        }
                    }
                    let x = 4.0 + ah + 12.0;
                    let w = c.w - 8.0 - ah - 24.0;
                    c.text(&m.title, R::new(x, 14.0, w, 14.0 * LH), txt(14.0, 500), t.c("onSurface"), Align::Left);
                    c.text(&m.artist, R::new(x, 14.0 + 14.0 * LH, w, 11.5 * LH), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left);
                    let tint = alpha(t.c("onSurface"), 0.08);
                    let (cy, right) = (4.0 + ah - 10.0 - 19.0, 4.0 + c.w - 8.0 - 12.0);
                    round_btn(c, R::new(right - 30.0, cy - 15.0, 30.0, 30.0), NEXT, "skip_next", 18.0, t.c("onSurface"), tint);
                    let pp = R::new(right - 30.0 - 4.0 - 38.0, cy - 19.0, 38.0, 38.0);
                    c.rect(pp, 12.0, t.c("primary"));
                    if c.hit(pp, PP) { c.rect(pp, 12.0, alpha(t.c("onPrimary"), 0.08)); }
                    c.icon(if m.playing { "pause" } else { "play_arrow" }, pp, ico(18.0, true), t.c("onPrimary"));
                    round_btn(c, R::new(pp.x - 4.0 - 30.0, cy - 15.0, 30.0, 30.0), PREV, "skip_previous", 18.0, t.c("onSurface"), tint);
                });
            }
            "lyrics" => self.paint_lyrics(c, cx, &m),
            _ => self.paint_cover(c, cx, &m, h),
        }
    }

    fn lyr_state(&self, cx: &Ctx, m: &Media) -> Option<Lyr> {
        #[cfg(debug_assertions)]
        if std::env::var_os("ZORUI_SAMPLE").is_some() { // `--render` has no network: a made-up song
            let l: Vec<(f64, String)> = (0..12).map(|i| (i as f64 * 7.0, format!("Sample lyric line number {i}, a little long to wrap around"))).collect();
            return Some(Lyr::Found(Arc::new(Lyrics::Synced(l))));
        }
        cx.cfg.bool("lyrics").then(|| lyrics::get(&Track { title: &m.title, artist: &m.artist, album: &m.album, dur: m.duration }))
            .filter(|l| !l.hidden_for(m))
    }

    fn paint_lyrics(&mut self, c: &mut Canvas, cx: &Ctx, m: &Media) {
        let t = &cx.theme;
        let fg = t.c("onPrimaryContainer");
        let card = R::new(4.0, 4.0, W2 - 8.0, 120.0 + 4.0 + 230.0);
        c.rect(card, 23.0, alpha(t.c("primaryContainer"), 0.8));
        let art = self.art_for(c.g, m, (120.0 * c.scale).round() as u32).cloned();
        let lyr = self.lyr_state(cx, m);
        let pos = m.now();
        c.clip_round(card, 23.0, |c| {
            let a = R::new(4.0, 4.0, 120.0, 120.0);
            c.rect(a, 0.0, t.c("secondaryContainer"));
            match &art { Some(i) => c.image(i, a, 1.0), None => c.icon("music_note", a, ico(20.0, true), t.c("onSecondaryContainer")) }
            let (x, w) = (4.0 + 120.0 + 14.0, W2 - 8.0 - 120.0 - 28.0);
            let (head, sub) = if m.artist.is_empty() { (m.title.as_str(), "") } else { (m.artist.as_str(), m.title.as_str()) };
            c.text(head, R::new(x, 4.0 + 14.0, w, 16.0 * LH), txt(16.0, 500), fg, Align::Left);
            c.text(sub, R::new(x, 4.0 + 14.0 + 16.0 * LH, w, 13.5 * LH), txt(13.5, 400), alpha(fg, 0.8), Align::Left);
            let tint = alpha(t.c("onSurface"), 0.08);
            let (cy, right) = (4.0 + 120.0 - 10.0 - 23.0, 4.0 + W2 - 8.0 - 14.0);
            round_btn(c, R::new(right - 34.0, cy - 17.0, 34.0, 34.0), NEXT, "skip_next", 20.0, fg, tint);
            let pr = R::new(right - 34.0 - 6.0 - 46.0, cy - 23.0, 46.0, 46.0);
            let cookie = polygon(&cookie_points(pr.x + 23.0, pr.y + 23.0, 23.0, 8, 0.1, 120));
            if let Some(p) = &cookie { c.fill_path(p, t.c("primary")); }
            if c.hit(pr, PP) { if let Some(p) = &cookie { c.fill_path(p, alpha(t.c("onPrimary"), 0.08)); } }
            c.icon(if m.playing { "pause" } else { "play_arrow" }, pr, ico(20.0, true), t.c("onPrimary"));
            round_btn(c, R::new(pr.x - 6.0 - 34.0, cy - 17.0, 34.0, 34.0), PREV, "skip_previous", 20.0, fg, tint);
        });
        let box_ = R::new(4.0, 4.0 + 120.0 + 4.0, W2 - 8.0, 230.0);
        c.clip_round(card, 23.0, |c| self.paint_lyr(c, cx, box_, lyr.as_ref(), pos, m.playing, fg, true, 14.5));
    }

    fn paint_cover(&mut self, c: &mut Canvas, cx: &Ctx, m: &Media, _h: f32) {
        let t = &cx.theme;
        self.poll_mixer(cx);
        let vol = self.volume(cx);
        let ch = c.h - 8.0;
        let card = R::new(4.0, 4.0, W2 - 8.0, ch);
        c.rect(card, 23.0, 0xFF22_2222);
        // blurred cover (cached per song and size)
        let (pw, ph) = ((card.w * c.scale).round() as u32, (card.h * c.scale).round() as u32);
        let surf = t.c("surface");
        if self.bg.as_ref().is_none_or(|b| !same_art(b.0.as_ref(), m.art.as_ref()) || b.1 != (pw, ph, surf)) {
            self.bg = cover_bg(c.g, m.art.as_ref().map(|a| a.as_slice()), pw, ph, c.scale, alpha(surf, 1.0)).map(|p| (m.art.clone(), (pw, ph, surf), p));
        }
        if let Some((_, _, bg)) = &self.bg { c.clip_round(card, 23.0, |c| c.image(bg, card, 1.0)); }
        let (x0, y0) = (18.0, 18.0);
        let right = 4.0 + W2 - 8.0 - 14.0;
        let art_r = R::new(x0, y0, 150.0, 150.0);
        c.shadow(6.0, 18.0, 0.35, |c, col| if let Some(col) = col { c.rect(art_r, 17.0, col) });
        let art = self.art_for(c.g, m, (150.0 * c.scale).round() as u32).cloned();
        c.clip_round(art_r, 17.0, |c| {
            c.rect(art_r, 0.0, 0x1FFF_FFFF);
            match &art { Some(i) => c.image(i, art_r, 1.0), None => c.icon("music_note", art_r, ico(20.0, true), WHITE) }
        });
        let lyr = self.lyr_state(cx, m);
        let pos = m.now();
        let lyr_r = R::new(x0 + 150.0 + 14.0, y0, right - (x0 + 150.0 + 14.0), 150.0);
        self.paint_lyr(c, cx, lyr_r, lyr.as_ref(), pos, m.playing, WHITE, false, 13.0);
        let ty = y0 + 150.0 + 12.0;
        let mut items = vec![
            Item { s: m.title.clone(), r: R::new(x0, ty, right - x0, 16.0 * LH), f: txt(16.0, 500), col: WHITE, al: Align::Left, wrap: false },
            Item { s: m.artist.clone(), r: R::new(x0, ty + 16.0 * LH, right - x0, 14.0 * LH), f: txt(12.5, 400), col: alpha(WHITE, 0.85), al: Align::Left, wrap: false },
        ];
        let my = ty + 16.0 * LH + 14.0 * LH;
        let time = format!("{}{}", fmt_time(pos), if m.duration > 0.0 { format!(" / {}", fmt_time(m.duration)) } else { String::new() });
        items.push(Item { s: time, r: R::new(x0, my, 200.0, 46.0), f: txt(13.0, 400), col: alpha(WHITE, 0.9), al: Align::Left, wrap: false });
        c.shadow(1.0, 6.0, 0.45, |c, col| for i in &items {
            let col = col.unwrap_or(i.col);
            if i.wrap { c.para(&i.s, i.r, i.f, col, i.al) } else { c.text(&i.s, i.r, i.f, col, i.al) }
        });
        let tint = alpha(WHITE, 0.15);
        let pp = R::new(right - 46.0, my, 46.0, 46.0);
        c.rect(pp, 17.0, alpha(WHITE, 0.85));
        if c.hit(pp, PP) { c.rect(pp, 17.0, 0x1400_0000); }
        c.icon(if m.playing { "pause" } else { "play_arrow" }, pp, ico(20.0, true), 0xFF22_2222);
        let sy = my + 46.0 + 6.0;
        round_btn(c, R::new(x0, sy, 34.0, 34.0), PREV, "skip_previous", 20.0, WHITE, tint);
        round_btn(c, R::new(right - 34.0, sy, 34.0, 34.0), NEXT, "skip_next", 20.0, WHITE, tint);
        self.seek = R::new(x0 + 34.0 + 6.0, sy + 6.0, right - x0 - 2.0 * (34.0 + 6.0), 22.0);
        ui::wave(c, self.seek, SEEK, self.scrub.get().unwrap_or_else(|| m.progress()), m.playing, WHITE);
        if let Some((level, muted, _)) = vol {
            let vy = sy + 34.0 + 4.0;
            let v = self.drag_v.unwrap_or(if muted { 0.0 } else { level.round() });
            let a = Audio { volume: v, muted: muted && self.drag_v.is_none(), mic_muted: None, mic_volume: None };
            round_btn(c, R::new(x0, vy, 30.0, 30.0), MUTE, bar::volume_icon(Some(&a)), 18.0, WHITE, tint);
            let track = R::new(x0 + 30.0 + 8.0, vy, right - x0 - 30.0 - 8.0 - 38.0 - 8.0, 30.0);
            self.vol = track;
            let (ty2, f) = (vy + 15.0, (v / 100.0).clamp(0.0, 1.0));
            c.rect(R::new(track.x, ty2 - 2.0, track.w, 4.0), 2.0, alpha(WHITE, 0.3));
            c.rect(R::new(track.x, ty2 - 2.0, track.w * f, 4.0), 2.0, WHITE);
            c.shadow(1.0, 4.0, 0.4, |c, col| c.circle(track.x + 7.0 + (track.w - 14.0) * f, ty2, 7.0, col.unwrap_or(WHITE)));
            c.hit(track, VOL);
            c.text(&format!("{}%", v as i32), R::new(right - 38.0, vy, 38.0, 30.0), txt(12.0, 400), alpha(WHITE, 0.85), Align::Right);
        }
        if m.playing { self.anim = 50; }
    }

    /// `.lyr`: synced lyrics with the current line in the middle and a slide when it changes, plain lyrics as a scrolling list,
    /// or a status line. `centre`: lyrics card (centred, padding 3/10) vs cover card (left, padding-right 18 %).
    #[allow(clippy::too_many_arguments)] // the args mirror the callers; a signature refactor is out of scope
    fn paint_lyr(&mut self, c: &mut Canvas, cx: &Ctx, r: R, lyr: Option<&Lyr>, pos: f64, playing: bool, fg: Color, centre: bool, cur_size: f32) {
        let now = hub::now_ms();
        let msg = |c: &mut Canvas, s: &str| c.text(s, r, txt(13.0, 400), alpha(fg, 0.6), Align::Center);
        let lyr = match lyr {
            None => return,
            Some(Lyr::Searching) => return msg(c, "Searching lyrics…"),
            Some(Lyr::Missing) => return msg(c, "No lyrics found"),
            Some(Lyr::Found(l)) => l.clone(),
        };
        let wrap_w = if centre { r.w - 20.0 } else { r.w * 0.82 };
        match &*lyr {
            Lyrics::Plain(lines) => {
                let hs: Vec<f32> = lines.iter().map(|l| c.g.measure_wrapped(l, txt(13.0, 400), wrap_w) + 6.0).collect();
                self.plain_h = hs.iter().sum();
                c.hit(r, LYR);
                let mut sc = std::mem::take(&mut self.scroll);
                sc.region(c, cx, r, self.plain_h, |c, top| {
                    let mut y = top;
                    for (l, h) in lines.iter().zip(&hs) {
                        if y + h > r.y && y < r.y + r.h {
                            let al = if centre { Align::Center } else { Align::Left };
                            c.para(l, R::new(r.x + if centre { 10.0 } else { 0.0 }, y + 3.0, wrap_w, h - 6.0), txt(13.0, 400), alpha(fg, 0.45), al);
                        }
                        y += h;
                    }
                });
                self.scroll = sc;
            }
            Lyrics::Synced(lines) => {
                let idx = lyrics::current_index(lines, pos);
                let fresh = !self.lkey.as_ref().is_some_and(|k| Arc::ptr_eq(k, &lyr));
                let (base, curf) = (txt(13.0, 400), txt(cur_size, 500));
                let wk = (wrap_w * 10.0) as u32;
                if self.heights.as_ref().is_none_or(|h| !Arc::ptr_eq(&h.0, &lyr) || h.1 != wk) {
                    let hs = lines.iter().map(|(_, s)| c.g.measure_wrapped(if s.is_empty() { "♪" } else { s }, base, wrap_w) + 6.0).collect();
                    self.heights = Some((lyr.clone(), wk, hs));
                }
                let hs = self.heights.as_ref().map(|h| &h.2).cloned().unwrap_or_default();
                let cur_h = |i: usize, c: &Canvas| { let s = &lines[i].1; c.g.measure_wrapped(if s.is_empty() { "♪" } else { s }, curf, wrap_w) + 6.0 };
                let offset = |i: Option<usize>, c: &Canvas| -> f32 {
                    let i = i.unwrap_or(0);
                    if lines.is_empty() { return 0.0; }
                    let top: f32 = hs.iter().take(i).sum();
                    let h = if idx.is_some() { cur_h(i, c) } else { hs.get(i).copied().unwrap_or(0.0) };
                    r.h / 2.0 - top - h / 2.0
                };
                let target = offset(idx, c);
                if fresh { self.lkey = Some(lyr.clone()); self.idx = idx; self.prev = None; self.since = now - 10_000.0; self.from = target; }
                else if idx != self.idx { self.prev = self.idx; self.from = self.shown(now, target); self.idx = idx; self.since = now; }
                let off = self.shown(now, target);
                let e = ease(((now - self.since) / 400.0) as f32);
                if (now - self.since) < 600.0 { self.anim = 16; } else if playing { self.anim = 100; }
                let (mut y, prev) = (off, self.prev);
                c.clip_round(r, 0.0, |c| for (i, (_, s)) in lines.iter().enumerate() {
                    let is_cur = Some(i) == idx;
                    let h = if is_cur { cur_h(i, c) } else { hs.get(i).copied().unwrap_or(0.0) };
                    let k = if is_cur { e } else if Some(i) == prev { 1.0 - e } else { 0.0 };
                    let (top, mid) = (y, y + h / 2.0);
                    y += h;
                    if top + h * 1.2 < 0.0 || top - h * 0.2 > r.h { continue; }
                    // mask-image: transparent -> opaque over the top 22 %, and back over the bottom 22 %
                    let edge = (mid / (r.h * 0.22)).min((r.h - mid) / (r.h * 0.22)).clamp(0.0, 1.0);
                    let sc = 1.0 + 0.18 * k;
                    let size = (if idx.is_some() && k > 0.0 { 13.0 + (cur_size - 13.0) * k } else { 13.0 }) * sc;
                    let f = txt(size, if k > 0.5 { 500 } else { 400 });
                    let txt_s = if s.is_empty() { "♪" } else { s.as_str() };
                    let w = wrap_w * sc;
                    let th = c.g.measure_wrapped(txt_s, f, w);
                    let (x, al) = if centre { (r.x + r.w / 2.0 - w / 2.0, Align::Center) } else { (r.x, Align::Left) };
                    c.para(txt_s, R::new(x, r.y + mid - th / 2.0, w, th + 2.0), f, alpha(fg, (0.45 + 0.55 * k) * edge), al);
                });
            }
        }
    }

    /// The slide position `since` ms ago towards `target` (cubic-bezier(.22,1,.36,1) over 0.6 s, close to an ease-out).
    fn shown(&self, now: f64, target: f32) -> f32 { self.from + (target - self.from) * ease(((now - self.since) / 600.0) as f32) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer_app::{coalesce, parse_mixer, session_app};
    #[test]
    fn app_names() {
        assert_eq!(session_app("Spotify.exe"), "spotify");
        assert_eq!(session_app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "microsoft.zunemusic");
        assert_eq!(session_app("C:\\x\\Foo.exe"), "foo");
        assert_eq!(fmt_time(73.9), "1:13");
    }
    #[test]
    fn mixer_list() {
        let v = serde_json::json!({ "apps": [{ "app": "Spotify", "volume": 40, "muted": false }, { "bad": 1 }] });
        assert_eq!(parse_mixer(&v), vec![("Spotify".to_string(), 40.0, false)]);
        assert!(parse_mixer(&serde_json::Value::Null).is_empty());
    }
    #[test]
    fn drag_sends_last_volume() {
        let cmds = vec![Cmd::Volume("a".into(), 1.0), Cmd::Volume("a".into(), 2.0), Cmd::Mute("a".into(), true), Cmd::Volume("a".into(), 3.0)];
        let out = coalesce(cmds);
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], Cmd::Volume(_, v) if *v == 2.0));
    }
    #[test]
    fn blur_keeps_flat_colour() {
        let mut px = vec![100u8; 8 * 8 * 4];
        blur4(&mut px, 8, 8, 2);
        assert!(px.iter().all(|&b| b == 100));
        let mut one = vec![0u8; 9 * 9 * 4];
        one[(4 * 9 + 4) * 4] = 255;
        blur4(&mut one, 9, 9, 1);
        assert!(one[(4 * 9 + 3) * 4] > 0 && one[(4 * 9 + 4) * 4] < 255);
    }
}
