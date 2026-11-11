//! Shared, finite theme transitions and cached wallpaper brightness for native surfaces.
use super::*;
use std::{cell::Cell, time::Instant};

pub(super) struct MonitorTheme {
    bounds: [i32; 4],
    pub colors: HashMap<String, Color>,
    regions: Vec<(f64, f64)>,
    wallpaper: Option<String>,
}
pub(super) struct Transition {
    from: HashMap<String, Color>,
    to: HashMap<String, Color>,
    monitors: Vec<(HashMap<String, Color>, HashMap<String, Color>)>,
    start: Option<Instant>,
}

pub(super) fn regions(a: &Value) -> Vec<(f64, f64)> {
    a["regions"]
        .as_array()
        .into_iter()
        .flatten()
        .take(64)
        .map(|r| {
            let mean = r[0].as_f64().unwrap_or(0.5);
            let spread = r[1].as_f64().unwrap_or(0.5);
            (
                r[2].as_f64().unwrap_or((mean - 2.0 * spread).max(0.0)),
                r[3].as_f64().unwrap_or((mean + 2.0 * spread).min(1.0)),
            )
        })
        .collect()
}
pub(super) fn monitors(raw: &Value) -> Vec<MonitorTheme> {
    raw["monitors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let b = &m["bounds"];
            let bounds = [
                b[0].as_i64()? as i32,
                b[1].as_i64()? as i32,
                b[2].as_i64()? as i32,
                b[3].as_i64()? as i32,
            ];
            let colors = m["colors"]
                .as_object()?
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), parse_hex(v.as_str()?)?)))
                .collect();
            Some(MonitorTheme {
                bounds,
                colors,
                regions: regions(&m["analysis"]),
                wallpaper: m["wallpaper"].as_str().map(String::from),
            })
        })
        .collect()
}

type Geometry = ([i32; 4], [i32; 4]);
thread_local! { static SURFACE: Cell<Option<Geometry>> = const { Cell::new(None) }; }
pub struct SurfaceScope(Option<Geometry>);
impl Drop for SurfaceScope {
    fn drop(&mut self) {
        SURFACE.with(|s| s.set(self.0));
    }
}
/// Set once per window paint. No file access or image processing during drawing.
pub fn surface_scope(rect: [i32; 4], monitor: [i32; 4]) -> SurfaceScope {
    SurfaceScope(SURFACE.with(|s| s.replace(Some((rect, monitor)))))
}

/// Hide a Settings color-picker preview from every other native window.
pub struct PreviewScope(Option<HashMap<String, Color>>);
impl Drop for PreviewScope {
    fn drop(&mut self) {
        set_preview(self.0.take());
    }
}
pub fn hide_preview() -> PreviewScope {
    PreviewScope(set_preview(None))
}

fn lum(c: Color) -> f64 {
    let f = |shift: u32| {
        let s = ((c >> shift) & 255u32) as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(16) + 0.7152 * f(8) + 0.0722 * f(0)
}

impl Theme {
    pub fn monitor_wallpaper(&self) -> Option<&str> {
        self.monitor()?.wallpaper.as_deref()
    }
    pub fn target_color(&self, name: &str) -> Color {
        if let Some(t) = &self.transition {
            if let Some(m) = self.monitor() {
                if let Some(i) = self.monitors.iter().position(|own| std::ptr::eq(own, m)) {
                    if let Some(c) = t.monitors[i].1.get(name) {
                        return *c;
                    }
                }
            }
            if let Some(c) = t.to.get(name) {
                return *c;
            }
        }
        self.c(name)
    }
    pub(super) fn monitor(&self) -> Option<&MonitorTheme> {
        let rect = SURFACE.with(|s| s.get())?.0;
        let (x, y) = ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2);
        self.monitors
            .iter()
            .find(|m| x >= m.bounds[0] && x < m.bounds[2] && y >= m.bounds[1] && y < m.bounds[3])
    }

    /// Minimum opacity for the cached region, preserving a more opaque manual setting.
    pub fn surface_opacity(&self, manual: f32) -> f32 {
        let regions = self.monitor().map(|m| &m.regions).unwrap_or(&self.regions);
        if regions.len() != 64 {
            return manual;
        }
        let geo = SURFACE.with(|s| s.get());
        let (mut low, mut high) = (1.0f64, 0.0f64);
        for (i, &(lo, hi)) in regions.iter().enumerate() {
            let intersects = geo
                .map(|(r, m)| {
                    let (w, h) = ((m[2] - m[0]).max(1), (m[3] - m[1]).max(1));
                    let (x, y) = ((i % 8) as i32, (i / 8) as i32);
                    r[0] < m[0] + (x + 1) * w / 8
                        && r[2] > m[0] + x * w / 8
                        && r[1] < m[1] + (y + 1) * h / 8
                        && r[3] > m[1] + y * h / 8
                })
                .unwrap_or(true);
            if intersects {
                low = low.min(lo);
                high = high.max(hi);
            }
        }
        let bg = lum(self.target_color("background"));
        let fg = lum(self.target_color("onSurfaceVariant"));
        let alpha = if fg > bg {
            let limit = (fg + 0.05) / 4.5 - 0.05;
            if high <= limit {
                0.0
            } else {
                (high - limit) / (high - bg).max(0.0001)
            }
        } else {
            let limit = (fg + 0.05) * 4.5 - 0.05;
            if low >= limit {
                0.0
            } else {
                (limit - low) / (bg - low).max(0.0001)
            }
        };
        manual.max(alpha.clamp(0.0, 1.0) as f32)
    }

    pub fn transition_from(&mut self, old: &Theme, animate: bool) {
        if !animate {
            return;
        }
        let same_monitors = old.monitors.len() == self.monitors.len()
            && old
                .monitors
                .iter()
                .zip(&self.monitors)
                .all(|(a, b)| a.bounds == b.bounds && a.colors == b.colors);
        if old.colors == self.colors && same_monitors {
            return;
        }
        let monitors: Vec<_> = self
            .monitors
            .iter()
            .map(|m| {
                let from = old
                    .monitors
                    .iter()
                    .find(|o| o.bounds == m.bounds)
                    .map(|o| &o.colors)
                    .unwrap_or(&old.colors);
                (from.clone(), m.colors.clone())
            })
            .collect();
        for (m, (from, to)) in self.monitors.iter_mut().zip(&monitors) {
            if !to.is_empty() {
                m.colors.clone_from(from);
            }
        }
        let to = self.colors.clone();
        self.colors = old.colors.clone();
        self.transition = Some(Transition {
            from: old.colors.clone(),
            to,
            monitors,
            start: None,
        });
    }
    pub fn animating(&self) -> bool {
        self.transition.is_some()
    }
    /// Called only by the existing frame timer, which stops again at 200 ms.
    pub fn advance(&mut self) -> bool {
        let Some(t) = &mut self.transition else {
            return false;
        };
        // Wallpaper changes can rebuild desktop windows before the first frame is painted.
        // Start the clock at that first frame so the rebuild cannot consume the animation.
        let fraction = (t
            .start
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_secs_f64()
            / 0.2)
            .min(1.0);
        let eased = 1.0 - (1.0 - fraction).powi(3);
        interpolate_map(&mut self.colors, &t.from, &t.to, eased);
        for (m, (from, to)) in self.monitors.iter_mut().zip(&t.monitors) {
            interpolate_map(&mut m.colors, from, to, eased);
        }
        if fraction >= 1.0 {
            self.transition = None;
        }
        true
    }
}

fn interpolate_map(
    out: &mut HashMap<String, Color>,
    from: &HashMap<String, Color>,
    to: &HashMap<String, Color>,
    t: f64,
) {
    if t >= 1.0 {
        out.clone_from(to);
        return;
    }
    for (k, b) in to {
        out.insert(k.clone(), mix(*from.get(k).unwrap_or(b), *b, t));
    }
}

/// OKLab interpolation avoids muddy midpoints and hue wrap discontinuities.
fn mix(a: Color, b: Color, t: f64) -> Color {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let lab = |c: Color| {
        let f = |shift: u32| {
            let v = ((c >> shift) & 255u32) as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (f(16), f(8), f(0));
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        [
            0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
        ]
    };
    let (a, b) = (lab(a), lab(b));
    let [l, aa, bb] = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
    let ll = (l + 0.3963377774 * aa + 0.2158037573 * bb).powi(3);
    let mm = (l - 0.1055613458 * aa - 0.0638541728 * bb).powi(3);
    let ss = (l - 0.0894841775 * aa - 1.291485548 * bb).powi(3);
    let byte = |v: f64| {
        let v = if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    0xFF000000
        | byte(4.0767416621 * ll - 3.3077115913 * mm + 0.2309699292 * ss) << 16
        | byte(-1.2684380046 * ll + 2.6097574011 * mm - 0.3413193965 * ss) << 8
        | byte(-0.0041960863 * ll - 0.7034186147 * mm + 1.707614701 * ss)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn perceptual_transition_preserves_endpoints_and_finishes() {
        assert_eq!(mix(0xFF123456, 0xFFABCDEF, 0.0), 0xFF123456);
        assert_eq!(mix(0xFF123456, 0xFFABCDEF, 1.0), 0xFFABCDEF);
        let old = Theme::parse("");
        let mut new = Theme::parse(r##"{"colors":{"primary":"#123456"}}"##);
        new.transition_from(&old, true);
        assert!(new.animating());
        new.transition.as_mut().unwrap().start =
            Some(Instant::now() - std::time::Duration::from_secs(1));
        assert!(new.advance());
        assert!(!new.animating());
        assert_eq!(new.c("primary"), 0xFF123456);
        assert!(!new.advance());
    }
    #[test]
    fn bright_wallpaper_increases_opacity_without_reducing_manual_opacity() {
        let mut t = Theme::parse("");
        t.regions = vec![(1.0, 1.0); 64];
        assert!(t.surface_opacity(0.4) > 0.8);
        assert_eq!(t.surface_opacity(1.0), 1.0);
    }
    #[test]
    fn monitor_palettes_route_by_window_position() {
        let t = Theme::parse(
            r##"{"monitors":[{"bounds":[-1920,0,0,1080],"colors":{"primary":"#123456"}}]}"##,
        );
        {
            let _scope = surface_scope([-1000, 0, -900, 40], [-1920, 0, 0, 1080]);
            assert_eq!(t.c("primary"), 0xFF123456);
        }
        assert_ne!(t.c("primary"), 0xFF123456);
    }

    #[test]
    fn monitor_transition_starts_with_previous_colors_after_window_rebuild() {
        let old = Theme::parse(
            r##"{"colors":{"primary":"#112233"},"monitors":[{"bounds":[0,0,1920,1080],"colors":{"primary":"#112233"}}]}"##,
        );
        let mut new = Theme::parse(
            r##"{"colors":{"primary":"#AABBCC"},"monitors":[{"bounds":[0,0,1920,1080],"colors":{"primary":"#AABBCC"}}]}"##,
        );
        new.transition_from(&old, true);
        let _scope = surface_scope([10, 10, 100, 50], [0, 0, 1920, 1080]);
        assert_eq!(new.c("primary"), old.c("primary"));
        assert!(new.transition.as_ref().unwrap().start.is_none());
        new.advance();
        assert!(new.animating());
        new.transition.as_mut().unwrap().start =
            Some(Instant::now() - std::time::Duration::from_millis(100));
        new.advance();
        assert_ne!(new.c("primary"), old.c("primary"));
        assert_ne!(new.c("primary"), 0xFFAABBCC);
        new.transition.as_mut().unwrap().start =
            Some(Instant::now() - std::time::Duration::from_secs(1));
        new.advance();
        assert_eq!(new.c("primary"), 0xFFAABBCC);
        assert!(!new.animating());
    }
}
