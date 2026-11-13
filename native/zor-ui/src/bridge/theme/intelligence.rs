//! Bounded wallpaper analysis cache and saved preferences. No renderer or system writes.
use super::*;
use material_colors::{
    hct::Hct,
    quantize::{Quantizer, QuantizerCelebi},
};
use std::io::Read;

const VERSION: u64 = 1;
const LIMIT: usize = 64;

/// Stable across moves and process versions. Streams bytes without retaining another image.
fn identity(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hash = 0xcbf29ce484222325u64;
    let mut buf = [0; 32768];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        for b in &buf[..n] {
            hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3);
        }
    }
    Some(format!("wall-v1-{hash:016x}"))
}

pub(super) fn luminance(rgb: Rgb) -> f64 {
    let linear = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb.red) + 0.7152 * linear(rgb.green) + 0.0722 * linear(rgb.blue)
}

pub(super) fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Quantization considers population; ranking limits chroma and rejects nearly black/white seeds.
fn candidates(px: &[u8], fallback: Rgb) -> Vec<String> {
    let pixels: Vec<_> = px
        .as_chunks::<3>().0.iter()
        .map(|p| Rgb::new(p[0], p[1], p[2]))
        .collect();
    let q = QuantizerCelebi::quantize(&pixels, 48);
    let total = pixels.len().max(1) as f64;
    let mut ranked: Vec<_> = q
        .color_to_count
        .into_iter()
        .filter_map(|(rgb, count)| {
            let h = Hct::new(rgb);
            if count as f64 / total < 0.005
                || h.get_tone() < 8.0
                || h.get_tone() > 95.0
                || h.get_chroma() < 8.0
            {
                return None;
            }
            let score = (count as f64 / total).sqrt() * 60.0 + h.get_chroma().min(65.0) * 0.6
                - (h.get_chroma() - 80.0).max(0.0) * 0.25;
            Some((score, rgb_hex(rgb), h))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut selected: Vec<(String, Hct)> = Vec::new();
    for (_, hex, h) in ranked {
        // Hue separation matters more than tiny differences in exposure.
        if selected.iter().all(|(_, old)| {
            let gap = (old.get_hue() - h.get_hue()).abs();
            gap.min(360.0 - gap) >= 25.0
        }) {
            selected.push((hex, h));
        }
        if selected.len() == 5 {
            break;
        }
    }
    if selected.is_empty() {
        return vec![rgb_hex(fallback)];
    }
    selected.into_iter().map(|(hex, _)| hex).collect()
}

/// Analysis is persistent, small, and independent of mode/scheme. Pixels are discarded immediately.
pub(super) fn analysis(path: &Path, dir: &Path, regenerate: bool) -> Option<Value> {
    let file = dir.join("wallpaper-analysis.json");
    let mut cache = load_json(&file, json!({}));
    if cache["version"] != VERSION {
        cache = json!({"version": VERSION, "entries": {}});
    }
    let id = identity(path)?;
    if !regenerate && cache["entries"][&id].is_object() {
        return Some(cache["entries"][&id].clone());
    }
    let (w, h, px) = super::load_analysis_rgb(path)?;
    let fallback = source_from_rgb(&px, w, h, 128);
    let seeds = candidates(&px, fallback);
    // A coarse spatial grid is enough for cached surface adaptation, without retaining pixels.
    let mut regions = Vec::with_capacity(64);
    for gy in 0..8 {
        for gx in 0..8 {
            let (mut sum, mut squares, mut n) = (0.0, 0.0, 0.0f64);
            let (mut low, mut high) = (1.0f64, 0.0f64);
            for y in gy * h / 8..((gy + 1) * h / 8).max(gy * h / 8 + 1).min(h) {
                for x in gx * w / 8..((gx + 1) * w / 8).max(gx * w / 8 + 1).min(w) {
                    let i = (y * w + x) * 3;
                    let l = luminance(Rgb::new(px[i], px[i + 1], px[i + 2]));
                    sum += l;
                    squares += l * l;
                    n += 1.0;
                    low = low.min(l);
                    high = high.max(l);
                }
            }
            let mean = sum / n.max(1.0);
            regions.push(json!([
                mean,
                (squares / n.max(1.0) - mean * mean).max(0.0).sqrt(),
                low,
                high
            ]));
        }
    }
    let result =
        json!({"identity": id, "seeds": seeds, "autoSeed": rgb_hex(fallback), "regions": regions});
    if !cache["entries"].is_object() {
        cache["entries"] = json!({});
    }
    let entries = cache["entries"].as_object_mut()?;
    if entries.len() >= LIMIT {
        if let Some(key) = entries.keys().find(|k| **k != id).cloned() {
            entries.remove(&key);
        }
    }
    entries.insert(id, result.clone());
    save_json(&file, &cache);
    Some(result)
}

pub(super) fn profile(dir: &Path, id: &str) -> Value {
    load_json(&dir.join("wallpaper-themes.json"), Value::Null)["profiles"][id].clone()
}

pub(super) fn save_profile(dir: &Path, id: &str, value: &Value) -> bool {
    let path = dir.join("wallpaper-themes.json");
    let mut db = load_json(&path, json!({}));
    if !db.is_object() {
        db = json!({});
    }
    if !db["profiles"].is_object() {
        db["profiles"] = json!({});
    }
    db["version"] = json!(VERSION);
    db["profiles"][id] = value.clone();
    save_json(&path, &db)
}

pub(super) fn validate_profile(p: &Value) -> Result<(), String> {
    if !p.is_object() {
        return Err("Theme preferences must be an object".into());
    }
    if !["automatic", "curated", "custom"].contains(&p["selection"].as_str().unwrap_or("automatic"))
    {
        return Err("Unknown theme selection".into());
    }
    if let Some(seed) = p["seed"].as_str() {
        if hex_rgb(seed).is_none() {
            return Err("Invalid theme seed".into());
        }
    }
    if let Some(s) = p["scheme"].as_str() {
        if s != "auto" && !SCHEMES.contains(&s) {
            return Err("Unknown color scheme".into());
        }
    }
    if let Some(s) = p["mode"].as_str() {
        if !["dark", "light"].contains(&s) {
            return Err("Unknown theme mode".into());
        }
    }
    if !p["intensity"].is_null()
        && !p["intensity"]
            .as_f64()
            .is_some_and(|v| (0.0..=1.5).contains(&v))
    {
        return Err("Accent intensity must be between 0 and 1.5".into());
    }
    for mode in ["dark", "light"] {
        if !p["overrides"][mode].is_null() && !p["overrides"][mode].is_object() {
            return Err("Color overrides must be an object".into());
        }
        for (_, v) in p["overrides"][mode].as_object().into_iter().flatten() {
            if v.as_str().and_then(hex_rgb).is_none() {
                return Err("Invalid color override".into());
            }
        }
    }
    Ok(())
}

/// Shared by preview and apply. Auto retains the original seed as the neutral-image fallback.
pub(super) fn generate_profile(
    a: &Value,
    p: &Value,
    dark: bool,
    scheme: &str,
) -> (
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    String,
    Rgb,
) {
    let auto = p["selection"].as_str().unwrap_or("automatic") == "automatic";
    let seed = if auto {
        a["seeds"][0].as_str()
    } else {
        p["seed"].as_str()
    }
    .and_then(hex_rgb)
    .or_else(|| a["autoSeed"].as_str().and_then(hex_rgb))
    .unwrap_or(Rgb::new(103, 80, 164));
    let h = Hct::new(seed);
    let seed: Rgb = Hct::from(
        h.get_hue(),
        (h.get_chroma() * p["intensity"].as_f64().unwrap_or(1.0)).min(80.0),
        h.get_tone(),
    )
    .into();
    let (mut colors, term, used) = generate(
        seed,
        dark,
        if scheme == "auto" {
            "tonal-spot"
        } else {
            scheme
        },
        scheme == "auto",
    );
    if p["selection"] == "custom" {
        for (key, value) in p["overrides"][if dark { "dark" } else { "light" }]
            .as_object()
            .into_iter()
            .flatten()
        {
            if colors.contains_key(key) && value.as_str().and_then(hex_rgb).is_some() {
                colors.insert(key.clone(), value.as_str().unwrap().to_string());
            }
        }
    }
    // Color overrides must retain readable foreground/background pairs.
    for (fg, bg) in [
        ("onSurface", "surface"),
        ("onSurfaceVariant", "surfaceContainerLow"),
        ("onPrimary", "primary"),
        ("onPrimaryContainer", "primaryContainer"),
        ("onSecondary", "secondary"),
        ("onSecondaryContainer", "secondaryContainer"),
        ("onTertiary", "tertiary"),
        ("onTertiaryContainer", "tertiaryContainer"),
        ("onBackground", "background"),
        ("onError", "error"),
    ] {
        if let (Some(f), Some(b)) = (
            colors.get(fg).and_then(|v| hex_rgb(v)),
            colors.get(bg).and_then(|v| hex_rgb(v)),
        ) {
            if contrast(f, b) < 4.5 {
                let black = Rgb::new(0, 0, 0);
                let white = Rgb::new(255, 255, 255);
                colors.insert(
                    fg.into(),
                    rgb_hex(if contrast(black, b) > contrast(white, b) {
                        black
                    } else {
                        white
                    }),
                );
            }
        }
    }
    // Native text uses onSurface across all neutral containers. A custom white surface in a
    // dark scheme must not leave black text on the remaining dark containers.
    let surface = colors["surface"].clone();
    let foreground = hex_rgb(&colors["onSurface"]).unwrap();
    let muted = hex_rgb(&colors["onSurfaceVariant"]).unwrap();
    for bg in [
        "background",
        "surface",
        "surfaceDim",
        "surfaceBright",
        "surfaceContainerLowest",
        "surfaceContainerLow",
        "surfaceContainer",
        "surfaceContainerHigh",
        "surfaceContainerHighest",
    ] {
        if let Some(color) = colors.get(bg).and_then(|v| hex_rgb(v)) {
            if contrast(foreground, color) < 4.5 {
                colors.insert(bg.into(), surface.clone());
            }
            if contrast(muted, hex_rgb(&colors[bg]).unwrap()) < 4.5 {
                colors.insert("onSurfaceVariant".into(), colors["onSurface"].clone());
            }
        }
    }
    let background = hex_rgb(&colors["background"]).unwrap();
    if contrast(hex_rgb(&colors["onBackground"]).unwrap(), background) < 4.5 {
        colors.insert("onBackground".into(), colors["onSurface"].clone());
    }
    (colors, term, used, seed)
}

/// Persist only generated color maps, bounded across wallpaper/configuration combinations.
pub(super) fn cached_generate(
    a: &Value,
    p: &Value,
    dark: bool,
    scheme: &str,
    dir: &Path,
) -> (
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    String,
    Rgb,
) {
    let path = dir.join("wallpaper-palettes.json");
    let key = format!(
        "{}:{}:{dark}:{scheme}:{}",
        a["identity"],
        a["seeds"],
        json!({"selection":p["selection"], "seed":p["seed"], "intensity":p["intensity"], "overrides":p["overrides"]})
    );
    let mut db = load_json(&path, Value::Null);
    if db["version"] != VERSION || !db["entries"].is_object() {
        db = json!({"version":VERSION, "entries":{}});
    }
    let old = &db["entries"][&key];
    if let (Ok(colors), Ok(term), Some(seed), Some(used)) = (
        serde_json::from_value::<BTreeMap<String, String>>(old["colors"].clone()),
        serde_json::from_value::<BTreeMap<String, String>>(old["term"].clone()),
        old["seed"].as_str().and_then(hex_rgb),
        old["scheme"].as_str(),
    ) {
        let complete = [
            "primary",
            "outlineVariant",
            "surface",
            "background",
            "onSurface",
            "onSurfaceVariant",
            "surfaceContainerLow",
            "secondaryContainer",
            "onSecondaryContainer",
            "outline",
        ]
        .iter()
        .all(|k| colors.contains_key(*k));
        if complete
            && colors.len() > 40
            && term.len() == 16
            && SCHEMES.contains(&used)
            && colors
                .values()
                .chain(term.values())
                .all(|v| hex_rgb(v).is_some())
        {
            return (colors, term, used.to_string(), seed);
        }
    }
    let (colors, term, used, seed) = generate_profile(a, p, dark, scheme);
    let entries = db["entries"].as_object_mut().unwrap();
    if entries.len() >= LIMIT {
        if let Some(k) = entries.keys().next().cloned() {
            entries.remove(&k);
        }
    }
    entries.insert(
        key,
        json!({"colors":colors, "term":term, "scheme":used, "seed":rgb_hex(seed)}),
    );
    save_json(&path, &db);
    (colors, term, used, seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_cache_separates_configuration_and_has_a_fixed_limit() {
        let dir = super::super::tests::tmp("palette-cache");
        let a = json!({"identity":"sample", "seeds":["#3377BB"], "autoSeed":"#3377BB"});
        let p = json!({"intensity":1.0});
        let first = cached_generate(&a, &p, true, "auto", &dir);
        assert_eq!(first.0, cached_generate(&a, &p, true, "auto", &dir).0);
        assert_eq!(
            load_json(&dir.join("wallpaper-palettes.json"), Value::Null)["entries"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert_ne!(first.0, cached_generate(&a, &p, false, "auto", &dir).0);
        assert_ne!(
            first.0,
            cached_generate(&a, &json!({"intensity":0.0}), true, "auto", &dir).0
        );
        let mut entries = serde_json::Map::new();
        for i in 0..LIMIT {
            entries.insert(format!("old-{i}"), Value::Null);
        }
        save_json(
            &dir.join("wallpaper-palettes.json"),
            &json!({"version":VERSION,"entries":entries}),
        );
        cached_generate(&a, &p, true, "auto", &dir);
        assert_eq!(
            load_json(&dir.join("wallpaper-palettes.json"), Value::Null)["entries"]
                .as_object()
                .unwrap()
                .len(),
            LIMIT
        );
    }
    #[test]
    fn ranking_favors_population_and_separates_hues() {
        let mut px = Vec::new();
        for (rgb, n) in [
            ([200, 40, 50], 500),
            ([205, 45, 55], 100),
            ([30, 120, 210], 200),
            ([40, 170, 65], 200),
        ] {
            for _ in 0..n {
                px.extend(rgb);
            }
        }
        let seeds = candidates(&px, Rgb::new(103, 80, 164));
        assert_eq!(seeds.len(), 3);
        assert!(hex_rgb(&seeds[0]).unwrap().red > 150);
        assert_eq!(
            candidates(&vec![128; 300], Rgb::new(128, 128, 128)),
            vec!["#808080"]
        );
    }
    #[test]
    fn overrides_are_validated_and_contrast_is_repaired() {
        assert!(validate_profile(&json!({"intensity": 9})).is_err());
        assert!(validate_profile(&json!({"seed":"bad"})).is_err());
        assert!(validate_profile(&json!({})).is_ok());
        let p = json!({"selection":"custom", "overrides":{"dark":{"primary":"#FFFFFF","onPrimary":"#FFFFFF"}}});
        let (c, ..) = generate_profile(&Value::Null, &p, true, "auto");
        assert!(
            contrast(
                hex_rgb(&c["primary"]).unwrap(),
                hex_rgb(&c["onPrimary"]).unwrap()
            ) >= 4.5
        );
        let p = json!({"selection":"custom", "overrides":{"dark":{"surface":"#FFFFFF"}}});
        let (c, ..) = generate_profile(&Value::Null, &p, true, "auto");
        for bg in [
            "surface",
            "background",
            "surfaceContainerLow",
            "surfaceContainerHigh",
        ] {
            assert!(contrast(hex_rgb(&c[bg]).unwrap(), hex_rgb(&c["onSurface"]).unwrap()) >= 4.5);
        }
    }
    #[test]
    fn identities_survive_moves_and_preferences_restore_without_images() {
        let dir = std::env::temp_dir().join(format!("zor-wall-id-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::write(&a, b"wallpaper").unwrap();
        std::fs::copy(&a, &b).unwrap();
        let id = identity(&a).unwrap();
        assert_eq!(Some(id.clone()), identity(&b));
        let p = json!({"selection":"curated", "seed":"#123456", "intensity":0.8});
        assert!(save_profile(&dir, &id, &p));
        assert_eq!(profile(&dir, &id), p);
        std::fs::write(&b, b"changed").unwrap();
        assert_ne!(Some(id), identity(&b));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn analysis_cache_reuses_results_and_regeneration_replaces_them() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let dir = super::super::tests::tmp("analysis-cache");
        let image = dir.join("wall.jpg");
        super::super::write_rgb_jpeg(16, 16, &[200, 40, 60].repeat(256), &image).unwrap();
        let first = analysis(&image, &dir, false).unwrap();
        assert_eq!(first["regions"].as_array().unwrap().len(), 64);
        let id = first["identity"].as_str().unwrap();
        let path = dir.join("wallpaper-analysis.json");
        let mut db = load_json(&path, Value::Null);
        db["entries"][id]["cached"] = json!(true);
        save_json(&path, &db);
        assert_eq!(analysis(&image, &dir, false).unwrap()["cached"], true);
        assert!(analysis(&image, &dir, true).unwrap()["cached"].is_null());
        // Unknown cache versions rebuild safely; saved preferences are in a separate file.
        db["version"] = json!(0);
        save_json(&path, &db);
        assert!(analysis(&image, &dir, false).unwrap()["cached"].is_null());
        assert_eq!(load_json(&path, Value::Null)["version"], VERSION);
    }
}
