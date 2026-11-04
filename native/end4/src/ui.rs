//! Pieces every popup uses: panel background, chips, icon buttons, toggle, slider, wave progress,
//! tabs (all sized like base.css), a scroll region and a text box. Draw helpers register their own
//! hit spot with the id you pass, so the view only has to match on `Input::Click(id, ..)`.

use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, Color, Font, Gfx, R},
    hub, util,
};

/// Google Sans Flex line height per px of font size.
pub const LH: f32 = 1.252;

/// `--layer0`: the background colour with the configured transparency.
pub fn layer0(cx: &Ctx) -> Color { alpha(cx.theme.c("background"), 1.0 - cx.cfg.get("transparency").as_f64().unwrap_or(0.0) as f32) }

/// The popup background (bar hover popups, sidebars): shadow, 1px border, `--layer0` fill. `r` is the
/// card; leave ~12 px around it in the window for the shadow.
pub fn panel(c: &mut Canvas, cx: &Ctx, r: R, radius: f32) {
    c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(r, radius, col) });
    c.rect(r, radius, alpha(cx.theme.c("outlineVariant"), 0.45));
    c.rect(R::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0), radius - 1.0, layer0(cx));
}

/// `.card`: `--layer1` (surfaceContainerLow) fill, radius 23 by default (--round-lg).
pub fn card(c: &mut Canvas, cx: &Ctx, r: R, radius: f32) { c.rect(r, radius, cx.theme.c("surfaceContainerLow")); }

/// Titled placeholder for views that aren't ported yet.
pub fn stub(c: &mut Canvas, cx: &Ctx, name: &str) {
    let r = R::new(6.0, 6.0, (c.w - 12.0).max(1.0), (c.h - 12.0).max(1.0));
    panel(c, cx, r, 23.0);
    c.text(&format!("{name}: not ported yet"), R::new(r.x + 20.0, r.y + 14.0, r.w - 40.0, 24.0), txt(15.0, 500), cx.theme.c("onSurface"), Align::Left);
}

fn hover_bg(c: &mut Canvas, cx: &Ctx, r: R, radius: f32, over: bool) { if over { c.rect(r, radius, alpha(cx.theme.c("onSurface"), 0.08)); } }

/// `.chip` (height 32, 13 px text, optional icon); `sel` is `.chips .chip.sel`. Returns its rect (x, y is the top-left).
pub fn chip(c: &mut Canvas, cx: &Ctx, x: f32, y: f32, id: u32, icon: Option<&str>, label: &str, sel: bool) -> R {
    let t = &cx.theme;
    let iw = if icon.is_some() { 18.0 + 6.0 } else { 0.0 };
    let r = R::new(x, y, 14.0 + iw + c.g.measure(label, txt(13.0, 400)).0.ceil() + 14.0, 32.0);
    let (bg, fg) = if sel { (t.c("secondaryContainer"), t.c("onSecondaryContainer")) } else { (t.c("surfaceContainer"), t.c("onSurface")) };
    c.rect(r, 16.0, bg);
    let over = c.hit(r, id);
    hover_bg(c, cx, r, 16.0, over);
    let mut tx = r.x + 14.0;
    if let Some(i) = icon { c.icon(i, R::new(tx, r.y, 18.0, 32.0), ico(18.0, false), fg); tx += iw; }
    c.text(label, R::new(tx, r.y, r.w - 14.0 - (tx - r.x), 32.0), txt(13.0, 400), fg, Align::Left);
    r
}

#[derive(Clone, Copy, PartialEq)]
pub enum IconKind { Plain, Tonal, Filled }

/// `.icon-btn` (36 px circle by default; `r` sets the size): hover tint, or `.tonal` / `.filled` fill.
/// Returns true while hovered.
pub fn icon_btn(c: &mut Canvas, cx: &Ctx, r: R, id: u32, icon: &str, kind: IconKind, isize: f32, fill: bool) -> bool {
    let t = &cx.theme;
    let (bg, fg) = match kind {
        IconKind::Plain => (None, t.c("onSurface")),
        IconKind::Tonal => (Some(t.c("secondaryContainer")), t.c("onSecondaryContainer")),
        IconKind::Filled => (Some(t.c("primary")), t.c("onPrimary")),
    };
    if let Some(b) = bg { c.rect(r, r.h / 2.0, b); }
    let over = c.hit(r, id);
    hover_bg(c, cx, r, r.h / 2.0, over);
    c.icon(icon, r, ico(isize, fill), fg);
    over
}

/// The settings switch, 52x32 at (x, y). Returns its rect.
pub fn toggle(c: &mut Canvas, cx: &Ctx, x: f32, y: f32, id: u32, on: bool) -> R {
    let t = &cx.theme;
    let r = R::new(x, y, 52.0, 32.0);
    let (bg, edge, knob) = if on { (t.c("primary"), t.c("primary"), t.c("onPrimary")) } else { (t.c("surfaceContainerHighest"), t.c("outline"), t.c("outline")) };
    c.rect(r, 16.0, edge);
    c.rect(R::new(x + 2.0, y + 2.0, 48.0, 28.0), 14.0, bg);
    c.circle(x + if on { 36.0 } else { 16.0 }, y + 16.0, if on { 12.0 } else { 8.0 }, knob);
    c.hit(r, id);
    r
}

/// The M3 slider: 44 px track, `value` 0-1 filled, optional icon on the left and a label on the right.
/// Read the new value from `Input::Drag(id, x, ..)` with `slider_value`.
pub fn slider(c: &mut Canvas, cx: &Ctx, r: R, id: u32, icon: Option<&str>, value: f32, label: &str) {
    let t = &cx.theme;
    let value = value.clamp(0.0, 1.0);
    c.rect(r, 17.0, t.c("secondaryContainer"));
    c.clip_round(r, 17.0, |c| c.rect(R::new(r.x, r.y, (r.w * value).max(0.0), r.h), 6.0, t.c("primary")));
    if let Some(i) = icon { c.icon(i, R::new(r.x + 12.0, r.y, 20.0, r.h), ico(20.0, false), t.c("onPrimary")); }
    let lw = c.g.measure(label, txt(13.0, 400)).0;
    let high = r.w * value > r.w - 14.0 - lw; // `.slider.high`: the fill reached the label
    c.text(label, R::new(r.x + r.w - 14.0 - lw - 2.0, r.y, lw + 4.0, r.h), txt(13.0, 400), if high { t.c("onPrimary") } else { t.c("onSecondaryContainer") }, Align::Right);
    c.hit(r, id);
}

/// 0-1 position of window x inside the slider / wave rect `r`.
pub fn slider_value(r: R, x: f32) -> f32 { ((x - r.x) / r.w).clamp(0.0, 1.0) }

/// waveSvg() in core.js: the squiggly progress bar, 22 px high at (r.x, r.y), width r.w. `color` is the line colour.
/// Animates while `playing`: ask for ~50 ms frames then.
pub fn wave(c: &mut Canvas, r: R, id: u32, p: f32, playing: bool, color: Color) {
    let mid = r.y + 11.0;
    let x = p.clamp(0.0, 1.0) * r.w;
    let phase = if playing { (hub::now_ms() / 160.0) as f32 } else { 0.0 };
    let mut pts = vec![(r.x, mid)];
    let mut i = 2.0;
    while i <= x { pts.push((r.x + i, mid + 3.0 * (i / 4.0 - phase).sin())); i += 2.0; }
    c.polyline(&pts, 3.0, color);
    if x + 8.0 < r.w { c.line((r.x + x + 8.0, mid), (r.x + r.w, mid), 3.0, alpha(color, 0.4)); }
    c.rect(R::new(r.x + (x - 1.5).max(0.0), r.y + 3.0, 3.0, 16.0), 1.5, color);
    c.hit(R::new(r.x, r.y, r.w, 22.0), id);
}

/// ring() in core.js: track + value arc centred at (cx_, cy_), `size` px wide.
pub fn ring(c: &mut Canvas, cx: &Ctx, cx_: f32, cy_: f32, size: f32, stroke: f32, value: f32) {
    let r = (size - stroke) / 2.0;
    c.arc(cx_, cy_, r, 1.0, stroke, cx.theme.c("secondaryContainer"));
    c.arc(cx_, cy_, r, value, stroke, cx.theme.c("primary"));
}

/// `.tabbar` in `r` (height 46): hit ids are `id0 + index`. items: (icon, label); either may be empty.
pub fn tabs(c: &mut Canvas, cx: &Ctx, r: R, id0: u32, items: &[(&str, &str)], sel: usize) {
    let t = &cx.theme;
    c.rect(r, r.h / 2.0, t.c("surfaceContainerLow"));
    if items.is_empty() { return; }
    let w = (r.w - 8.0 - 2.0 * (items.len() - 1) as f32) / items.len() as f32;
    for (i, (icon, label)) in items.iter().enumerate() {
        let b = R::new(r.x + 4.0 + i as f32 * (w + 2.0), r.y + 4.0, w, r.h - 8.0);
        let on = i == sel;
        let fg = if on { t.c("onSecondaryContainer") } else { t.c("onSurfaceVariant") };
        if on { c.rect(b, b.h / 2.0, t.c("secondaryContainer")); }
        let over = c.hit(b, id0 + i as u32);
        if !on { hover_bg(c, cx, b, b.h / 2.0, over); }
        let tw = if label.is_empty() { 0.0 } else { c.g.measure(label, txt(12.5, 400)).0.ceil() };
        let iw = if icon.is_empty() { 0.0 } else { 19.0 + if tw > 0.0 { 6.0 } else { 0.0 } };
        let mut x = b.x + (b.w - tw - iw) / 2.0;
        if !icon.is_empty() { c.icon(icon, R::new(x, b.y, 19.0, b.h), ico(19.0, false), fg); x += iw; }
        if tw > 0.0 { c.text(label, R::new(x, b.y, tw + 2.0, b.h), txt(12.5, 400), fg, Align::Left); }
    }
}

/// A caret that blinks (visible for 500 ms, hidden for 500 ms). Views using it need `frame_ms` 500.
pub fn blink() -> bool { (hub::now_ms() / 500.0) as u64 % 2 == 0 }

// ------------------------------------------------------------------ scrolling

/// Vertical scroll offset of a clipped list. Keep one per scrollable area.
#[derive(Default)]
pub struct Scroll { pub y: f32 }

impl Scroll {
    /// Wheel step: three lines per notch (`d` is Input::Wheel's delta, >0 up).
    pub fn wheel(&mut self, d: i32, content_h: f32, view_h: f32) {
        self.y -= d as f32 / 120.0 * 48.0;
        self.clamp(content_h, view_h);
    }
    pub fn clamp(&mut self, content_h: f32, view_h: f32) { self.y = self.y.clamp(0.0, (content_h - view_h).max(0.0)); }
    /// Draws `body(canvas, top)` clipped to `r`, where `top` is the y of the content's first pixel
    /// (r.y - scroll), plus a thin scrollbar. Skip items outside the view yourself: hit spots ignore the clip.
    pub fn region(&mut self, c: &mut Canvas, cx: &Ctx, r: R, content_h: f32, body: impl FnOnce(&mut Canvas, f32)) {
        self.clamp(content_h, r.h);
        let top = r.y - self.y;
        c.clip_round(r, 0.0, |c| body(c, top));
        if content_h > r.h {
            let th = (r.h * r.h / content_h).max(24.0);
            let ty = r.y + self.y / (content_h - r.h) * (r.h - th);
            c.rect(R::new(r.x + r.w - 4.0, ty, 3.0, th), 1.5, alpha(cx.theme.c("outline"), 0.6));
        }
    }
}

// ------------------------------------------------------------------ text box

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit { None, Changed, Submit }

/// A text field. Feed it `Input::Key` / `Input::Char`; position the caret from `Input::Drag(.., Start)`
/// with `click`. Selection is only "everything" (Ctrl+A): typing then replaces the text.
pub struct TextBox {
    pub text: String,
    /// Byte index of the caret.
    pub caret: usize,
    pub all_selected: bool,
    pub multiline: bool,
    /// Multiline: Enter submits and Shift+Enter adds a line (a chat box); else Enter adds a line.
    pub enter_submits: bool,
    /// Single line: how far the text is scrolled left (set by `draw`).
    off: std::cell::Cell<f32>,
    pending: Option<(f32, f32)>,
}

const BACK: u32 = 0x08;
const ENTER: u32 = 0x0D;
const END: u32 = 0x23;
const HOME: u32 = 0x24;
const LEFT: u32 = 0x25;
const UP: u32 = 0x26;
const RIGHT: u32 = 0x27;
const DOWN: u32 = 0x28;
const DELETE: u32 = 0x2E;

impl TextBox {
    pub fn new(multiline: bool) -> TextBox { TextBox { text: String::new(), caret: 0, all_selected: false, multiline, enter_submits: false, off: Default::default(), pending: None } }

    pub fn set(&mut self, s: &str) { self.text = s.to_string(); self.caret = s.len(); self.all_selected = false; }

    fn prev(&self) -> usize { self.text[..self.caret].char_indices().next_back().map_or(0, |c| c.0) }
    fn next(&self) -> usize { self.text[self.caret..].chars().next().map_or(self.caret, |c| self.caret + c.len_utf8()) }
    /// Start and end byte of the caret's line.
    fn line(&self) -> (usize, usize) {
        let s = self.text[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        (s, self.text[self.caret..].find('\n').map_or(self.text.len(), |i| self.caret + i))
    }

    fn insert(&mut self, s: &str) {
        if self.all_selected { self.text.clear(); self.caret = 0; self.all_selected = false; }
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    pub fn ch(&mut self, c: char) -> Edit {
        if c.is_control() { return Edit::None; }
        self.insert(c.encode_utf8(&mut [0; 4]));
        Edit::Changed
    }

    /// Shortcut: Up/Down move by logical lines, not wrapped rows.
    pub fn key(&mut self, vk: u32, ctrl: bool, shift: bool, _alt: bool) -> Edit {
        if ctrl {
            match vk {
                0x41 => { self.all_selected = !self.text.is_empty(); self.caret = self.text.len(); }
                0x43 => if self.all_selected { util::clipboard_set(&self.text); },
                0x58 => if self.all_selected { util::clipboard_set(&self.text); self.set(""); return Edit::Changed; },
                0x56 => {
                    let Some(p) = util::clipboard_get() else { return Edit::None };
                    let p = p.replace("\r\n", "\n").replace('\r', "\n");
                    self.insert(&if self.multiline { p } else { p.replace('\n', " ") });
                    return Edit::Changed;
                }
                _ => {}
            }
            return Edit::None;
        }
        match vk {
            ENTER if !self.multiline || (self.enter_submits && !shift) => return Edit::Submit,
            ENTER => self.insert("\n"),
            BACK if self.all_selected => self.set(""),
            BACK if self.caret > 0 => { let p = self.prev(); self.text.replace_range(p..self.caret, ""); self.caret = p; }
            DELETE if self.all_selected => self.set(""),
            DELETE if self.caret < self.text.len() => { let n = self.next(); self.text.replace_range(self.caret..n, ""); }
            LEFT => { self.caret = if self.all_selected { 0 } else { self.prev() }; self.all_selected = false; return Edit::None; }
            RIGHT => { self.caret = if self.all_selected { self.text.len() } else { self.next() }; self.all_selected = false; return Edit::None; }
            HOME => { self.caret = if self.multiline { self.line().0 } else { 0 }; self.all_selected = false; return Edit::None; }
            END => { self.caret = if self.multiline { self.line().1 } else { self.text.len() }; self.all_selected = false; return Edit::None; }
            UP | DOWN if self.multiline => {
                self.all_selected = false;
                let (s, e) = self.line();
                let col = self.text[s..self.caret].chars().count();
                let target = if vk == UP { if s == 0 { return Edit::None } else { self.text[..s - 1].rfind('\n').map_or(0, |i| i + 1) } } else if e >= self.text.len() { return Edit::None } else { e + 1 };
                let end = self.text[target..].find('\n').map_or(self.text.len(), |i| target + i);
                self.caret = self.text[target..end].char_indices().nth(col).map_or(end, |c| target + c.0);
                return Edit::None;
            }
            _ => return Edit::None,
        }
        Edit::Changed
    }

    /// A press at (x, y) (window CSS px, from `Drag Start`) moves the caret there at the next `draw`, which knows the rect and font.
    pub fn press(&mut self, x: f32, y: f32) { self.all_selected = false; self.pending = Some((x, y)); }

    /// Text (or the muted `placeholder`), the "everything selected" tint and a blinking caret while `focus`.
    /// `r` is the text area; a single line is centred vertically and scrolls sideways to keep the caret in view.
    pub fn draw(&mut self, c: &mut Canvas, cx: &Ctx, r: R, f: Font, placeholder: &str, focus: bool) {
        let t = &cx.theme;
        let (fg, muted) = (t.c("onSurface"), alpha(t.c("onSurfaceVariant"), 0.8));
        let wrap = self.multiline;
        if let Some((x, y)) = self.pending.take() {
            self.caret = if wrap { c.g.hit_index(&self.text, f, r.w, true, x - r.x, y - r.y) } else { c.g.hit_index(&self.text, f, r.w, false, x - r.x + self.off.get(), 0.0) };
        }
        let (cxp, cyp, ch) = c.g.caret(&self.text, f, r.w, wrap, self.caret);
        if !wrap { self.off.set((cxp - (r.w - 2.0)).max(0.0)); }
        let off = self.off.get();
        c.clip_round(r, 0.0, |c| {
            if self.text.is_empty() {
                if wrap { c.para(placeholder, r, f, muted, Align::Left) } else { c.text(placeholder, r, f, muted, Align::Left) }
            } else {
                if self.all_selected {
                    let (w, h) = if wrap { (r.w, c.g.measure_wrapped(&self.text, f, r.w)) } else { c.g.measure(&self.text, f) };
                    c.rect(R::new(r.x - off, if wrap { r.y } else { r.y + (r.h - h) / 2.0 }, w, h), 3.0, alpha(t.c("primary"), 0.3));
                }
                if wrap { c.para(&self.text, r, f, fg, Align::Left) }
                else { c.text(&self.text, R::new(r.x - off, r.y, c.g.measure(&self.text, f).0.ceil() + 8.0, r.h), f, fg, Align::Left) }
            }
            if focus && blink() {
                let y = if wrap { r.y + cyp } else { r.y + (r.h - ch) / 2.0 };
                c.rect(R::new(r.x + cxp - off, y, 1.5, ch), 0.75, t.c("primary"));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(tb: &mut TextBox, s: &str) { for c in s.chars() { tb.ch(c); } }

    #[test]
    fn single_line_editing() {
        let mut tb = TextBox::new(false);
        typed(&mut tb, "héllo");
        assert_eq!((tb.text.as_str(), tb.caret), ("héllo", 6));
        tb.key(LEFT, false, false, false);
        tb.key(BACK, false, false, false); // removes the second l
        assert_eq!(tb.text, "hélo");
        tb.key(HOME, false, false, false);
        tb.key(DELETE, false, false, false);
        tb.key(DELETE, false, false, false); // deletes the two-byte é whole
        assert_eq!(tb.text, "lo");
        tb.key(END, false, false, false);
        assert_eq!(tb.caret, 2);
        assert_eq!(tb.key(ENTER, false, false, false), Edit::Submit);
        assert_eq!(tb.ch('\n'), Edit::None); // control chars never insert
    }

    #[test]
    fn select_all_replaces() {
        let mut tb = TextBox::new(false);
        typed(&mut tb, "abc");
        tb.key(0x41, true, false, false);
        assert!(tb.all_selected);
        typed(&mut tb, "x");
        assert_eq!((tb.text.as_str(), tb.caret), ("x", 1));
        tb.key(0x41, true, false, false);
        tb.key(BACK, false, false, false);
        assert_eq!(tb.text, "");
    }

    #[test]
    fn multiline_lines() {
        let mut tb = TextBox::new(true);
        typed(&mut tb, "ab");
        tb.key(ENTER, false, false, false);
        typed(&mut tb, "cdef");
        assert_eq!(tb.text, "ab\ncdef");
        tb.key(UP, false, false, false); // column 4 is past "ab": end of line 1
        assert_eq!(tb.caret, 2);
        tb.key(DOWN, false, false, false);
        assert_eq!(tb.caret, 5); // column 2 of line 2
        tb.key(HOME, false, false, false);
        assert_eq!(tb.caret, 3);
        tb.enter_submits = true;
        assert_eq!(tb.key(ENTER, false, true, false), Edit::Changed); // Shift+Enter: new line
        assert_eq!(tb.key(ENTER, false, false, false), Edit::Submit);
    }

    #[test]
    fn scroll_clamps() {
        let mut s = Scroll::default();
        s.wheel(-120, 500.0, 200.0);
        assert_eq!(s.y, 48.0);
        s.wheel(120, 500.0, 200.0);
        s.wheel(120, 500.0, 200.0);
        assert_eq!(s.y, 0.0);
        s.wheel(-1200, 500.0, 200.0);
        assert_eq!(s.y, 300.0);
        assert_eq!(slider_value(R::new(10.0, 0.0, 100.0, 44.0), 60.0), 0.5);
    }
}
