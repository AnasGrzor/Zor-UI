//! The settings form kit: `Pen` lays rows out down a column (and draws them when they are on screen);
//! the `impl Settings` helpers below are the controls of settings.js (sw, seg, num, text, slider, pick).
//! Every control registers an `Act`; its hit id is 1000 + the act's index, which `Settings::input` maps back.

use super::{Field, Kind, Settings};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, Color, R},
    ui::{self, LH},
};
use serde_json::{json, Value};
use std::rc::Rc;

pub const ACT0: u32 = 1000;

#[derive(Clone)]
pub enum Act {
    Page(usize),
    Switch(String),
    Seg(String, Value),
    /// path, delta, min, max
    Step(String, i64, i64, i64),
    Slider(String, Sl, R),
    Select(String, R, Rc<Vec<(Value, String)>>),
    Field(String, R),
    /// A named button: ("bu", "snip"), ("apply-local", id)...; settings.rs matches on the name.
    Do(&'static str, String),
    /// A search hit: page index, section title.
    Goto(usize, String),
}

/// A range slider: min, max, step and how its value reads.
#[derive(Clone, Copy)]
pub struct Sl { pub min: f64, pub max: f64, pub step: f64, pub fmt: Fmt }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fmt { Px, Pct, PctInt, Kelvin, Weight, Fixed1 }

/// `${v}` of a JS number: whole numbers without a decimal point.
pub fn num_str(v: f64) -> String { if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") } }

/// Math.round: halves go up.
fn round(v: f64) -> i64 { (v + 0.5).floor() as i64 }

pub fn fmt_value(f: Fmt, v: f64) -> String {
    match f {
        Fmt::Px => format!("{}px", num_str(v)),
        Fmt::Pct => format!("{}%", round(v * 100.0)),
        Fmt::PctInt => format!("{}%", num_str(v)),
        Fmt::Kelvin => format!("{}K", num_str(v)),
        Fmt::Weight => if v == 0.0 { "Normal".into() } else if v > 0.0 { format!("+{}", num_str(v)) } else { num_str(v) },
        Fmt::Fixed1 => format!("{v:.1}"),
    }
}

/// The slider value for a 0-1 position: snapped to `step` from `min`, without float dust (0.30000000000000004).
pub fn snap(s: &Sl, frac: f64) -> f64 {
    let n = ((s.max - s.min) * frac.clamp(0.0, 1.0) / s.step).round();
    let v = (s.min + n * s.step).clamp(s.min, s.max);
    (v * 1e6).round() / 1e6
}

/// JSON for a slider value: whole numbers stay integers (config.json had them as 170, not 170.0).
pub fn num_json(v: f64) -> Value { if v.fract() == 0.0 { json!(v as i64) } else { json!(v) } }

pub struct Pen<'p, 'a> {
    pub c: &'p mut Canvas<'a>,
    pub cx: &'p Ctx,
    pub x: f32,
    pub w: f32,
    pub y: f32,
    /// Window y range that is on screen: rows outside are laid out but not drawn (hit spots ignore clips).
    pub lo: f32,
    pub hi: f32,
    /// Measuring pass: lay out only, to learn the card heights and the content height.
    pub dry: bool,
    pub cards: Vec<f32>,
    ci: usize,
    pub measured: Vec<f32>,
    pub acts: Vec<Act>,
    first: bool,
    pub t: f32,
    /// Each `h2` title and its y (measuring pass), for "open Settings at this section".
    pub heads: Vec<(String, f32)>,
    /// Search index (measuring pass): (section, label, sub text) of every control and title.
    pub index: Vec<(String, String, String)>,
    sect: String,
}

impl<'p, 'a> Pen<'p, 'a> {
    pub fn new(c: &'p mut Canvas<'a>, cx: &'p Ctx, x: f32, y: f32, w: f32, lo: f32, hi: f32) -> Pen<'p, 'a> {
        Pen { c, cx, x, w, y, lo, hi, dry: false, cards: vec![], ci: 0, measured: vec![], acts: vec![], first: true, heads: vec![], index: vec![], sect: String::new(), t: (crate::hub::now_ms() % 100_000.0) as f32 / 1000.0 }
    }
    /// Records a control for search (before `row`, which hides rows in the measuring pass).
    pub fn tag(&mut self, label: &str, sub: &str) { if self.dry { self.index.push((self.sect.clone(), label.into(), sub.into())); } }
    pub fn col(&self, name: &str) -> Color { self.cx.theme.c(name) }
    pub fn show(&self, h: f32) -> bool { !self.dry && self.y + h > self.lo && self.y < self.hi }
    pub fn act(&mut self, a: Act) -> u32 { self.acts.push(a); ACT0 + self.acts.len() as u32 - 1 }
    /// Space between a card's children.
    fn lead(&mut self) { if self.first { self.first = false } else { self.y += 12.0 } }

    /// A `.card` around what `body` lays out: padding 14, 12 between children, 10 below.
    pub fn card(&mut self, body: impl FnOnce(&mut Pen)) {
        let (x, w, y0) = (self.x, self.w, self.y);
        let h = self.cards.get(self.ci).copied().unwrap_or(0.0);
        self.ci += 1;
        if h > 0.0 && self.show(h) { ui::card(self.c, self.cx, R::new(x, y0, w, h), 23.0); }
        (self.x, self.w, self.y, self.first) = (x + 14.0, w - 28.0, y0 + 14.0, true);
        body(self);
        self.y += 14.0;
        self.measured.push(self.y - y0);
        (self.x, self.w, self.first) = (x, w, false);
        self.y += 10.0;
    }

    /// `h2`: a 32 px round icon and a 17 px title.
    pub fn h2(&mut self, icon: &str, title: &str, extra: &str) {
        self.y += 6.0;
        if self.dry { self.heads.push((title.to_string(), self.y)); self.index.push((title.into(), title.into(), String::new())); self.sect = title.into(); }
        if self.show(32.0) {
            let (x, y) = (self.x, self.y);
            self.c.circle(x + 16.0, y + 16.0, 16.0, self.col("surfaceContainer"));
            self.c.icon(icon, R::new(x, y, 32.0, 32.0), ico(18.0, false), self.col("onSurface"));
            let tw = self.c.g.measure(title, txt(17.0, 500)).0.ceil() + 2.0;
            self.c.text(title, R::new(x + 42.0, y, tw, 32.0), txt(17.0, 500), self.col("onSurface"), Align::Left);
            if !extra.is_empty() { self.c.text(extra, R::new(x + 42.0 + tw + 10.0, y + 2.0, self.w - tw - 52.0, 32.0), txt(12.0, 400), self.col("onSurfaceVariant"), Align::Left); }
        }
        self.y += 42.0;
    }

    /// A row slot of height `h`; its rect when it is on screen.
    pub fn row(&mut self, h: f32) -> Option<R> {
        self.lead();
        let r = self.show(h).then(|| R::new(self.x, self.y, self.w, h));
        self.y += h;
        r
    }

    /// Wrapped muted text (12 px by default).
    pub fn note(&mut self, s: &str, size: f32) {
        self.lead();
        let h = self.c.g.measure_wrapped(s, txt(size, 400), self.w).ceil();
        if self.show(h) { self.c.para(s, R::new(self.x, self.y, self.w, h), txt(size, 400), self.col("onSurfaceVariant"), Align::Left); }
        self.y += h;
    }

    /// `.spinner` (28 px, margin 20): `row` of it.
    pub fn spinner_block(&mut self) {
        self.lead();
        if self.show(68.0) { self.spinner(self.x + self.w / 2.0, self.y + 34.0, 28.0); }
        self.y += 68.0;
    }

    pub fn spinner(&mut self, cx_: f32, cy_: f32, size: f32) {
        let r = (size - 3.0) / 2.0;
        let a0 = self.t * std::f32::consts::TAU / 0.8;
        self.c.arc(cx_, cy_, r, 1.0, 3.0, self.col("secondaryContainer"));
        let pts: Vec<(f32, f32)> = (0..=8).map(|i| { let a = a0 + i as f32 * std::f32::consts::FRAC_PI_2 / 8.0; (cx_ + r * a.cos(), cy_ + r * a.sin()) }).collect();
        self.c.polyline(&pts, 3.0, self.col("primary"));
    }

    /// Icon, label and small text on the left of a row, leaving `right` px for its control.
    pub fn left(&mut self, r: R, ic: &str, label: &str, sub: &str, right: f32) {
        let mut x = r.x;
        if !ic.is_empty() { self.c.icon(ic, R::new(x, r.y, 20.0, r.h), ico(20.0, false), self.col("onSurfaceVariant")); x += 32.0; }
        let w = (r.x + r.w - right - 12.0 - x).max(20.0);
        if sub.is_empty() { self.c.text(label, R::new(x, r.y, w, r.h), txt(13.5, 400), self.col("onSurface"), Align::Left); return; }
        let (h1, h2) = (13.5 * LH, 11.5 * LH);
        let top = r.y + (r.h - h1 - h2) / 2.0;
        self.c.text(label, R::new(x, top, w, h1), txt(13.5, 400), self.col("onSurface"), Align::Left);
        self.c.text(sub, R::new(x, top + h1, w, h2), txt(11.5, 400), self.col("onSurfaceVariant"), Align::Left);
    }

    pub fn field_box(&mut self, r: R, focus: bool, radius: f32) {
        self.c.rect(r, radius, if focus { self.col("primary") } else { self.col("outlineVariant") });
        self.c.rect(R::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0), radius - 1.0, self.col("surfaceContainer"));
    }

    /// A chip at the right end of `r` (vertically centred); returns nothing, registers `act`.
    pub fn chip_right(&mut self, r: R, icon: Option<&str>, label: &str, act: Act) -> f32 {
        let w = chip_w(self.c, label, icon.is_some());
        let id = self.act(act);
        ui::chip(self.c, self.cx, r.x + r.w - w, r.y + (r.h - 32.0) / 2.0, id, icon, label, false);
        w
    }

    /// Chips that wrap, `sel` highlighted; one `Act` each.
    pub fn chips(&mut self, items: Vec<(String, Option<&'static str>, bool, Act)>) {
        self.lead();
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut placed = vec![];
        for (label, ic, sel, act) in items {
            let w = chip_w(self.c, &label, ic.is_some());
            if x > 0.0 && x + w > self.w { x = 0.0; y += 38.0; }
            placed.push((x, y, label, ic, sel, act));
            x += w + 6.0;
        }
        let h = if placed.is_empty() { 0.0 } else { y + 32.0 };
        if self.show(h) {
            let base = self.y;
            for (x, y, label, ic, sel, act) in placed {
                let at = base + y;
                if at + 32.0 < self.lo || at > self.hi { continue; }
                let id = self.act(act);
                ui::chip(self.c, self.cx, self.x + x, at, id, ic, &label, sel);
            }
        }
        self.y += h;
    }
}

/// Does option value `opt` stand for the stored value `cur`? An unset value matches "" and []; a stored list
/// matches a preset list that starts with it (3 saved labels still match the 10-label preset).
pub fn is_choice(opt: &Value, cur: &Value) -> bool {
    if opt == cur || ((*opt == json!("") || *opt == json!([])) && cur.is_null()) { return true; }
    match (opt.as_array(), cur.as_array()) { (Some(o), Some(c)) => !c.is_empty() && o.starts_with(c), _ => false }
}

pub fn chip_w(c: &Canvas, label: &str, icon: bool) -> f32 { 14.0 + if icon { 24.0 } else { 0.0 } + c.g.measure(label, txt(13.0, 400)).0.ceil() + 14.0 }

// ---------------------------------------------------------------- controls (settings.js: sw, seg, num, text, slider, pick)

impl Settings {
    /// `sw`: a label with a switch.
    pub fn sw(&mut self, p: &mut Pen, path: &str, label: &str, sub: &str, ic: &str) {
        p.tag(label, sub);
        let Some(r) = p.row(40.0) else { return };
        let on = self.gb(p.cx, path);
        p.left(r, ic, label, sub, 52.0);
        let id = p.act(Act::Switch(path.into()));
        ui::toggle(p.c, p.cx, r.x + r.w - 52.0, r.y + 4.0, id, on);
    }

    /// `seg`: a segmented choice. Options are (value, label).
    pub fn seg(&mut self, p: &mut Pen, path: &str, opts: &[(&str, &str)], label: &str, ic: &str) {
        p.tag(label, "");
        let Some(r) = p.row(40.0) else { return };
        let cur = self.gs(p.cx, path);
        let widths: Vec<f32> = opts.iter().map(|o| 28.0 + p.c.g.measure(o.1, txt(12.5, 400)).0.ceil()).collect();
        let total: f32 = widths.iter().sum::<f32>() + 2.0 * (opts.len() as f32 - 1.0);
        p.left(r, ic, label, "", total);
        let (mut x, y) = (r.x + r.w - total, r.y + 2.0);
        let group = R::new(x, y, total, 36.0);
        p.c.clip_round(group, 18.0, |c| {
            for ((v, l), w) in opts.iter().zip(&widths) {
                let b = R::new(x, y, *w, 36.0);
                let sel = *v == cur;
                c.rect(b, 6.0, if sel { p.cx.theme.c("primary") } else { p.cx.theme.c("surfaceContainer") });
                let id = ACT0 + p.acts.len() as u32;
                p.acts.push(Act::Seg(path.into(), json!(v)));
                if c.hit(b, id) && !sel { c.rect(b, 6.0, alpha(p.cx.theme.c("onSurface"), 0.08)); }
                c.text(l, R::new(b.x + 14.0, y, *w - 28.0, 36.0), txt(12.5, 400), p.cx.theme.c(if sel { "onPrimary" } else { "onSurface" }), Align::Center);
                x += w + 2.0;
            }
        });
    }

    /// `num`: - [value] + stepper, whole numbers between min and max.
    pub fn num(&mut self, p: &mut Pen, path: &str, label: &str, min: i64, max: i64, ic: &str) {
        p.tag(label, "");
        let Some(r) = p.row(40.0) else { return };
        p.left(r, ic, label, "", 36.0 * 2.0 + 56.0 + 8.0);
        let x = r.x + r.w - (36.0 + 4.0 + 56.0 + 4.0 + 36.0);
        let y = r.y + 2.0;
        let id = p.act(Act::Step(path.into(), -1, min, max));
        ui::icon_btn(p.c, p.cx, R::new(x, y, 36.0, 36.0), id, "remove", ui::IconKind::Plain, 20.0, false);
        let v = self.gn(p.cx, path);
        let fr = R::new(x + 40.0, y, 56.0, 36.0);
        self.field_ui(p, path, Some(&num_str(v)), Kind::Num(min, max), fr, "", 12.0, true);
        let id = p.act(Act::Step(path.into(), 1, min, max));
        ui::icon_btn(p.c, p.cx, R::new(x + 100.0, y, 36.0, 36.0), id, "add", ui::IconKind::Plain, 20.0, false);
    }

    /// `text`: a label with a text field (max 300 wide); a list in the config shows comma separated.
    pub fn text(&mut self, p: &mut Pen, path: &str, label: &str, sub: &str, ic: &str) { self.text_ph(p, path, label, sub, ic, "") }

    pub fn text_ph(&mut self, p: &mut Pen, path: &str, label: &str, sub: &str, ic: &str, placeholder: &str) {
        p.tag(label, sub);
        let Some(r) = p.row(40.0) else { return };
        let w = 300.0f32.min(r.w * 0.6);
        p.left(r, ic, label, sub, w);
        let v = self.get(p.cx, path);
        let (init, list) = match &v {
            Value::Array(a) => (a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "), true),
            Value::String(s) => (s.clone(), false),
            Value::Null => (String::new(), false),
            o => (o.to_string(), false),
        };
        self.field_ui(p, path, Some(&init), Kind::Text { list }, R::new(r.x + r.w - w, r.y, w, 40.0), placeholder, 17.0, false);
    }

    /// `slider`: a native-style range input with its value on the right.
    pub fn slider(&mut self, p: &mut Pen, path: &str, label: &str, sl: Sl, ic: &str) {
        p.tag(label, "");
        let Some(r) = p.row(40.0) else { return };
        p.left(r, ic, label, "", 220.0 + 12.0 + 56.0);
        let v = match &self.drag_val { Some((k, v)) if k == path => *v, _ => self.gn(p.cx, path) };
        let tr = R::new(r.x + r.w - 56.0 - 12.0 - 220.0, r.y, 220.0, 40.0);
        let frac = ((v - sl.min) / (sl.max - sl.min)).clamp(0.0, 1.0) as f32;
        let (x0, x1, my) = (tr.x + 8.0, tr.x + tr.w - 8.0, tr.y + 20.0);
        p.c.rect(R::new(tr.x, my - 2.0, tr.w, 4.0), 2.0, p.col("surfaceContainerHighest"));
        p.c.rect(R::new(tr.x, my - 2.0, (x1 - x0) * frac + 8.0, 4.0), 2.0, p.col("primary"));
        p.c.circle(x0 + (x1 - x0) * frac, my, 8.0, p.col("primary"));
        let id = p.act(Act::Slider(path.into(), sl, R::new(x0, tr.y, x1 - x0, 40.0)));
        p.c.hit(R::new(tr.x, tr.y, tr.w, 40.0), id);
        p.c.text(&fmt_value(sl.fmt, v), R::new(r.x + r.w - 56.0, r.y, 56.0, 40.0), txt(14.0, 400), p.col("onSurfaceVariant"), Align::Right);
    }

    /// `pick`: a label with a drop-down (the `<select>`); `opts` are (value, label).
    pub fn pick(&mut self, p: &mut Pen, path: &str, opts: Vec<(Value, String)>, label: &str, sub: &str, ic: &str, w: f32) {
        p.tag(label, sub);
        let Some(r) = p.row(40.0) else { return };
        p.left(r, ic, label, sub, w);
        self.select_box(p, path, opts, R::new(r.x + r.w - w, r.y, w, 40.0));
    }

    /// The closed `<select>`: its current choice and a chevron.
    pub fn select_box(&mut self, p: &mut Pen, path: &str, opts: Vec<(Value, String)>, fr: R) {
        let cur = self.get(p.cx, path);
        let shown = opts.iter().find(|o| is_choice(&o.0, &cur)).map_or_else(|| if cur.is_array() { "Custom".to_string() } else { cur.as_str().unwrap_or("").to_string() }, |o| o.1.clone());
        let open = self.menu.as_ref().is_some_and(|m| m.path == path);
        p.field_box(fr, open, 17.0);
        let id = p.act(Act::Select(path.into(), fr, Rc::new(opts)));
        let over = p.c.hit(fr, id);
        if over { p.c.rect(fr, 17.0, alpha(p.col("onSurface"), 0.05)); }
        p.c.text(&shown, R::new(fr.x + 14.0, fr.y, fr.w - 44.0, fr.h), txt(14.0, 400), p.col("onSurface"), Align::Left);
        p.c.icon("arrow_drop_down", R::new(fr.x + fr.w - 30.0, fr.y, 24.0, fr.h), ico(22.0, false), p.col("onSurfaceVariant"));
    }

    /// A text input in `r`. `init`: the text it shows while not being edited (None: it keeps its own, like a search box).
    pub fn field_ui(&mut self, p: &mut Pen, key: &str, init: Option<&str>, kind: Kind, r: R, placeholder: &str, radius: f32, centre: bool) {
        let foc = self.focus.as_deref() == Some(key) && self.active;
        let multi = r.h > 60.0;
        let f = self.fields.entry(key.to_string()).or_insert_with(|| Field::new(multi));
        f.kind = kind;
        if let Some(init) = init { if self.focus.as_deref() != Some(key) && f.tb.text != init { f.tb.set(init); f.orig = init.to_string(); } }
        p.field_box(r, self.focus.as_deref() == Some(key), radius);
        let id = p.act(Act::Field(key.into(), r));
        p.c.hit(r, id);
        let font = txt(14.0, 400);
        let area = if multi { R::new(r.x + 14.0, r.y + 10.0, r.w - 28.0, r.h - 20.0) } else { R::new(r.x + 14.0, r.y + 1.0, r.w - 28.0, r.h - 2.0) };
        if f.kind == Kind::Secret {
            // the real text stays in `tb` (paste, edit); the box shows dots, and the caret follows by character count
            let dots = "\u{2022}".repeat(f.tb.text.chars().count());
            f.mask.text = dots;
            f.mask.caret = f.tb.text[..f.tb.caret].chars().count() * 3;
            f.mask.all_selected = f.tb.all_selected;
            f.mask.draw(p.c, p.cx, area, font, placeholder, foc);
        } else if centre && !foc {
            p.c.text(&f.tb.text, R::new(r.x, r.y, r.w, r.h), font, p.col("onSurface"), Align::Center);
        } else {
            f.tb.draw(p.c, p.cx, area, font, placeholder, foc);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn choices_match_like_the_dropdown_shows_them() {
        use super::is_choice;
        assert!(is_choice(&json!("a"), &json!("a")));
        assert!(!is_choice(&json!("a"), &json!("b")));
        assert!(is_choice(&json!(""), &Value::Null), "unset is the empty option");
        assert!(is_choice(&json!([]), &Value::Null), "unset is the empty list option");
        let kanji = json!(["一", "二", "三", "四"]);
        assert!(is_choice(&kanji, &json!(["一", "二", "三"])), "a shorter saved list still matches its preset");
        assert!(!is_choice(&kanji, &json!(["I", "II"])));
        assert!(!is_choice(&kanji, &json!([])), "an empty list is the Numbers option, not Kanji");
    }

    use super::*;

    #[test]
    fn values_read_like_the_js() {
        assert_eq!(fmt_value(Fmt::Px, 170.0), "170px");
        assert_eq!(fmt_value(Fmt::Pct, 0.4), "40%");
        assert_eq!(fmt_value(Fmt::Pct, 0.15), "15%");
        assert_eq!(fmt_value(Fmt::Pct, 0.125), "13%"); // Math.round: halves up
        assert_eq!(fmt_value(Fmt::PctInt, 34.0), "34%");
        assert_eq!(fmt_value(Fmt::Kelvin, 4500.0), "4500K");
        assert_eq!(fmt_value(Fmt::Weight, 0.0), "Normal");
        assert_eq!(fmt_value(Fmt::Weight, 100.0), "+100");
        assert_eq!(fmt_value(Fmt::Weight, -50.0), "-50");
        assert_eq!(fmt_value(Fmt::Fixed1, 0.5), "0.5");
        assert_eq!(fmt_value(Fmt::Fixed1, 2.0), "2.0");
    }

    #[test]
    fn slider_snaps_to_its_step() {
        let s = Sl { min: 0.0, max: 0.6, step: 0.02, fmt: Fmt::Pct };
        assert_eq!(snap(&s, 0.5), 0.3);
        assert_eq!(snap(&s, 2.0), 0.6);
        assert_eq!(snap(&s, -1.0), 0.0);
        let k = Sl { min: 2500.0, max: 6000.0, step: 100.0, fmt: Fmt::Kelvin };
        assert_eq!(snap(&k, 0.5), 4300.0); // 17.5 steps rounds up
        assert_eq!(num_json(170.0), json!(170));
        assert_eq!(num_json(0.3), json!(0.3));
    }
}
