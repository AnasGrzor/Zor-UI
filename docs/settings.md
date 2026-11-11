# Zor UI — Settings UI Redesign

## Objective

Redesign the existing Zor UI Settings application into a premium, cohesive, fast, native desktop customization experience.

Repository: [https://github.com/AnasGrzor/Zor-UI](https://github.com/AnasGrzor/Zor-UI)

## Phase 1 — Audit

- Inspect the current Rust Settings implementation, rendering primitives, navigation, state management, and configuration persistence.
- Identify duplicated controls, inconsistent spacing, confusing navigation, and unnecessary redraws.
- Preserve all existing features and configuration compatibility.

## Phase 2 — Design System

- Establish consistent spacing, typography, corner radii, surface hierarchy, and interaction states.
- Introduce reusable setting rows, switches, sliders, dropdowns, color pickers, section headers, and preview containers.
- Support dark/light modes and wallpaper-derived Material You colors.
- Add polished hover, focus, active, disabled, and keyboard-navigation states.

## Phase 3 — Navigation

- Introduce a clean, collapsible navigation sidebar.
- Add settings search with category and keyword matching.
- Group related settings logically and provide clear page headings.
- Preserve navigation state when switching categories.

## Phase 4 — Interactive Studios

- Create an Appearance Studio with live wallpaper, theme, and transparency previews.
- Create a Bar &amp; Island Studio with layout and behavior previews.
- Improve Buddy customization with character, voice, and personality previews.
- Reuse existing native rendering functions whenever possible.
- Provide Apply, Cancel, and Reset controls for changes requiring confirmation.

## Phase 5 — Usability

- Provide immediate feedback after changing settings.
- Add tooltips and concise descriptions for advanced options.
- Detect conflicting keybindings.
- Show clear loading, empty, error, and success states.
- Ensure keyboard accessibility and DPI-aware layouts.

## Phase 6 — Performance

- Retain the existing native Rust architecture.
- Avoid WebView, Electron, and heavyweight dependencies.
- Cache expensive preview resources.
- Redraw only affected regions where supported.
- Avoid unnecessary allocations and background work.
- Preserve responsiveness while applying settings.

## Acceptance Criteria

- No existing settings are lost.
- Existing config.json files remain compatible.
- All visual settings have consistent, functional previews.
- Search and keyboard navigation work reliably.
- No significant increase in idle memory or CPU usage.
- The interface feels cohesive with the rest of Zor UI.

First audit the repository and produce a file-level implementation plan. Then implement the redesign incrementally, testing each phase. Do not replace working functionality merely to achieve visual changes.