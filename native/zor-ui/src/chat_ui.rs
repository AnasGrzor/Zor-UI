//! The chat drawing every chat in the app shares: the message list (`transcript`), the input bar (`input`), Markdown (`md`),
//! and the small pieces around them. The sidebar's AI tab and the buddy chat window both draw with it, so the two can't drift
//! apart. It lives here, not in ui.rs, so the general widget file stays small.

pub mod input;
pub mod md;
pub mod models;
pub mod transcript;

use crate::{app::{self, Ctx}, gfx::{alpha, Canvas, R}};
use std::sync::{Arc, Mutex};

/// Repaint everything (a thread delivered something).
pub fn wake() { app::post(Box::new(|_| {})); }

/// Where a thread leaves its answers for a view to pick up at the next paint.
pub struct Inbox<T>(Arc<Mutex<Vec<T>>>);
impl<T> Clone for Inbox<T> { fn clone(&self) -> Self { Inbox(self.0.clone()) } }
impl<T> Inbox<T> {
    pub fn new() -> Inbox<T> { Inbox(Arc::new(Mutex::new(vec![]))) }
    pub fn push(&self, v: T) { if let Ok(mut q) = self.0.lock() { q.push(v); } }
    pub fn take(&self) -> Vec<T> { self.0.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default() }
}

/// Three pulsing dots centred on (x, y): the "busy" mark (`.dots` / `.spinner`).
pub fn dots(c: &mut Canvas, cx: &Ctx, x: f32, y: f32) {
    let phase = crate::hub::now_ms() / 1000.0;
    for i in 0..3 {
        let a = 0.2 + 0.8 * (((phase - i as f64 * 0.15) * std::f64::consts::TAU).sin() * 0.5 + 0.5) as f32;
        c.circle(x + (i as f32 - 1.0) * 10.0, y, 3.0, alpha(cx.theme.c("primary"), a));
    }
}

/// `.field`: bordered input surface.
pub fn field(c: &mut Canvas, cx: &Ctx, r: R, focus: bool) {
    let edge = if focus { cx.theme.c("primary") } else { cx.theme.c("outlineVariant") };
    c.rect(r, 17.0, edge);
    c.rect(R::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0), 16.0, cx.theme.c("surfaceContainer"));
}
