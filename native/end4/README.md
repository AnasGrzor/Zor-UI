# Zor UI native shell (`end4.exe`)

The Zor UI widgets (formerly end4-pC) drawn by one small Windows process instead of a Zebar browser window per widget.
The bar on two monitors plus four desktop cards take about 32 MB working set (9 MB private) here;
the same widgets in Zebar used about 1 GB.

It reads the same files as the Zebar pack (`~/.glzr/zebar/end4-pC/config.json`, `theme.json`, the fonts in
`assets/fonts`). Card positions live in `%LOCALAPPDATA%\end4-pC\state.json`.

## What it draws

`config.json` → `"native"` lists the parts this app has taken over; Zebar skips them.

| Part | Key | Ported |
|---|---|---|
| Desktop cards | `"desktop": true` | clock (all 4 styles), weather, week, greeting, calendar, world clock, resources, text, image 1-3 |
| Bar | `"bar": true` | every module (layout, styles and groups from Settings > Bar), Dynamic Island, hover popups, tray |
| Gojo | `"gojo": true` | native only (`gojo.rs`): a small Gojo on the bar. He walks, sits on the bar's edge and sleeps after 3 idle minutes. Hover lifts his blindfold, click makes him talk, double-click fires Hollow Purple, and right-click turns on Infinity, which keeps the pointer 60 px away for 10 s. Drag him to move him. He hides behind full-screen apps. He also comments on new tracks, the volume, the battery, the network, the weather, notifications and a busy CPU. |
| Island panel | `"island-panel": true` | media, timer, notifications, agents, recording |
| Overview | `"overview": true` | windows, workspaces, clipboard history |
| Left / right sidebar | `"sidebar-left"`, `"sidebar-right"` | left: home, AI chat, translator, anime, football. Right: toggles, sliders, mixer, notifications, calendar, to-do, timer |
| Settings | `"settings": true` | every page, including the wallpaper and preset galleries |
| Dock, visualizer | `"dock"`, `"visualizer"` | both (each also has its own on/off switch in Settings) |
| Theme | `"theme": true` | wallpaper and color changes run in Rust (`src/bridge/theme.rs`); false uses `switchwall.ps1` |

Zebar is now only a fallback: every part above is native. A part whose key is false (or missing) is drawn by
Zebar instead, and the bar and GlazeWM's keys open it with `zebar start-widget`, like before. `desktop.js` keeps
the list of native cards in `NATIVE`; update both when a card is ported.

## Helper commands (`src/bridge`)

The Python and PowerShell helpers are ported module by module: `sys` (config, night light, mixer, dock icons),
`web` (secrets, translate, booru, AI chat, presets), `football`, `notify` (toasts, agents), `we` (Wallpaper
Engine), `media` (visualizer, recorder, file picker), `toggles` (toggles.ps1) and `theme` (switchwall.ps1 and the
Material You colors). Each command prints the same JSON as the script. One that is not ported yet (the color picker,
the screen translator, brightness and game mode, and the yande.re and danbooru sources, whose TLS handshake `curl.exe`
fails on some networks) runs the script. `end4.exe --bridge <command>` and `end4.exe --script <toggles|switchwall>`
run a command from a terminal; `END4_PYTHON_BRIDGE=1` forces the scripts.

## Opening popups

`end4.exe --open <name>` (name: `island-panel`, `overview`, `sidebar-left`, `sidebar-right`, `settings`) asks the
running instance (WM_COPYDATA to its message window) to open the popup, or close it if it's open. Native when the
part is in `"native"`, else Zebar's copy; with no instance running it starts Zebar's copy. `install.ps1` points the
GlazeWM keys at it, and the bridge has `native-open <name>` for Zebar's own code. Popups sit on the clicked bar's
monitor, else GlazeWM's focused one, take keyboard focus, and close on Esc or when they lose focus.

## Checking drawing without windows

`end4.exe --render <part> <out.png> [w h scale]` paints one view offscreen (real config and theme, sample media)
and writes a PNG: a desk card id, `bar`, `dock`, `visualizer`, or a popup name. w and h are CSS px.

## Shared state with Zebar

The Zebar widgets keep the timer, recording, agents and "open on this monitor" hints in localStorage.
With a native bar, `lib/core.js` mirrors those keys over `ws://127.0.0.1:6133/state` (`hub.rs`), and the
hub pushes every change back. Only Zebar's page origins may connect. A WebSocket because Zebar's request
caching swallows `fetch()` to that port.

The bar also runs the bridge streams the Zebar bar used to run (`agents-watch`, `notifications-watch`,
`we-sync`).

## Build

Needs Rust (`rustup`, the GNU toolchain is enough):

```
cargo build --release
```

`windows` is held at 0.58 and `Cargo.lock` pins tokio 1.40, parking_lot 0.12.3 and uuid 1.10: newer
releases link through `windows-link` (raw-dylib), which needs `dlltool` + an assembler that rustup's GNU
toolchain doesn't ship. Keep the lock file when updating.

`install.ps1` builds it when `cargo` is on PATH, copies `end4.exe` to `%LOCALAPPDATA%\end4-pC`, and turns on every missing
`"native"` key (`desktop`, `bar`, `visualizer`, `dock`, the popups). GlazeWM starts it with Zebar (`startup_commands`) and ignores its
windows (`window_rules`).

`end4.exe --all` draws every ported part whatever `"native"` says, for comparing next to Zebar.

## How it draws

- `gfx.rs`: tiny-skia for shapes and pictures, DirectWrite for text (variable font weights, Material Symbols
  ligatures), all on the CPU. Direct2D was tried first; its software mode loads WARP and cost ~10 MB more.
- `win.rs`: one layered window per widget (per-pixel alpha), repainted only when its content changes.
  Bars are app bars (they reserve 56 px like Zebar's `dockToEdge`).
- `app.rs`: shared state, a 1 s tick, config/theme reload on file change, jobs from background threads.
- Data: `glaze.rs` (GlazeWM IPC WebSocket), `media.rs` (media session, Core Audio), `sys.rs` (network,
  battery, keyboard, CPU, Luxon date tokens), `tray.rs` (systray-util, the crate Zebar uses), `weather.rs`.

Known limits: the bars must dock before the tray starts (systray-util's stand-in `Shell_TrayWnd` swallows
app bar messages), so bars rebuilt after a monitor change don't reserve their strip until a restart.
Tray icon tooltips and the bar visualizer aren't drawn yet.
