//! lib/barlayout.js: which bar modules show, on which side, in what order (Settings > Bar layout).

use serde_json::{json, Value};

/// (id, default side, label, icon), in default order.
pub const BAR_MODULES: [(&str, &str, &str, &str); 14] = [
    ("launcher", "left", "Sidebar button", "auto_awesome"), ("window", "left", "Active window", "web_asset"),
    ("resources", "center", "CPU & RAM", "memory"), ("media", "center", "Media", "music_note"),
    ("island", "center", "Dynamic Island", "pill"),
    ("workspaces", "center", "Workspaces", "grid_view"), ("clock", "center", "Clock", "schedule"),
    ("utils", "center", "Utility buttons", "build"), ("weather", "center", "Weather", "partly_cloudy_day"),
    ("claude", "hidden", "Claude usage", "smart_toy"), ("wm", "right", "Tiling status", "view_quilt"), ("netspeed", "hidden", "Network speed", "swap_vert"),
    ("tray", "right", "System tray", "apps"), ("system", "right", "Quick settings", "tune"),
];
pub const SIDES: [&str; 4] = ["left", "center", "right", "hidden"];

pub type Layout = Vec<(String, String)>;

/// The saved layout (config bar.modules) with unknown ids dropped and missing modules appended on their default side.
pub fn bar_layout(saved: &Value, net_speed: bool) -> Layout {
    let mut list: Layout = vec![];
    for m in saved.as_array().into_iter().flatten() {
        let (Some(id), Some(side)) = (m.get(0).and_then(Value::as_str), m.get(1).and_then(Value::as_str)) else { continue };
        if BAR_MODULES.iter().any(|k| k.0 == id) && SIDES.contains(&side) && !list.iter().any(|x| x.0 == id) { list.push((id.into(), side.into())); }
    }
    for (id, side, ..) in BAR_MODULES {
        if !list.iter().any(|x| x.0 == id) { list.push((id.into(), if id == "netspeed" && net_speed { "right" } else { side }.into())); }
    }
    list
}

/// Moves a module one step (dir -1 / 1) among the modules on its own side.
pub fn move_module(list: &Layout, id: &str, dir: i32) -> Layout {
    let mut out = list.clone();
    let Some(i) = out.iter().position(|m| m.0 == id) else { return out };
    let mut j = i as i32 + dir;
    while j >= 0 && (j as usize) < out.len() && out[j as usize].1 != out[i].1 { j += dir; }
    if j >= 0 && (j as usize) < out.len() { out.swap(i, j as usize); }
    out
}

/// Puts a module on `side` at position `index` among that side's modules (drag and drop, arrow keys).
pub fn place_module(list: &Layout, id: &str, side: &str, index: usize) -> Layout {
    let rest: Layout = list.iter().filter(|m| m.0 != id).cloned().collect();
    let mut by_side: Vec<Layout> = SIDES.iter().map(|sd| rest.iter().filter(|m| m.1 == *sd).cloned().collect()).collect();
    if let Some(k) = SIDES.iter().position(|s| *s == side) {
        let col = &mut by_side[k];
        col.insert(index.min(col.len()), (id.into(), side.into()));
    }
    by_side.concat()
}

/// The JSON written to bar.modules: [[id, side], ...].
pub fn to_json(l: &Layout) -> Value { Value::Array(l.iter().map(|(i, s)| json!([i, s])).collect()) }

pub fn info(id: &str) -> (&'static str, &'static str) {
    BAR_MODULES.iter().find(|m| m.0 == id).map_or(("", ""), |m| (m.2, m.3))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids(l: &Layout, side: &str) -> Vec<String> { l.iter().filter(|m| m.1 == side).map(|m| m.0.clone()).collect() }

    #[test]
    fn layout_fills_in_and_drops() {
        let saved = json!([["clock", "left"], ["bogus", "left"], ["clock", "right"], ["tray", "nowhere"], ["island", "hidden"]]);
        let l = bar_layout(&saved, true);
        assert_eq!(l.len(), 14);
        assert_eq!(l[0], ("clock".into(), "left".into()));
        assert_eq!(l[1], ("island".into(), "hidden".into()));
        assert_eq!(l.iter().find(|m| m.0 == "netspeed").unwrap().1, "right"); // the old showNetSpeed switch
        assert_eq!(l.iter().find(|m| m.0 == "tray").unwrap().1, "right");
        assert_eq!(bar_layout(&Value::Null, false).iter().find(|m| m.0 == "netspeed").unwrap().1, "hidden");
    }

    #[test]
    fn move_stays_on_side() {
        let l = bar_layout(&Value::Null, false);
        let m = move_module(&l, "weather", -1); // above it on the centre side is "utils"
        assert_eq!(ids(&m, "center"), ["resources", "media", "island", "workspaces", "clock", "weather", "utils"]);
        assert_eq!(move_module(&l, "launcher", -1), l); // already first on its side
        assert_eq!(ids(&move_module(&l, "launcher", 1), "left"), ["window", "launcher"]);
    }

    #[test]
    fn place_across_sides() {
        let l = bar_layout(&Value::Null, false);
        let p = place_module(&l, "clock", "left", 1);
        assert_eq!(ids(&p, "left"), ["launcher", "clock", "window"]);
        assert!(!ids(&p, "center").contains(&"clock".to_string()));
        let end = place_module(&l, "clock", "hidden", usize::MAX);
        assert_eq!(ids(&end, "hidden"), ["claude", "netspeed", "clock"]);
        assert_eq!(p.len(), 14);
        assert_eq!(to_json(&p)[0], json!(["launcher", "left"])); // sides stay grouped left, centre, right, hidden
    }
}
