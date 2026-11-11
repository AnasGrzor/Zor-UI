# Settings UI — Polish Work

Optional enhancements beyond the core redesign in [settings.md](settings.md). Each item refines an already-working feature; none changes the config schema or breaks existing tests.

---

## 1. Appearance Studio — Transparency Live Swatch

**File:** `native/zor-ui/src/settings/pages.rs` → `fn quick()`  
**Below:** the "Bar & panel transparency" slider (~line 441)

Add a small live swatch that draws `surfaceContainerHigh` at the current transparency value over a wallpaper thumbnail, so the user sees the glass effect immediately while dragging the slider. Reuse the `glass_preview` pattern (backdrop crop → overlay → blit).

**Effort:** ~30 lines.

---

## 2. Appearance Studio — WCAG Contrast Badge

**File:** `native/zor-ui/src/settings/wallpaper.rs` → theme preview box (~line 398)

Compute the luminance contrast ratio between the preview's `onSurface` and `surface` colors and render a small badge:

- `AA ✓` (ratio ≥ 4.5) in `primaryContainer` / `onPrimaryContainer`
- `Fail` (ratio < 4.5) in `errorContainer` / `onErrorContainer`

Uses the standard WCAG 2.1 relative-luminance formula. Pure math on colors already available from `cached_wallpaper_preview`.

**Effort:** ~15 lines.

---

## 3. Buddy Preview — Voice Test Chip

**File:** `native/zor-ui/src/settings/pages.rs` → `fn buddy_preview()`  
**After:** the `stances_box` call (~line 931)

Add a "Preview voice" chip that fires `Act::Do("voice-test", String::new())`. The handler already exists in `settings.rs` line 790. This lets users hear the buddy's voice without scrolling down to the Voice page.

**Effort:** ~5 lines.

---

## 4. Buddy Preview — Reset Colors Chip

**File:** `native/zor-ui/src/settings/pages.rs` → `fn buddies()`  
**After:** the hair/shirt/pants/eyes color pickers (~line 1018)

Add a "Reset colors" chip that clears all four color overrides (`hair`, `shirt`, `pants`, `eyes`) back to `""` (character's own). Requires a small handler in `settings.rs` `do_named`:

```rust
"reset-buddy-colors" => {
    for f in ["hair", "shirt", "pants", "eyes"] {
        self.set(&format!("buddies.{arg}.{f}"), json!(""));
    }
}
```

**Effort:** ~10 lines total.

---

## 5. Release Build Verification

Run a full release build to confirm no release-only issues (optimisation-level differences, dead-code warnings promoted to errors, link-time failures):

```powershell
cargo build --release --manifest-path native/zor-ui/Cargo.toml
```

**Effort:** Build time only, no code changes expected.
