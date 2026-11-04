//! Anime tab (tab-anime.js): booru / Wallhaven search, a two-column grid of previews, open post, download,
//! set as wallpaper (bridge `download-wallpaper`, which re-themes the PC with --apply: only on a real click).

use super::{call, config_set, dots, err_box, img::Imgs, Inbox, Tab};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    hub,
    ui::{self, Edit, Scroll, TextBox},
    win::{Input, Phase},
};
use serde_json::Value;

const SOURCES: [(&str, &str); 4] = [("yandere", "yande.re"), ("konachan", "Konachan"), ("danbooru", "Danbooru"), ("wallhaven", "Wallhaven")];
const SRC0: u32 = 401;
const TEXT: u32 = 410;
const MORE: u32 = 411;
const ITEM0: u32 = 1000;
const GAP: f32 = 8.0;

#[derive(Clone, Copy, PartialEq)]
enum Act { Idle, Busy, Done(bool, f64) }

struct Item { preview: String, file: String, page: String, w: f32, h: f32, dl: Act, wall: Act }

enum Msg { Page(u32, Value), Done(u32, usize, bool, bool) }

pub struct Anime {
    source: String,
    nsfw: bool,
    tags: TextBox,
    items: Vec<Item>,
    page: u32,
    loading: bool,
    error: String,
    gen: u32,
    scroll: Scroll,
    dims: (f32, f32),
    imgs: Imgs,
    inbox: Inbox<Msg>,
    started: bool,
}

/// Columns for masonry: each item goes to the shorter column (CSS columns would fill the first one top-down).
/// Returns (column, y, height) per item and the total height.
fn masonry(aspects: &[f32], colw: f32) -> (Vec<(usize, f32, f32)>, f32) {
    let mut cols = [0.0f32; 2];
    let out = aspects.iter().map(|a| {
        let k = if cols[1] < cols[0] { 1 } else { 0 };
        let h = (colw * a).round().max(24.0);
        let y = cols[k];
        cols[k] += h + GAP;
        (k, y, h)
    }).collect();
    (out, cols[0].max(cols[1]))
}

impl Anime {
    pub fn new() -> Anime {
        Anime { source: "yandere".into(), nsfw: false, tags: TextBox::new(false), items: vec![], page: 1, loading: false, error: String::new(), gen: 0,
            scroll: Scroll::default(), dims: (0.0, 0.0), imgs: Imgs::default(), inbox: Inbox::new(), started: false }
    }

    fn load(&mut self, reset: bool, cx: &Ctx) {
        if self.loading { return; }
        if reset { self.gen += 1; self.page = 1; self.items.clear(); self.scroll.y = 0.0; self.imgs.clear(); }
        self.loading = true;
        self.error.clear();
        let mut args = vec![self.source.clone(), self.page.to_string()];
        args.extend(self.tags.text.split_whitespace().map(String::from));
        let (gen, inbox) = (self.gen, self.inbox.clone());
        call(cx, "booru", args, move |v| inbox.push(Msg::Page(gen, v)));
    }

    fn drain(&mut self) {
        for m in self.inbox.take() {
            match m {
                Msg::Page(gen, v) if gen == self.gen => {
                    self.loading = false;
                    if v.is_null() { self.error = "No answer from the bridge.".into(); }
                    else if let Some(e) = v["error"].as_str() { self.error = e.into(); }
                    else {
                        self.items.extend(v["items"].as_array().into_iter().flatten().map(|it| Item {
                            preview: it["preview"].as_str().unwrap_or("").into(), file: it["file"].as_str().unwrap_or("").into(), page: it["page"].as_str().unwrap_or("").into(),
                            w: it["w"].as_f64().unwrap_or(0.0) as f32, h: it["h"].as_f64().unwrap_or(0.0) as f32, dl: Act::Idle, wall: Act::Idle,
                        }));
                        self.page += 1;
                    }
                }
                Msg::Done(gen, i, apply, ok) if gen == self.gen => {
                    if let Some(it) = self.items.get_mut(i) { *(if apply { &mut it.wall } else { &mut it.dl }) = Act::Done(ok, hub::now_ms() + 2500.0); }
                }
                _ => {}
            }
        }
    }

    fn act(&mut self, i: usize, apply: bool, cx: &Ctx) {
        let Some(it) = self.items.get_mut(i) else { return };
        *(if apply { &mut it.wall } else { &mut it.dl }) = Act::Busy;
        let mut args = vec![it.file.clone()];
        if apply { args.push("--apply".into()); }
        let (gen, inbox) = (self.gen, self.inbox.clone());
        call(cx, "download-wallpaper", args, move |v| inbox.push(Msg::Done(gen, i, apply, v["ok"].as_bool().unwrap_or(false))));
    }
}

impl Tab for Anime {
    fn show(&mut self, cx: &Ctx) {
        if self.started { return; }
        self.started = true;
        self.source = match cx.cfg.str("booru.source") { "" => "yandere", "waifuim" => "wallhaven", s => s }.into();
        self.nsfw = cx.cfg.bool("booru.allowNsfw");
        if let Some(t) = std::env::var("END4_RENDER_TAGS").ok() { self.tags.set(&t); }
        self.load(true, cx);
    }

    fn frame_ms(&self) -> u32 { if self.loading || self.imgs.busy() { 100 } else { 500 } }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        self.drain();
        self.imgs.begin();
        let t = &cx.theme;
        // source chips
        let mut x = r.x;
        for (i, (id, label)) in SOURCES.iter().enumerate() {
            let cr = ui::chip(c, cx, x, r.y, SRC0 + i as u32, None, label, *id == self.source);
            x += cr.w + 6.0;
        }
        // search bar
        let sr = R::new(r.x, r.y + 42.0, r.w, 44.0);
        c.rect(sr, 22.0, t.c("surfaceContainer"));
        c.hit(sr, TEXT);
        c.icon("search", R::new(sr.x + 14.0, sr.y, 24.0, 44.0), ico(22.0, false), t.c("onSurfaceVariant"));
        let tag_w = if self.nsfw { 40.0 } else { 0.0 };
        self.tags.draw(c, cx, R::new(sr.x + 46.0, sr.y, sr.w - 46.0 - 12.0 - tag_w, 44.0), txt(14.0, 400), "Tags, e.g. landscape scenery", true);
        if self.nsfw {
            let b = R::new(sr.x + sr.w - 8.0 - 32.0, sr.y + 10.0, 32.0, 24.0);
            c.rect(b, 12.0, t.c("error"));
            c.text("18+", b, txt(11.0, 400), t.c("onError"), Align::Center);
        }

        // grid
        let view = R::new(r.x, r.y + 96.0, r.w, (r.h - 96.0).max(40.0));
        let colw = ((view.w - 8.0 - GAP) / 2.0).floor();
        let aspects: Vec<f32> = self.items.iter().map(|it| if it.w > 0.0 && it.h > 0.0 { it.h / it.w } else { 1.0 }).collect();
        let (pos, grid_h) = masonry(&aspects, colw);
        // footer: error, spinner, load more or the empty note
        let footer = if !self.error.is_empty() { c.g.measure_wrapped(&self.error, txt(13.0, 400), view.w - 32.0) + 30.0 } else { 0.0 } + 56.0;
        let content = grid_h + footer;
        self.dims = (content, view.h);
        c.hit(view, 412);
        let hover = c.hover;
        let scale = c.scale;
        let now = hub::now_ms();
        let (error, loading, empty) = (self.error.clone(), self.loading, self.items.is_empty());
        let Anime { scroll, imgs, items, .. } = self;
        scroll.region(c, cx, view, content, |c, top| {
            for (i, ((col, y, h), it)) in pos.iter().zip(items.iter()).enumerate() {
                let rect = R::new(view.x + *col as f32 * (colw + GAP), top + y, colw, *h);
                if rect.y + rect.h < view.y || rect.y > view.y + view.h { continue; }
                c.clip_round(rect, 17.0, |c| {
                    c.rect(rect, 0.0, t.c("surfaceContainer"));
                    if let Some(p) = imgs.get(c.g, &it.preview, (colw * scale).round() as u32, (*h * scale).round() as u32) { c.image(p, rect, 1.0); }
                });
                let base = ITEM0 + i as u32 * 4;
                c.hit(rect, base);
                if (base..base + 4).contains(&hover) {
                    for (j, (icon, st)) in [("open_in_new", Act::Idle), ("download", it.dl), ("wallpaper", it.wall)].iter().enumerate() {
                        let b = R::new(rect.x + rect.w - 6.0 - 32.0 * (3 - j) as f32 - 4.0 * (2 - j) as f32, rect.y + rect.h - 38.0, 32.0, 32.0);
                        let over = c.hit(b, base + 1 + j as u32);
                        c.circle(b.x + 16.0, b.y + 16.0, 16.0, if over { t.c("primary") } else { alpha(ui::layer0(cx), 0.85) });
                        let shown = match st { Act::Busy => "sync", Act::Done(true, until) if *until > now => "check", Act::Done(false, until) if *until > now => "error", _ => icon };
                        c.icon(shown, b, ico(18.0, false), if over { t.c("onPrimary") } else { t.c("onSurface") });
                    }
                }
            }
            // footer
            let fy = top + grid_h + 6.0;
            let mut y = fy;
            if !error.is_empty() { y += err_box(c, cx, R::new(view.x, y, view.w - 8.0, 0.0), &error) + 10.0; }
            if loading { dots(c, cx, view.x + view.w / 2.0, y + 20.0); }
            else if !empty {
                let w = c.g.measure("Load more", txt(13.0, 400)).0.ceil() + 28.0;
                ui::chip(c, cx, view.x + (view.w - w) / 2.0, y, MORE, None, "Load more", false);
            }
            else if error.is_empty() {
                c.icon("image_search", R::new(view.x, y, view.w, 48.0), ico(44.0, false), t.c("onSurfaceVariant"));
                c.text("Nothing found", R::new(view.x, y + 50.0, view.w, 22.0), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Center);
            }
        });
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        match ev {
            Input::Click(id, _) if (SRC0..SRC0 + 4).contains(id) => {
                self.source = SOURCES[(*id - SRC0) as usize].0.into();
                config_set(cx, "booru.source", Value::from(self.source.clone()));
                self.load(true, cx);
            }
            Input::Click(MORE, _) => self.load(false, cx),
            Input::Click(id, _) if *id > ITEM0 => {
                let (i, k) = (((*id - ITEM0) / 4) as usize, (*id - ITEM0) % 4);
                match k {
                    1 => if let Some(it) = self.items.get(i) { call(cx, "open", vec![it.page.clone()], |_| {}); },
                    2 | 3 => self.act(i, k == 3, cx),
                    _ => {}
                }
            }
            Input::Drag(TEXT, x, y, Phase::Start) => self.tags.press(*x, *y),
            Input::Key { vk, ctrl, shift, alt } => if self.tags.key(*vk, *ctrl, *shift, *alt) == Edit::Submit { self.load(true, cx); },
            Input::Char(ch) => { self.tags.ch(*ch); }
            Input::Wheel(_, d) => {
                self.scroll.wheel(*d, self.dims.0, self.dims.1);
                if !self.loading && !self.items.is_empty() && self.error.is_empty() && self.scroll.y + self.dims.1 > self.dims.0 - 300.0 { self.load(false, cx); }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masonry_fills_the_shorter_column() {
        let (pos, total) = masonry(&[2.0, 1.0, 1.0, 1.0], 100.0);
        assert_eq!(pos[0], (0, 0.0, 200.0));
        assert_eq!(pos[1], (1, 0.0, 100.0));
        assert_eq!(pos[2], (1, 108.0, 100.0)); // column 2 (108) is shorter than column 1 (208)
        assert_eq!(pos[3], (0, 208.0, 100.0));
        assert_eq!(total, 316.0);
    }
}
