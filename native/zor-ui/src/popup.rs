//! Popups (sidebars, overview, island panel, settings): which view each name opens, where it goes
//! (the zpack.json presets) and the open window's bookkeeping. App::open does the opening.

use crate::{
    bar::Edge,
    island::Island, overview::Overview, settings::Settings, sidebar_left::SidebarLeft, sidebar_right::SidebarRight,
    win::{View, Win},
};
use windows::Win32::Foundation::RECT;


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
    // "settings:presets" opens the settings on that page (used by `--render`); an unknown page keeps the first
    if let Some(page) = name.strip_prefix("settings:") { return Some(Box::new(Settings::new(area).on_page(page))); }
    Some(match name {
        // island.version picks the panel: "new" is the activity model in island.rs, "current" the committed one in island_legacy.rs
        "island-panel" if crate::conf::Config::load().str("island.version") == "current" => Box::new(crate::island_legacy::Island::new(area)),
        "island-panel" => Box::new(Island::new(area)),
        "tray-flyout" => Box::new(crate::tray_flyout::TrayFlyout::default()),
        "overview" => Box::new(Overview::new(area)),
        "sidebar-left" => Box::new(SidebarLeft::new(area)),
        "sidebar-right" => Box::new(SidebarRight::new(area)),
        "settings" => Box::new(Settings::new(area)),
        "keybinds" => Box::new(Settings::new(area).on_page("keys")),
        crate::buddy_chat::NAME => Box::new(crate::buddy_chat::BuddyChat::new(area)),
        _ => return None,
    })
}

/// zpack.json preset sizes for a monitor of `w` x `h` CSS px with the bar on `edge`.
pub fn area(name: &str, w: f32, h: f32, edge: Edge) -> (f32, f32) {
    let (bar_w, bar_h) = if edge.vertical() { (edge.thick(), 0.0) } else { (0.0, edge.thick()) };
    let portrait = h > w;
    let name = if name.starts_with("settings:") { "settings" } else { name };
    match name {
        "sidebar-left" | "sidebar-right" => (480.0_f32.min(w - bar_w), (h - bar_h).max(200.0)),
        // a tall screen has room for rows, not for a wider window
        "overview" => ((w * if portrait { 0.94 } else { 0.82 }).round(), (h * if portrait { 0.8 } else { 0.74 }).round()),
        // the pages need about 900 px; a narrow screen gets all it has
        "settings" | "keybinds" => ((w * 0.64).max(900.0_f32.min(w - 24.0)).round(), (h * 0.8).min(900.0).round()), // a portrait screen would stretch it into a tall empty column
        "island-panel" => ((664.0_f32).min(w - bar_w - 24.0), (h - bar_h - 24.0).max(1.0)), // monitor bounds include the shadow and a margin below the bar
        // a chat reads best at about the width of a page; a small screen gets what it has, never less than the window's minimum
        crate::buddy_chat::NAME => ((w * 0.6).clamp(720.0, 1100.0).min((w - bar_w - 24.0).max(720.0)).round(), (h * 0.78).clamp(480.0, 900.0).round()),
        _ => (400.0, 300.0),
    }
}

/// Top-left (physical px) of a window `px` big on monitor `mon` at DPI scale `s`, with the bar on `edge`.
/// `at` is where the island panel hangs from.
pub fn spot(name: &str, mon: RECT, s: f32, px: (i32, i32), at: Option<(i32, i32)>, edge: Edge) -> (i32, i32) {
    let free = {
        let t = (edge.thick() * s).round() as i32;
        match edge {
            Edge::Top => RECT { top: mon.top + t, ..mon },
            Edge::Bottom => RECT { bottom: mon.bottom - t, ..mon },
            Edge::Left => RECT { left: mon.left + t, ..mon },
            Edge::Right => RECT { right: mon.right - t, ..mon },
        }
    };
    let centre = (mon.left + (mon.right - mon.left - px.0) / 2, mon.top + (mon.bottom - mon.top - px.1) / 2);
    let within = |v: i32, lo: i32, hi: i32| v.clamp(lo, hi.max(lo));
    match name {
        "sidebar-left" => (free.left, free.top),
        "sidebar-right" => (free.right - px.0, free.top),
        "island-panel" | "tray-flyout" => {
            let (ax, ay) = at.unwrap_or(((free.left + free.right) / 2, free.top));
            match edge {
                Edge::Top => (within(ax - px.0 / 2, mon.left, mon.right - px.0), within(ay, free.top, free.bottom - px.1)),
                Edge::Bottom => (within(ax - px.0 / 2, mon.left, mon.right - px.0), within(ay - px.1, free.top, free.bottom - px.1)),
                Edge::Left => (ax, within(ay - px.1 / 2, mon.top, mon.bottom - px.1)),
                Edge::Right => (ax - px.0, within(ay - px.1 / 2, mon.top, mon.bottom - px.1)),
            }
        }
        _ => centre,
    }
}

/// Where the island panel's morph starts, in window px: a pill-sized shape on the window's face toward the bar,
/// centred on the pill (`at`, the anchor `spot` used; the window's middle without one). `pos` is the window's
/// top-left and `win` its size, both physical px.
pub fn grow_from(edge: Edge, at: Option<(i32, i32)>, pos: (i32, i32), win: (i32, i32), pill: (i32, i32)) -> crate::gfx::R {
    let (ax, ay) = at.unwrap_or((pos.0 + win.0 / 2, pos.1 + win.1 / 2));
    let (w, h) = (pill.0.min(win.0) as f32, pill.1.min(win.1) as f32);
    let x = ((ax - pos.0) as f32 - w / 2.0).clamp(0.0, win.0 as f32 - w);
    let y = ((ay - pos.1) as f32 - h / 2.0).clamp(0.0, win.1 as f32 - h);
    match edge {
        Edge::Top => crate::gfx::R::new(x, 0.0, w, h),
        Edge::Bottom => crate::gfx::R::new(x, win.1 as f32 - h, w, h),
        Edge::Left => crate::gfx::R::new(0.0, y, w, h),
        Edge::Right => crate::gfx::R::new(win.0 as f32 - w, y, w, h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MON: RECT = RECT { left: -1920, top: 0, right: 0, bottom: 1080 };

    #[test]
    fn settings_page_names() {
        assert!(make("settings:presets", (900.0, 800.0)).is_some() && make("settings:nope", (900.0, 800.0)).is_some());
        assert!(make("bogus:presets", (900.0, 800.0)).is_none());
        assert_eq!(area("settings:island", 1920.0, 1080.0, Edge::Top), area("settings", 1920.0, 1080.0, Edge::Top));
    }

    #[test]
    fn presets() {
        assert_eq!(area("sidebar-left", 1920.0, 1080.0, Edge::Top), (480.0, 1024.0));
        assert_eq!(area("overview", 1920.0, 1080.0, Edge::Top), (1574.0, 799.0));
        assert_eq!(spot("sidebar-right", MON, 1.0, (480, 1000), None, Edge::Top), (-480, 56));
        assert_eq!(spot("settings", MON, 1.0, (1000, 800), None, Edge::Top), (-1460, 140));
        // centred under the island but kept on the monitor
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-960, 60)), Edge::Top), (-1172, 60));
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-1900, 60)), Edge::Top), (-1920, 60));
    }

    #[test]
    fn the_panel_grows_from_the_pill_on_the_face_toward_the_bar() {
        use crate::gfx::R;
        // under a centred pill at the top; the window was kept on the monitor, so the pill sits off-centre in it
        assert_eq!(grow_from(Edge::Top, Some((500, 60)), (278, 60), (444, 717), (200, 32)), R::new(122.0, 0.0, 200.0, 32.0));
        assert_eq!(grow_from(Edge::Top, Some((10, 60)), (0, 60), (444, 717), (200, 32)), R::new(0.0, 0.0, 200.0, 32.0), "kept inside the window");
        assert_eq!(grow_from(Edge::Bottom, Some((500, 1020)), (278, 300), (444, 717), (200, 32)), R::new(122.0, 685.0, 200.0, 32.0));
        assert_eq!(grow_from(Edge::Left, Some((70, 400)), (70, 150), (444, 500), (32, 200)), R::new(0.0, 150.0, 32.0, 200.0));
        assert_eq!(grow_from(Edge::Right, Some((1850, 400)), (1406, 150), (444, 500), (32, 200)), R::new(412.0, 150.0, 32.0, 200.0));
        // no pill (a shortcut): the middle of the bar-side face
        assert_eq!(grow_from(Edge::Top, None, (278, 60), (444, 717), (160, 32)), R::new(142.0, 0.0, 160.0, 32.0));
    }

    #[test]
    fn follows_the_bar_edge() {
        // sidebars use what the bar leaves
        assert_eq!(area("sidebar-left", 1920.0, 1080.0, Edge::Bottom), (480.0, 1024.0));
        assert_eq!(area("sidebar-left", 1920.0, 1080.0, Edge::Left), (480.0, 1080.0));
        assert_eq!(spot("sidebar-left", MON, 1.0, (480, 1000), None, Edge::Bottom), (-1920, 0));
        assert_eq!(spot("sidebar-left", MON, 1.0, (480, 1000), None, Edge::Left), (-1852, 0));
        assert_eq!(spot("sidebar-right", MON, 1.0, (480, 1000), None, Edge::Right), (-548, 0));
        // the island panel hangs off whichever face of the bar looks into the screen
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-960, 1020)), Edge::Bottom), (-1172, 520));
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-1850, 400)), Edge::Left), (-1850, 150));
        assert_eq!(spot("island-panel", MON, 1.0, (424, 500), Some((-70, 100)), Edge::Right), (-494, 0));
    }

    #[test]
    fn island_bounds_follow_the_monitor_and_keep_the_anchor_on_screen() {
        let (w, h) = area("island-panel", 480.0, 640.0, Edge::Top);
        assert!(w <= 480.0 && h <= 640.0 - Edge::Top.thick());
        let (x, y) = spot("island-panel", MON, 1.0, (664, 1000), Some((-960, 100)), Edge::Top);
        assert!(x >= MON.left && x + 664 <= MON.right);
        assert!(y >= 56 && y + 1000 <= MON.bottom);
        let (_, y) = spot("island-panel", MON, 1.0, (664, 1000), Some((-960, 800)), Edge::Bottom);
        assert!(y >= MON.top && y + 1000 <= MON.bottom - 56);
    }

    #[test]
    fn tall_screens_get_wider_windows() {
        // a 1080 x 1920 monitor: settings keeps its 900 px, the overview uses nearly all the width
        assert_eq!(area("settings", 1080.0, 1920.0, Edge::Top), (900.0, 900.0));
        assert_eq!(area("overview", 1080.0, 1920.0, Edge::Top), (1015.0, 1536.0));
        assert_eq!(area("sidebar-left", 500.0, 1000.0, Edge::Left).0, 432.0);
        assert_eq!(area("settings", 800.0, 1280.0, Edge::Top).0, 776.0);
    }
}
