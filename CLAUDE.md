# Project rules

## Everything runs headless

No process we start may open a visible window, console, or terminal on the user's PC. This covers the app's own child processes.

- App code: start every child process through `bridge::common::hidden()` (or set `CREATE_NO_WINDOW`, `0x0800_0000`, on the command yourself). Never set `DETACHED_PROCESS`, since it removes the console that `powershell.exe` and `cmd.exe` need. Only `zor-setup` has its own copy, in `job.rs`, and it must keep the same flag.
- Do not close other sessions' terminals or windows. They may hold unsaved work. Ask the user first.
- A new spawn that skips the hidden flag is a bug. Fix it in the same change. `native/zor-ui/src/headless_test.rs` checks this in both crates and fails on any unflagged `Command::new(`. Intentionally visible spawns go in its `VISIBLE_ON_PURPOSE` list, with a reason.

**Why:** the user lost focus to Claude terminal popups opened while agents worked on their PC.

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

## Every new feature logs what it does

A feature isn't done until its log shows what happened. The log is how the user and the next agent find out why something didn't work.

- Log through `crate::log::line` (it writes to `%LOCALAPPDATA%\zor-ui\zorui.log`). Bridge code that runs as a child process writes `{"error": ...}` lines that the app logs.
- Log when the feature starts, each decision it makes (what it picked, what it skipped and why), what it got as input, what it did, and the result.
- Every error path and every fallback writes a line that says what failed and what the feature did instead. A silent `return`, `.ok()` or `let _ =` on a path the user would notice is a bug.
- Log what was heard, asked or chosen, but never secrets, keys, tokens or passwords.
- Don't log once per frame or once per audio chunk. Log events.

**Why:** the wake word failed for hours and the log had no line saying what it heard or why it ignored it, so nobody could tell what was wrong.

## Verify the work in the real app

"Tests pass" and "it compiles" are not proof that a feature works, and a report may not use them as proof. Before saying a feature works, run it and watch it work.

`.claude/verify-gate.js` (a Stop hook in `.claude/settings.json`) enforces this. If the app's code under `native/` changed during a turn, the agent can't finish until the installed app is the new release build, the real program has run since the last change, and zorui.log has been read since that run.

- Run the real build: the installed copy after a deploy, or the bridge directly (`zorui.exe --bridge <stream> ...`). Use the feature the way the user will.
- When the real input needs the user (their voice, their device), make a stand-in: a Windows text-to-speech WAV, a sample file, a test window. Feed it through the same code path the feature uses.
- Read zorui.log after the run and check that the feature's lines are there and say what you expected.
- Check what it costs: memory (private bytes) and CPU of the running app before and after, for anything that runs all the time.
- If it doesn't work, or works only partly, fix it and run it again. Keep iterating until everything the user asked for works. Don't hand back a half-working feature, and don't take a shortcut that skips part of the request.
- In the report, say what you ran and what you saw (log lines, numbers, output). If something truly can't be checked from here, name it, say why, say what you checked instead, and tell the user exactly how to test it.

**Why:** the user got features that compiled and passed tests but did nothing in the real app.
