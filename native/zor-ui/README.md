# Zor UI native shell (`zorui.exe`)

The Zor UI widgets (formerly end4-pC) drawn by one small Windows process. The first version used a Zebar browser window per widget.
The bar on two monitors plus four desktop cards take about 32 MB working set (9 MB private) here;
the same widgets in Zebar used about 1 GB.

It reads `~/.glzr/zor-ui/config.json`, `theme.json` and the fonts in `assets/fonts`.
Card positions live in `%LOCALAPPDATA%\zor-ui\state.json`.

## What it draws

`config.json` → `"native"` turns a part on or off. Every part is on by default except the buddy.

| Part | Key | Ported |
|---|---|---|
| Desktop cards | `"desktop": true` | clock (all 4 styles), weather, week, greeting, calendar, world clock, resources, text, image 1-3 |
| Bar | `"bar": true` | every module (layout, styles and groups from Settings > Bar), Dynamic Island, hover popups, tray |
| Gojo and buddies | `"gojo": true` (buddy 1), `"buddies": {"b": {"on": true}}` (buddy 2) | native only (`buddy.rs`, `cast.rs`): small buddies on the bar. Settings > Buddies picks Gojo, Geto or Yuji for each, renames them, sets how much they talk, turns walking off, and makes the two partners, who walk to each other and trade lines. Gojo: he walks, sits on the bar's edge and sleeps after 3 idle minutes. Hover lifts his blindfold, click makes him talk, double-click fires Hollow Purple, and right-click turns on Infinity, which keeps the pointer 60 px away for 10 s. Drag him to move him. He hides behind full-screen apps. He also comments on new tracks, the volume, the battery, the network, the weather, notifications and a busy CPU. |
| Island panel | `"island-panel": true` | media, timer, notifications, agents, recording |
| Overview | `"overview": true` | windows, workspaces, clipboard history |
| Left / right sidebar | `"sidebar-left"`, `"sidebar-right"` | left: home, AI chat, translator, anime, football. Right: toggles, sliders, mixer, notifications, calendar, to-do, timer |
| Settings | `"settings": true` | every page, including the wallpaper and preset galleries |
| Dock, visualizer | `"dock"`, `"visualizer"` | both (each also has its own on/off switch in Settings) |
| Theme | `"theme": true` | wallpaper and color changes (`src/bridge/theme.rs`) |

## Helper commands (`src/bridge`)

`sys` (config, night light, mixer, dock icons), `web` (secrets, translate, booru, AI chat, presets), `football`,
`notify` (toasts, agents), `we` (Wallpaper Engine), `media` (visualizer, recorder, file picker), `picker` and
`screen_translate`, `listen` (voice: WASAPI mic and sherpa-onnx, whose DLLs download on first use), `agent_hook`
(`--agent` and `--install-agents` for coding-agent hooks), `toggles` and `theme` (switchwall and the Material You
colors). `zorui.exe --bridge <command>` and `zorui.exe --script <toggles|switchwall>` run a command from a terminal
and print its JSON.

## Opening popups

`zorui.exe --open <name>` (name: `island-panel`, `overview`, `sidebar-left`, `sidebar-right`, `settings`) asks the
running instance (WM_COPYDATA to its message window) to open the popup, or close it if it's open. `install.ps1` points the
GlazeWM keys at it. With no instance running it does nothing. Popups sit on the clicked bar's
monitor, else GlazeWM's focused one, take keyboard focus, and close on Esc or when they lose focus.

## Checking drawing without windows

`zorui.exe --render <part> <out.png> [w h scale]` paints one view offscreen (real config and theme, sample media)
and writes a PNG: a desk card id, `bar`, `dock`, `visualizer`, or a popup name. w and h are CSS px.

## Shared state

The timer, recording, agents and "open on this monitor" hints live in `hub.rs` and are saved to
`%LOCALAPPDATA%\zor-ui\shared.json`.

The bar runs the bridge streams `agents-watch`, `notifications-watch` and `we-sync`.

## Build

Needs Rust (`rustup`, the GNU toolchain is enough):

```
cargo build --release
```

`windows` is held at 0.58 and `Cargo.lock` pins tokio 1.40, parking_lot 0.12.3 and uuid 1.10: newer
releases link through `windows-link` (raw-dylib), which needs `dlltool` + an assembler that rustup's GNU
toolchain doesn't ship. Keep the lock file when updating.

`install.ps1` copies the prebuilt `zorui.exe` (or builds it when `cargo` is on PATH) to `%LOCALAPPDATA%\zor-ui`.
GlazeWM starts it (`startup_commands`) and ignores its windows (`window_rules`).

`zorui.exe --all` draws every part whatever `"native"` says.

## How it draws

- `gfx.rs`: tiny-skia for shapes and pictures, DirectWrite for text (variable font weights, Material Symbols
  ligatures), all on the CPU. Direct2D was tried first; its software mode loads WARP and cost ~10 MB more.
- `win.rs`: one layered window per widget (per-pixel alpha), repainted only when its content changes.
  Bars are app bars (they reserve 56 px).
- `app.rs`: shared state, a 1 s tick, config/theme reload on file change, jobs from background threads.
- Data: `glaze.rs` (GlazeWM IPC WebSocket), `media.rs` (media session, Core Audio), `sys.rs` (network,
  battery, keyboard, CPU, Luxon date tokens), `tray.rs` (systray-util), `weather.rs`.

Known limits: the bars must dock before the tray starts (systray-util's stand-in `Shell_TrayWnd` swallows
app bar messages), so bars rebuilt after a monitor change don't reserve their strip until a restart.
Tray icon tooltips and the bar visualizer aren't drawn yet.
