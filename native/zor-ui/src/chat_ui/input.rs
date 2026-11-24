//! The input bar under a chat: a text box that grows with its text, and a row of round buttons on its right, the last one being
//! send (or stop while a reply runs). The caller gives the extra buttons (a mic, Live) and handles their clicks.

use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Canvas, R},
    ui::{self, IconKind, TextBox},
};

/// Hit ids of the bar's own spots. Callers keep their ids clear of these.
pub const TEXT: u32 = 9_001;
pub const SEND: u32 = 9_002;

/// One round button's size, the space between two buttons, and the space between a button and the bar's edge.
pub const BTN: f32 = 36.0;
pub const BTN_GAP: f32 = 10.0;
pub const BTN_EDGE: f32 = 8.0;
/// The text's room inside the bar: left inset, and top and bottom inset.
const TEXT_X: f32 = 16.0;
const TEXT_Y: f32 = 15.0;
/// The tallest the text area grows before it scrolls inside itself.
const TEXT_MAX: f32 = 160.0;
const FONT: f32 = 13.5;

/// One extra button: its hit id, its icon, and whether it shows as on (a live mic).
pub struct Extra<'a> { pub id: u32, pub icon: &'a str, pub on: bool }

/// The bar's height for `text` in a bar `w` wide with `extras` buttons before send.
pub fn height(c: &Canvas, text: &str, w: f32, extras: usize) -> f32 {
    let tw = text_w(w, extras);
    let th = c.g.measure_wrapped(if text.is_empty() { "x" } else { text }, txt(FONT, 400), tw).clamp(17.0, TEXT_MAX);
    (th + 18.0).max(BTN) + 2.0 * BTN_EDGE
}

/// The text's width in a bar `w` wide: what the buttons and insets leave.
pub fn text_w(w: f32, extras: usize) -> f32 { w - TEXT_X - BTN_EDGE - BTN - extras as f32 * (BTN + BTN_GAP) - BTN_GAP }

/// Draws the bar at the bottom of `r` and returns its rectangle. `busy` turns send into a red stop.
pub fn paint(c: &mut Canvas, cx: &Ctx, r: R, input: &mut TextBox, placeholder: &str, focus: bool, busy: bool, extras: &[Extra]) -> R {
    let t = &cx.theme;
    let bar_h = height(c, &input.text, r.w, extras.len());
    let bar = R::new(r.x, r.y + r.h - bar_h, r.w, bar_h);
    c.rect(bar, 30.0, alpha(t.c("outline"), 0.35));
    c.rect(R::new(bar.x + 1.0, bar.y + 1.0, bar.w - 2.0, bar.h - 2.0), 29.0, t.c("surfaceContainer"));
    c.hit(bar, TEXT);
    input.draw(c, cx, R::new(bar.x + TEXT_X, bar.y + TEXT_Y, text_w(r.w, extras.len()), bar_h - 2.0 * TEXT_Y), txt(FONT, 400), placeholder, focus);
    let sb = R::new(bar.x + bar.w - BTN_EDGE - BTN, bar.y + bar.h - BTN_EDGE - BTN, BTN, BTN);
    for (i, e) in extras.iter().enumerate() {
        let b = R::new(sb.x - (extras.len() - i) as f32 * (BTN + BTN_GAP), sb.y, BTN, BTN);
        ui::icon_btn(c, cx, b, e.id, e.icon, if e.on { IconKind::Filled } else { IconKind::Tonal }, 22.0, e.on);
    }
    if busy {
        c.rect(sb, BTN / 2.0, t.c("error"));
        if c.hit(sb, SEND) { c.rect(sb, BTN / 2.0, alpha(t.c("onError"), 0.1)); }
        c.icon("stop", sb, ico(24.0, false), t.c("onError"));
    } else { ui::icon_btn(c, cx, sb, SEND, "arrow_upward", IconKind::Filled, 24.0, false); }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_extra_button_takes_its_room_from_the_text() {
        assert_eq!(text_w(400.0, 0) - text_w(400.0, 2), 2.0 * (BTN + BTN_GAP));
        assert!(text_w(400.0, 2) > 200.0, "two extras still leave room to type at 400 px");
    }
}
