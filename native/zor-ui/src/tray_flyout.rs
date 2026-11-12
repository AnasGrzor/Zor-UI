//! The tray flyout: the icons the bar's tray leaves out ("bar.trayVisible"), in a small grid that
//! opens from the chevron, like the Windows taskbar's hidden icons. Clicks on an icon leave it open;
//! the app closes it on a click outside (`App::outside_check`).

use crate::{
    app::Ctx,
    bar::{rgba_to_pixmap, tray_icons},
    gfx::{alpha, Canvas, Gfx, Pixmap, R},
    win::{Input, View},
};
use std::{collections::HashMap, sync::Arc};

const COLS: usize = 4;
const CELL: f32 = 38.0;
const PAD: f32 = 8.0;
const MARGIN: f32 = 6.0; // room for the shadow around the card
const ICON: f32 = 20.0;

#[derive(Default)]
pub struct TrayFlyout {
    hover: u32,
    /// Tooltip of each icon, in grid order, as of the last paint.
    tips: Vec<String>,
    /// Per icon: the image it was drawn from (kept alive, so its address can't be reused), the px size, the pixmap.
    px: HashMap<String, (Arc<(Vec<u8>, u32, u32)>, u32, Pixmap)>,
}

/// Window size (CSS px) for `n` icons: up to `COLS` per row.
fn size_for(n: usize) -> (f32, f32) {
    let (cols, rows) = (n.clamp(1, COLS), n.max(1).div_ceil(COLS));
    (cols as f32 * CELL + 2.0 * (PAD + MARGIN), rows as f32 * CELL + 2.0 * (PAD + MARGIN))
}

/// Where icon `i` sits, relative to the window.
fn cell(i: usize) -> R {
    R::new(MARGIN + PAD + (i % COLS) as f32 * CELL, MARGIN + PAD + (i / COLS) as f32 * CELL, CELL, CELL)
}

impl View for TrayFlyout {
    fn size(&self, _g: &Gfx, cx: &Ctx, _scale: f32) -> (f32, f32) { size_for(tray_icons(cx).1.len()) }

    fn key(&self, cx: &Ctx) -> String { cx.ver.to_string() }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        let layer0 = crate::ui::layer0(cx);
        let card = R::new(MARGIN, MARGIN, c.w - 2.0 * MARGIN, c.h - 2.0 * MARGIN);
        c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(card, 18.0, col) });
        c.rect(card, 18.0, alpha(t.c("outlineVariant"), 0.45));
        c.rect(R::new(card.x + 1.0, card.y + 1.0, card.w - 2.0, card.h - 2.0), 17.0, layer0);
        let px = (ICON * c.scale).round() as u32;
        let icons = tray_icons(cx).1;
        self.tips = icons.iter().map(|i| i.tooltip.clone()).collect();
        // drop the images of icons that have gone, so the cache stays one entry per icon
        self.px.retain(|id, _| icons.iter().any(|i| &i.id == id));
        for (i, ic) in icons.iter().enumerate() {
            let Some(rgba) = ic.rgba.as_ref() else { continue };
            if !self.px.get(&ic.id).is_some_and(|e| Arc::ptr_eq(&e.0, rgba) && e.1 == px) {
                if let Some(p) = rgba_to_pixmap(&rgba.0, rgba.1, rgba.2, px) { self.px.insert(ic.id.clone(), (rgba.clone(), px, p)); }
            }
            let r = cell(i);
            if c.hit(r, 1 + i as u32) && self.hover == 1 + i as u32 { c.rect(r, 10.0, alpha(t.c("onSurface"), 0.08)); }
            if let Some((_, _, p)) = self.px.get(&ic.id) { c.image(p, R::new(r.x + (CELL - ICON) / 2.0, r.y + (CELL - ICON) / 2.0, ICON, ICON), 1.0); }
        }
    }

    fn hover_popup(&self) -> Option<(u32, R, String)> {
        let i = (self.hover as usize).checked_sub(1)?;
        let tip = self.tips.get(i).filter(|t| !t.is_empty())?;
        Some((self.hover, cell(i), tip.clone()))
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        match ev {
            Input::Hover(id) => self.hover = id,
            Input::Click(id, b) => {
                let Some(i) = (id as usize).checked_sub(1) else { return };
                let Some(ic) = tray_icons(cx).1.get(i).cloned() else { return };
                crate::tray::click_btn(&ic.id, b);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_grows_by_rows_after_four_icons() {
        let base = 2.0 * (PAD + MARGIN);
        assert_eq!(size_for(1), (CELL + base, CELL + base));
        assert_eq!(size_for(4), (4.0 * CELL + base, CELL + base));
        assert_eq!(size_for(5), (4.0 * CELL + base, 2.0 * CELL + base));
        assert_eq!(size_for(0), size_for(1));
    }

    #[test]
    fn hovering_an_icon_shows_its_tooltip() {
        let mut f = TrayFlyout { tips: vec!["Discord".into(), String::new()], ..Default::default() };
        assert!(f.hover_popup().is_none()); // nothing hovered
        f.hover = 1;
        let (id, r, tip) = f.hover_popup().unwrap();
        assert_eq!((id, tip.as_str(), r.x), (1, "Discord", cell(0).x));
        f.hover = 2; // an icon without a tooltip
        assert!(f.hover_popup().is_none());
        f.hover = 9; // past the last icon
        assert!(f.hover_popup().is_none());
    }

    #[test]
    fn cells_wrap_at_the_column_count() {
        let (a, b) = (cell(0), cell(COLS));
        assert_eq!((a.x, b.x), (b.x, a.x));
        assert_eq!(b.y, a.y + CELL);
        assert_eq!(cell(COLS - 1).x, a.x + (COLS - 1) as f32 * CELL);
    }
}
