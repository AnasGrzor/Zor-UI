//! Wallpaper palette edits stay in this view until Apply.
use super::form::Fmt;
use super::*;

#[derive(Clone)]
pub(super) struct Draft {
    pub analysis: Value,
    pub profile: Value,
    pub mode: String,
    pub scheme: String,
    pub per_monitor: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_edits_cancel_and_reset_without_persistent_or_desktop_changes() {
        let cx = Ctx::new(false);
        let mut s = Settings::new((1200.0, 800.0));
        s.info.analysis =
            json!({"identity":"wall-test", "seeds":["#BB3344", "#2288BB"], "autoSeed":"#BB3344"});
        s.info.mode = "dark".into();
        s.info.scheme = "auto".into();
        let cfg_before = cx.cfg.0.clone();
        let desktop_before = cx.theme.c("primary");
        s.wallpaper_action(&cx, "wall-candidate", "#2288BB");
        s.set("$theme.intensity", json!(0.6));
        s.set("$theme.perMonitor", json!(true));
        s.set("$theme.selection", json!("custom"));
        s.set("$theme.overrides.dark.primary", json!("#112233"));
        assert_eq!(s.get(&cx, "$theme.intensity"), json!(0.6));
        assert_eq!(s.get(&cx, "$theme.overrides.dark.primary"), "#112233");
        s.set("$theme.overrides.dark.primary", json!(""));
        assert!(
            s.wallpaper_draft.as_ref().unwrap().profile["overrides"]["dark"]["primary"].is_null()
        );
        s.wallpaper_action(&cx, "mode", "light");
        s.wallpaper_action(&cx, "scheme", "expressive");
        assert_eq!(cx.cfg.0, cfg_before);
        assert_eq!(cx.theme.c("primary"), desktop_before);
        assert!(conf::set_preview(None).is_none());
        assert!(s.over.is_empty());
        assert!(s.inbox.take().is_empty());
        s.wallpaper_action(&cx, "wall-reset", "");
        assert_eq!(
            s.wallpaper_draft.as_ref().unwrap().profile["selection"],
            "automatic"
        );
        s.wallpaper_action(&cx, "wall-cancel", "");
        assert!(s.wallpaper_draft.is_none());
        assert_eq!(cx.cfg.0, cfg_before);
    }

    #[test]
    fn draft_controls_render_at_narrow_and_wide_widths() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let dir = std::env::temp_dir().join("zor-wallpaper-preview");
        std::fs::create_dir_all(&dir).unwrap();
        let g = Gfx::new(&conf::pack_dir().join("assets/fonts"), &dir).unwrap();
        for (width, dark) in [(440u32, true), (800, false)] {
            let mut cx = Ctx::new(false);
            cx.theme = conf::Theme::parse(&json!({"mode":if dark {"dark"} else {"light"}, "colors": crate::bridge::theme::preview(0xFF2288BB, dark, "auto").into_iter().map(|(k,v)| (k, format!("#{:06X}", v & 0xFFFFFF))).collect::<std::collections::BTreeMap<_,_>>()}).to_string());
            let mut s = Settings::new((width as f32, 1600.0));
            s.info.analysis = json!({"identity":"render", "seeds":["#BB3344", "#2288BB", "#44AA66"], "autoSeed":"#BB3344"});
            s.info.mode = if dark { "dark" } else { "light" }.into();
            s.info.scheme = "auto".into();
            s.wallpaper_action(&cx, "wall-candidate", "#2288BB");
            s.set("$theme.selection", json!("custom"));
            let mut pixels = vec![0; width as usize * 1600 * 4];
            {
                let mut c = Canvas::new(&g, &mut pixels, width, 1600, 1.0, 0).unwrap();
                c.rect(
                    R::new(0.0, 0.0, width as f32, 1600.0),
                    0.0,
                    cx.theme.c("background"),
                );
                let mut p = Pen::new(&mut c, &cx, 12.0, 8.0, width as f32 - 24.0, 0.0, 1600.0);
                s.wallpaper_palettes(&mut p);
                assert!(p.acts.iter().any(|a| matches!(a, Act::Do("wall-apply", _))));
                assert!(p
                    .acts
                    .iter()
                    .any(|a| matches!(a, Act::Do("wall-cancel", _))));
            }
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
                let inv = 255 - px[3];
                for c in &mut px[..3] {
                    *c = c.saturating_add(inv / 8);
                }
                px[3] = 255;
            }
            let file = std::fs::File::create(dir.join(format!("{width}.png"))).unwrap();
            let mut enc = png::Encoder::new(file, width, 1600);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
    }
}

impl Settings {
    pub(super) fn wallpaper_override_color(&self, path: &str) -> Option<u32> {
        let role = path.strip_prefix("$theme.overrides.")?.split_once('.')?.1;
        let d = self.wallpaper_draft.as_ref()?;
        let key = format!(
            "{}:{}:{}:{}",
            d.analysis["identity"],
            d.profile,
            d.mode != "light",
            d.scheme
        );
        self.wallpaper_preview_cache
            .iter()
            .find(|(k, _)| *k == key)?
            .1
            .get(role)
            .copied()
    }
    fn cached_wallpaper_preview(
        &mut self,
        analysis: &Value,
        profile: &Value,
        dark: bool,
        scheme: &str,
    ) -> HashMap<String, u32> {
        let key = format!("{}:{profile}:{dark}:{scheme}", analysis["identity"]);
        if let Some((_, colors)) = self.wallpaper_preview_cache.iter().find(|(k, _)| *k == key) {
            return colors.clone();
        }
        let colors = crate::bridge::theme::preview_profile(analysis, profile, dark, scheme);
        if self.wallpaper_preview_cache.len() >= 16 {
            self.wallpaper_preview_cache.remove(0);
        }
        self.wallpaper_preview_cache.push((key, colors.clone()));
        colors
    }
    fn begin_wallpaper_draft(&mut self, cx: &Ctx) {
        if self.wallpaper_draft.is_none() && self.info.analysis.is_object() {
            let mut profile = self.info.profile.clone();
            if !profile.is_object() {
                profile = json!({});
            }
            self.wallpaper_draft = Some(Draft {
                analysis: self.info.analysis.clone(),
                profile,
                mode: self.info.mode.clone(),
                scheme: self.info.scheme.clone(),
                per_monitor: cx.cfg.bool("wallpaperTheme.perMonitor"),
            });
        }
    }

    pub(super) fn wallpaper_action(&mut self, cx: &Ctx, name: &str, arg: &str) {
        if name == "wall-palettes" {
            if !self.busy.is_empty() {
                return;
            }
            self.busy = "palettes".into();
            let mode = self.info.mode.clone();
            let scheme = self.info.scheme.clone();
            let per_monitor = cx.cfg.bool("wallpaperTheme.perMonitor");
            store::bridge(
                cx,
                &self.inbox,
                "theme-options",
                vec!["regenerate".into()],
                move |s, r| {
                    s.busy.clear();
                    if let Some(e) = store::failure(&r) {
                        return s.toast(&e);
                    }
                    let mut profile = r["profile"].clone();
                    if !profile.is_object() {
                        profile = json!({});
                    }
                    s.info.analysis = r["analysis"].clone();
                    s.wallpaper_preview_cache.clear();
                    s.wallpaper_draft = Some(Draft {
                        analysis: r["analysis"].clone(),
                        profile,
                        mode,
                        scheme,
                        per_monitor,
                    });
                },
            );
            return;
        }
        if name == "wall-cancel" {
            self.wallpaper_draft = None;
            return;
        }
        self.begin_wallpaper_draft(cx);
        let Some(draft) = self.wallpaper_draft.as_mut() else {
            return;
        };
        match name {
            "wall-candidate" => {
                draft.profile["seed"] = json!(arg);
                draft.profile["selection"] = json!("curated");
            }
            "wall-reset" => {
                draft.profile = json!({"selection":"automatic", "intensity":1.0});
                draft.scheme = "auto".into();
            }
            "mode" => draft.mode = arg.into(),
            "scheme" => draft.scheme = arg.into(),
            "wall-apply" => {
                if !self.busy.is_empty() {
                    return;
                }
                draft.profile["scheme"] = json!(draft.scheme);
                let profile = draft.profile.to_string();
                let mode = draft.mode.clone();
                let scheme = draft.scheme.clone();
                let id = draft.analysis["identity"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let per_monitor = draft.per_monitor;
                self.busy = "palette".into();
                // Apply commits palette and monitor policy in the same worker operation.
                store::script(
                    cx,
                    &self.inbox,
                    "switchwall",
                    vec![
                        "-ThemeOnly".into(),
                        "-Profile".into(),
                        profile,
                        "-Mode".into(),
                        mode,
                        "-Scheme".into(),
                        scheme,
                        "-ExpectedWallpaper".into(),
                        id,
                        "-PerMonitor".into(),
                        per_monitor.to_string(),
                    ],
                    |s, r| {
                        s.busy.clear();
                        if let Some(e) = store::failure(&r) {
                            s.toast(&e);
                        } else {
                            s.wallpaper_draft = None;
                            s.info.stamp = None;
                        }
                    },
                );
            }
            _ => {}
        }
    }

    pub(super) fn wallpaper_palettes(&mut self, p: &mut Pen) {
        p.h2("palette", "Wallpaper palettes", "");
        p.card(|p| {
            p.note(
                "Preview changes here, then apply them to your desktop.",
                12.0,
            );
            let analysis = self
                .wallpaper_draft
                .as_ref()
                .map(|d| &d.analysis)
                .unwrap_or(&self.info.analysis)
                .clone();
            let seeds = analysis["seeds"].as_array().cloned().unwrap_or_default();
            let dark = self
                .wallpaper_draft
                .as_ref()
                .map(|d| d.mode != "light")
                .unwrap_or(self.info.mode != "light");
            let scheme = self
                .wallpaper_draft
                .as_ref()
                .map(|d| d.scheme.clone())
                .unwrap_or_else(|| self.info.scheme.clone());
            if seeds.is_empty() {
                p.note(
                    "Generate palettes for your current wallpaper to get started.",
                    12.0,
                );
            }
            for seed in seeds.iter().filter_map(Value::as_str) {
                if let Some(r) = p.row(42.0) {
                    let color = conf::parse_hex(seed).unwrap_or(0xFF6750A4);
                    p.c.rect(R::new(r.x + 8.0, r.y + 7.0, 28.0, 28.0), 8.0, color);
                    let preview = self.cached_wallpaper_preview(
                        &analysis,
                        &json!({"selection":"curated", "seed":seed}),
                        dark,
                        &scheme,
                    );
                    for (i, key) in ["primary", "secondary", "tertiary", "surfaceContainerHigh"]
                        .iter()
                        .enumerate()
                    {
                        p.c.rect(
                            R::new(r.x + 48.0 + i as f32 * 25.0, r.y + 9.0, 22.0, 24.0),
                            6.0,
                            preview.get(*key).copied().unwrap_or(color),
                        );
                    }
                    p.c.text(
                        seed,
                        R::new(r.x + 160.0, r.y + 10.0, (r.w - 260.0).max(0.0), 22.0),
                        txt(13.0, 400),
                        p.col("onSurface"),
                        Align::Left,
                    );
                    p.chip_right(r, None, "Preview", Act::Do("wall-candidate", seed.into()));
                }
            }
            p.chips(vec![(
                "Regenerate palettes".into(),
                Some("refresh"),
                false,
                Act::Do("wall-palettes", String::new()),
            )]);
            if self.wallpaper_draft.is_some() {
                self.seg(
                    p,
                    "$theme.selection",
                    &[
                        ("automatic", "Auto"),
                        ("curated", "Curated"),
                        ("custom", "Custom"),
                    ],
                    "Palette selection",
                    "palette",
                );
                self.slider(
                    p,
                    "$theme.intensity",
                    "Accent intensity",
                    Sl {
                        min: 0.0,
                        max: 1.5,
                        step: 0.05,
                        fmt: Fmt::Pct,
                    },
                    "contrast",
                );
                self.sw(
                    p,
                    "$theme.perMonitor",
                    "Separate palettes per monitor",
                    "External apps use the global palette",
                    "desktop_windows",
                );
                let d = self.wallpaper_draft.as_ref().unwrap().clone();
                self.cached_wallpaper_preview(
                    &d.analysis,
                    &d.profile,
                    d.mode != "light",
                    &d.scheme,
                );
                if d.profile["selection"] == "custom" {
                    for (key, label) in [
                        ("primary", "Accent"),
                        ("secondary", "Secondary accent"),
                        ("surface", "Surface"),
                        ("onSurface", "Text"),
                    ] {
                        self.color(
                            p,
                            &format!("$theme.overrides.{}.{key}", d.mode),
                            label,
                            "Text contrast is corrected on preview and apply",
                            "colorize",
                        );
                    }
                }
                let mut profile = d.profile.clone();
                if let Some((path, value)) = &self.drag_val {
                    if path == "$theme.intensity" {
                        profile["intensity"] = json!(value);
                    }
                }
                let colors = self.cached_wallpaper_preview(
                    &d.analysis,
                    &profile,
                    d.mode != "light",
                    &d.scheme,
                );
                let col = |k: &str| colors.get(k).copied().unwrap_or(0xFF6750A4);
                if let Some(r) = p.row(130.0) {
                    p.c.rect(r, 12.0, col("surface"));
                    p.c.rect(
                        R::new(r.x + 12.0, r.y + 12.0, r.w - 24.0, 28.0),
                        8.0,
                        col("surfaceContainerHigh"),
                    );
                    p.c.text(
                        "Taskbar  ·  Island  ·  Sidebar",
                        R::new(r.x + 24.0, r.y + 18.0, r.w - 48.0, 20.0),
                        txt(12.0, 400),
                        col("onSurface"),
                        Align::Left,
                    );
                    p.c.text(
                        "Wallpaper theme preview",
                        R::new(r.x + 18.0, r.y + 52.0, r.w - 36.0, 22.0),
                        txt(16.0, 500),
                        col("onSurface"),
                        Align::Left,
                    );
                    let b = R::new(r.x + 18.0, r.y + 86.0, 120.0, 30.0);
                    p.c.rect(b, 12.0, col("primary"));
                    p.c.text(
                        "Primary action",
                        b,
                        txt(12.0, 500),
                        col("onPrimary"),
                        Align::Center,
                    );
                }
                p.chips(vec![
                    (
                        "Apply".into(),
                        Some("check"),
                        true,
                        Act::Do("wall-apply", String::new()),
                    ),
                    (
                        "Cancel".into(),
                        None,
                        false,
                        Act::Do("wall-cancel", String::new()),
                    ),
                    (
                        "Reset".into(),
                        None,
                        false,
                        Act::Do("wall-reset", String::new()),
                    ),
                ]);
            }
            if !self.busy.is_empty() {
                p.spinner_block();
            }
        });
    }
}
