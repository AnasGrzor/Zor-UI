# Dynamic Island 2.0: Phase 0 audit

This is the read-only repository audit that `docs/DynamicIsland2.md` (section 11, Phase 0) asks for. It changes no code. The findings in section 7 are candidates for Phase 1 and need separate approval.

## 1. Where the island lives

| Part | File | Key code |
|---|---|---|
| Compact pill (on the bar) | `native/zor-ui/src/bar.rs` | `island_list` line 535 (the scheduler), `island()` line 553 (render), `click` line 1116, `frame_ms` line 955 |
| Expanded panel (popup) | `native/zor-ui/src/island.rs` | `plan()` line 160 (section list), `paint` line 529, `input` line 563 |
| Panel window placement | `native/zor-ui/src/popup.rs` | `make` line 32, `area` line 54 (fixed `ISLAND_W` + 24 by 560), `spot` line 76 (hangs under the pill) |
| Motion | `native/zor-ui/src/anim.rs` | `Spring` (pill width), `fade` (activity change, 0.22 s), `grow` (panel open, 0.26 s), `mix` |
| Settings UI | `native/zor-ui/src/settings/pages.rs` | `island()` lines 258 to 313, `viz_preview` for `island.eqColor` at line 283 |
| Defaults | `native/zor-ui/src/conf.rs` | `island.*` lines 27 to 34, deep merge over config.json at line 47 |
| Shared runtime state | `native/zor-ui/src/hub.rs` | `shared.json`: timer, rec, agents, notifOpen, notifLast, closedAt.* |

There is no separate island module tree. The scheduler is one function, `Bar::island_list`. The layout is two hand-written renderers, one for the pill and one for the panel.

## 2. How activities reach the island

| Activity | Source | Transport | Read by |
|---|---|---|---|
| Media | `media::start_media` (app.rs:230) | `cx.media`, `cx.sessions` | bar `island()`, panel `now_playing` |
| Volume and mic OSD | `media::start_audio`, then `app::osd` (app.rs:780) | `cx.osd` with `until` | bar only |
| Mic and camera privacy | `App::privacy_check` on every 1 s tick (app.rs:347) | `cx.privacy`, plus a 4 s `cx.osd` notice | bar only |
| Notifications (banner) | `notifications-watch` stream (app.rs:240, notify.rs:210, polls DB mtime every 2 s) | `cx.notif` with `until`, `hub zor-ui.notifLast` | bar |
| Notifications (list) | panel calls bridge `notifications` once per open (island.rs:508) | `Island.notifs` | panel |
| Zor UI's own banners | assist.rs:55 and 73, claude_acct.rs:396, app.rs:415 | `cx.notif` | bar |
| Agents | hooks run `zorui.exe --agent` (agent_hook.rs:183), which write `agents/<id>/<session>.json`. `agents-watch` stats files every 0.5 s (notify.rs:408) and `agents_summary` folds them | `hub zor-ui.agents` (JSON string, one row per agent, not per session) | bar, panel, `permit::pending` |
| Claude permission | PermissionRequest hook blocks in `permit::await_answer` (permit.rs:77) for up to 110 s | `agents[].permit`. The answer is a file in `permits/` | panel Allow and Deny, voice |
| Buddy question | `drive::ask_island` (drive.rs:672) opens the panel | `drive::pending()` mutex | panel only |
| Timer and stopwatch | `hub::timer`, ended by `hub::tick` in `App::tick` (app.rs:446) | `hub zor-ui.timer`, `zor-ui.timerDone` | bar, panel, sidebar_right, desk |
| Recording | sidebar_right.rs:197 sets `zor-ui.rec`. app.rs:451 clears a dead one every 15 s | `hub zor-ui.rec` | bar, panel |
| Weather and clock | weather thread every 30 min (app.rs:207), `util::now` | `cx.weather` | idle state |

The bridge streams start only if the island is on the bar layout when the bar first starts (app.rs:235). Adding the island later needs a restart.

## 3. State and scheduling as built

`island_list` rebuilds a priority list from scratch on every paint. No activity state is stored, so an interrupted activity returns by recomputation. The order is:

1. `notification` and `osd`: win outright while `until` is in the future.
2. `agent` waiting.
3. `recording`.
4. `timer` (running or done).
5. `media`.
6. `agent` working (only when `island.agentWorking` is on).
7. Idle: time, date, weather, greeting, or nothing (`island.idle`).

`di_focus`, set by clicking a badge, overrides everything except the two "win outright" entries. Badges show every other entry.

Checked against the brief's scenarios:

- **A (music and agent):** works. Working ranks below media, waiting ranks above it, and media returns once the agent state clears.
- **B (multiple agents):** partly. The list has one `agent` entry, which shows the first waiting agent plus a `+N` count. A second working agent is reachable only in the panel. Idle agents never appear on the pill.
- **C (timer done):** works and stays until acted on. The pill has no one-click dismiss. Clicking it opens the panel.

## 4. Rendering lifecycle

- The bar repaints on every 1 s tick (its key includes the seconds) and on `cx.ver` bumps. `frame_ms` asks for 16 ms while the spring moves, the fade runs, or the visualizer is loud. It asks for 50 ms for the equalizer or a live mic, and 100 ms for a pulse.
- Pill width is spring-animated toward `pill_w`. On narrow screens `compact()` caps it (bar.rs:481).
- The panel's `key` changes every second. `size()` and `paint()` each call `plan()`, so the panel measures and parses everything twice per frame. Frames run at 50 ms while media plays, 100 ms while recording, and 500 ms while the reply caret blinks.
- The panel grows from the pill with `anim::grow`. A change in its size re-centres it (app.rs:317).

## 5. Settings persistence

- `config.json` in the pack folder is deep-merged over `conf::defaults()`. A new `island.*` key with a default needs no migration, and old configs keep working.
- Settings pages use `self.sw`, `num`, `slider`, `pick`, `seg`, and `color` with dotted keys, so a new option takes one line.
- The CLAUDE.md rule requires a live preview for every visual setting, built from the real draw code. Only `island.eqColor` has one (`viz_preview`). `island.textWidth` and `island.wideWidth` change the look and have none. This gap already exists.

## 6. Dependencies and shared helpers

`gfx::Canvas` (text, rect, clip_round, icon, image, measure_wrapped), `ui::{chip, icon_btn, panel, wave, Scroll, TextBox, Scrub}`, `bar::{El, draw_el, draw_eq, draw_viz}`, `anim::{Spring, fade, grow, mix}`, `hub::{get, set, get_json, timer, left, sw_elapsed, clock}`, `proc::{bridge, stream}`, `app::post`, and `tokens::ISLAND_W`. No new crate is needed for any phase.

## 7. Findings

Ordered by severity. None of them approves an agent operation. Every path that writes a decision was checked: `permit::decide` runs only from the panel's Allow and Deny chips, the reply box, and voice.

1. **A badge focus hides a permission request.** `di_focus` beats the waiting `agent` entry (bar.rs:559). If the user clicked the music badge earlier, a later Claude permission prompt shows only as a small badge, with no pulse and no colour. This breaks the brief's "Critical" rule.
2. **Clicking a waiting pill dismisses the request.** It does not open Allow and Deny. bar.rs:1161 runs `agents-dismiss`, which marks the session `done`. `await_in` then sees the session is no longer waiting and returns, so the island's Allow and Deny disappear and only the terminal prompt remains. This is safe, but it is the opposite of what scenario A expects.
3. **Codex permission prompts show Allow and Deny that do nothing useful.** `record` marks any PermissionRequest as `ask: permission` (agent_hook.rs:164), but only Claude's hook waits for an answer (agent_hook.rs:194). For Codex, Allow writes a decision file that nothing reads, and the file stays in `permits/`. The heading also says "Claude Code is asking" for every agent (island.rs:302).
4. **Agent rows are per agent, not per session.** `agents_summary` keeps only the first waiting or working session's message. No current tool, start time, or error state reaches the island. The hook payloads carry `tool_name`, so Phase 4 can add these without new detection.
5. **Per-frame waste.** `hub::get_json("zor-ui.agents")` re-parses the JSON 2 to 4 times per bar paint, per monitor, at up to 60 fps. The panel runs `plan()` twice per frame and clones the notification list each time. `hub::set` rewrites all of `shared.json` on every agent update.
6. **Polling.** `agents-watch` stats every agent file every 0.5 s. `notifications-watch` stats every 2 s and copies the whole toast database when anything changes. The privacy check runs every second. These are acceptable now but are the main idle cost.
7. **No reduced-motion support** anywhere. There is no `SPI_GETCLIENTAREAANIMATION` check.
8. **Badge index race.** A click resolves `island_list(cx).get(i)` again at click time, and the list can differ from the one that was painted.
9. **Streams need a restart.** They start only if the island was on the bar at launch (app.rs:235).

## 8. Features that must survive

Everything in brief section 2 maps to existing code: media ring, art, progress, equalizer, hover controls, and track expand; the seek wave; lyrics (synced and plain); OSD for volume and mic; privacy dots and notice; the recording pill and its stop button; pomodoro and stopwatch with phase-done; idle time, date, weather, greeting, and status icons; badges; the notification banner that opens in the panel; the notification list, detail, dismiss, and Clear all; the agents list with usage bars and reset times; Claude permit Allow, Deny, typed reply, and voice; the buddy question; the auto-switch cancel banner; click, right-click, and scroll actions; the narrow-bar cap; and the islands bar-style fill.

Existing tests in `island.rs`, `bar.rs`, `anim.rs`, `permit.rs`, `notify.rs`, and `agent_hook.rs` cover parts of this and must stay green.

## 9. Recommendation for Phase 1 onward

- **Keep the architecture smaller than the brief proposes.** `island_list` already works as the scheduler. One new file, `island/activity.rs`, is enough. It holds a typed `Activity { kind, priority, state, title, sub, progress, since, until, pinned }` and one function, `activities(cx) -> Vec<Activity>`, that builds the list once per frame. The pill and the panel both read from it. Six modules (manager, scheduler, layout, animation) would amount to a rewrite, which the brief itself forbids.
- **Keep it stateless.** Restoration already works because the list is recomputed every frame. The only state worth storing is the user's pin and a dismissed set, and both can live in `Bar`.
- **Fix these first, as separate small changes:** finding 1 (a waiting request beats `di_focus`), finding 2 (clicking a waiting pill opens the panel instead of dismissing), and finding 3 (Allow and Deny only for Claude, with the label set by agent). Together they deliver most of the brief's "Critical" behaviour in about 10 lines.
- **Parse agents once.** Do it in the `agents-watch` callback, store a typed `Vec<Agent>` on `Ctx`, and drop the per-frame `get_json`. This fixes finding 5 and gives Phase 1 its typed model.
- **Defer for now:** an AI scheduler, quiet hours, per-activity priority editing, and an expanded maximum-height setting. Each adds settings surface and preview obligations under CLAUDE.md. Add them when asked.

## 10. Verification

Phase 0 adds no code, so this audit needs no test run. Before Phase 1, run `cargo test` in `native/zor-ui` to record the baseline that later phases must keep. Line numbers refer to the uncommitted working tree on branch `phase2-take-over`.
