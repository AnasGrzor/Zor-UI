# Phase 2 plan: let the buddy act (take-over)

Written at the end of the Phase 1 session so a new session can start cold. Read `CLAUDE.md` first (project rules), then this file.

Phase 1 gave the buddy eyes: you ask a screen question by voice, it looks at every monitor, a vision model answers, and the buddy flies to the spot and rings it. Phase 2 gives it hands: after you say yes, the model moves the mouse and types to finish one task while you watch.

## 1. Where things stand (verified in the Phase 1 session)

All code is in `native/zor-ui/src`. The tree is not committed, and other work is happening in parallel in `assist.rs`, `settings.rs`, `settings/pages.rs` and `win.rs`. Re-read a file before editing it.

| Piece | File | What it does |
|---|---|---|
| Screen capture | `screen.rs` | Every monitor, pointer's monitor first, long side 1280 px, JPEG through WinRT. `Cap::label` tells the model which screen is which and where it sits ("left of screen 1"). `parse_point` reads `[POINT:x,y:label:screenN]`. `to_screen` maps a picture pixel to a real screen pixel, including monitors at negative coordinates. |
| Pointing | `buddy.rs` | `Mode::Point`: the buddy flies along an arc (`arc`, `fly_ease`, `flight_secs`), lands beside the spot (`point_feet`), draws a three-stroke ring and a label pill (`label_box`, `pill_rect`), then drops back to the bar. `assist::point_later` queues a spot, `assist::next_point` hands it to the buddy. |
| Voice and intents | `assist.rs` | `parse` turns text into `Intent` (Search, Open, Message, Screen, Chat). `run` does it. `screen_help` captures, asks the model and queues the point. `chat` runs one `ai-chat` conversation. |
| Chat with pictures | `bridge/web.rs`, `bridge/claude_cli.rs` | A message may carry `"images": [{"mime","data","label"}]`. Each provider adapter converts it (Gemini parts, Anthropic blocks, OpenAI-style data URLs, Claude Code stdin stream-json). `can_see(provider)` and `id_can_see(id)` say who can read images. The Gemini and ChatGPT command line tools cannot. |
| Contacts | `settings/pages.rs`, `settings.rs`, `assist.rs` | Own Settings page with add, edit (pencil), delete. `with_contact` holds the save rules. |
| WhatsApp Web | `assist.rs`, `webchat.rs` | `message_link`, `effective_app` (plain `whatsapp` falls back to the web when no app handles `whatsapp://`). `webchat::open_whatsapp_web` finds the live logged-in WhatsApp tab through UI Automation and loads the link there, and only opens a new tab when there is none. It never presses Send. |
| Settings | `settings/pages.rs`, `conf.rs` | `buddies.screen` (off by default) and `buddies.screenModel` (blank follows `ai.model`), shown on the Buddies page. |

How a spoken command reaches the code: the GlazeWM hotkey `alt+b` runs `zorui.exe --buddy listen`. It records, stops after about 1 s of silence, transcribes with Parakeet on the PC, then calls `assist::ask`. For tests you can send the same text by typing: `zorui.exe --buddy "where is the save button"`.

Config lives in `%USERPROFILE%\.glzr\zebar\zor-ui\config.json`. The installed exe is `%LOCALAPPDATA%\zor-ui\zorui.exe`. Logs and `crash.log` are in `%LOCALAPPDATA%\zor-ui`.

## 2. Goal and scope

**Goal.** One consented take-over: "take over and rename this file", "click Save", "fill in this form". The buddy shows what it is doing, you can stop it at any moment, and it hands control back when it is done or when you touch the mouse.

**In scope.** Consent, the action loop, the executor (mouse, keyboard, scroll), the safety layer, the overlay, a Settings switch, tests, a live test protocol.

**Out of scope for Phase 2.** A browser extension agent, background agents, scheduled routines, anything that runs without you present.

## 3. What to copy from HeyClicky (behaviour only)

A static analysis of the HeyClicky 1.1.5 Windows build (strings and metadata, nothing run) showed these behaviours. Copy the behaviour, not the code or prompts. HeyClicky is closed source. The open-source `farzaa/clicky` repo is MIT and only does seeing and pointing.

- Offers first, takes over only after a yes. An approval covers one conversation or step.
- Overlay text: "Hands off the mouse while I drive. Esc stops me."
- Action names: `screenshot`, `zoom` (with a region), `cursor_position`, `mouse_move`, `left_click`, `right_click`, `double_click`, `triple_click`, `type`, `keys`, `hold_key`, `scroll`, `wait`. Observation-only actions (screenshot, zoom, cursor_position) do not count as acting.
- Key aliases: cmd, command, win, super, meta, windows; opt, option, alt; esc, escape; return, enter; del, delete, forwarddelete, backspace; arrows; pageup, pagedown; space, spacebar.
- Stops and hands back when: Esc is pressed, the user moves the mouse (wait for it to rest), the user speaks, the front window refuses input (an app running as administrator), a safety check trips, the connection drops, a time limit passes.
- Refuses to type into password fields: "it's a password field, so type that one yourself".
- Releases any key it left held when the take-over ends.
- The model speaks one short sentence before each group of actions. The first one asks you to keep your hands off the mouse.
- The round limit and the base prompt are server side there. Here they are config and code.

## 4. Design for Zor UI

### 4.1 Trigger and consent
- New intent `Intent::Drive(goal)`, from phrases like "take over and ...", "do this for me: ...", "click ...". Keep it narrow, like `screen_question`: a general question must never start a take-over.
- Switch `buddies.drive`, off by default, in Settings > Buddies. Rows for round limit (`buddies.driveRounds`, default 25) and a time limit.
- A consent card near the buddy: the goal in plain words, **Go ahead** and **Cancel**. One approval per take-over, never remembered in v1.
- `Intent::Drive` while the switch is off says how to turn it on, like `SCREEN_OFF`.

### 4.2 The loop (`drive.rs`, new)
State: Idle, Asking, Running, Ending.
1. Capture with `screen::capture_all` (the pointer's monitor first).
2. Send the goal, the pictures and the actions so far to the model through `assist::chat` (provider agnostic).
3. Parse the reply, run the actions, speak the `say` line.
4. Stop on `done`, on the round or time limit, or on any hand-back trigger. Then tear down.

### 4.3 Model protocol: provider agnostic
Do not depend on a provider's computer-use tool. The Claude subscription path runs with tools off and the Gemini and ChatGPT command line tools take only text. Ask the model for plain JSON in its text reply:

```json
{"say": "I'll click Save.", "actions": [{"type": "left_click", "x": 640, "y": 360}], "done": false}
```

- Coordinates are in the picture's pixels, exactly as in Phase 1 (`screen.rs` labels give the size). Map them with `screen::to_screen`.
- Validate everything: unknown action types are ignored, coordinates are clamped, text length and actions per round are capped.
- A native computer-use adapter can be added later behind the same `Action` type. The Anthropic tool `computer_20251124` matches the action names above. Keep this optional.

### 4.4 Executor (`SendInput`)
- Mouse: absolute move with `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`, coordinates normalised over the virtual screen (0 to 65535). The app is per-monitor DPI aware, so use physical pixels. Test monitors at negative x (the second monitor is at x = -1920).
- Click, double and triple click, right click, drag, wheel (`MOUSEEVENTF_WHEEL`, `HWHEEL`).
- Text: `KEYEVENTF_UNICODE` per character. Do not use the clipboard (it overwrites the user's).
- Keys: virtual-key combos through an alias table. Always send key-up for every key-down, even on error.
- `webchat.rs` already has `key`, `send` and `bring_to_front` helpers, checked live against Edge (their callers' pure parts have unit tests, the `SendInput` calls themselves do not). Reuse them or move them into a shared module.

### 4.5 Safety layer (most important part)
1. Consent per take-over, and the switch off by default.
2. Overlay on every monitor: a visible border and "Hands off the mouse while I drive. Esc stops me." Exclude Zor's own windows from the captures with `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`. Check whether the BitBlt capture honours it. If not, switch capture to Windows.Graphics.Capture (this also fixes the Phase 1 limit that the bar and buddy show in screenshots).
3. Esc stops it: a low-level keyboard hook (`WH_KEYBOARD_LL`) while driving. Remove the hook at teardown.
4. User takes over: compare `GetCursorPos` with where the drive last put the pointer. HeyClicky's constants looked like about 24 px and 150 ms of rest. Treat those as starting points to tune. On a move, wait for the mouse to rest, then hand back.
5. User speaks (the hotkey) stops it.
6. Elevated front window: `SendInput` is blocked by UIPI. Detect it (the call returns 0, or compare process integrity) and stop with a clear message. Never leave a key down.
7. Password fields: before typing, ask UI Automation for the focused element and refuse if `IsPassword` is true.
8. Sends, deletes, payments: the prompt says to stop and ask. Add an `ask` action that pauses the loop and shows a consent card for that exact step. Consider a small app-side keyword guard as a second line.
9. Limits: round cap, wall time cap, actions per round cap.
10. Teardown always runs: release held keys, remove the hook, hide the overlay, restore the pointer if it was moved far.
11. Log each action to `zorui.log` with its screen coordinates.

### 4.6 Buddy
Reuse `Mode::Point`: the buddy flies to each click target and rings it before the click, then continues. One spoken line per group of actions. After the take-over he returns to the bar.

### 4.7 Settings (project rule)
Any setting that changes how something looks gets a live preview on the same page, drawn by the same code as the real feature. `buddies.drive` and the limits are not visual, so they are exempt. An overlay accent or border width would need a preview.

## 5. Milestones, each with tests

1. **Actions and mapping.** `Action` enum, JSON parsing and validation, key alias table, picture-to-screen mapping with negative monitors, scroll amounts. Pure functions and unit tests.
2. **Executor.** `SendInput` wrappers with guaranteed key release. Tests for the input structs built (do not send in tests).
3. **Safety layer.** Esc hook, user-mouse detection as a pure state machine (feed it positions and times), elevated-window and password checks, limits. Unit-test the state machine and the stop reasons.
4. **Loop and prompt.** `drive.rs`, the system prompt, reply parsing. Test the loop with a fake model and a fake executor (inject both).
5. **Consent card and overlay.** `win.rs` views. Render offscreen with `zorui.exe --render` where possible.
6. **Wire-up.** `Intent::Drive`, `buddies.drive` rows, config defaults in `conf.rs`, buddy flight to targets.
7. **Live test and hardening.** See section 7.

Rules from `CLAUDE.md` and the saved memory apply to every milestone: add tests for every change, run the full `cargo test`, read the diff, rebuild release, install and restart `zorui.exe`.

## 6. Pitfalls found in the Phase 1 session

- **Plan first.** A hook blocks edits until the user gives an explicit go-ahead for that round of edits. Write the plan, wait for "go". Do not work around the hook.
- **GlazeWM tiles every new window.** A test window ignores its requested size and position. Read its real rectangle with `GetWindowRect` after it appears, and put the target near its top-left so it stays visible.
- **Never click the user's live desktop blindly.** The user works at the same time. In one test a window opened by the session was covered by another app between steps, and a synthetic click copied text from that app and overwrote the user's clipboard. Re-check foreground and focus right before every synthetic key or click, or prefer `--render` for settings pages and `--bridge <cmd>` for data.
- **Installing the exe.** A supervisor restarts `zorui` right after it is stopped, so a copy over the running exe fails. Rename the running exe (`zorui.exe` to `zorui.exe.prev`), copy the new one in, stop only the main processes (not the `--agent` hook calls, which are short lived and belong to Claude Code), then start it. Remove `.prev`.
- **Do not re-theme.** Theme commands can flip the user's Windows dark/light mode (`switchwall.ps1`). The user is on dark mode. To preview a light theme, render offscreen with `USERPROFILE` pointed at a temp folder holding a copied config and fonts (`cp -rp`, so the font cache stays fresh).
- **Browser tab titles contain non-breaking spaces and a Memory usage suffix.** Match titles by letters, not by exact strings. `webchat::is_whatsapp_tab` shows how.
- **WhatsApp Web allows one live session.** A second tab shows "WhatsApp is open in another window" and drops the link's number and text. Always use the live tab. Verify the chat name and the text through UI Automation before pressing Enter.
- **The typed `--buddy` parser drops a trailing period** (`clean` trims punctuation). Voice transcription is similar.
- **Screenshots show the whole screen**, including other apps. Keep screen features off by default and tell the user what leaves the PC.
- **Contrast.** The theme colour is as pale as 1.7:1 on white in the dark theme. Draw anything that must be seen on arbitrary backgrounds with a dark and a white stroke, or on a pill.
- **`config.json` edits.** Read, change one key, write back, and check that a round trip with no change reproduces the file first. Other work edits it too.
- **Model reliability.** Small models (haiku) sometimes omit the screen number. Labels now say where each screen sits and the prompt demands the number, which fixed it in testing, but validate and fall back to the pointer's screen.

## 7. Live test protocol

Never test on real chats, real files or real accounts.

1. Build a throwaway target: a WinForms page (white, a button near the top-left) started with `Start-Process powershell -File`. Not hidden: a hidden launch hides the form.
2. Run the drive with a harmless goal ("click the Save button"). Confirm the click landed through the form's own state (the button changes a label), not only by a screenshot.
3. Check each hand-back trigger: press Esc, move the mouse, run the hotkey, switch the front window to an elevated one.
4. Check the second monitor (negative x) and a target near each screen edge.
5. Check that held keys are released (press nothing, then type normally).
6. Check the password refusal with a password box in the test form.
7. Check that the overlay and Zor's own windows are absent from the model's pictures.
8. Record a short contact sheet of the buddy's window during a drive to check the flight and ring (see the Phase 1 recording approach: enumerate windows titled `Zor UI / gojo`, crop each frame).

Acceptance: all hand-back triggers stop within about half a second, no key stays down, nothing is sent or deleted without its own consent, and the full test suite passes.

## 8. Decisions to make with the user before coding

- Which provider and model drives first. The Claude subscription path works with pictures through stdin stream-json. An API key path needs a key in Settings. Native computer-use tools can come later.
- Default round limit (25 suggested) and wall time limit (3 minutes suggested).
- Whether to hold a list of apps the drive must never touch (banking, password managers).
- Whether a take-over can start from voice only, or also from a typed command.

## 9. Start of the next session

```
cd native/zor-ui
cargo test                      # about 436 tests pass at the end of Phase 1
cargo build --release
zorui.exe --render settings:buddies out.png 1251 3300 1   # offscreen Settings page
zorui.exe --bridge ai-models                               # the model list
zorui.exe --buddy "where is the save button"               # same as speaking it
```

Read first: `src/screen.rs`, `src/assist.rs` (`screen_help`, `chat`, `Intent`), `src/buddy.rs` (`Mode::Point`), `src/webchat.rs` (the UI Automation and key-sending pattern), `src/bridge/web.rs` (`chat_request`, `can_see`), `src/settings/pages.rs`.
