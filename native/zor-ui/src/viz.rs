//! The desktop audio visualizer (visualizer.html, lib/viz.js drawBars 'desktop' style): a click-through window across the
//! bottom 200 px of the primary monitor, fed by the bridge's `visualizer` stream (one line of levels per frame).

use crate::{
    app::{self, App, Ctx},
    gfx::{alpha, Canvas, Gfx, R},
    hub, proc, util,
    win::{View, Win, Z},
};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};

const H: f32 = 200.0;
/// The window's handle and its (bars, x, y, width) so a config or monitor change rebuilds it.
static CUR: Mutex<Option<(isize, (u64, i32, i32, i32))>> = Mutex::new(None);

#[derive(Default)]
pub struct Shared {
    pub levels: Mutex<Vec<f32>>,
    /// The frame timer is running; the stream wakes the window only when it isn't.
    pub awake: AtomicBool,
}

/// Starts the bridge's `visualizer` stream (`bars` levels per line) into `sh`, waking the app when sound starts.
pub fn listen(cfg: &crate::conf::Config, bars: u64, sh: Arc<Shared>) -> Option<proc::Stream> {
    proc::stream(cfg, "visualizer", &[&bars.to_string()], move |line| {
        let Some(l) = parse_levels(line) else { return };
        let loud = l.iter().any(|&v| v > 0.0);
        if let Ok(mut g) = sh.levels.lock() { *g = l; }
        if loud && !sh.awake.swap(true, Ordering::Relaxed) { app::post(Box::new(|_| {})); } // starts the frame timer
    })
}

pub struct Viz {
    mon: (i32, i32, i32),
    sh: Arc<Shared>,
    peaks: Vec<f32>,
    last: f64,
    active: bool,
    _stream: Option<proc::Stream>,
}

/// One stream line -> levels (lines of 4 or fewer numbers are ignored).
pub fn parse_levels(line: &str) -> Option<Vec<f32>> {
    let v: Vec<f32> = line.split_whitespace().map(|s| s.parse().ok()).collect::<Option<_>>()?;
    (v.len() > 4).then_some(v)
}

/// Made-up levels for `n` bars (`--render`, and the Settings preview).
pub fn sample_levels(n: usize) -> Vec<f32> { (0..n).map(|i| ((i as f32 * 0.35).sin() * 0.4 + 0.5) * (1.0 - i as f32 / 90.0)).collect() }

/// What drawBars() draws per bar at 0-1 `level` in a canvas `h` high: the bar's height and its peak cap's top (distance from the bottom).
pub fn bar_geometry(level: f32, peak: f32, h: f32) -> (f32, f32) { ((level.clamp(0.0, 1.0) * h * 0.92).max(3.0), peak * h * 0.92 + 6.0) }

impl Viz {
    /// `mon`: the monitor's x, y and width in physical px, like `Bar::new`.
    pub fn new(mon: (i32, i32, i32)) -> Viz { Viz { mon, sh: Default::default(), peaks: vec![], last: 0.0, active: false, _stream: None } }

    fn start(&mut self, cfg: &crate::conf::Config, bars: u64) {
        self._stream = listen(cfg, bars, self.sh.clone());
    }
}

impl View for Viz {
    fn size(&self, _: &Gfx, _: &Ctx, scale: f32) -> (f32, f32) { (self.mon.2 as f32 / scale, H) }

    fn frame_ms(&self) -> u32 { if self.active { 16 } else { 0 } }

    fn paint(&mut self, c: &mut Canvas, cx: &Ctx) {
        #[cfg(debug_assertions)]
        let sample = std::env::var_os("ZORUI_SAMPLE").is_some(); // `--render`: made-up levels
        #[cfg(not(debug_assertions))]
        let sample = false;
        let levels: Vec<f32> = if sample {
            sample_levels(64)
        } else { self.sh.levels.lock().map(|g| g.clone()).unwrap_or_default() };
        let levels = if levels.is_empty() { vec![0.0; cx.cfg.get("visualizer.bars").as_u64().unwrap_or(48).clamp(8, 256) as usize] } else { levels };
        let now = hub::now_ms();
        let dt = if self.last == 0.0 { 16.7 } else { (now - self.last).min(100.0) };
        self.last = now;
        if self.peaks.len() != levels.len() { self.peaks = vec![0.0; levels.len()]; }
        let n = levels.len();
        let (w, h) = (c.w, c.h);
        let primary = cx.cfg.viz_color("visualizer.desktopColor").unwrap_or_else(|| cx.theme.c("primary"));
        let gap = (w / n.max(1) as f32 * 0.22).max(2.0);
        let bw = (w / n.max(1) as f32 - gap).max(1.0);
        for i in 0..n {
            let v = levels[i].clamp(0.0, 1.0);
            self.peaks[i] = v.max(self.peaks[i] - 0.008 * dt as f32 / 16.7); // falls 0.008 per 60 Hz frame
            let (bh, cap) = bar_geometry(v, self.peaks[i], h);
            let x = i as f32 * (bw + gap) + gap / 2.0;
            let r = (bw / 2.0).min(4.0);
            // linear-gradient(to top, primary, primary 40 %): strips of the colour at their height
            let step = 10.0;
            let mut y = h;
            while y > h - bh + r * 2.0 {
                let top = (y - step).max(h - bh + r * 2.0);
                let mid = (h - (y + top) / 2.0) / h;
                c.rect(R::new(x, top, bw, y - top), 0.0, alpha(primary, 1.0 - 0.6 * mid));
                y = top;
            }
            let tr = bh.min(r * 2.0 + 0.5);
            c.rect(R::new(x, h - bh, bw, tr), r.min(tr / 2.0), alpha(primary, 1.0 - 0.6 * (bh / h)));
            c.rect(R::new(x, h - cap, bw, 2.0), 0.0, primary);
        }
        self.active = !sample && (levels.iter().any(|&v| v > 0.0) || self.peaks.iter().any(|&p| p > 0.0));
        if !self.active { self.sh.awake.store(false, Ordering::Relaxed); }
    }
}

/// Opens / closes the visualizer window to match config.json (called where `desk::sync` is).
pub fn sync(app: &mut App) {
    let want = app.cx.native("visualizer") && app.cx.cfg.bool("visualizer.desktop");
    let bars = app.cx.cfg.get("visualizer.bars").as_u64().unwrap_or(48).clamp(8, 256);
    let (mons, _) = util::monitors();
    let mon = mons.iter().find(|m| m.left == 0 && m.top == 0).or(mons.first()).copied();
    let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
    let spec = mon.map(|m| (bars, m.left, m.bottom - (H * scale).round() as i32, m.right - m.left));
    let mut cur = CUR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((h, old)) = *cur {
        if want && Some(old) == spec { return; }
        app.wins.retain(|w| w.hwnd.0 as isize != h); // dropping the window drops the stream, which kills the bridge process
        *cur = None;
    }
    let (true, Some(m), Some((_, x, y, w))) = (want, mon, spec) else { return };
    let mut v = Viz::new((m.left, y, w));
    v.start(&app.cx.cfg, bars);
    let Ok(mut win) = Win::new(Box::new(v), "Zor UI / visualizer", x, y, Z::Bottom, true) else { return };
    win.update(&app.g, &app.cx, true);
    win.move_to(x, y);
    win.show();
    *cur = Some((win.hwnd.0 as isize, spec.unwrap_or_default()));
    app.wins.push(win);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lines() {
        assert_eq!(parse_levels("0.10 0.50 1.00 0.00 0.25\n"), Some(vec![0.1, 0.5, 1.0, 0.0, 0.25]));
        assert_eq!(parse_levels("0.1 0.2"), None); // too short, like `v.length > 4`
        assert_eq!(parse_levels("a b c d e f"), None);
    }
    #[test]
    fn sample_levels_are_valid_levels() {
        let l = sample_levels(64);
        assert_eq!(l.len(), 64);
        assert!(l.iter().all(|v| (0.0..=1.0).contains(v)) && l.iter().any(|&v| v > 0.3));
    }
    #[test]
    fn bars_are_at_least_three_px() {
        assert_eq!(bar_geometry(0.0, 0.0, 200.0).0, 3.0);
        assert_eq!(bar_geometry(1.0, 0.5, 200.0), (184.0, 98.0));
        assert_eq!(bar_geometry(2.0, 0.0, 200.0).0, 184.0); // clamped
    }
}
