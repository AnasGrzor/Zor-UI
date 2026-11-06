//! The right-click context menu: a views returns `Item`s from `View::context`, `Win::mouse` asks for the
//! menu on a right-click, and the app shows it next to the pointer. It never takes focus, so opening it
//! doesn't close the sidebar it was opened from; the app closes it on the next click elsewhere or Esc.

use crate::{
    app::{Action, Ctx},
    gfx::{alpha, ico, txt, Align, Canvas, Gfx, R},
    ui,
    win::{Btn, Input, View},
};

pub struct Item {
    pub icon: &'static str,
    pub label: String,
    /// Runs on the UI thread when the item is clicked.
    pub run: Box<dyn FnOnce(&mut Ctx)>,
}

impl Item {
    pub fn new(icon: &'static str, label: &str, run: impl FnOnce(&mut Ctx) + 'static) -> Item {
        Item { icon, label: label.into(), run: Box::new(run) }
    }
}

const PAD: f32 = 12.0; // room for the shadow
const ROW: f32 = 36.0;

pub struct Menu { items: Vec<Option<Item>>, labels: Vec<(&'static str, String)> }

impl Menu {
    pub fn new(items: Vec<Item>) -> Menu {
        let labels = items.iter().map(|i| (i.icon, i.label.clone())).collect();
        Menu { items: items.into_iter().map(Some).collect(), labels }
    }
}

impl View for Menu {
    fn size(&self, g: &Gfx, _cx: &Ctx, _scale: f32) -> (f32, f32) {
        let w = self.labels.iter().map(|l| g.measure(&l.1, txt(13.5, 400)).0.ceil()).fold(120.0, f32::max);
        (w + 2.0 * PAD + 60.0, self.labels.len() as f32 * ROW + 16.0 + 2.0 * PAD)
    }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let t = &cx.theme;
        ui::panel(c, cx, R::new(PAD, PAD, c.w - 2.0 * PAD, c.h - 2.0 * PAD), 14.0);
        for (i, (icon, label)) in self.labels.iter().enumerate() {
            let r = R::new(PAD + 6.0, PAD + 8.0 + i as f32 * ROW, c.w - 2.0 * PAD - 12.0, ROW);
            if c.hit(r, i as u32 + 1) { c.rect(r, 10.0, alpha(t.c("onSurface"), 0.1)); }
            c.icon(icon, R::new(r.x + 8.0, r.y, 20.0, r.h), ico(19.0, false), t.c("onSurfaceVariant"));
            c.text(label, R::new(r.x + 38.0, r.y, r.w - 44.0, r.h), txt(13.5, 400), t.c("onSurface"), Align::Left);
        }
    }

    fn input(&mut self, ev: Input, cx: &mut Ctx) {
        if let Input::Click(id, Btn::Left) = ev {
            if let Some(item) = self.items.get_mut(id as usize - 1).and_then(Option::take) { (item.run)(cx); }
            cx.actions.push(Action::CloseMenu);
        }
    }

    /// Asks the app's frame timer to run: it polls for the click or Esc that dismisses the menu.
    fn frame_ms(&self) -> u32 { 16 }
}
