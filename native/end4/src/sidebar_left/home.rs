//! Home tab: the media player (art, title, synced lyrics, seek bar, controls, session chips) and the
//! four resource rings.

use super::Tab;
use crate::{
    app::Ctx,
    bar::pretty_app,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    hub, lyrics::{self, Lyr, Lyrics, Track}, media::{self, Media},
    ui,
    win::{Input, Phase},
};
use tiny_skia::Pixmap;

const PREV: u32 = 101;
const PLAY: u32 = 102;
const NEXT: u32 = 103;
const SEEK: u32 = 104;
const CHIP0: u32 = 120;
const RES_H: f32 = 100.0;

#[derive(Default)]
pub struct Home {
    chosen: Option<String>,
    art: Option<((String, u32), Option<Pixmap>)>,
    bar: Option<R>,
    /// Seek bar being dragged / just released: (fraction, show until ms).
    scrub: Option<(f32, f64)>,
    scroll: ui::Scroll,
    plain: (f32, f32),
    /// Heights of the synced lyric lines, per track and width.
    heights: Option<((String, u32), Vec<f32>)>,
    sessions: Vec<(String, u32)>,
    playing: bool,
}

/// fmtTime() in core.js.
pub fn fmt_time(s: f64) -> String { let s = s.max(0.0) as u64; format!("{}:{:02}", s / 60, s % 60) }

/// "3.2 MiB/s": bytes per second scaled to the IEC unit, one decimal under 10.
pub fn speed(bps: f64) -> String {
    let (mut v, mut u) = (bps.max(0.0), 0);
    while v >= 1024.0 && u < 4 { v /= 1024.0; u += 1; }
    let unit = ["B", "KiB", "MiB", "GiB", "TiB"][u];
    format!("{:.*} {unit}/s", if v < 10.0 { 1 } else { 0 }, v)
}

/// appFromSession() in sidebar-left.js, from the pretty app name.
fn app_label(m: &Media) -> String {
    let n = pretty_app(&m.id);
    if n.chars().count() > 18 { n.chars().take(18).collect::<String>() + "\u{2026}" } else if n.is_empty() { "Player".into() } else { n }
}

impl Home {
    fn current<'a>(&self, cx: &'a Ctx) -> Option<&'a Media> {
        cx.sessions.iter().find(|m| Some(&m.id) == self.chosen.as_ref()).or(cx.media.as_ref()).or(cx.sessions.first())
    }

    fn stat(c: &mut Canvas, cx: &Ctx, r: R, ic: &str, v: f32, val: &str, label: &str) {
        ui::card(c, cx, r, 17.0);
        let (mx, my) = (r.x + r.w / 2.0, r.y + 10.0 + 22.0);
        ui::ring(c, cx, mx, my, 44.0, 4.0, v);
        c.icon(ic, R::new(mx - 22.0, my - 22.0, 44.0, 44.0), ico(20.0, false), cx.theme.c("primary"));
        c.text(val, R::new(r.x, r.y + 58.0, r.w, 20.0), txt(15.0, 500), cx.theme.c("onSurface"), Align::Center);
        c.text(label, R::new(r.x, r.y + 78.0, r.w, 14.0), txt(11.0, 400), cx.theme.c("onSurfaceVariant"), Align::Center);
    }

    fn resources(c: &mut Canvas, cx: &Ctx, r: R) {
        let w = (r.w - 18.0) / 4.0;
        let net = cx.net.as_ref().map_or("\u{2014}".to_string(), |n| speed(n.rx));
        let pct = |v: f32| format!("{}%", (v * 100.0).round());
        let cells = [("memory", cx.cpu, pct(cx.cpu), "CPU"), ("memory_alt", cx.mem, pct(cx.mem), "RAM"), ("hard_drive", cx.disk, pct(cx.disk), "C:"), ("download", 0.0, net, "Network")];
        for (i, (ic, v, val, label)) in cells.iter().enumerate() {
            Self::stat(c, cx, R::new(r.x + i as f32 * (w + 6.0), r.y, w, r.h), ic, *v, val, label);
        }
    }

    fn art(&mut self, c: &Canvas, m: &Media, px: u32) -> Option<&Pixmap> {
        let key = (format!("{}|{}|{}", m.id, m.title, m.artist), px);
        if self.art.as_ref().map(|a| &a.0) != Some(&key) {
            let img = m.art.as_ref().and_then(|b| c.g.load_image_bytes(b, px, px));
            self.art = Some((key, img));
        }
        self.art.as_ref()?.1.as_ref()
    }

    /// The lyrics box: synced lines slide so the current one is centred; plain lyrics scroll.
    fn lyrics(&mut self, c: &mut Canvas, cx: &Ctx, m: &Media, r: R, pos: f64) {
        let t = &cx.theme;
        let note = |c: &mut Canvas, s: &str| c.text(s, r, txt(13.0, 400), t.c("onSurfaceVariant"), Align::Center);
        if !cx.cfg.bool("lyrics") { return; }
        match lyrics::get(&Track { title: &m.title, artist: &m.artist, album: &m.album, dur: m.duration }) {
            Lyr::Searching => note(c, "Searching lyrics\u{2026}"),
            l @ Lyr::Missing if l.hidden_for(m) => {}
            Lyr::Missing => note(c, "No lyrics found"),
            Lyr::Found(l) => {
                let f = txt(14.0, 400);
                let w = r.w - 16.0;
                let key = (format!("{}|{}|{}", m.id, m.title, m.artist), r.w as u32);
                let texts: Vec<&str> = match &*l {
                    Lyrics::Synced(v) => v.iter().map(|x| if x.1.is_empty() { "\u{266A}" } else { x.1.as_str() }).collect(),
                    Lyrics::Plain(v) => v.iter().map(|s| s.as_str()).collect(),
                };
                if self.heights.as_ref().map(|h| &h.0) != Some(&key) {
                    self.heights = Some((key, texts.iter().map(|s| c.g.measure_wrapped(s, f, w).max(14.0 * 1.25) + 6.0).collect()));
                }
                let hs = &self.heights.as_ref().unwrap().1;
                let (muted, fg) = (t.c("onSurfaceVariant"), t.c("onSurface"));
                match &*l {
                    Lyrics::Plain(_) => {
                        let total: f32 = hs.iter().sum();
                        self.plain = (total, r.h);
                        c.hit(r, 105);
                        self.scroll.region(c, cx, r, total, |c, top| {
                            let mut y = top;
                            for (s, h) in texts.iter().zip(hs) {
                                if y + h > r.y && y < r.y + r.h { c.para(s, R::new(r.x + 8.0, y + 3.0, w, h - 6.0), f, alpha(muted, 0.55), Align::Center); }
                                y += h;
                            }
                        });
                    }
                    Lyrics::Synced(v) => {
                        let idx = lyrics::current_index(v, pos);
                        let cur = idx.unwrap_or(0).min(hs.len().saturating_sub(1));
                        let offs: Vec<f32> = hs.iter().scan(0.0, |a, h| { let o = *a; *a += h; Some(o) }).collect();
                        let mid = offs.get(cur).map_or(0.0, |o| o + hs[cur] / 2.0);
                        c.clip_round(r, 0.0, |c| {
                            for (i, s) in texts.iter().enumerate() {
                                let y = r.y + r.h / 2.0 - mid + offs[i];
                                if y + hs[i] < r.y || y > r.y + r.h { continue; }
                                let d = ((y + hs[i] / 2.0 - r.y) / r.h).clamp(0.0, 1.0);
                                let fade = (d / 0.25).min(1.0).min((1.0 - d) / 0.25).clamp(0.0, 1.0); // mask-image: transparent edges
                                let is_cur = idx == Some(i);
                                let (col, font) = if is_cur { (alpha(fg, fade), txt(14.0 * 1.15, 500)) } else { (alpha(muted, 0.55 * fade), f) };
                                c.para(s, R::new(r.x + 8.0, y + 3.0, w, hs[i] - 6.0), font, col, Align::Center);
                            }
                        });
                    }
                }
            }
        }
    }
}

impl Tab for Home {
    fn frame_ms(&self) -> u32 { if self.playing { 250 } else { 0 } }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        let t = &cx.theme;
        self.playing = cx.media.as_ref().is_some_and(|m| m.playing);
        let player = R::new(r.x, r.y, r.w, r.h - RES_H - 10.0);
        Self::resources(c, cx, R::new(r.x, r.y + r.h - RES_H, r.w, RES_H));
        ui::card(c, cx, player, 23.0);
        let Some(m) = self.current(cx).cloned() else {
            c.icon("music_off", R::new(player.x, player.y + player.h / 2.0 - 56.0, player.w, 56.0), ico(56.0, false), t.c("onSurfaceVariant"));
            c.text("Nothing playing", R::new(player.x, player.y + player.h / 2.0 + 8.0, player.w, 20.0), txt(14.0, 400), t.c("onSurfaceVariant"), Align::Center);
            return;
        };
        let (x, w) = (player.x + 14.0, player.w - 28.0);
        let mut y = player.y + 14.0;

        // session chips are laid out first: they take the room at the bottom
        let multi = cx.sessions.len() > 1;
        let labels: Vec<(String, f32)> = cx.sessions.iter().map(|s| { let l = app_label(s); let lw = c.g.measure(&l, txt(13.0, 400)).0.ceil() + 28.0; (l, lw) }).collect();
        let mut rows: Vec<Vec<usize>> = vec![];
        if multi {
            let mut used = 0.0;
            for (i, (_, lw)) in labels.iter().enumerate() {
                if rows.is_empty() || used + lw > w { rows.push(vec![]); used = 0.0; }
                rows.last_mut().unwrap().push(i);
                used += lw + 6.0;
            }
        }
        let chips_h = if multi { rows.len() as f32 * 32.0 + (rows.len() as f32 - 1.0) * 6.0 + 12.0 } else { 0.0 };

        // art
        let s = w.min(c.h * 0.42);
        let art = R::new(x + (w - s) / 2.0, y, s, s);
        let px = (s * c.scale).round() as u32;
        let img = self.art(c, &m, px).cloned();
        c.clip_round(art, 23.0, |c| match &img {
            Some(p) => c.image(p, art, 1.0),
            None => {
                c.rect(art, 0.0, t.c("primaryContainer"));
                c.icon("music_note", art, ico(72.0, true), alpha(t.c("onPrimaryContainer"), 0.6)); // shortcut: flat fill instead of the 135deg gradient
            }
        });
        y += s + 12.0;

        // title / artist
        c.text(&m.title, R::new(x, y, w, 25.0), txt(19.0, 500), t.c("onSurface"), Align::Center);
        let artist = if m.artist.is_empty() { &m.album } else { &m.artist };
        c.text(artist, R::new(x, y + 25.0, w, 18.0), txt(14.0, 400), t.c("onSurfaceVariant"), Align::Center);
        y += 43.0 + 12.0;

        // lyrics fill what's left
        let tail = 16.0 + 12.0 + 56.0 + chips_h + 12.0;
        let lh = (player.y + player.h - 14.0 - tail - y).max(60.0);
        let pos = self.scrub.filter(|s| s.1 > hub::now_ms()).map_or(m.now(), |s| s.0 as f64 * m.duration);
        self.lyrics(c, cx, &m, R::new(x, y, w, lh), pos);
        y += lh + 12.0;

        // progress
        let lw = c.g.measure(&fmt_time(m.duration), txt(12.0, 400)).0.max(c.g.measure("0:00", txt(12.0, 400)).0).ceil() + 2.0;
        let muted = t.c("onSurfaceVariant");
        c.text(&fmt_time(pos), R::new(x, y, lw, 16.0), txt(12.0, 400), muted, Align::Left);
        if m.duration > 0.0 { c.text(&fmt_time(m.duration), R::new(x + w - lw, y, lw, 16.0), txt(12.0, 400), muted, Align::Right); }
        let bar = R::new(x + lw + 10.0, y + 4.0, w - 2.0 * lw - 20.0, 8.0);
        c.rect(bar, 3.0, t.c("secondaryContainer"));
        let p = if m.duration > 0.0 { (pos / m.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        c.rect(R::new(bar.x, bar.y, bar.w * p, 8.0), 3.0, t.c("primary"));
        c.hit(R::new(bar.x, bar.y - 8.0, bar.w, 24.0), SEEK);
        self.bar = Some(bar);
        y += 16.0 + 12.0;

        // controls: flex 1 / 1.6 / 1 with 8 px gaps
        let u = (w - 16.0) / 3.6;
        let btn = |c: &mut Canvas, x: f32, bw: f32, id: u32, icon: &str, on: bool, main: bool| {
            let b = R::new(x, y, bw, 56.0);
            let (bg, fg) = if main { (t.c("primary"), t.c("onPrimary")) } else { (t.c("surfaceContainerHigh"), t.c("onSurface")) };
            let over = on && c.hit(b, id);
            c.rect(b, if main { 30.0 } else { 23.0 }, if over && !main { t.c("surfaceContainerHighest") } else { bg });
            if over && main { c.rect(b, 30.0, alpha(t.c("onPrimary"), 0.08)); }
            c.icon(icon, b, ico(26.0, true), if on { fg } else { alpha(fg, 0.4) });
        };
        btn(c, x, u, PREV, "skip_previous", m.can_prev, false);
        btn(c, x + u + 8.0, u * 1.6, PLAY, if m.playing { "pause" } else { "play_arrow" }, true, true);
        btn(c, x + u * 2.6 + 16.0, u, NEXT, "skip_next", m.can_next, false);
        y += 56.0 + 12.0;

        // session chips, centred per row
        for row in &rows {
            let total: f32 = row.iter().map(|&i| labels[i].1).sum::<f32>() + 6.0 * (row.len() as f32 - 1.0);
            let mut cxp = x + (w - total) / 2.0;
            for &i in row {
                ui::chip(c, cx, cxp, y, CHIP0 + i as u32, None, &labels[i].0, cx.sessions[i].id == m.id);
                cxp += labels[i].1 + 6.0;
            }
            y += 38.0;
        }
        self.sessions = cx.sessions.iter().enumerate().map(|(i, s)| (s.id.clone(), CHIP0 + i as u32)).collect();
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        let Some(m) = self.current(cx).cloned() else { return };
        match ev {
            Input::Click(PREV, _) if m.can_prev => media::control_of(&m.id, "previous"),
            Input::Click(PLAY, _) => media::control_of(&m.id, "togglePlayPause"),
            Input::Click(NEXT, _) if m.can_next => media::control_of(&m.id, "next"),
            Input::Click(id, _) if self.sessions.iter().any(|s| s.1 == *id) => self.chosen = self.sessions.iter().find(|s| s.1 == *id).map(|s| s.0.clone()),
            Input::Drag(SEEK, x, _, ph) if m.duration > 0.0 => {
                let Some(bar) = self.bar else { return };
                let v = ui::slider_value(bar, *x);
                self.scrub = Some((v, hub::now_ms() + if *ph == Phase::End { 1500.0 } else { 60_000.0 }));
                if *ph == Phase::End { media::seek(&m.id, v as f64 * m.duration); }
            }
            Input::Wheel(_, d) => self.scroll.wheel(*d, self.plain.0, self.plain.1),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_time(73.9), "1:13");
        assert_eq!(fmt_time(-3.0), "0:00");
        assert_eq!(speed(0.0), "0.0 B/s");
        assert_eq!(speed(2048.0), "2.0 KiB/s");
        assert_eq!(speed(15.0 * 1024.0 * 1024.0), "15 MiB/s");
    }
}
