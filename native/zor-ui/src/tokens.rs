//! Design tokens: the numbers the shared widgets (`ui.rs`) and the Settings layout (`settings/form.rs`)
//! draw with. Each value equals what was hard-coded there before, so naming them changes no pixels.
//! Change a value here and every widget that uses it follows.

// ---- spacing ----
/// Space between two rows in a Settings card. The divider line sits in the middle of it.
pub const ROW_GAP: f32 = 29.0;
/// Height of the divider line.
pub const HAIRLINE: f32 = 1.0;
/// Where the divider starts inside the row gap, so there is the same air above and below it.
pub const DIVIDER_Y: f32 = (ROW_GAP - HAIRLINE) / 2.0;
/// Settings groups have no card box. Space above the first row and below the last one, and the side inset
/// that lines the rows up with the group's title text.
pub const SECTION_PAD: f32 = 6.0;
pub const SECTION_INSET: f32 = 4.0;
/// Size of a group heading's plain accent icon, and the gap to its title.
pub const HEAD_ICON: f32 = 24.0;
pub const HEAD_GAP: f32 = 10.0;

// ---- corners ----
/// Border width of a panel.
pub const BORDER: f32 = 1.0;
/// Chip height. A chip is a pill, so its corner radius is half of this.
pub const CHIP_H: f32 = 32.0;
pub const CHIP_R: f32 = CHIP_H / 2.0;
/// Slider track corner radius, and the radius of its fill.
pub const SLIDER_R: f32 = 17.0;
pub const SLIDER_FILL_R: f32 = 6.0;

// ---- controls ----
/// Space inside a chip, left and right, and between its icon and label.
pub const CHIP_PAD: f32 = 14.0;
pub const CHIP_ICON: f32 = 18.0;
pub const CHIP_ICON_GAP: f32 = 6.0;
/// The Settings switch: track size, the ring around its fill, and the knob radius off and on.
pub const SWITCH_W: f32 = 52.0;
pub const SWITCH_H: f32 = 32.0;
pub const SWITCH_RING: f32 = 2.0;
pub const KNOB_OFF_R: f32 = 8.0;
pub const KNOB_ON_R: f32 = 12.0;

// ---- states and elevation ----
/// Tint of `onSurface` over a control the pointer is on.
pub const HOVER_A: f32 = 0.08;
/// Opacity of the 1 px border of a popup panel.
pub const PANEL_EDGE_A: f32 = 0.45;
/// Drop shadow under a popup panel: offset down, blur and opacity.
pub const SHADOW_DY: f32 = 6.0;
pub const SHADOW_BLUR: f32 = 24.0;
pub const SHADOW_A: f32 = 0.35;

// ---- bar spacing ----
/// Width of a workspace button, and of the active one (a wider accent pill) on a horizontal bar.
pub const WS_W: f32 = 26.0;
pub const WS_ACTIVE_W: f32 = 44.0;

// ---- settings navigation rail ----
/// One page in the rail: its height, the step to the next one, and the corner radius of its highlight.
/// Tight enough that all 13 pages fit a default-height window without scrolling.
pub const NAV_ITEM_H: f32 = 38.0;
pub const NAV_STEP: f32 = 45.0;
pub const NAV_R: f32 = 12.0;
/// Left edge and width of the rail, the divider line between rail and page, and the left edge of the page.
/// The rail ends 16 px before the divider and the page starts 35 px after it.
pub const NAV_X: f32 = 22.0;
pub const NAV_W: f32 = 200.0;
pub const DIVIDER_X: f32 = 238.0;
pub const PAGE_X: f32 = 274.0;
/// Collapsed rail dimensions for compact mode / icon-only navigation.
pub const NAV_COLLAPSED_W: f32 = 48.0;
pub const DIVIDER_COLLAPSED_X: f32 = NAV_X + NAV_COLLAPSED_W + 16.0;
pub const PAGE_COLLAPSED_X: f32 = DIVIDER_COLLAPSED_X + 35.0;
/// Top edge of the page area. The title starts here, so this is the air above it (the rail starts at 22).
pub const PAGE_Y: f32 = 44.0;
/// Corner radius for interactive preview studios and cards.
pub const STUDIO_R: f32 = 16.0;

// ---- island panel ----
/// Width of the Dynamic Island panel (the window adds a margin each side for the shadow), and the transport buttons in it.
pub const ISLAND_W: f32 = 420.0;
pub const TRANSPORT_BTN: f32 = 44.0;

// ---- quick panel (right sidebar) ----
/// Space between the panel edge and its content.
pub const QP_PAD: f32 = 18.0;
/// A toggle tile: height, and the gap between tiles (across and down).
pub const TILE_H: f32 = 58.0;
pub const TILE_GAP: f32 = 10.0;
/// A volume, mic or brightness slider: height, and the gap between two of them.
pub const QP_SLIDER_H: f32 = 44.0;
pub const QP_SLIDER_GAP: f32 = 10.0;

// ---- glass, accent and surface steps ----
/// Opacity of the light 1 px border around a glass bar group.
pub const GLASS_EDGE_A: f32 = 0.55;
/// Accent boost: the wallpaper's `primary` is pushed toward the brighter, more saturated Tidepool accent.
/// Value is raised only in a dark theme, so `onPrimary` text keeps its contrast in a light one.
pub const ACCENT_SAT: f32 = 1.15;
pub const ACCENT_VAL: f32 = 1.05;
/// Idle chips sit one step above the `surfaceContainerLow` cards they are drawn on.
pub const CONTROL_SURFACE: &str = "surfaceContainerHigh";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_divider_has_the_same_air_above_and_below() {
        assert_eq!(DIVIDER_Y, ROW_GAP - DIVIDER_Y - HAIRLINE);
        assert!(SECTION_PAD < ROW_GAP / 2.0, "a group edge is tighter than a divider gap, since no box frames it");
    }

    #[test]
    fn a_settings_row_is_at_least_as_tall_as_its_pitch_needs() {
        // a 40 px control plus the gap gives a row pitch near 70 px, and the divider falls on a whole pixel
        assert!(40.0 + ROW_GAP >= 68.0);
        assert_eq!(DIVIDER_Y.fract(), 0.0);
    }

    #[test]
    fn chips_and_the_switch_are_pills() {
        assert_eq!(CHIP_R * 2.0, CHIP_H);
        let inner = SWITCH_H - 2.0 * SWITCH_RING;
        assert!(KNOB_ON_R * 2.0 <= inner, "the knob fits inside the switch fill");
        assert!(KNOB_OFF_R < KNOB_ON_R, "the knob grows when the switch is on");
        assert!(SWITCH_W > SWITCH_H, "the switch is wider than it is tall");
    }

    #[test]
    fn nav_items_do_not_touch_and_have_square_ish_highlights() {
        assert!(NAV_STEP > NAV_ITEM_H, "a gap between two pages");
        assert!(NAV_R < NAV_ITEM_H / 2.0, "a rounded box, not a pill");
    }

    #[test]
    fn rail_divider_and_page_have_room_between_them() {
        assert!(DIVIDER_X - (NAV_X + NAV_W) >= 16.0, "rail to divider");
        assert!(PAGE_X - (DIVIDER_X + 1.0) >= 32.0, "divider to page");
        assert!(DIVIDER_COLLAPSED_X - (NAV_X + NAV_COLLAPSED_W) >= 16.0, "collapsed rail to divider");
        assert!(PAGE_COLLAPSED_X - (DIVIDER_COLLAPSED_X + 1.0) >= 32.0, "collapsed divider to page");
    }

    #[test]
    fn the_page_title_has_air_above_it() {
        assert!(PAGE_Y >= NAV_X + 16.0, "more room above the title than the rail margin");
    }

    #[test]
    fn the_island_panel_fits_its_controls() {
        // previous, play (60 px) and next sit in a row inside the panel's 15 px border and padding
        assert!(ISLAND_W - 30.0 >= 2.0 * TRANSPORT_BTN + 60.0 + 2.0 * 10.0);
        assert!(TRANSPORT_BTN >= 44.0, "a comfortable tap target");
    }

    #[test]
    fn alphas_are_fractions() {
        for a in [HOVER_A, PANEL_EDGE_A, SHADOW_A, GLASS_EDGE_A] { assert!(a > 0.0 && a < 1.0); }
    }
}
