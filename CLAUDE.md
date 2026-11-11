# Project rules

## Every customizable setting gets a live preview

When you add a setting that changes how something looks (a color, size, shape, width, style, font, blur, opacity), add a preview of it to the same Settings page, next to the control. A switch that only turns a feature on or off is exempt, and so is a setting that changes behavior with no visual result.

- The preview draws with the same code the real feature uses (share the draw function, as `bar::draw_viz` does for the bar visualizer), so it can't drift from the real thing. If the real code can't be shared, say why in a comment.
- It reads the value through the Settings getters (`self.gs` / `self.get`), not `cx.cfg`, so an edit shows at once, before config.json catches up.
- It uses made-up sample data when the real data needs live input (audio, media, weather). Put the sample in a named function so a test can check it.
- It shows each place the setting applies (island, bar, desktop), one box each, labeled.
- It appears only when the setting itself is visible (inside the same `if` as its switch).
- If a preview needs a picture that isn't available, show a one-line note saying what to pick, as `glass_preview` does.
- Add a unit test for any logic the preview adds (color fallback, sample data). Pixels need no test.

Existing examples: `glass_preview` and `viz_preview` in `native/zor-ui/src/settings/pages.rs`.

## Padding: nothing is squeezed

Every control, card, chip, row, badge and text block gets room around it. Text never touches the edge of its box, and two boxes never share an edge.

- Cards and panels: at least 12 px inside, on every side. Use the existing `CARD_PAD` / `PAD` constants, not a new number.
- Chips, pills and buttons: at least 6 px above and below inside their row, and 8 px between neighbors. A chip row gets its own top and bottom padding, as `CHIPS_PAD` does.
- Gaps between sections, cards and agents: at least 8 px. Sections that stack get a gap, never zero.
- Text inside a fixed-height box: keep the box at least as tall as the line height plus 4 px. A label that does not fit wraps or shortens, it never overlaps its neighbor.
- Use the constants in the file being drawn. If a value is missing, add a named constant next to the others, with a comment on what it spaces.
- Check at the narrowest width the panel can reach (see `compact()` in `bar.rs`), not only at the default width.
- Layout tests: when a height function such as `agent_h` or `session_h` changes, update its test, so the reserved height matches what the draw code uses.

**Why:** squeezed rows read as broken, and the island is small, so every missing pixel shows.
