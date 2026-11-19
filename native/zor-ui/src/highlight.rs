//! Screen help's highlight: when the model points at a spot (assist.rs), UI Automation names the control under it, and a
//! click-through box is drawn around that control for a few seconds while the buddy points at it. Apps without UI Automation,
//! or a spot whose control is a whole pane, get no box; the buddy's ring still marks the spot.

use crate::{
    app::{App, Ctx},
    gfx::{alpha, Canvas, Gfx, R},
    win::{View, Win, Z},
};
use std::{sync::Mutex, time::Instant};
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::{
        Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED},
        Threading::GetCurrentProcessId,
    },
    UI::Accessibility::{CUIAutomation, IUIAutomation},
};

/// Seconds the box stays: the buddy's flight (at most 1.4 s) and his stay at the spot (`buddy::POINT_HOLD`, 8 s), rounded down.
const HOLD_S: f32 = 9.0;
/// Room around the control for the box's strokes and glow (CSS px).
const PAD: f32 = 8.0;
/// Ancestors checked above the control under the spot: a button's text or icon is often what the point lands on.
const CLIMB: usize = 4;

/// The box to show (screen px: left, top, right, bottom) and when it was asked for.
static SPOT: Mutex<Option<([i32; 4], Instant)>> = Mutex::new(None);
/// The box window's handle and the request it draws.
static CUR: Mutex<Option<(isize, [i32; 4], Instant)>> = Mutex::new(None);

/// Whether a control's box is worth drawing for a point at `at` on monitor `mon`: it holds the point, is not a sliver,
/// and is not most of the screen (a pane or the whole window says nothing).
pub fn fits(r: [i32; 4], at: (i32, i32), mon: [i32; 4]) -> bool {
    let (w, h, mw, mh) = (r[2] - r[0], r[3] - r[1], mon[2] - mon[0], mon[3] - mon[1]);
    let holds = (r[0]..=r[2]).contains(&at.0) && (r[1]..=r[3]).contains(&at.1);
    holds && w >= 8 && h >= 8 && w * 10 <= mw * 6 && h * 10 <= mh * 4
}

/// The box to draw from the control under the point and its ancestors, nearest first, each with whether you can act on it
/// (a button, a field, a link). The nearest such one that fits wins; else the control itself if it fits.
pub fn pick(chain: &[(bool, [i32; 4])], at: (i32, i32), mon: [i32; 4]) -> Option<[i32; 4]> {
    chain.iter().find(|(acts, r)| *acts && fits(*r, at, mon)).or(chain.first().filter(|(_, r)| fits(*r, at, mon))).map(|c| c.1)
}

/// The box of the control at screen point (x, y), through UI Automation. None for our own windows, apps that don't answer,
/// or a control too big to say anything.
pub fn snap(x: i32, y: i32) -> Option<[i32; 4]> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut e = uia.ElementFromPoint(POINT { x, y }).ok()?;
        if e.CurrentProcessId().ok()? as u32 == GetCurrentProcessId() { return None; }
        let walker = uia.ControlViewWalker().ok()?;
        let mut chain = vec![];
        for _ in 0..=CLIMB {
            if let Ok(r) = e.CurrentBoundingRectangle() {
                let acts = e.CurrentControlType().is_ok_and(|t| crate::drive::role_of(t.0).is_some());
                chain.push((acts, [r.left, r.top, r.right, r.bottom]));
            }
            let Ok(up) = walker.GetParentElement(&e) else { break };
            e = up;
        }
        pick(&chain, (x, y), monitor((x, y)))
    }
}

/// The monitor nearest a screen point (left, top, right, bottom).
fn monitor(at: (i32, i32)) -> [i32; 4] {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { let _ = GetMonitorInfoW(MonitorFromPoint(POINT { x: at.0, y: at.1 }, MONITOR_DEFAULTTONEAREST), &mut mi); }
    let m = mi.rcMonitor;
    [m.left, m.top, m.right, m.bottom]
}

/// Shows a box around a control already known (screen help's list) if it `fits`; false when it doesn't.
pub fn show_fit(r: [i32; 4], at: (i32, i32)) -> bool {
    let ok = fits(r, at, monitor(at));
    if ok { show(r); }
    ok
}

/// Shows a box around `r` (screen px), replacing any box still up.
pub fn show(r: [i32; 4]) {
    if let Ok(mut s) = SPOT.lock() { *s = Some((r, Instant::now())); }
    crate::app::post(Box::new(|_| {})); // the app makes the window in `sync`
}

/// The box's opacity `el` seconds after it was asked for: in over 0.25 s, out over the last 0.4 s.
pub fn fade(el: f32) -> f32 { (el / 0.25).min(1.0).min((HOLD_S - el) / 0.4).max(0.0) }

struct Outline { r: [i32; 4], t0: Instant, gone: bool }

impl View for Outline {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) {
        ((self.r[2] - self.r[0]) as f32 / scale + 2.0 * PAD, (self.r[3] - self.r[1]) as f32 / scale + 2.0 * PAD)
    }
    fn frame_ms(&self) -> u32 { if self.gone { 500 } else { 33 } }
    fn key(&self, _: &Ctx) -> String { format!("{}", self.t0.elapsed().as_millis() / 33) }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        let el = self.t0.elapsed().as_secs_f32();
        let a = fade(el);
        if el > HOLD_S && !std::mem::replace(&mut self.gone, true) { crate::app::post(Box::new(|_| {})); } // `sync` closes it
        if a <= 0.0 { return; }
        let (col, pulse) = (cx.theme.c("primary"), (el * 4.0).sin() * 0.5 + 0.5);
        let g = 1.0 + 1.5 * pulse;
        let r = R::new(PAD - g, PAD - g, c.w - 2.0 * (PAD - g), c.h - 2.0 * (PAD - g));
        let rad = 8.0f32.min(r.h / 2.0);
        c.rect(r, rad, alpha(col, 0.10 * a)); // a light tint over the control
        // three strokes, as on the buddy's ring, so the box shows on light and dark pages alike
        c.outline(r, rad, 5.0, alpha(0xFF101018, 0.55 * a));
        c.outline(r, rad, 3.4, alpha(0xFFFFFFFF, 0.95 * a));
        c.outline(r, rad, 1.8, alpha(col, a));
    }
}

/// Opens, replaces or closes the box window to match the last request. Cheap when nothing changed.
pub fn sync(app: &mut App) {
    let want = SPOT.lock().ok().and_then(|s| *s).filter(|(_, t0)| t0.elapsed().as_secs_f32() <= HOLD_S);
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if cur.map(|c| (c.1, c.2)) == want { return; }
    if let Some((h, ..)) = cur.take() { app.wins.retain(|w| w.hwnd.0 as isize != h); }
    let Some((r, t0)) = want else { return };
    let Ok(mut w) = Win::new(Box::new(Outline { r, t0, gone: false }), "Zor UI / highlight", r[0], r[1], Z::Top, true) else { return };
    crate::drive::hide_from_capture(w.hwnd); // the next screen question sees the app, not the box
    w.update(&app.g, &app.cx, true);
    let pad = (PAD * w.scale()).round() as i32;
    w.move_to(r[0] - pad, r[1] - pad);
    w.show();
    w.pin();
    *cur = Some((w.hwnd.0 as isize, r, t0));
    app.wins.push(w);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: [i32; 4] = [0, 0, 1920, 1080];

    #[test]
    fn a_box_must_hold_the_point_and_be_neither_a_sliver_nor_a_pane() {
        assert!(fits([100, 100, 180, 132], (140, 116), MON), "a button");
        assert!(!fits([100, 100, 180, 132], (300, 116), MON), "the point is elsewhere");
        assert!(!fits([100, 100, 105, 132], (102, 116), MON), "too thin");
        assert!(!fits([0, 0, 1920, 1080], (500, 500), MON), "the whole screen");
        assert!(!fits([0, 40, 1400, 1080], (500, 500), MON), "a pane");
        assert!(fits([200, 50, 1300, 90], (500, 70), MON), "a wide address bar is fine");
    }

    #[test]
    fn the_nearest_control_you_can_act_on_wins_over_its_text() {
        let label = (false, [110, 108, 170, 124]);
        let button = (true, [100, 100, 180, 132]);
        let window = (false, [0, 0, 1920, 1080]);
        assert_eq!(pick(&[label, button, window], (140, 116), MON), Some(button.1));
        assert_eq!(pick(&[label, window], (140, 116), MON), Some(label.1), "no button: the text itself");
        assert_eq!(pick(&[window], (140, 116), MON), None, "only a pane: no box");
        assert_eq!(pick(&[], (140, 116), MON), None);
        let huge_button = (true, [0, 0, 1900, 1000]);
        assert_eq!(pick(&[label, huge_button], (140, 116), MON), Some(label.1), "an act-on control that is a pane doesn't count");
    }

    #[test]
    fn the_box_fades_in_and_out_within_its_time() {
        assert_eq!(fade(0.0), 0.0);
        assert_eq!(fade(1.0), 1.0);
        assert!(fade(HOLD_S - 0.2) < 1.0 && fade(HOLD_S - 0.2) > 0.0);
        assert_eq!(fade(HOLD_S + 1.0), 0.0);
    }
}
