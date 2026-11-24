//! The model picker every chat shares: a chip with the model's name that opens a dropdown of the models, grouped by provider,
//! each provider marked with whether its key is saved. The list comes from the bridge's `ai-models` (`set`); the caller saves
//! the pick (`click` returns its id).

use crate::{
    app::Ctx,
    gfx::{alpha, ico, txt, Align, Canvas, R},
    ui::{self, Scroll},
};
use serde_json::Value;

/// Hit ids: the chip, the open menu's panel, and the rows (`ROW0` plus the model's index). Callers keep clear of these.
pub const CHIP: u32 = 9_010;
pub const MENU: u32 = 9_011;
pub const ROW0: u32 = 9_100;
const ROWS_MAX: u32 = 400;

/// The chip's height and the space inside it; the menu's rows, its provider headings, and its inside margin.
pub const CHIP_H: f32 = 32.0;
const CHIP_X: f32 = 14.0;
const ROW_H: f32 = 46.0;
const HEAD_H: f32 = 28.0;
const MENU_PAD: f32 = 6.0;
const MENU_R: f32 = 23.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Model { pub id: String, pub name: String, pub provider: String }

#[derive(Default)]
pub struct ModelMenu {
    pub models: Vec<Model>,
    /// Which providers have a key saved (`{"gemini": true, ...}`).
    pub keys: Value,
    pub open: bool,
    scroll: Scroll,
    dims: (f32, f32),
}

fn capitalize(s: &str) -> String { let mut c = s.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }

/// The models in provider groups, in the order the providers first appear: (provider, indexes into `models`).
pub fn groups(models: &[Model]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = vec![];
    for (i, m) in models.iter().enumerate() {
        match out.iter_mut().find(|g| g.0 == m.provider) { Some(g) => g.1.push(i), None => out.push((m.provider.clone(), vec![i])) }
    }
    out
}

impl ModelMenu {
    /// Takes the bridge's `ai-models` answer: `{"models": [{id, name, provider}], "keys": {...}}`.
    pub fn set(&mut self, v: &Value) {
        self.models = v["models"].as_array().into_iter().flatten().map(|m| Model {
            id: m["id"].as_str().unwrap_or("").into(), name: m["name"].as_str().unwrap_or("").into(), provider: m["provider"].as_str().unwrap_or("").into(),
        }).collect();
        self.keys = v["keys"].clone();
    }

    /// The model `id` as the list knows it, or a stand-in with the id as its name.
    pub fn info(&self, id: &str) -> Model {
        self.models.iter().find(|m| m.id == id).cloned().unwrap_or(Model { id: id.into(), name: id.into(), provider: "?".into() })
    }

    /// The chip at (x, y), at most `max_w` wide, showing `name`. Returns its rectangle.
    pub fn chip(c: &mut Canvas, cx: &Ctx, x: f32, y: f32, max_w: f32, name: &str) -> R {
        let t = &cx.theme;
        let nw = c.g.measure(name, txt(13.0, 400)).0.ceil();
        let cw = (CHIP_X + 18.0 + 6.0 + nw + 6.0 + 18.0 + CHIP_X).min(max_w);
        let chip = R::new(x, y, cw, CHIP_H);
        c.rect(chip, CHIP_H / 2.0, t.c("secondaryContainer"));
        if c.hit(chip, CHIP) { c.rect(chip, CHIP_H / 2.0, alpha(t.c("onSecondaryContainer"), crate::tokens::HOVER_A)); }
        let fg = t.c("onSecondaryContainer");
        c.icon("neurology", R::new(chip.x + CHIP_X, chip.y, 18.0, CHIP_H), ico(18.0, false), fg);
        c.text(name, R::new(chip.x + CHIP_X + 24.0, chip.y, cw - 2.0 * (CHIP_X + 24.0), CHIP_H), txt(13.0, 400), fg, Align::Left);
        c.icon("expand_more", R::new(chip.x + cw - CHIP_X - 18.0, chip.y, 18.0, CHIP_H), ico(18.0, false), fg);
        chip
    }

    /// The open menu: `at` gives its x, y and width; it grows down to at most `max_h`. `cur` is ticked.
    pub fn paint(&mut self, c: &mut Canvas, cx: &Ctx, at: R, max_h: f32, cur: &str) {
        if !self.open { return; }
        let t = &cx.theme;
        let groups = groups(&self.models);
        let content: f32 = if groups.is_empty() { 40.0 } else { groups.iter().map(|g| HEAD_H + g.1.len() as f32 * ROW_H).sum() };
        let mr = R::new(at.x, at.y, at.w, (content + 2.0 * MENU_PAD).min(max_h));
        c.shadow(6.0, 24.0, 0.35, |c, col| if let Some(col) = col { c.rect(mr, MENU_R, col) });
        c.rect(mr, MENU_R, t.c("surfaceContainerHigh"));
        c.hit(mr, MENU);
        let inner = R::new(mr.x + MENU_PAD, mr.y + MENU_PAD, mr.w - 2.0 * MENU_PAD, mr.h - 2.0 * MENU_PAD);
        self.dims = (content, inner.h);
        let (models, keys) = (&self.models, &self.keys);
        self.scroll.region(c, cx, inner, content, |c, top| {
            if groups.is_empty() { c.text("Loading models\u{2026}", R::new(inner.x + 12.0, top, inner.w, 40.0), txt(13.0, 400), t.c("onSurfaceVariant"), Align::Left); }
            let mut y = top;
            for (p, idxs) in &groups {
                let head = capitalize(p);
                c.text(&head, R::new(inner.x + 12.0, y + 10.0, 200.0, 16.0), txt(11.5, 400), t.c("onSurfaceVariant"), Align::Left);
                if p != "ollama" {
                    let ok = keys[p.as_str()].as_bool().unwrap_or(false);
                    let hw = c.g.measure(&head, txt(11.5, 400)).0;
                    c.text(if ok { " \u{00B7} key set" } else { " \u{00B7} no key" }, R::new(inner.x + 12.0 + hw, y + 10.0, 120.0, 16.0), txt(11.5, 400), if ok { t.c("primary") } else { t.c("error") }, Align::Left);
                }
                y += HEAD_H;
                for &i in idxs {
                    let m = &models[i];
                    let b = R::new(inner.x, y, inner.w - ui::bar_room(content, inner.h), ROW_H);
                    if y + ROW_H >= inner.y && y <= inner.y + inner.h {
                        let sel = m.id == cur;
                        if sel { c.rect(b, 17.0, t.c("secondaryContainer")); }
                        if c.hit(b, ROW0 + i as u32) && !sel { c.rect(b, 17.0, alpha(t.c("onSurface"), crate::tokens::HOVER_A)); }
                        c.text(&m.name, R::new(b.x + 12.0, b.y + 6.0, b.w - 24.0, 18.0), txt(13.5, 400), t.c("onSurface"), Align::Left);
                        c.text(&m.id, R::new(b.x + 12.0, b.y + 25.0, b.w - 24.0, 14.0), txt(11.0, 400), t.c("onSurfaceVariant"), Align::Left);
                    }
                    y += ROW_H;
                }
            }
        });
    }

    pub fn wheel(&mut self, d: i32) { self.scroll.wheel(d, self.dims.0, self.dims.1); }

    /// A click while the menu may be open. The chip opens or closes it; a row picks that model (its id is returned and the
    /// menu closes); a click on the panel does nothing; any other click closes the menu. True in `.0` when the click was the
    /// menu's (the caller then ignores it).
    pub fn click(&mut self, id: u32) -> (bool, Option<String>) {
        match id {
            CHIP => { self.open = !self.open; self.scroll = Scroll::default(); (true, None) }
            MENU if self.open => (true, None),
            i if self.open && (ROW0..ROW0 + ROWS_MAX).contains(&i) => {
                self.open = false;
                (true, self.models.get((i - ROW0) as usize).map(|m| m.id.clone()))
            }
            _ if self.open => { self.open = false; (true, None) }
            _ => (false, None),
        }
    }

    /// True for the menu's background spots, which show no hand.
    pub fn is_background(id: u32) -> bool { id == MENU }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> ModelMenu {
        let mut m = ModelMenu::default();
        m.set(&serde_json::json!({ "models": [
            { "id": "gemini-2.5-flash", "name": "Gemini 2.5 Flash", "provider": "gemini" },
            { "id": "claude-sonnet-5-5", "name": "Claude Sonnet 5.5", "provider": "anthropic" },
            { "id": "gemini-2.5-pro", "name": "Gemini 2.5 Pro", "provider": "gemini" },
        ], "keys": { "gemini": true } }));
        m
    }

    #[test]
    fn models_group_by_provider_in_first_seen_order() {
        let g = groups(&menu().models);
        assert_eq!(g, vec![("gemini".to_string(), vec![0, 2]), ("anthropic".to_string(), vec![1])]);
        assert_eq!(menu().info("nope").name, "nope", "an unknown model shows its id");
    }

    #[test]
    fn the_chip_opens_a_row_picks_and_a_click_outside_closes() {
        let mut m = menu();
        assert_eq!(m.click(CHIP), (true, None));
        assert!(m.open);
        assert_eq!(m.click(MENU), (true, None), "the panel itself does nothing");
        assert_eq!(m.click(ROW0 + 1), (true, Some("claude-sonnet-5-5".into())));
        assert!(!m.open, "a pick closes it");
        assert_eq!(m.click(ROW0 + 1), (false, None), "a closed menu takes no clicks");
        m.click(CHIP);
        assert_eq!(m.click(5), (true, None), "a click elsewhere closes it");
        assert!(!m.open);
    }
}
