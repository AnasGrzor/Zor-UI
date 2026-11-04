//! Popups (sidebars, overview, island panel, settings): which view each name opens, where it goes
//! (the zpack.json presets) and the open window's bookkeeping. App::open does the opening.

use crate::{
    island::Island, overview::Overview, settings::Settings, sidebar_left::SidebarLeft, sidebar_right::SidebarRight,
    win::{View, Win},
};
use windows::Win32::Foundation::RECT;

/// Names `App::open` / `end4.exe --open` handle natively when config "native" has them.
pub const NAMES: [&str; 5] = ["island-panel", "overview", "sidebar-left", "sidebar-right", "settings"];

/// An open popup.
pub struct Pop {
    pub name: String,
    pub win: Win,
    /// The monitor it opened on (physical px).
    pub mon: RECT,
    /// Where the island panel hangs from (physical px): its top-centre.
    pub at: Option<(i32, i32)>,
    /// Losing focus closes it once this many ms (unix) have passed, unless pinned.
    pub armed_at: f64,
    pub pinned: bool,
    /// Window size at the last placement; a change re-centres it (the island panel grows and shrinks).
    pub px: (i32, i32),
}

/// The view for a popup name. `area` is the CSS-px size from `area()`; views that size themselves ignore it.
pub fn make(name: &str, area: (f32, f32)) -> Option<Box<dyn View>> {
    Some(match name {
        "island-panel" => Box::new(Island::new(area)),
        "overview" => Box::new(Overview::new(area)),
        "sidebar-left" => Box::new(SidebarLeft::new(area)),
        "sidebar-right" => Box::new(SidebarRight::new(area)),
        "settings" => Box::new(Settings::new(area)),
        _ => return None,
    })
}

/// zpack.json preset sizes for a monitor of `w` x `h` CSS px.
pub fn area(name: &str, w: f32, h: f32) -> (f32, f32) {
    match name {
        "sidebar-left" | "sidebar-right" => (480.0, (h - 56.0).max(200.0)),
        "overview" => ((w * 0.82).round(), (h * 0.74).round()),
        "settings" => ((w * 0.64).round(), (h * 0.8).round()),
        "island-panel" => (424.0, 560.0),
        _ => (400.0, 300.0),
    }
}

/// Top-left (physical px) of a window `px` big on monitor `mon` at DPI scale `s`.
pub fn spot(name: &str, mon: RECT, s: f32, px: (i32, i32), at: Option<(i32, i32)>) -> (i32, i32) {
    let top = mon.top + (56.0 * s).round() as i32;
    let centre = (mon.left + (mon.right - mon.left - px.0) / 2, mon.top + (mon.bottom - mon.top - px.1) / 2);
    match name {
        "sidebar-left" => (mon.left, top),
        "sidebar-right" => (mon.right - px.0, top),
        "island-panel" => {
            let (ax, ay) = at.unwrap_or(((mon.left + mon.right) / 2, top));
            ((ax - px.0 / 2).clamp(mon.left, (mon.right - px.0).max(mon.left)), ay)
        }
        _ => centre,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MON: RECT = RECT { left: -1920, top: 0, right: 0, bottom: 1080 };

    #[test]
    fn presets() {
        assert_eq!(area("sidebar-left", 1920.0, 1080.0), (480.0, 1024.0));
        assert_eq!(area("overview", 1920.0, 1080.0), (1574.0, 799.0));
        assert_eq!(spot("sidebar-right", MON, 1.0, (480, 1000), None), (-480, 56));
        assert_eq!(spot("settings", MON, 1.0, (1000, 800), None), (-1460, 140));
        // centred under the island but kept on the monitor
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-960, 60))), (-1172, 60));
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-1900, 60))), (-1920, 60));
    }
}
