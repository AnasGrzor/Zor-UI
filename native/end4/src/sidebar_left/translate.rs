//! Translate tab (tab-translate.js): source / target pickers, input box, bridge `translate`, copy result.

use super::{call, config_set, dots, err_box, field, Inbox, Tab};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    hub, ui::{self, Edit, IconKind, Scroll, TextBox},
    util,
    win::{Input, Phase},
};
use serde_json::{json, Value};

pub const LANGS: [(&str, &str); 51] = [
    ("auto", "Detect language"), ("af", "Afrikaans"), ("ar", "Arabic"), ("bn", "Bengali"), ("bg", "Bulgarian"), ("ca", "Catalan"), ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"), ("hr", "Croatian"), ("cs", "Czech"), ("da", "Danish"), ("nl", "Dutch"), ("en", "English"), ("et", "Estonian"), ("fil", "Filipino"),
    ("fi", "Finnish"), ("fr", "French"), ("de", "German"), ("el", "Greek"), ("gu", "Gujarati"), ("he", "Hebrew"), ("hi", "Hindi"), ("hu", "Hungarian"), ("id", "Indonesian"),
    ("it", "Italian"), ("ja", "Japanese"), ("ko", "Korean"), ("lv", "Latvian"), ("lt", "Lithuanian"), ("ms", "Malay"), ("mr", "Marathi"), ("no", "Norwegian"), ("fa", "Persian"),
    ("pl", "Polish"), ("pt", "Portuguese"), ("pa", "Punjabi"), ("ro", "Romanian"), ("ru", "Russian"), ("sr", "Serbian"), ("sk", "Slovak"), ("es", "Spanish"), ("sw", "Swahili"),
    ("sv", "Swedish"), ("ta", "Tamil"), ("te", "Telugu"), ("th", "Thai"), ("tr", "Turkish"), ("uk", "Ukrainian"), ("ur", "Urdu"), ("vi", "Vietnamese"),
];

const SRC: u32 = 301;
const TGT: u32 = 302;
const SWAP: u32 = 303;
const TEXT: u32 = 304;
const COPY: u32 = 305;
const PICK0: u32 = 320;
const ROW: f32 = 36.0;

fn name(code: &str) -> &str { LANGS.iter().find(|l| l.0 == code).map_or(code, |l| l.1) }

enum Out { Empty, Text(String), Err(String) }

pub struct Translate {
    src: String,
    tgt: String,
    input: TextBox,
    out: Out,
    detected: Option<String>,
    loading: bool,
    seq: u32,
    due: Option<f64>,
    inbox: Inbox<(u32, Value)>,
    scroll: Scroll,
    out_dims: (f32, f32),
    /// Which picker is open: 0 source, 1 target.
    menu: Option<usize>,
    menu_scroll: Scroll,
    menu_dims: (f32, f32),
    ready: bool,
    copied_at: f64,
}

impl Translate {
    pub fn new() -> Translate {
        Translate { src: "auto".into(), tgt: "en".into(), input: TextBox::new(true), out: Out::Empty, detected: None, loading: false, seq: 0, due: None, inbox: Inbox::new(),
            scroll: Scroll::default(), out_dims: (0.0, 0.0), menu: None, menu_scroll: Scroll::default(), menu_dims: (0.0, 0.0), ready: false, copied_at: 0.0 }
    }

    fn run(&mut self, cx: &Ctx) {
        self.due = None;
        let t = self.input.text.trim().to_string();
        if t.is_empty() { self.seq += 1; self.out = Out::Empty; self.detected = None; self.loading = false; return; }
        self.seq += 1;
        self.loading = true;
        let (my, inbox) = (self.seq, self.inbox.clone());
        call(cx, "translate", vec![self.tgt.clone(), self.src.clone(), t], move |v| inbox.push((my, v)));
    }

    fn drain(&mut self) {
        for (seq, v) in self.inbox.take() {
            if seq != self.seq { continue; }
            self.loading = false;
            self.scroll.y = 0.0;
            if let Some(e) = v["error"].as_str() { self.out = Out::Err(e.into()); continue; }
            if v.is_null() { self.out = Out::Err("Translation failed: the bridge did not answer.".into()); continue; }
            self.out = Out::Text(v["text"].as_str().unwrap_or("").into());
            self.detected = (self.src == "auto").then(|| v["detected"].as_str().map(String::from)).flatten();
        }
    }

    fn persist(&self, cx: &Ctx) { config_set(cx, "translator", json!({ "source": self.src, "target": self.tgt })); }

    fn swap(&mut self, cx: &Ctx) {
        if self.src == "auto" { self.src = self.detected.clone().filter(|d| LANGS.iter().any(|l| l.0 == d)).unwrap_or_else(|| "en".into()); }
        std::mem::swap(&mut self.src, &mut self.tgt);
        if let Out::Text(t) = &self.out { if !t.is_empty() { self.input.set(&t.clone()); } }
        self.persist(cx);
        self.run(cx);
    }

    fn picker(&self, c: &mut Canvas, cx: &Ctx, r: R, id: u32, code: &str, open: bool) {
        let t = &cx.theme;
        field(c, cx, r, open);
        if c.hit(r, id) { c.rect(r, 17.0, alpha(t.c("onSurface"), 0.06)); }
        c.text(name(code), R::new(r.x + 14.0, r.y, r.w - 44.0, r.h), txt(14.0, 400), t.c("onSurface"), Align::Left);
        c.icon("expand_more", R::new(r.x + r.w - 34.0, r.y, 24.0, r.h), ico(20.0, false), t.c("onSurfaceVariant"));
    }
}

impl Tab for Translate {
    fn show(&mut self, cx: &Ctx) {
        if self.ready { return; }
        self.ready = true;
        let s = |k: &str, d: &str| Some(cx.cfg.str(k)).filter(|v| !v.is_empty()).unwrap_or(d).to_string();
        (self.src, self.tgt) = (s("translator.source", "auto"), s("translator.target", "en"));
        if std::env::var_os("END4_RENDER_DEMO").is_some() { self.input.set("Hello world, how are you today?"); self.tgt = "fr".into(); self.run(cx); }
    }

    fn frame_ms(&self) -> u32 { if self.loading || self.due.is_some() { 100 } else if self.menu.is_none() { 500 } else { 0 } }
    fn captures_esc(&self) -> bool { self.menu.is_some() }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx, r: R) {
        self.drain();
        if self.due.is_some_and(|d| hub::now_ms() >= d) { self.run(cx); }
        let t = &cx.theme;
        let pw = (r.w - 36.0 - 12.0) / 2.0;
        let (srcr, tgtr) = (R::new(r.x, r.y, pw, 42.0), R::new(r.x + pw + 48.0, r.y, pw, 42.0));
        let half = ((r.h - 42.0 - 20.0) / 2.0).max(120.0);

        // input
        let inr = R::new(r.x, r.y + 52.0, r.w, half);
        field(c, cx, inr, self.menu.is_none());
        c.hit(inr, TEXT);
        self.input.draw(c, cx, R::new(inr.x + 14.0, inr.y + 10.0, inr.w - 28.0, inr.h - 20.0), txt(15.0, 400), "Enter text to translate", self.menu.is_none());

        // output
        let outr = R::new(r.x, inr.y + half + 10.0, r.w, half);
        ui::card(c, cx, outr, 17.0);
        c.hit(outr, 306);
        let body = R::new(outr.x + 14.0, outr.y + 12.0, outr.w - 28.0, outr.h - 12.0 - 44.0);
        let f = txt(15.0, 400);
        match &self.out {
            Out::Empty => c.text("Translation", R::new(body.x, body.y, body.w, 24.0), f, t.c("onSurfaceVariant"), Align::Left),
            Out::Err(e) => { err_box(c, cx, R::new(body.x, body.y, body.w, 0.0), e); }
            Out::Text(s) => {
                let h = c.g.measure_wrapped(s, f, body.w - 6.0);
                self.out_dims = (h, body.h);
                let col = alpha(t.c("onSurface"), if self.loading { 0.5 } else { 1.0 });
                self.scroll.region(c, cx, body, h, |c, top| c.para(s, R::new(body.x, top, body.w - 6.0, h), f, col, Align::Left));
            }
        }
        if self.loading && matches!(self.out, Out::Empty) { dots(c, cx, outr.x + outr.w / 2.0, outr.y + outr.h / 2.0); }
        if self.loading && !matches!(self.out, Out::Empty) { dots(c, cx, outr.x + outr.w - 40.0, outr.y + 24.0); }
        if let (Some(d), true) = (&self.detected, self.src == "auto") {
            c.text(&format!("Detected: {}", name(d)), R::new(outr.x + 14.0, outr.y + outr.h - 40.0, outr.w - 70.0, 36.0), txt(12.0, 400), t.c("onSurfaceVariant"), Align::Left);
        }
        let copied = hub::now_ms() - self.copied_at < 1200.0;
        ui::icon_btn(c, cx, R::new(outr.x + outr.w - 42.0, outr.y + outr.h - 42.0, 36.0, 36.0), COPY, if copied { "check" } else { "content_copy" }, IconKind::Plain, 20.0, false);

        // pickers
        self.picker(c, cx, srcr, SRC, &self.src, self.menu == Some(0));
        ui::icon_btn(c, cx, R::new(r.x + pw + 6.0, r.y + 3.0, 36.0, 36.0), SWAP, "swap_horiz", IconKind::Tonal, 24.0, false);
        self.picker(c, cx, tgtr, TGT, &self.tgt, self.menu == Some(1));

        // open picker list, on top
        if let Some(which) = self.menu {
            let anchor = if which == 0 { srcr } else { tgtr };
            let rows: Vec<&(&str, &str)> = LANGS.iter().filter(|l| which == 0 || l.0 != "auto").collect();
            let content = rows.len() as f32 * ROW;
            let mr = R::new(anchor.x, anchor.y + 46.0, anchor.w, (content + 12.0).min(r.h - 56.0).min(9.0 * ROW + 12.0));
            c.shadow(4.0, 16.0, 0.35, |c, col| if let Some(col) = col { c.rect(mr, 17.0, col) });
            c.rect(mr, 17.0, t.c("surfaceContainerHigh"));
            let inner = R::new(mr.x + 6.0, mr.y + 6.0, mr.w - 12.0, mr.h - 12.0);
            c.hit(mr, 399);
            self.menu_dims = (content, inner.h);
            let cur = if which == 0 { self.src.clone() } else { self.tgt.clone() };
            self.menu_scroll.region(c, cx, inner, content, |c, top| {
                for (i, l) in rows.iter().enumerate() {
                    let rr = R::new(inner.x, top + i as f32 * ROW, inner.w - 6.0, ROW);
                    if rr.y + ROW < inner.y || rr.y > inner.y + inner.h { continue; }
                    let sel = l.0 == cur;
                    if sel { c.rect(rr, 12.0, t.c("secondaryContainer")); }
                    let id = PICK0 + LANGS.iter().position(|x| x.0 == l.0).unwrap_or(0) as u32;
                    if c.hit(R::new(rr.x, rr.y.max(inner.y), rr.w, rr.h.min(inner.y + inner.h - rr.y.max(inner.y)).max(0.0)), id) && !sel { c.rect(rr, 12.0, alpha(t.c("onSurface"), 0.08)); }
                    c.text(l.1, R::new(rr.x + 12.0, rr.y, rr.w - 24.0, ROW), txt(13.5, 400), if sel { t.c("onSecondaryContainer") } else { t.c("onSurface") }, Align::Left);
                }
            });
        }
    }

    fn input(&mut self, ev: &Input, cx: &mut Ctx, _hover: u32) {
        let mut changed = false;
        match ev {
            Input::Click(id @ (SRC | TGT), _) => { let w = (*id == TGT) as usize; self.menu = if self.menu == Some(w) { None } else { Some(w) }; self.menu_scroll.y = 0.0; }
            Input::Click(SWAP, _) => { self.menu = None; self.swap(cx); }
            Input::Click(COPY, _) => if let Out::Text(t) = &self.out { util::clipboard_set(t); self.copied_at = hub::now_ms(); },
            Input::Click(id, _) if (PICK0..PICK0 + LANGS.len() as u32).contains(id) => {
                let code = LANGS[(*id - PICK0) as usize].0.to_string();
                if self.menu == Some(0) { self.src = code } else { self.tgt = code }
                self.menu = None;
                self.persist(cx);
                self.run(cx);
            }
            Input::Click(399, _) => {}
            Input::Click(..) if self.menu.is_some() => self.menu = None,
            Input::Drag(TEXT, x, y, Phase::Start) => { self.menu = None; self.input.press(*x, *y); }
            Input::Key { vk: 0x1B, .. } => self.menu = None,
            Input::Key { vk, ctrl, shift, alt } if self.menu.is_none() => changed = self.input.key(*vk, *ctrl, *shift, *alt) == Edit::Changed,
            Input::Char(ch) if self.menu.is_none() => changed = self.input.ch(*ch) == Edit::Changed,
            Input::Wheel(_, d) => if self.menu.is_some() { self.menu_scroll.wheel(*d, self.menu_dims.0, self.menu_dims.1) } else { self.scroll.wheel(*d, self.out_dims.0, self.out_dims.1) },
            _ => {}
        }
        if changed { self.due = Some(hub::now_ms() + 650.0); }
    }
}
