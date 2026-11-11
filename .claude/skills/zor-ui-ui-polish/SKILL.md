---
name: zor-ui-ui-polish
description: Fix spacing, padding and density in the Zor UI native app (Rust, tiny-skia, Win32). Use when the user says a Zor UI screen looks cramped, squeezed, congested, crowded or too tight, or asks for UI polish, padding, gaps or density in the Dynamic Island panel, the bar, the sidebars or the Settings pages.
---

# Zor UI spacing polish

Zor UI has no web UI. Each screen is drawn by hand in Rust: a draw function places every rect and text run at
computed x and y, and a separate height function reserves the room. Spacing bugs come from three things: a gap
that is too small, a magic number that drifts from its neighbors, and a height function that no longer matches
its draw code.

Code: `native/zor-ui/src`. Build and test in the worktree the work is in (currently `D:\zor-ui\zor-ui-agents\native\zor-ui`).
The rules come from the "Padding: nothing is squeezed" section of `CLAUDE.md` at the repo root. Read it first.

## Spacing tokens

`tokens.rs` has one scale on a 4 px grid, plus roles built from it:

- Scale: `SP_XS` 4, `SP_S` 8, `SP_M` 12, `SP_L` 16, `SP_XL` 24, `SP_XXL` 32.
- Roles: `CARD_INSET` (inside a card, every side), `CHIP_EDGE_PAD` (above and below a chip), `USAGE_GAP` (above a
  usage row), `BLOCK_GAP` (above a count or hint line after a block), `ITEM_GAP` (between cards), `SECTION_GAP`
  (between sections or agents).
- The test `the_spacing_scale_is_a_4_px_grid_and_grows` fails if a step or role is off the grid.

Use these names. Add a new step to the scale only when none fits, and keep it on the grid. Older screens still
use local constants (`PAD`, `CARD_PAD`, `SEP`); when you touch one, point it at a token.

## Minimums (CLAUDE.md)

- Cards and panels: 12 px inside on every side.
- Chips, pills and buttons: 6 px above and below inside their row, 8 px between neighbors.
- Gaps between sections, cards and agents: 8 px or more, never zero.
- A fixed-height text box is at least its line height + 4 px (`lh(size)` in `island.rs`).
- Text never touches its box edge, and two boxes never share an edge.

## Where things are

- `island.rs`: the Dynamic Island panel. `W` is `tokens::ISLAND_W` (420). `plan()` lists the sections with their
  heights; the paint loop adds `SEP` between them. Height and draw pairs: `agents_h` / `agents()`, `agent_h` /
  the per-agent body in `agents()`, `session_h` / `session()` and `session_body()`, `ask_section_h` / `ask()`,
  `CHIPS_ROW` / `chips()`, `TABS_H` / `tabs()`.
- `tokens.rs`: shared spacing and layout values for `ui.rs` widgets and Settings.
- `ui.rs`: shared widgets (`chip`, `panel`, `icon_btn`, `wave`, `Scroll`, `TextBox`). A fix here reaches every caller.
- `settings/form.rs`, `settings/pages.rs`, `settings/picker.rs`: Settings layout and previews.
- `bar.rs`: `compact()` shrinks the bar on narrow screens.

## Steps

1. **Find the screen.** Grep a visible label to find its draw code, then its height function or its line in `plan()`.
   Read the constants at the top of that file first.
2. **Measure before changing.** Walk the block top to bottom and list every y step and x inset: the literal, what it
   separates, and the constant if one is used. Flag each value under the minimums. Check the height function adds up
   to the same total in every branch.
3. **Use the scale.** Spacing is a multiple of 4. A 2 px value is allowed only for optical nudges such as centering an icon.
   Do not renumber values that already meet the rules just to fit the scale.
4. **Apply the rules.**
   - Card content: draw at `x + CARD_INSET`, `y + CARD_INSET`, width `w - 2.0 * CARD_INSET`, as `session()` does.
   - Inner spacing is never larger than outer spacing: lines inside an item sit closer than items, items closer than groups.
   - A label and its value on one row: at least `SP_S` between them. Take right-aligned widths from `measure()`.
   - A fixed gap after a header row, before its body (`SESS_LEAD`, `USAGE_GAP`).
   - Long text truncates or wraps inside its rect. It never runs into the next item.
5. **Keep heights in step.** Every px added to a draw function goes into its `*_h` function or `plan()` line in the
   same change, in every branch (open, folded, empty, filtered, with and without usage or subagents, idle line,
   "+N more"). Update the height test in the same change.
6. **Test.** `cargo test --bin zorui -- island` (or `settings`, `bar`, `tokens`, `ui`), then the full
   `cargo test --bin zorui`. Everything must pass.
7. **Check the narrowest case.** The island content width is `W - 2.0 * PAD`, so check the worst content: long folder
   names, three-digit counts, a model name beside a pill, every usage bar. For the bar, check the width where
   `compact()` kicks in.
8. **Be honest.** Unless a screenshot was taken and looked at, say "tests pass, not visually checked". Do not claim it
   "looks good".

## Checklist

- [ ] Constants read; every gap in the block listed; values under the minimums flagged.
- [ ] No new magic number. Any new constant is named, commented, and sits with the others.
- [ ] Card padding 12 px or more on all four sides, including the bottom (last line's y plus its height).
- [ ] Chips and buttons: 6 px above and below, 8 px between neighbors.
- [ ] Hit rect equals the drawn rect, or is deliberately larger. Never smaller or offset.
- [ ] Every height function matches its draw code in every branch.
- [ ] Height test updated; `cargo test --bin zorui` passes.
- [ ] Narrowest width and longest content checked.
- [ ] Settings changes that alter looks have a live preview (CLAUDE.md, first section).
- [ ] The final message says whether it was checked visually.

## Common mistakes

- Changing a draw function and not its `*_h` function, so the next section overlaps or a blank gap appears.
- Adding a gap in the open branch of `agent_h` but not the folded one.
- A new literal (`y += 7.0`) instead of a token, or the same value typed twice and drifting apart.
- Moving the rect passed to `c.rect` without the one passed to `c.hit`, so clicks land in the wrong place.
- Changing `ui::chip` or a `tokens` value without checking every caller.
- Counting a gap twice: `session_h` already includes `SESS_GAP`, and `plan()` heights do not include `SEP`.
- Growing padding until the narrow layout overflows.
- Changing a value that a `tokens.rs` test pins without reading why the test exists.

## References

- Material Design, spacing methods (4 dp and 8 dp increments): https://material.io/design/layout/spacing-methods.html
- Material 3, spacing: https://m3.material.io/foundations/layout/understanding-layout/spacing
- Material 3, state layers (hover 8 %, matches `tokens::HOVER_A`): https://m3.material.io/foundations/interaction/states/state-layers
- Apple HIG, layout: https://developer.apple.com/design/human-interface-guidelines/layout
- Apple HIG, accessibility (44 x 44 pt targets): https://developer.apple.com/design/human-interface-guidelines/accessibility
- Refactoring UI, start with too much space, then remove: https://www.refactoringui.com/
