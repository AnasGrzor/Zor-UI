# Zor UI — Wallpaper Intelligence v2

## Objective

Upgrade Zor UI's existing wallpaper theming system into an intelligent, adaptive, lightweight theme engine. Improve color selection, visual harmony, glass effects, and customization without rewriting existing functionality or increasing idle memory usage significantly.

## Existing Architecture

Inspect and build upon:

- `native/zor-ui/src/bridge/[theme.rs](http://theme.rs)` — Wallpaper processing and theme application.
- `native/zor-ui/src/bridge/theme/[palette.rs](http://palette.rs)` — Material You palette generation.
- `native/zor-ui/src/settings/[pages.rs](http://pages.rs)` — Wallpaper and color settings.
- `native/zor-ui/src/settings/[picker.rs](http://picker.rs)` — Color picker and previews.
- `native/zor-ui/src/[conf.rs](http://conf.rs)` — Theme configuration and state.
- `native/zor-ui/src/[glass.rs](http://glass.rs)` — Glass rendering.

The existing implementation already supports Material You colors, HCT, multiple color schemes, per-monitor wallpapers, smart cropping, Wallpaper Engine, and live previews. Preserve these capabilities.

## Required Improvements

### 1. Intelligent Multi-Color Extraction

- Extract 3–5 meaningful candidate colors from each wallpaper instead of relying exclusively on one seed.
- Rank candidates using chroma, population, perceptual distance, and suitability for UI theming.
- Generate multiple Material You palette options using the existing `material_colors` dependency.
- Avoid muddy, oversaturated, or visually unpleasant combinations.
- Keep the existing Auto mode as a backward-compatible fallback.

### 2. Adaptive Glass Effects

- Automatically adjust glass tint, opacity, and contrast based on the wallpaper behind UI elements.
- Ensure text, icons, and controls remain readable against bright or complex wallpapers.
- Support the taskbar, Dynamic Island, widgets, sidebars, and desktop cards where applicable.
- Preserve manual glass customization and existing rendering effects.
- Cache calculations rather than repeatedly sampling wallpaper pixels during rendering.

### 3. Persistent Wallpaper Themes

- Associate generated palettes and user overrides with a stable wallpaper identity.
- Automatically restore saved preferences when a previously used wallpaper returns.
- Support automatic, curated, and custom modes.
- Preserve existing configuration compatibility.
- Avoid storing duplicate wallpaper images unnecessarily.

### 4. Better Theme Previews

- Display multiple generated palettes in Settings.
- Preview their effects on representative Zor UI components before applying.
- Allow users to adjust accent intensity and override individual theme colors.
- Provide Apply, Cancel, and Reset actions.
- Ensure previews never modify persistent settings until Apply is confirmed.

### 5. Smooth Theme Transitions

- Animate color changes when switching wallpapers or themes.
- Interpolate colors in a perceptually appropriate color space.
- Keep animations short and smooth.
- Respect reduced-motion settings.
- Avoid unnecessary repaints and CPU usage.

### 6. Per-Monitor Palette Support

- Extend the existing per-monitor wallpaper system to optionally generate separate palettes.
- Allow users to choose between a unified global theme and monitor-specific themes.
- Preserve the existing global theme as the default.
- Avoid duplicating theme-generation work unnecessarily.

## Engineering Constraints

- Rust-native implementation.
- Reuse existing libraries and architecture wherever possible.
- No AI models, Python processes, or continuously running background services.
- No unnecessary new dependencies.
- Perform expensive wallpaper analysis only when the wallpaper changes or the user requests regeneration.
- Cache generated palettes using wallpaper identity and relevant configuration.
- Keep idle CPU usage effectively unchanged.
- Minimize additional memory allocations.
- Never block the UI rendering thread with image processing.
- Preserve compatibility with Wallpaper Engine, GlazeWM, Flow Launcher, and Windows Terminal.
- Maintain existing settings, presets, and manual color overrides.

## Implementation Instructions

1. Audit the current theme pipeline and identify which requirements already exist.
2. Map the exact Rust modules, interfaces, and state transitions that need changes.
3. Implement improvements incrementally, reusing existing functionality.
4. Add unit tests for palette ranking, caching, contrast, configuration migration, and theme restoration.
5. Test static wallpapers, animated wallpapers, multi-monitor setups, and light/dark modes.
6. Verify that previews cannot accidentally change Windows system settings.
7. Run formatting, compilation, and relevant tests.
8. Document any remaining limitations and performance trade-offs.

## Acceptance Criteria

- Wallpaper changes produce several visually distinct, usable palette choices.
- Auto mode chooses an appropriate palette without user intervention.
- Glass surfaces remain readable across bright and dark wallpapers.
- Returning to a wallpaper restores its saved theme preferences.
- Theme previews are reversible and do not affect the active desktop until applied.
- Existing wallpaper functionality remains intact.
- No measurable increase in steady-state CPU usage and no significant idle RAM regression.
- Theme generation happens asynchronously without UI freezes.

## Important

**Do not rebuild the existing theme engine or introduce a parallel theming system.** Extend the current Material You pipeline and shared theme infrastructure.

Prioritize correctness, visual quality, modularity, and low resource consumption over adding dependencies or complexity.

Before implementing, inspect the latest repository state rather than assuming the referenced files are unchanged. Complete the work in logical, testable stages, and report the exact files changed and features delivered.