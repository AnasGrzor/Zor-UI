//! A small Markdown renderer for chat replies (md() in core.js): `parse` -> blocks, `layout` -> positioned
//! words (flowed by hand: the canvas draws one style per call), `draw` -> pixels plus link / copy hit spots.

use crate::{app::Ctx, gfx::{alpha, ico, txt, Align, Canvas, Font, Gfx, R}};

#[derive(Clone, Debug, PartialEq)]
pub enum Span { Text(String), Bold(String), Italic(String), Code(String), Link(String, String) }

#[derive(Clone, Debug, PartialEq)]
pub enum Block { Heading(usize, Vec<Span>), Para(Vec<Span>), Item(String, Vec<Span>), Quote(Vec<Span>), Code(String, String) }

// ------------------------------------------------------------------ parsing

pub fn parse(src: &str) -> Vec<Block> {
    let (mut out, mut rest) = (vec![], src);
    while let Some(i) = rest.find("```") {
        lines(&rest[..i], &mut out);
        let after = &rest[i + 3..];
        let le = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
        let body = &after[le..];
        let body = body.strip_prefix("\r\n").or_else(|| body.strip_prefix('\n')).unwrap_or(body);
        let (code, next) = body.find("```").map_or((body, ""), |j| (&body[..j], &body[j + 3..]));
        out.push(Block::Code(after[..le].to_string(), code.strip_suffix('\n').unwrap_or(code).to_string()));
        rest = next;
    }
    lines(rest, &mut out);
    out
}

fn lines(text: &str, out: &mut Vec<Block>) {
    let (mut ordered, mut n) = (None::<bool>, 0);
    for line in text.lines() {
        let item = list_item(line);
        if item.is_none() { (ordered, n) = (None, 0); }
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if (1..=3).contains(&hashes) && line[hashes..].starts_with(char::is_whitespace) {
            out.push(Block::Heading(hashes, inline(line[hashes..].trim_start())));
        } else if let Some((num, rest)) = item {
            n += 1;
            let numbered = *ordered.get_or_insert(num);
            out.push(Block::Item(if numbered { format!("{n}.") } else { "\u{2022}".into() }, inline(rest)));
        } else if let Some(q) = line.strip_prefix('>') {
            out.push(Block::Quote(inline(q.strip_prefix(char::is_whitespace).unwrap_or(q))));
        } else if !line.trim().is_empty() {
            out.push(Block::Para(inline(line)));
        }
    }
}

/// `- x`, `* x`, `1. x` -> (is numbered, text).
fn list_item(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let (num, rest) = if digits > 0 { (true, t[digits..].strip_prefix('.')?) } else { (false, t.strip_prefix(['-', '*'])?) };
    rest.starts_with(char::is_whitespace).then(|| (num, rest.trim_start()))
}

/// Code, **bold**, *italic* and [text](http://url), left to right; anything unmatched stays literal.
pub fn inline(s: &str) -> Vec<Span> {
    let (mut out, mut text, mut i) = (vec![], String::new(), 0);
    let flush = |out: &mut Vec<Span>, text: &mut String| if !text.is_empty() { out.push(Span::Text(std::mem::take(text))); };
    while i < s.len() {
        let rest = &s[i..];
        let found: Option<(Span, usize)> = if let Some(r) = rest.strip_prefix('`') {
            r.find('`').filter(|&e| e > 0).map(|e| (Span::Code(r[..e].into()), e + 2))
        } else if let Some(r) = rest.strip_prefix("**") {
            r.find('*').filter(|&e| e > 0 && r[e..].starts_with("**")).map(|e| (Span::Bold(r[..e].into()), e + 4))
        } else if let Some(r) = rest.strip_prefix('*') {
            let ok = r.chars().next().is_some_and(|c| !c.is_whitespace() && c != '*');
            r.find('*').filter(|_| ok).map(|e| (Span::Italic(r[..e].into()), e + 2))
        } else if let Some(r) = rest.strip_prefix('[') {
            r.split_once("](").and_then(|(label, tail)| {
                let url = &tail[..tail.find(')')?];
                (!label.is_empty() && !label.contains(']') && (url.starts_with("http:") || url.starts_with("https:")) && !url.contains(char::is_whitespace))
                    .then(|| (Span::Link(label.into(), url.into()), label.len() + url.len() + 4))
            })
        } else { None };
        match found {
            Some((span, used)) => { flush(&mut out, &mut text); out.push(span); i += used; }
            None => { let c = rest.chars().next().unwrap(); text.push(c); i += c.len_utf8(); }
        }
    }
    flush(&mut out, &mut text);
    out
}

// ------------------------------------------------------------------ layout

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Col { Fg, Muted, Primary }

pub enum Op {
    Word { x: f32, y: f32, w: f32, h: f32, s: String, size: f32, weight: u16, col: Col, link: Option<usize> },
    /// Inline code background.
    Pill(R),
    Bar(R),
    Code { r: R, lang: String, code: String },
    /// Plain wrapped text (user messages, notes).
    Para { r: R, s: String, size: f32, col: Col, center: bool },
}

#[derive(Default)]
pub struct Laid { pub w: f32, pub h: f32, pub ops: Vec<Op>, pub links: Vec<String> }

struct Tok { s: String, size: f32, weight: u16, col: Col, link: Option<usize>, code: bool, gap: bool }

/// Flows `spans` at `x0`, `y0` into lines no wider than `width`. Returns (height, widest line).
#[allow(clippy::too_many_arguments)]
pub fn flow(g: &Gfx, spans: &[Span], x0: f32, y0: f32, width: f32, size: f32, weight: u16, col: Col, center: bool, ops: &mut Vec<Op>, links: &mut Vec<String>) -> (f32, f32) {
    let mut toks: Vec<Tok> = vec![];
    let mut gap = false;
    for sp in spans {
        let (t, w, c, link, code) = match sp {
            Span::Text(t) => (t.as_str(), weight, col, None, false),
            Span::Bold(t) => (t.as_str(), 700, col, None, false),
            Span::Italic(t) => (t.as_str(), weight, col, None, false), // shortcut: no italic face; upgrade with a font axis
            Span::Code(t) => (t.as_str(), weight, col, None, true),
            Span::Link(t, u) => { links.push(u.clone()); (t.as_str(), weight, Col::Primary, Some(links.len() - 1), false) }
        };
        if code { toks.push(Tok { s: t.into(), size: size * 12.0 / 13.5, weight: w, col: c, link, code, gap }); gap = false; continue; }
        gap |= t.starts_with(char::is_whitespace);
        for word in t.split_whitespace() {
            toks.push(Tok { s: word.into(), size, weight: w, col: c, link, code, gap });
            gap = true;
        }
        gap = t.ends_with(char::is_whitespace);
    }
    let (lh, space) = (size * 1.5, g.measure(" ", txt(size, 400)).0);
    let (mut x, mut y, mut maxw, mut start) = (0.0f32, 0.0f32, 0.0f32, ops.len());
    let end_line = |ops: &mut Vec<Op>, start: usize, x: f32| {
        if center && x < width {
            for op in &mut ops[start..] { match op { Op::Word { x: ox, .. } => *ox += (width - x) / 2.0, Op::Pill(r) => r.x += (width - x) / 2.0, _ => {} } }
        }
    };
    for tok in &toks {
        let f = txt(tok.size, tok.weight);
        let pad = if tok.code { 10.0 } else { 0.0 };
        let pieces = if g.measure(&tok.s, f).0 + pad > width { chunks(g, &tok.s, f, width - pad) } else { vec![tok.s.clone()] };
        for (k, piece) in pieces.iter().enumerate() {
            let w = g.measure(piece, f).0.ceil() + pad;
            let gapw = if tok.gap && k == 0 && x > 0.0 { space } else { 0.0 };
            if x > 0.0 && (x + gapw + w > width || k > 0) {
                end_line(ops, start, x);
                maxw = maxw.max(x);
                (x, y, start) = (0.0, y + lh, ops.len());
            } else { x += gapw; }
            if tok.code { ops.push(Op::Pill(R::new(x0 + x, y0 + y + (lh - 18.0) / 2.0, w, 18.0))); }
            ops.push(Op::Word { x: x0 + x + pad / 2.0, y: y0 + y, w: w - pad + 2.0, h: lh, s: piece.clone(), size: tok.size, weight: tok.weight, col: tok.col, link: tok.link });
            x += w;
        }
    }
    if toks.is_empty() { return (0.0, 0.0); }
    end_line(ops, start, x);
    (y + lh, maxw.max(x))
}

/// `s` cut into pieces no wider than `w` (overflow-wrap: anywhere).
fn chunks(g: &Gfx, s: &str, f: Font, w: f32) -> Vec<String> {
    let (mut out, mut cur) = (vec![], String::new());
    for ch in s.chars() {
        cur.push(ch);
        if g.measure(&cur, f).0 > w && cur.chars().count() > 1 {
            cur.pop();
            out.push(std::mem::take(&mut cur));
            cur.push(ch);
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

const BODY: f32 = 13.5;

/// The reply laid out at `width`.
pub fn layout(g: &Gfx, src: &str, width: f32) -> Laid {
    let mut l = Laid::default();
    let (mut y, mut bottom, mut run, mut first) = (0.0f32, 0.0f32, false, true);
    for b in parse(src) {
        let is_item = matches!(b, Block::Item(..));
        let (top, after) = match &b { Block::Para(_) => (0.0, 8.0), Block::Heading(..) => (8.0, 4.0), Block::Item(..) => (if run { 0.0 } else { 4.0 }, 8.0), Block::Quote(_) => (4.0, 4.0), Block::Code(..) => (6.0, 6.0) };
        // adjacent margins collapse to the larger one
        y += if first { top } else if run && is_item { 0.0 } else { bottom.max(top) };
        (first, run, bottom) = (false, is_item, after);
        match b {
            Block::Para(sp) => { let (h, w) = flow(g, &sp, 0.0, y, width, BODY, 400, Col::Fg, false, &mut l.ops, &mut l.links); y += h; l.w = l.w.max(w); }
            Block::Heading(n, sp) => {
                let (h, w) = flow(g, &sp, 0.0, y, width, [15.8, 13.5, 11.2][n - 1], 600, Col::Fg, false, &mut l.ops, &mut l.links);
                y += h; l.w = l.w.max(w);
            }
            Block::Item(mark, sp) => {
                l.ops.push(Op::Word { x: 4.0, y, w: 16.0, h: BODY * 1.5, s: mark, size: BODY, weight: 400, col: Col::Fg, link: None });
                let (h, w) = flow(g, &sp, 20.0, y, width - 20.0, BODY, 400, Col::Fg, false, &mut l.ops, &mut l.links);
                y += h; l.w = l.w.max(w + 20.0);
            }
            Block::Quote(sp) => {
                let (h, w) = flow(g, &sp, 13.0, y, width - 13.0, BODY, 400, Col::Muted, false, &mut l.ops, &mut l.links);
                l.ops.push(Op::Bar(R::new(0.0, y, 3.0, h)));
                y += h; l.w = l.w.max(w + 13.0);
            }
            Block::Code(lang, code) => {
                let h = 34.0 + 20.0 + g.measure_wrapped(&code, txt(12.0, 400), width - 24.0).max(18.0);
                l.ops.push(Op::Code { r: R::new(0.0, y, width, h), lang: if lang.is_empty() { "code".into() } else { lang }, code });
                y += h; l.w = width;
            }
        }
    }
    l.h = y;
    l
}

// ------------------------------------------------------------------ drawing

pub const LINK0: u32 = 20_000;
pub const COPY0: u32 = 30_000;

/// Links and code blocks seen while drawing, indexed by hit id - LINK0 / COPY0.
#[derive(Default)]
pub struct Sink { pub links: Vec<String>, pub copies: Vec<String> }

pub fn colour(cx: &Ctx, c: Col) -> u32 { cx.theme.c(match c { Col::Fg => "onSurface", Col::Muted => "onSurfaceVariant", Col::Primary => "primary" }) }

/// Draws `l` with its top-left at (ox, oy). Ops outside `view` (y range) are skipped but still numbered.
/// `copied`: the copy-button id that shows a check.
pub fn draw(c: &mut Canvas, cx: &Ctx, l: &Laid, ox: f32, oy: f32, sink: &mut Sink, view: (f32, f32), copied: Option<u32>) {
    let lbase = sink.links.len() as u32;
    sink.links.extend(l.links.iter().cloned());
    let t = &cx.theme;
    for op in &l.ops {
        let (top, h) = match op { Op::Word { y, h, .. } => (*y, *h), Op::Pill(r) | Op::Bar(r) => (r.y, r.h), Op::Code { r, .. } | Op::Para { r, .. } => (r.y, r.h) };
        let id = if let Op::Code { code, .. } = op { sink.copies.push(code.clone()); COPY0 + sink.copies.len() as u32 - 1 } else { 0 };
        if oy + top + h < view.0 || oy + top > view.1 { continue; }
        match op {
            Op::Word { x, y, w, h, s, size, weight, col, link } => {
                let r = R::new(ox + x, oy + y, *w, *h);
                c.text(s, r, txt(*size, *weight), colour(cx, *col), Align::Left);
                if let Some(li) = link {
                    c.hit(r, LINK0 + lbase + *li as u32);
                    c.rect(R::new(r.x, r.y + r.h / 2.0 + size * 0.62, r.w - 2.0, 1.0), 0.5, colour(cx, *col));
                }
            }
            Op::Pill(r) => c.rect(R::new(ox + r.x, oy + r.y, r.w, r.h), 5.0, t.c("surfaceContainerHigh")),
            Op::Bar(r) => c.rect(R::new(ox + r.x, oy + r.y, r.w, r.h), 1.5, t.c("outlineVariant")),
            Op::Para { r, s, size, col, center } => c.para(s, R::new(ox + r.x, oy + r.y, r.w, r.h), txt(*size, 400), colour(cx, *col), if *center { Align::Center } else { Align::Left }),
            Op::Code { r, lang, code } => {
                let r = R::new(ox + r.x, oy + r.y, r.w, r.h);
                c.rect(r, 17.0, t.c("surfaceContainerLowest"));
                c.clip_round(r, 17.0, |c| c.rect(R::new(r.x, r.y, r.w, 34.0), 0.0, t.c("surfaceContainer")));
                c.text(lang, R::new(r.x + 12.0, r.y, r.w - 60.0, 34.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
                let b = R::new(r.x + r.w - 32.0, r.y + 4.0, 26.0, 26.0);
                if c.hit(b, id) { c.circle(b.x + 13.0, b.y + 13.0, 13.0, alpha(t.c("onSurface"), 0.08)); }
                c.icon(if copied == Some(id) { "check" } else { "content_copy" }, b, ico(15.0, false), t.c("onSurface"));
                c.para(code, R::new(r.x + 12.0, r.y + 44.0, r.w - 24.0, r.h - 54.0), txt(12.0, 400), t.c("onSurface"), Align::Left);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(s: &str) -> Span { Span::Text(s.into()) }

    #[test]
    fn inline_styles() {
        assert_eq!(inline("a **b** *c* `d` [e](https://x.y/z) f"), vec![t("a "), Span::Bold("b".into()), t(" "), Span::Italic("c".into()), t(" "), Span::Code("d".into()), t(" "), Span::Link("e".into(), "https://x.y/z".into()), t(" f")]);
        assert_eq!(inline("2 * 3 * 4"), vec![t("2 * 3 * 4")]); // a space after the star: not italic
        assert_eq!(inline("[x](ftp://no) `open"), vec![t("[x](ftp://no) `open")]);
        assert_eq!(inline("**open"), vec![t("**open")]);
    }

    #[test]
    fn blocks() {
        let b = parse("# Title\nhello\n- a\n- b\n\n1. one\n2. two\n> quoted\n```rust\nfn x() {}\n```\nafter");
        assert_eq!(b[0], Block::Heading(1, vec![t("Title")]));
        assert_eq!(b[1], Block::Para(vec![t("hello")]));
        assert_eq!(b[2], Block::Item("\u{2022}".into(), vec![t("a")]));
        assert_eq!(b[3], Block::Item("\u{2022}".into(), vec![t("b")]));
        assert_eq!(b[4], Block::Item("1.".into(), vec![t("one")]));
        assert_eq!(b[5], Block::Item("2.".into(), vec![t("two")]));
        assert_eq!(b[6], Block::Quote(vec![t("quoted")]));
        assert_eq!(b[7], Block::Code("rust".into(), "fn x() {}".into()));
        assert_eq!(b[8], Block::Para(vec![t("after")]));
    }

    #[test]
    fn streaming_fence_is_open() {
        // a reply cut off mid code block still renders the code so far
        assert_eq!(parse("text\n```py\nprint(1)"), vec![Block::Para(vec![t("text")]), Block::Code("py".into(), "print(1)".into())]);
        assert_eq!(parse("#nospace"), vec![Block::Para(vec![t("#nospace")])]);
    }
}
