# Zor UI

Zor UI is a desktop shell for Windows 10 and 11. One small native process (`end4.exe`, written in Rust) draws a
Material You bar with a Dynamic Island, sidebars with AI chat, an overview, a settings app, desktop widgets, a
dock, a visualizer and small animated buddies that live on your bar. Colors come from your wallpaper, and
[GlazeWM](https://github.com/glzr-io/glazewm) does the tiling.

It started as a port of [end4-pC](https://github.com/pctrade/end4-pC), pctrade's fork of
[illogical-impulse](https://github.com/end-4/dots-hyprland), and it still follows that look and workflow. It has
since grown its own parts: the native renderer, the island's AI agent status, the buddies, and a Claude usage
watcher.

![Desktop](docs/desktop-dark.jpg)

| AI chat | Translator | Anime | Football |
|:---:|:---:|:---:|:---:|
| ![AI](docs/left-ai.jpg) | ![Translator](docs/left-translate.jpg) | ![Anime](docs/left-anime.jpg) | ![Football](docs/left-football.jpg) |

| Settings | Wallpaper Engine library | Presets gallery |
|:---:|:---:|:---:|
| ![Settings](docs/settings.jpg) | ![Wallpaper Engine](docs/settings-wallpapers.jpg) | ![Presets](docs/settings-presets.jpg) |

| Light mode | Overview |
|:---:|:---:|
| ![Light](docs/desktop-light.jpg) | ![Overview](docs/overview.jpg) |

> Screenshots were rendered from the real widget code with sample data.

## What it has

- **Bar and Dynamic Island.** Workspaces, clock, weather, system icons, tray, media and the active window, with a
  layout you can rearrange by drag and drop. The island expands for the song, timer, notifications, screen
  recording and AI agents.
- **AI agents in the island.** Claude Code, Codex, Gemini CLI or anything that can pipe JSON to
  `scripts/end4_agent.py` shows up when it needs a permission or an answer, with its usage limits in the panel.
- **Sidebars.** Left: player with lyrics, AI chat (Gemini, Claude, OpenAI, Mistral, OpenRouter, Ollama), translator,
  anime and wallpaper search, football. Right: toggles, brightness and volume sliders, per-app mixer,
  notifications, calendar, to-do, pomodoro and stopwatch.
- **Buddies.** Small animated characters that walk on your bar, talk, comment on what your PC is doing and keep
  each other company. See [Buddies](#buddies).
- **Settings app.** Every option, plus galleries for Wallpaper Engine wallpapers and community presets.
- **Desktop widgets, dock, overview, visualizer.** Draggable desktop cards, a dock, a window and clipboard
  overview, and an audio visualizer.
- **Theming.** Material You colors generated from your wallpaper, applied to the bar, Windows light and dark mode,
  Windows Terminal and Flow Launcher.
- **Tools.** Screen recording, screen translate (OCR), color picker, night light, game mode, brightness for
  external monitors over DDC/CI, and a power plan switcher.

## Buddies

Buddies are small characters that stand on the bar, sit on its edge and on the top of open windows, sleep when you
leave, and fall back onto the bar if you drop them. Turn them on in **Settings > Buddies**.

| Setting | What it does |
|---|---|
| Show on the bar | Two slots. Buddy 1 is `"native": {"gojo": true}` in `config.json`, buddy 2 is `"buddies": {"b": {"on": true}}`. |
| Character | Gojo, Geto or Yuji. Each has its own look, lines and double-click move. |
| Name | What their partner calls them. Empty uses the character's name. |
| Talks | Quiet (no comments or chat), Normal or Chatty. |
| Walks around | Off keeps a buddy in place, sitting or standing. |
| Make them partners | With both on, they walk next to each other, face each other and trade lines. |

**What you can do to a buddy.** Hover to get their attention, click to hear a line, double-click for their move
(Gojo fires Hollow Purple), and drag to carry them. Gojo's right-click turns on Infinity, which keeps the pointer
60 px away for 10 seconds. A buddy hides behind full-screen apps.

**What they notice.** A new song, a volume or mute change, a low battery or a charger change, the network going
down or coming back, rain, snow or thunder, a new notification, and a busy CPU. With two buddies on, only buddy 1
comments on the desktop, so you don't hear every remark twice.

**Claude usage.** Gojo warns you when your Claude plan usage crosses 80% and again at 95%, whichever of the
5-hour and 7-day windows is higher. The numbers come from the same endpoint Claude Code's `/usage` reads, using the
sign-in Claude Code already stores in `~/.claude/.credentials.json`. The check runs every 3 minutes, only while a
buddy is on, and the token goes to `curl.exe` through stdin, so it never shows in the process list. If Claude Code
isn't signed in, or its token has expired, the warning stays quiet until Claude Code runs again. The endpoint is
undocumented and could change.

**Voice.** Buddies can read their lines aloud with a Windows voice. It's off by default. Add
`"gojoVoice": true` under `"native"` in `config.json` to turn it on. It uses an English Windows voice, so it doesn't
sound like any character, and each new line cuts off the one still playing.

The characters are drawn from shapes in `native/end4/src/gojo.rs`, with palettes and lines in `cast.rs`. Gojo,
Geto and Yuji belong to their rights holders. They are here as personal fan art.

## How it is built

The first version put every widget in its own [Zebar](https://github.com/glzr-io/zebar) browser window. The bar on
two monitors plus four desktop cards used about 1 GB of memory. The native shell draws the same widgets in one
process and uses about 32 MB.

| Part | Technology |
|---|---|
| Bar, island, sidebars, overview, settings, desktop cards, dock, visualizer, buddies | `end4.exe` (Rust). CPU drawing with tiny-skia and DirectWrite, one layered window per widget. |
| Tiling, workspaces, keybinds | [GlazeWM](https://github.com/glzr-io/glazewm) |
| Material You colors | The same algorithm as end4's `generate_colors_material.py`, in Rust (`src/bridge/theme.rs`) and in `scripts/end4_theme.py` |
| Fallback widgets | A [Zebar](https://github.com/glzr-io/zebar) widget pack (`zebar/zor-ui`). Any part not listed under `"native"` in `config.json` is drawn here instead. |
| Helper commands (AI chat, translate, booru, football, notifications, Wallpaper Engine, media) | Rust modules in `native/end4/src/bridge`. A few still run as Python or PowerShell scripts. |

Developer notes, build steps and the list of ported parts are in [native/end4/README.md](native/end4/README.md).

## Relation to end4-pC

end4-pC is about 100k lines of QML on **Quickshell**, which only runs on Linux and BSD, and it relies on
Hyprland, PipeWire, BlueZ, UPower, NetworkManager and MPRIS. Zor UI rebuilds the design with Windows tools:

| end4-pC on Hyprland | Zor UI on Windows |
|---|---|
| Hyprland tiling, workspaces, keybinds | [GlazeWM](https://github.com/glzr-io/glazewm) |
| Quickshell bar, sidebars, overview, settings, desktop widgets | The native `end4.exe`, with Zebar widgets as a fallback |
| `switchwall.sh` | A Rust port of `scripts/switchwall.ps1` (wallpaper, colors, Windows light and dark, lock screen picture) |
| mpvpaper live wallpapers | **Wallpaper Engine** (if installed through Steam) or [Lively Wallpaper](https://github.com/rocksdanister/lively), themed from a frame or preview |
| cava | WASAPI loopback and FFT, drawn natively |
| AI chat (Gemini, Mistral, Ollama…) | The same providers plus Claude, OpenAI and OpenRouter |
| Translator (`trans`, Google) | Google Translate |
| Anime (booru) and online wallpapers | yande.re, Konachan, Danbooru and Wallhaven |
| Football (`football.py`, ESPN) | end4's own `football.py` logic, ported |
| Notifications (Quickshell server) | Windows' notification history, shown in the right sidebar and the island |
| Night light (hyprsunset) | Gamma ramp on every display, the way f.lux does it |
| Presets (end4-pCpresets) | The same gallery: wallpaper, scheme, workspace labels and transparency get applied |
| Fuzzel / search launcher | [Flow Launcher](https://github.com/Flow-Launcher/Flow.Launcher) with a generated theme |
| Terminal colors | A Windows Terminal color scheme named `zor-ui` |
| Google Sans Flex + Material Symbols Rounded | Same fonts, bundled in `zebar/zor-ui/assets/fonts` |

## Install

The step-by-step guide, with requirements, Rust setup and troubleshooting, is in [docs/INSTALL.md](docs/INSTALL.md).
In short, run this in PowerShell from the folder you unzipped:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

If you use Wallpaper Engine, that's all: Zor UI finds it in your Steam libraries. Add `-WithLively` only if you
don't have Wallpaper Engine and want video wallpapers.

| Flag | What it does |
|---|---|
| `-Mod win` | Use the Windows key like end4's Super key. The default is `alt`, because Windows reserves some Win combos (Win+L, for example). |
| `-WithLively` | Also install Lively Wallpaper, for video wallpapers without Wallpaper Engine. |
| `-HideTaskbar` | Turn on taskbar auto-hide so only the Zor UI bar shows. |
| `-Wallpaper C:\path\wall.jpg` | Image (or video) to theme from. The default is your current wallpaper. |
| `-UserName Anas` | Name shown in the greeting. |
| `-Workspaces 10` | Number of workspace dots in the bar. |
| `-SkipDeps` | Skip winget (if GlazeWM, Zebar, Flow Launcher and Python are already installed). |

The installer:
1. Installs GlazeWM, Zebar, Flow Launcher, Python 3.12, JetBrains Mono Nerd Font (and Lively with `-WithLively`) using winget.
2. Copies the scripts to `%LOCALAPPDATA%\zor-ui` and creates a Python venv with `materialyoucolor`, `pillow`, `numpy`, `soundcard`, `imageio-ffmpeg` and `psutil`.
3. Builds `end4.exe` when `cargo` is on PATH, copies it to `%LOCALAPPDATA%\zor-ui` and turns on every native part. Without Rust it falls back to the Zebar widgets.
4. Installs the Zebar widget pack to `~\.glzr\zebar\zor-ui`.
5. Writes `~\.glzr\glazewm\config.yaml`. Your old config is backed up as `config.yaml.bak-<date>`.
6. Selects the Zor UI Flow Launcher theme, generates colors from your wallpaper, and starts GlazeWM, which starts the shell.

Re-running `install.ps1` updates everything and keeps your `config.json` settings. If you installed it under its old name, `end4-pC`, it moves your settings, saved state and Python environment to the new `zor-ui` folders first.
`uninstall.ps1` restores the backed-up configs, turns night light off, and removes everything it added.

## Keybinds

`Mod` is Alt by default, or Win with `-Mod win`.

| Keys | Action |
|---|---|
| `Mod+A` | Left sidebar: Home (player, lyrics), AI, Translator, Anime, Football |
| `Mod+N` | Right sidebar: toggles, sliders, notifications, calendar, to-do, pomodoro and stopwatch |
| `Mod+I` | Settings (colors, presets gallery, buddies, API keys, options) |
| `Mod+/` | Keybinds cheatsheet (also Settings > Keybinds) |
| ``Alt+` `` (or `Win+Tab`) | Overview |
| `Alt+Space` | Launcher (Flow Launcher) |
| `Mod+Ctrl+T` | Pick a wallpaper (image or video) and regenerate colors |
| `Mod+Ctrl+D` | Toggle dark / light |
| `Mod+Ctrl+R` | Restart the bar and reload GlazeWM |
| `Mod+Shift+G` | Game mode: stop the bar, widgets, launcher and live wallpaper to free RAM. Press again to bring them back. |
| `Mod+Enter` or `Mod+T` | Terminal |
| `Mod+E` / `Mod+W` / `Mod+C` | File Explorer / Edge / VS Code |
| `Mod+Shift+S` | Screen snip |
| `Mod+Q` / `Mod+F` / `Mod+Shift+Space` | Close / fullscreen / float |
| `Mod+1…0` / `Mod+Shift+1…0` | Go to workspace / move window there |
| `Mod+Arrows` / `Mod+Shift+Arrows` | Focus / move |
| `Mod+R` | Resize mode (arrows, Esc to leave) |

In the sidebars, `Ctrl+PageUp/PageDown` switches tabs, the pin button keeps a sidebar open, and `Esc` closes it.

## Using it

**AI chat.** Add a key once, either in Settings > Services or by typing `/key gemini YOUR_KEY` in the chat
(also `anthropic`, `openai`, `mistral` or `openrouter`). Keys are encrypted with Windows DPAPI for your
account. If [Ollama](https://ollama.com) is running, its models show up automatically. Other commands:
`/model`, `/prompt`, `/temp`, `/clear`, `/help`.

**AI agents.** Run `python scripts/end4_agent.py --install` to add the hooks and status line to Claude Code and the
hooks to Codex. It backs up the files it changes. After that, the island shows an agent that is waiting for you,
and the island panel shows its 5-hour and 7-day usage. Any other agent can report by piping JSON into
`end4_agent.py <agent>`. The format is described at the top of that script.

**Anime and wallpapers.** Pick a source and search by tags. Hover an image to open its post, download it
to `Pictures\Wallpapers`, or set it as your wallpaper (which also re-themes everything). Results are
safe-rated unless you turn on **Allow NSFW** in Settings > Services.

**Football.** Fixtures for the next week, live scores (refreshed every minute), win/draw/loss
probabilities and league tables. Click a match for expected goals and corners. Choose leagues in Settings.

**Timer.** The right sidebar's Timer tab has a pomodoro (25/5/15 minutes by default, set in Settings > Bar &
desktop) and a stopwatch with laps. Both keep running when the sidebar closes. While one runs, the bar shows
its time. When a pomodoro phase ends, Windows shows a notification and the bar chip turns into a reminder.

**Desktop widgets.** Settings > Bar & desktop picks the cards shown on the desktop: clock, weather, week,
greeting, media, calendar, to-do (the same list as the sidebar), notes, timer, world clock, resources, lyrics,
image cards and custom text. The clock has four styles: cookie, analog, digital and pixel. Each card is its own
window: drag it anywhere, on either monitor, and it stays there. **Reset layout** puts every card back in a
column on the left.

**Right sidebar tools.** **App volume** sets each app's volume and mute. **Screen record** records the primary
monitor (video only) to `Videos\Recordings`; a red chip in the bar shows the time and stops it. **Screen
translate** freezes the screen: drag over some text, and Windows' OCR reads it and the translation (into the
translator's language) is drawn on top and copied. Windows only reads languages added in Settings > Time &
language > Language. **Color picker** copies the hex code of any pixel, and **Power plan** switches between
Windows' power plans. The brightness slider acts on the monitor the sidebar is on, including external monitors
that support DDC/CI.

**Bar.** Hover the clock, CPU/RAM, weather, network speed or system icons for details. When the volume or
microphone changes, the island shows the new level for a moment.

**Overview.** Type `;` (or press the paste button) to see clipboard history (`Win+V` must be turned on) and
copy an item back.

**Dock.** Settings > Bar & desktop > Dock adds a dock along the bottom: pinned apps, then every app with an open
window. Click focuses an app (again cycles its windows) or starts a pinned one; right-click pins or unpins it.
It's off by default because Windows' taskbar does the same job.

**Wallpaper Engine.** Settings > Wallpapers lists everything you've subscribed to on the Workshop, plus your own
projects. Search by name or tag, filter by type, and click one to set it. Colors come from a frame for video
wallpapers and from the preview image for scenes and web wallpapers. With **Follow Wallpaper Engine** on (the
default), Zor UI also re-themes when you change wallpaper in Wallpaper Engine's own window or a playlist moves
on. **Stop live wallpaper** returns to your normal wallpaper, and so does picking an image with `Mod+Ctrl+T`.

**Presets.** Settings > Presets lists the community gallery. **Apply** downloads the preset's wallpaper and
applies its color scheme, workspace labels (for example 一 二 三) and transparency. You can also save your own
setup and switch back later.

## How some of these work on Windows

- **Notifications** come from Windows' notification history, the same data Action Center shows.
  Dismissing one hides it in the sidebar. To clear it in Windows too, use **Notification center**.
- **Night light** warms the screen through the display gamma, the way f.lux does. If a display or HDR
  mode refuses it, the toggle opens Windows' own Night light settings instead.
- **Lock screen.** Windows' secure lock screen can't be replaced by another app. Instead, your wallpaper is copied
  to the Windows lock screen whenever it changes. You can turn this off in Settings, and some managed PCs block it.
- **Live wallpapers** use Wallpaper Engine's command line (`openWallpaper`, `closeWallpaper`, `getWallpaper`),
  so Zor UI works alongside it rather than replacing it. For scene and web wallpapers, only the preview image is
  available as a still, so the colors change but the Windows wallpaper and lock screen stay as they are. Without
  Wallpaper Engine, videos play in Lively. If Wallpaper Engine is installed outside your Steam libraries, set
  `wallpaperEngine.path` in `config.json`.
- **Visualizer** captures what your speakers are playing (WASAPI loopback). Apps that use exclusive mode are
  off-limits to it.

## Customize

Everything is in **Settings** (`Mod+I`). It edits `~\.glzr\zebar\zor-ui\config.json`, which you can also open
from Settings > Config file. The `"native"` section lists which parts the native shell draws. Command-line
equivalents for theming:

- `switchwall.ps1 -Scheme expressive` (also `tonal-spot`, `vibrant`, `content`, `fidelity`, `fruit-salad`, `monochrome`, `neutral`, `rainbow`, `auto`)
- `switchwall.ps1 -Color '#6750A4'` themes from a color and keeps your wallpaper
- In Windows Terminal, pick the **zor-ui** color scheme once. It updates every time the colors change.

## Still not done

- end4's AI **tools**: there, the AI can run shell commands and edit the shell config. Here it only chats, on purpose.
- Uploading your own preset to the gallery. The gallery stores Linux configs, so Windows setups are saved locally instead.
- Hyprland-only settings pages (animations, decorations, monitor layout). GlazeWM's equivalents live in its `config.yaml`.
- A do-not-disturb toggle. Windows 11 has no API for it.
- Wi-Fi and Bluetooth device lists. Use the toggle's right-click, which opens Windows Settings.
- Recording sound with the screen. The recorder captures video only.
- More than two buddies, other languages for their lines, and a custom voice.
- Tray icon tooltips, and bars rebuilt after a monitor change only reserve their strip after a restart.

## Requirements and notes

- Windows 10 or 11. Zebar's fallback widgets need WebView2 (built into Windows 11; on Windows 10, install the
  WebView2 runtime if Zebar fails to start).
- Building the native shell needs Rust. The GNU toolchain is enough.
- The fallback widgets load Zebar's client library from esm.sh the first time they start. Zebar then caches it for a week.
- Those widgets can only run four scripts: `toggles.ps1`, `switchwall.ps1`, `media-art.ps1` and `end4_bridge.py`,
  and only from the paths `install.ps1` writes into `zpack.json`.

## License

GPL-3.0, the same as end4-pC. `scripts/football.py` is end4-pC's, and the color generation is adapted from its
`generate_colors_material.py`. Google Sans Flex is under the SIL Open Font License, and Material Symbols is under
Apache 2.0 (license files are in `assets/fonts`).
