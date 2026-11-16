//! Shared inline LocalSend controls for both island versions.
use crate::{app::Ctx, gfx::{Canvas, Gfx, R, txt, Align}, ui};
pub const SEND_REFRESH: u32 = 450;
pub const SEND_PHONE: u32 = 460;
pub const SEND_PHONE_MAX: usize = 20;
// Twelve pixels above and below each phone label.
const SEND_ROW_H: f32 = 44.0;
pub struct Panel { status: crate::localsend::Status, hint: String }
impl Panel {
    pub fn new(cx: &Ctx) -> Self {
        let status = crate::localsend::status();
        let alias = cx.state.v["localsendPhone"]["alias"].as_str().unwrap_or("");
        let hint = if crate::localsend::drag_hover() { "Release to send to your phone".into() }
            else if !status.message.is_empty() { status.message.clone() }
            else if alias.is_empty() { "Drop files on the island to send to your phone. Open LocalSend on the same Wi-Fi.".into() }
            else { format!("Drop files to send to {alias}.") };
        Self { status, hint }
    }
    pub fn height(&self, g: &Gfx, width: f32) -> f32 {
        20.0 + 8.0 + g.measure_wrapped(&self.hint, txt(12.5, 400), width)
            + 12.0 + 32.0 + self.status.phones.len().min(SEND_PHONE_MAX) as f32 * (SEND_ROW_H + 8.0)
    }
    pub fn paint(&self, c: &mut Canvas, cx: &Ctx, x: f32, y: f32, width: f32) {
        c.text("LocalSend", R::new(x, y, width, 20.0), txt(14.0, 600), cx.theme.c(if crate::localsend::drag_hover() { "primary" } else { "onSurface" }), Align::Left);
        let text_h = c.g.measure_wrapped(&self.hint, txt(12.5, 400), width);
        c.para(&self.hint, R::new(x, y + 28.0, width, text_h), txt(12.5, 400), cx.theme.c("onSurfaceVariant"), Align::Left);
        let mut sy = y + 28.0 + text_h + 12.0;
        if self.status.busy {
            let label = if self.status.discovering { "Finding phone..." } else { "Transfer in progress..." };
            c.text(label, R::new(x, sy, width, 32.0), txt(12.5, 400), cx.theme.c("onSurfaceVariant"), Align::Left);
        } else {
            ui::chip(c, cx, x, sy, SEND_REFRESH, Some("refresh"), "Find phone", false);
        }
        sy += 40.0;
        for (i, phone) in self.status.phones.iter().take(SEND_PHONE_MAX).enumerate() {
            let selected = cx.state.v["localsendPhone"]["fingerprint"].as_str() == Some(phone.fingerprint.as_str());
            let r = R::new(x, sy, width, SEND_ROW_H);
            c.rect(r, 12.0, cx.theme.c(if selected { "secondaryContainer" } else { "surfaceContainerHigh" }));
            if !self.status.busy { c.hit(r, SEND_PHONE + i as u32); }
            c.text(&phone.alias, R::new(x + crate::tokens::CARD_INSET, sy + 12.0, (width - 2.0 * crate::tokens::CARD_INSET).max(0.0), 20.0), txt(13.0, 400), cx.theme.c(if selected { "onSecondaryContainer" } else { "onSurface" }), Align::Left);
            sy += SEND_ROW_H + 8.0;
        }
    }
}
