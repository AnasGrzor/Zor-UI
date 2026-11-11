# Wallpaper intelligence implementation

The existing native Material You pipeline still owns theme generation and application. No dependencies, processes, polling loops, or alternative theme engines were added.

## Pipeline and state changes

1. `bridge/theme.rs` resolves the existing static image or live wallpaper frame on its worker thread. WIC scales analysis input to at most 128 pixels on each axis before allocating RGB pixels.
2. `bridge/theme/intelligence.rs` streams a stable content fingerprint, then checks the versioned analysis cache. New images go through the existing Celebi quantizer. Population, HCT chroma, tone and hue separation select up to five candidates. Neutral or single-color images can have fewer meaningful choices and retain the original Auto seed fallback.
3. Saved wallpaper preferences select automatic, curated or custom colors. Intensity adjusts seed chroma. The same generator handles preview and Apply, repairs foreground contrast, and retains the existing Material schemes and terminal harmonization.
4. The generated palette cache includes wallpaper identity, extracted seeds, mode, scheme, intensity and overrides. Both disk caches hold at most 64 entries. The Settings preview cache holds at most 16 palettes and disappears with the Settings view. Cached records contain colors and brightness statistics, never wallpaper copies.
5. Apply writes the existing `theme.json`, state and external-app configurations. The existing Windows theme synchronization remains in `switchwall`. Wallpaper and lock-screen writes remain guarded by `-ThemeOnly`.
6. The existing file watcher loads the palette. A 200 ms OKLab transition uses the existing frame timer, then releases its temporary maps and stops requesting frames. Windows client animation preferences and `wallpaperTheme.reducedMotion` disable transitions.

Analysis and theme generation serialize within the native process. This prevents Settings regeneration and automatic wallpaper updates from colliding on cache files without holding a lock around Wallpaper Engine callbacks.

## Persistence and settings

The pack folder contains three small JSON files:

- `wallpaper-analysis.json` stores candidate seeds and an 8 by 8 grid of linear brightness statistics.
- `wallpaper-palettes.json` stores generated color maps by relevant configuration.
- `wallpaper-themes.json` stores user preferences independently of disposable caches.

Static wallpaper preferences follow a content fingerprint across file moves. Live wallpaper preferences follow the existing live source identifier, so replacing a preview frame does not lose the saved selection. This fingerprint is a versioned FNV-1a identifier for local cache lookup, not a security checksum.

Existing configurations need no migration. New defaults are:

```json
{
  "wallpaperTheme": {
    "perMonitor": false,
    "adaptiveGlass": true,
    "reducedMotion": false
  }
}
```

Quick Settings displays generated palette swatches. Preview creates a view-local draft; mode, scheme, intensity, monitor policy and custom overrides stay in that draft. Apply commits it. Cancel discards it. Reset selects automatic colors with default intensity and scheme, and still requires Apply. Custom colors have separate light and dark overrides. Unsafe text colors can be corrected for readability.

For wallpapers created before this implementation, use Regenerate palettes once or switch wallpapers to populate the analysis metadata. Existing immediate mode/scheme actions remain available until a wallpaper has that metadata.

The Settings seed picker retains its existing reversible preview. Other native windows temporarily hide that preview while painting. A wallpaper draft never installs a desktop-wide preview or calls a system writer. Apply checks the current wallpaper fingerprint and rejects a stale draft.

The internal bridge command `theme-options [regenerate]` returns analysis and saved preferences. It can write the disposable analysis cache, but cannot change theme preferences, Windows settings, wallpaper, GlazeWM, Flow Launcher or Terminal. The existing `switchwall` command accepts `-Profile <JSON> -ExpectedWallpaper <identity> -PerMonitor <true|false>` with `-ThemeOnly` to apply a draft.

## Glass and monitors

Native bar, tray and panel backgrounds use the shared opacity calculation. It checks the cached grid cells intersecting the window and increases opacity when wallpaper brightness would reduce secondary text contrast. Manual opacity can remain more opaque. Turning off Adapt glass to wallpaper restores manual transparency behavior.

Desktop glass cards can use the theme background tint and adaptive strength while retaining the existing blur, bend and fringe renderer. Existing explicit manual tints remain authoritative unless Adaptive card tint is enabled. Adaptive tint limits white frosting and sheen so those effects do not undo the darker tint.

Per-monitor palettes are optional. Device bounds route colors during window painting, and crossing a brightness-grid cell or monitor invalidates the window's render key. Each distinct source palette generates once per operation, with persistent cache reuse afterwards. Unified mode still uses per-monitor brightness data when monitors have different wallpaper assignments. Glass backdrops use the fitted wallpaper returned by Windows where available. External applications continue to receive the global palette.

## Verification and limits

Automated tests cover candidate ranking and neutral fallback, stable identity, saved preference restoration, cache reuse and regeneration, cache version recovery, contrast repair, old configuration defaults, stale Apply rejection, draft cancellation/reset, monitor routing, and transition completion. Native offscreen rendering exercises narrow dark and wide light Settings layouts without changing the desktop.

The complete `zorui` test suite passed with 925 tests passed and 8 ignored. Offscreen preview images are written to `%TEMP%/zor-wallpaper-preview/440.png` and `800.png`. New Rust modules are formatted with rustfmt; existing modules retain their surrounding repository style.

Live follow-up validation on November 11, 2026 used two connected 1920 by 1080 monitors, VG248 and DELL P2222H. Remaining hardware checks include monitor hot-plugging, mixed DPI, Lively playback, and a controlled performance comparison against the previous build.

### Live results

- Generated palettes in the actual Settings window. The original Angel wallpaper produced two meaningful candidates; the red-purple wallpaper produced four. Settings remained responsive.
- Selected a candidate and cancelled. Hash checks confirmed `theme.json` and `config.json` stayed unchanged before Apply.
- Applied a curated candidate with separate monitor palettes enabled. The live shell changed colors and saved the selection.
- Assigned a different wallpaper to the second monitor. The two monitor palettes had distinct primary colors, `#C8C5D0` and `#F7B6B9`, and the Windows wallpaper calls succeeded on both screens.
- Switched the global wallpaper, then returned to the original. The saved curated seed and dark mode returned. External theme writes reported success for GlazeWM and Flow Launcher.
- Started the installed Wallpaper Engine scene `4k Red Moon`. Verified actual desktop playback and a theme generated from the scene preview. Two desktop captures about a second apart differed at 229,199 pixels in a wallpaper-only region below the bar. Wallpaper Engine was stopped again afterward.
- Checked actual light and dark Settings rendering. On the optimized build, a live pixel sample recorded intermediate colors during a transition. Enabling reduced motion produced only the two endpoint colors.
- Compared adaptive and manual bar transparency over the bright wallpaper. An interior background pixel was RGB 19,19,21 with adaptation and RGB 61,65,71 with manual transparency. Desktop card glass was enabled during the exercise, but its screenshot was occluded, so card appearance is not claimed as visually verified.

The live checks found and fixed two issues. Settings now invalidates its monitor thumbnail list when `theme.json` changes, including changes initiated outside Settings. Monitor transitions now begin with the previous colors, and their clock starts at the first animation frame so desktop rebuilding cannot consume the transition time. A regression test covers the monitor transition start, midpoint and completion. Follow-up checks passed all 4 theme-effect tests and 99 Settings tests, and both debug and optimized builds compiled.

Two approximately ten-second samples of the optimized shell, with Settings closed and both animated buddies running, measured 0.538% and 0.668% total machine CPU on 12 logical processors. Mean working sets were 58.16 MB and 50.17 MB. These samples were before and after a theme application on the same build. They do not establish a regression result against the previous version or long-term idle behavior.

After testing, both monitors were returned to the original static wallpaper. The original theme file was restored byte-for-byte, regenerated external palette colors matched all original theme colors, and the configuration keys changed during testing were restored. Wallpaper Engine was left stopped. The updated optimized shell was left running.

Brightness is a coarse approximation from a scaled wallpaper frame. Tiny highlights, animation after the sampled frame, and other windows behind an always-on-top surface can differ from the cached image. There is deliberately no continuous pixel sampling. Very unusual custom color combinations may flatten some neutral container colors to retain readable text. Native colors animate; external applications reload their final colors immediately.

Within one process, theme operations serialize. Multiple independent CLI processes writing the same pack simultaneously still share the repository's existing last-writer behavior. User preference records are retained rather than evicted with the bounded caches.

## Files changed

- `native/zor-ui/src/bridge/theme.rs`: scaled analysis decode, cache/profile integration, bridge preview data, Apply validation and monitor generation.
- `native/zor-ui/src/bridge/theme/intelligence.rs`: ranking, identity, persistence, cached palette generation and contrast repair.
- `native/zor-ui/src/conf.rs`: compatible defaults and shared theme state.
- `native/zor-ui/src/conf/theme_effects.rs`: monitor routing, brightness adaptation, preview isolation and OKLab transitions.
- `native/zor-ui/src/app.rs`: transition lifecycle on the existing timer.
- `native/zor-ui/src/win.rs`: per-window monitor/brightness context and preview isolation.
- `native/zor-ui/src/ui.rs`, `bar.rs`, `tray_flyout.rs`: shared adaptive opacity.
- `native/zor-ui/src/glass.rs`, `desk.rs`: adaptive card tint and monitor wallpaper backdrops.
- `native/zor-ui/src/settings.rs`, `settings/pages.rs`, `settings/form.rs`: draft state, controls and color-picker integration.
- `native/zor-ui/src/settings/wallpaper.rs`: candidate swatches, representative component preview, Apply/Cancel/Reset and isolation/render tests.
- `docs/walls-implementation.md`: this implementation and validation record.

The separately requested Impeccable skill update also installed its hook configuration under `.codex`. Existing unrelated working-tree changes were preserved.
