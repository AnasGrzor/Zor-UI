//! The message list of a chat: the user's words in bubbles on the right, replies flat on the left under an avatar and a name,
//! drawn as Markdown, with an optional status chip under them, and notes as centred pills. A reply can carry its tool steps,
//! the way Claude shows them: folded into one "Used N tools" row from the first step (busy dots on it while one runs), and a
//! click on it unfolds each step with its icon, words and status. Each message is laid out once per
//! width and cached. The list scrolls and sticks to the bottom while new messages arrive, until the user scrolls up.

use super::{dots, md};
use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    hub, ui::Scroll, util,
};

/// Who a message is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role { User, Assistant, Note }

/// One message as the list shows it. `who` is the name over a reply (the model, "Buddy"). `chip` is a short status under a
/// reply ("Done", "Failed"), empty for none; `chip_warn` draws it in the error colour. `pending`: the reply is still being
/// written (busy dots while it has no text, its tool steps shown open). `tools_open`: a finished reply's steps are unfolded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message { pub role: Role, pub text: String, pub who: String, pub error: bool, pub chip: String, pub chip_warn: bool, pub pending: bool, pub tools: Vec<Tool>, pub tools_open: bool }

/// One tool step of a reply: a Material icon, the words, and its status (running, done, failed, declined or blocked).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tool { pub icon: String, pub label: String, pub status: String }

impl Default for Role { fn default() -> Role { Role::Note } }

/// Space between two messages.
pub const GAP: f32 = 10.0;
/// Inside a bubble: left and right, top and bottom.
const BUBBLE_X: f32 = 14.0;
const BUBBLE_Y: f32 = 10.0;
/// A reply's head: the avatar and name line over its text.
pub const ASST_HEAD: f32 = 38.0;
/// The room an empty reply keeps for its busy dots.
pub const EMPTY_H: f32 = 20.0;
/// The status chip under a reply: its gap above, and its height (an 11.5 px line plus 4 px and more).
pub const CHIP_GAP: f32 = 8.0;
pub const CHIP_BOX: f32 = 22.0;
/// Inside the status chip, left and right.
const CHIP_X: f32 = 10.0;
/// One tool step's row (a 12.5 px line plus room), the folded "Used N tools" row, and the gap under the steps.
pub const TOOL_ROW: f32 = 26.0;
pub const TOOL_SUM: f32 = 30.0;
pub const TOOLS_GAP: f32 = 8.0;
/// Inside a tool row: the icon's size and the room it takes before the words, and the status mark's room on the right.
const TOOL_ICON: f32 = 16.0;
const TOOL_TEXT_X: f32 = 26.0;
const TOOL_MARK: f32 = 28.0;
/// The hit ids of the folded rows: `TOOLS0` plus the message's index.
pub const TOOLS0: u32 = 40_000;
/// Where the name over a reply starts (after the avatar), and the room kept after it.
const WHO_X: f32 = 42.0;
const WHO_END: f32 = 14.0;
/// The bubble corner radius; the corner by the speaker is squarer (`TAIL_R`).
const BUBBLE_R: f32 = 23.0;
const TAIL_R: f32 = 6.0;

/// The height of a reply whose Markdown is `md_h` tall: the head, its tool steps (`tools_h`), the text (or room for the dots
/// when it is still empty), and the chip row when it has a chip.
pub fn asst_h(md_h: f32, empty: bool, chip: bool, tools_h: f32) -> f32 {
    ASST_HEAD + tools_h + md_h.max(if empty { EMPTY_H } else { 0.0 }) + if chip { CHIP_GAP + CHIP_BOX } else { 0.0 }
}

/// The height of `n` tool steps: the folded row, and the rows under it when `open`. Nothing for no steps.
pub fn tools_h(n: usize, open: bool) -> f32 {
    if n == 0 { return 0.0; }
    TOOL_SUM + if open { n as f32 * TOOL_ROW } else { 0.0 } + TOOLS_GAP
}

/// The folded row's words: "Used 3 tools", with how many failed.
pub fn tools_summary(tools: &[Tool]) -> String {
    let n = tools.len();
    let failed = tools.iter().filter(|t| t.status == "failed").count();
    let base = format!("Used {n} tool{}", if n == 1 { "" } else { "s" });
    if failed > 0 { format!("{base} \u{00b7} {failed} failed") } else { base }
}

/// A message laid out for the current width.
enum Lay { Note { w: f32, h: f32 }, User { w: f32, h: f32 }, Asst { laid: md::Laid, w: f32, h: f32 } }
impl Lay { fn size(&self) -> (f32, f32) { match self { Lay::Note { w, h } | Lay::User { w, h } | Lay::Asst { w, h, .. } => (*w, *h) } } }

/// What a layout was made for: text length, error, width, chip, tool steps (how many, still running, unfolded), and whether the
/// reply is still being written; a change lays the message out again.
type Tag = (usize, bool, u32, usize, (usize, usize, bool), bool);

/// A bubble of `col`; the corner next to the speaker is squarer (opaque colours only).
pub fn bubble(c: &mut Canvas, r: R, col: u32, tail_right: bool) {
    c.rect(r, BUBBLE_R, col);
    let patch = if tail_right { R::new(r.x + r.w - BUBBLE_R, r.y + r.h - BUBBLE_R, BUBBLE_R, BUBBLE_R) } else { R::new(r.x, r.y + r.h - BUBBLE_R, BUBBLE_R, BUBBLE_R) };
    c.rect(patch, TAIL_R, col);
}

#[derive(Default)]
pub struct Transcript {
    lays: Vec<Option<(Tag, Lay)>>,
    pub scroll: Scroll,
    /// Pinned to the bottom: new messages scroll into view.
    pub stick: bool,
    /// (content height, view height) of the last paint, for the wheel.
    pub dims: (f32, f32),
    sink: md::Sink,
    copied: Option<(u32, f64)>,
}

impl Transcript {
    pub fn new() -> Transcript { Transcript { stick: true, ..Default::default() } }

    /// Forgets the layouts and goes back to the bottom (another chat is shown).
    pub fn reset(&mut self) { self.lays.clear(); self.stick = true; self.scroll = Scroll::default(); }

    fn lay(&mut self, g: &Gfx, msgs: &[Message], i: usize, vw: f32) {
        let maxw = ((vw - 4.0) * 0.92).floor();
        let inner = maxw - 2.0 * BUBBLE_X;
        let m = &msgs[i];
        let running = m.tools.iter().filter(|t| t.status == "running").count();
        let tag: Tag = (m.text.len(), m.error, maxw as u32, m.chip.len(), (m.tools.len(), running, m.tools_open), m.pending);
        if self.lays.len() < msgs.len() { self.lays.resize_with(msgs.len(), || None); }
        if self.lays[i].as_ref().is_some_and(|l| l.0 == tag) { return; }
        let l = match m.role {
            Role::Note => { let w = (g.measure(&m.text, txt(11.5, 400)).0.ceil() + 26.0).min(vw - 4.0); Lay::Note { w, h: g.measure_wrapped(&m.text, txt(11.5, 400), w - 24.0) + 10.0 } }
            Role::User => { let tw = (g.measure(&m.text, txt(13.5, 400)).0.ceil() + 2.0).min(inner); Lay::User { w: tw + 2.0 * BUBBLE_X, h: g.measure_wrapped(&m.text, txt(13.5, 400), tw) + 2.0 * BUBBLE_Y } }
            Role::Assistant => {
                let laid = md::layout(g, &m.text, inner);
                // the name is measured in the font it is drawn in, from where it starts, so it is never cut short
                let who = WHO_X + g.measure(&m.who, txt(11.5, 500)).0.ceil() + WHO_END;
                let chip = if m.chip.is_empty() { 0.0 } else { g.measure(&m.chip, txt(11.5, 500)).0 + 2.0 * CHIP_X };
                let w = if m.tools.is_empty() { (laid.w.ceil().max(chip) + 2.0 * BUBBLE_X).max(who).max(60.0) } else { maxw };
                let h = asst_h(laid.h, m.text.is_empty(), !m.chip.is_empty(), tools_h(m.tools.len(), m.tools_open));
                Lay::Asst { laid, w, h }
            }
        };
        self.lays[i] = Some((tag, l));
    }

    /// Draws `msgs` in `view`. `avatar` is the icon of the replies. The content is `view.w` less the scrollbar's room.
    pub fn paint(&mut self, c: &mut Canvas, cx: &Ctx, view: R, msgs: &[Message], avatar: &str) {
        let t = &cx.theme;
        let n = msgs.len();
        self.lays.truncate(n);
        let cw = view.w - crate::tokens::BAR_ROOM; // content width: the scrollbar keeps the right BAR_ROOM
        for i in 0..n { self.lay(c.g, msgs, i, cw); }
        let mut sink = md::Sink::default();
        let copied = self.copied.filter(|x| hub::now_ms() - x.1 < 1200.0).map(|x| x.0);
        let mut sc = std::mem::take(&mut self.scroll);
        if self.stick { sc.y = f32::MAX; }
        let sizes: Vec<(f32, f32)> = self.lays.iter().take(n).map(|l| l.as_ref().map_or((0.0, 0.0), |l| l.1.size())).collect();
        let content = sizes.iter().map(|s| s.1 + GAP).sum::<f32>() + 4.0 - GAP;
        self.dims = (content, view.h);
        let lays = &self.lays;
        sc.region(c, cx, view, content, |c, top| {
            let mut y = top + 2.0;
            for (i, (w, h)) in sizes.iter().enumerate() {
                let Some((_, lay)) = &lays[i] else { continue };
                let m = &msgs[i];
                if y + h >= view.y && y <= view.y + view.h {
                    match lay {
                        Lay::Note { .. } => {
                            let b = R::new(view.x + (cw - w) / 2.0, y, *w, *h);
                            c.rect(b, h / 2.0, t.c("surfaceContainerLow"));
                            c.para(&m.text, R::new(b.x + 12.0, b.y + 5.0, b.w - 24.0, b.h - 10.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Center);
                        }
                        Lay::User { .. } => {
                            let b = R::new(view.x + cw - w, y, *w, *h);
                            bubble(c, b, t.c("primary"), true);
                            c.para(&m.text, R::new(b.x + BUBBLE_X, b.y + BUBBLE_Y, b.w - 2.0 * BUBBLE_X, b.h - 2.0 * BUBBLE_Y), txt(13.5, 400), t.c("onPrimary"), Align::Left);
                        }
                        Lay::Asst { laid, .. } => {
                            let b = R::new(view.x + 2.0, y, *w, *h);
                            // flat reply (no bubble) under an avatar + name; only errors get a tinted card
                            if m.error { c.rect(b, BUBBLE_R, t.c("surfaceContainerLow")); c.rect(b, BUBBLE_R, alpha(t.c("error"), 0.16)); }
                            c.circle(b.x + 25.0, b.y + 17.0, 11.0, t.c("primaryContainer"));
                            c.icon(avatar, R::new(b.x + 19.0, b.y + 11.0, 12.0, 12.0), ico(12.0, false), t.c("onPrimaryContainer"));
                            c.text(&m.who, R::new(b.x + WHO_X, b.y + 9.0, b.w - WHO_X - WHO_END, 16.0), txt(11.5, 500), t.c("onSurfaceVariant"), Align::Left);
                            let th = tools_h(m.tools.len(), m.tools_open);
                            draw_tools(c, cx, R::new(b.x + BUBBLE_X, b.y + ASST_HEAD, b.w - 2.0 * BUBBLE_X, th), m, TOOLS0 + i as u32);
                            let top = b.y + ASST_HEAD + th;
                            if m.text.is_empty() && m.pending { dots(c, cx, b.x + BUBBLE_X + 16.0, top + 10.0); }
                            md::draw(c, cx, laid, b.x + BUBBLE_X, top, &mut sink, (view.y, view.y + view.h), copied);
                            if !m.chip.is_empty() {
                                let cwid = c.g.measure(&m.chip, txt(11.5, 500)).0.ceil() + 2.0 * CHIP_X;
                                let chip = R::new(b.x + BUBBLE_X, b.y + b.h - CHIP_BOX, cwid, CHIP_BOX);
                                let (bg, fg) = if m.chip_warn { (alpha(t.c("error"), 0.16), t.c("error")) } else { (t.c("surfaceContainerHigh"), t.c("onSurfaceVariant")) };
                                c.rect(chip, CHIP_BOX / 2.0, bg);
                                c.text(&m.chip, chip, txt(11.5, 500), fg, Align::Center);
                            }
                        }
                    }
                } else if let Lay::Asst { laid, .. } = lay {
                    // off-screen: still number its links and code blocks so hit ids stay in step
                    md::draw(c, cx, laid, 0.0, y, &mut sink, (-1.0e9, -1.0e9), None);
                    let _ = m;
                }
                y += h + GAP;
            }
        });
        self.scroll = sc;
        self.sink = sink;
    }

    /// A wheel turn over the list: it leaves the bottom, and comes back to it when scrolled all the way down.
    pub fn wheel(&mut self, d: i32) {
        self.stick = false;
        self.scroll.wheel(d, self.dims.0, self.dims.1);
        self.stick = self.scroll.y >= self.dims.0 - self.dims.1 - 1.0;
    }

    /// A click on a link, a code block's copy button, or a folded tool row. A copy is done here; a link's URL and the message
    /// whose tools to fold or unfold are returned for the caller. None for any other id.
    pub fn click(&mut self, id: u32) -> Option<Clicked> {
        if (TOOLS0..TOOLS0 + 10_000).contains(&id) { return Some(Clicked::Tools((id - TOOLS0) as usize)); }
        if (md::LINK0..md::LINK0 + 5000).contains(&id) {
            return self.sink.links.get((id - md::LINK0) as usize).cloned().map(Clicked::Link);
        }
        if (md::COPY0..md::COPY0 + 5000).contains(&id) {
            if let Some(code) = self.sink.copies.get((id - md::COPY0) as usize) { util::clipboard_set(code); self.copied = Some((id, hub::now_ms())); }
            return Some(Clicked::Copied);
        }
        None
    }
}

/// What a click in the list did.
#[derive(Debug, PartialEq)]
pub enum Clicked { Link(String), Copied, Tools(usize) }

/// A reply's tool steps in `r`: the folded row (hit id `id`, busy dots while a step runs) and the rows under it when open.
fn draw_tools(c: &mut Canvas, cx: &Ctx, r: R, m: &Message, id: u32) {
    if m.tools.is_empty() { return; }
    let t = &cx.theme;
    let mut y = r.y;
    let row = R::new(r.x - 6.0, y, r.w + 6.0, TOOL_SUM - 4.0);
    if c.hit(row, id) { c.rect(row, row.h / 2.0, alpha(t.c("onSurface"), crate::tokens::HOVER_A)); }
    c.icon("build", R::new(r.x, y, TOOL_ICON, row.h), ico(TOOL_ICON, false), t.c("onSurfaceVariant"));
    let words = tools_summary(&m.tools);
    let ww = c.g.measure(&words, txt(12.5, 500)).0.ceil();
    c.text(&words, R::new(r.x + TOOL_TEXT_X, y, ww + 2.0, row.h), txt(12.5, 500), t.c("onSurfaceVariant"), Align::Left);
    let chevron = R::new(r.x + TOOL_TEXT_X + ww + 6.0, y, 18.0, row.h);
    c.icon(if m.tools_open { "expand_less" } else { "expand_more" }, chevron, ico(18.0, false), t.c("onSurfaceVariant"));
    if m.tools.iter().any(|x| x.status == "running") { dots(c, cx, chevron.x + chevron.w + 22.0, y + row.h / 2.0); }
    y += TOOL_SUM;
    if !m.tools_open { return; }
    for tool in &m.tools {
        let row = R::new(r.x, y, r.w, TOOL_ROW);
        c.icon(&tool.icon, R::new(row.x, row.y, TOOL_ICON, row.h), ico(TOOL_ICON, false), t.c("onSurfaceVariant"));
        let col = if tool.status == "running" { t.c("onSurface") } else { t.c("onSurfaceVariant") };
        c.text(&tool.label, R::new(row.x + TOOL_TEXT_X, row.y, row.w - TOOL_TEXT_X - TOOL_MARK, row.h), txt(12.5, 400), col, Align::Left);
        let mark = R::new(row.x + row.w - TOOL_MARK + 6.0, row.y, TOOL_MARK - 6.0, row.h);
        match tool.status.as_str() {
            "running" => dots(c, cx, mark.x + mark.w / 2.0, mark.y + mark.h / 2.0),
            "done" => c.icon("check", mark, ico(16.0, false), t.c("primary")),
            "failed" => c.icon("close", mark, ico(16.0, false), t.c("error")),
            _ => c.icon("block", mark, ico(16.0, false), t.c("onSurfaceVariant")),
        }
        y += TOOL_ROW;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_reserves_its_head_tools_text_and_chip() {
        assert_eq!(asst_h(40.0, false, false, 0.0), ASST_HEAD + 40.0);
        assert_eq!(asst_h(0.0, true, false, 0.0), ASST_HEAD + EMPTY_H, "an empty reply keeps room for its dots");
        assert_eq!(asst_h(40.0, false, true, 0.0), ASST_HEAD + 40.0 + CHIP_GAP + CHIP_BOX);
        assert_eq!(asst_h(40.0, false, false, tools_h(2, false)), ASST_HEAD + TOOL_SUM + TOOLS_GAP + 40.0);
        assert!(CHIP_BOX >= 11.5 * 1.3 + 4.0, "the chip is taller than its line plus 4 px");
        assert!(TOOL_ROW >= 12.5 * 1.3 + 4.0, "a tool row is taller than its line plus 4 px");
    }

    #[test]
    fn tool_steps_are_folded_until_opened() {
        assert_eq!(tools_h(0, true), 0.0, "no steps take no room");
        assert_eq!(tools_h(3, false), TOOL_SUM + TOOLS_GAP, "folded, running or done");
        assert_eq!(tools_h(3, true), TOOL_SUM + 3.0 * TOOL_ROW + TOOLS_GAP);
        let tool = |status: &str| Tool { icon: "search".into(), label: "x".into(), status: status.into() };
        assert_eq!(tools_summary(&[tool("done")]), "Used 1 tool");
        assert_eq!(tools_summary(&[tool("done"), tool("failed"), tool("done")]), "Used 3 tools \u{00b7} 1 failed");
    }
}
