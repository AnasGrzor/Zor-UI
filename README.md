# end4-pC for Windows

A Windows port of [end4-pC](https://github.com/pctrade/end4-pC), pctrade's fork of
[illogical-impulse](https://github.com/end-4/dots-hyprland). It recreates the look and
workflow on Windows 10/11: a Material You bar, sidebars with AI chat, a translator, anime,
football and notifications, an overview, a settings app with the community presets gallery,
desktop widgets, a cava-style visualizer, live wallpapers (Wallpaper Engine or Lively), tiling,
and colors generated from your wallpaper.

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

## Why it isn't a straight port

end4-pC is about 100k lines of QML running on **Quickshell**, which only supports Linux and BSD.
It also relies on Hyprland, PipeWire, BlueZ, UPower, NetworkManager and MPRIS. This port rebuilds
the same design with Windows tools:

| end4-pC on Hyprland | Windows port |
|---|---|
| Hyprland tiling, workspaces, keybinds | [GlazeWM](https://github.com/glzr-io/glazewm) |
| Quickshell bar, sidebars, overview, settings, desktop widgets | [Zebar](https://github.com/glzr-io/zebar) widget pack (`zebar/end4-pC`) |
| Material You colors (`generate_colors_material.py`) | The same color code in `scripts/end4_theme.py`, so palettes match |
| `switchwall.sh` | `scripts/switchwall.ps1` (wallpaper, colors, Windows light/dark, lock screen picture) |
| mpvpaper live wallpapers | **Wallpaper Engine** (if installed through Steam) or [Lively Wallpaper](https://github.com/rocksdanister/lively), themed from a frame or preview |
| cava | `end4_bridge.py visualizer`: WASAPI loopback + FFT, drawn by Zebar |
| AI chat (Gemini, Mistral, Ollama…) | Same providers plus Claude, OpenAI and OpenRouter, streamed through `end4_bridge.py` |
| Translator (`trans`, Google) | Google Translate through `end4_bridge.py` |
| Anime (booru) and online wallpapers | yande.re, Konachan, Danbooru and Wallhaven |
| Football (`football.py`, ESPN) | end4's own `football.py`, unchanged apart from its cache folder |
| Notifications (Quickshell server) | Windows' notification history, shown in the right sidebar |
| Night light (hyprsunset) | Gamma ramp on every display (the way f.lux does it) |
| Presets (end4-pCpresets) | The same gallery: wallpaper, scheme, workspace labels and transparency get applied |
| Fuzzel / search launcher | [Flow Launcher](https://github.com/Flow-Launcher/Flow.Launcher) with a generated theme |
| Terminal colors | A Windows Terminal color scheme named `end4-pC` |
| Google Sans Flex + Material Symbols Rounded | Same fonts, bundled in `zebar/end4-pC/assets/fonts` |

## Install

Run this in PowerShell from the folder you unzipped:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

If you use Wallpaper Engine, that's all: end4 finds it in your Steam libraries. Add `-WithLively` only if you don't have Wallpaper Engine and want video wallpapers.

| Flag | What it does |
|---|---|
| `-Mod win` | Use the Windows key like end4's Super key. The default is `alt`, because Windows reserves some Win combos (Win+L, for example). |
| `-WithLively` | Also install Lively Wallpaper, for video wallpapers without Wallpaper Engine. |
| `-HideTaskbar` | Turn on taskbar auto-hide so only the end4 bar shows. |
| `-Wallpaper C:\path\wall.jpg` | Image (or video) to theme from. The default is your current wallpaper. |
| `-UserName Anas` | Name shown in the greeting. |
| `-Workspaces 10` | Number of workspace dots in the bar. |
| `-SkipDeps` | Skip winget (if GlazeWM, Zebar, Flow Launcher and Python are already installed). |

The installer:
1. Installs GlazeWM, Zebar, Flow Launcher, Python 3.12, JetBrains Mono Nerd Font (and Lively with `-WithLively`) using winget.
2. Copies the scripts to `%LOCALAPPDATA%\end4-pC` and creates a Python venv with `materialyoucolor`, `pillow`, `numpy`, `soundcard`, `imageio-ffmpeg` and `psutil`.
3. Installs the widget pack to `~\.glzr\zebar\end4-pC`. The bar, desktop widgets and visualizer start with Zebar.
4. Writes `~\.glzr\glazewm\config.yaml`. Your old config is backed up as `config.yaml.bak-<date>`.
5. Selects the end4-pC Flow Launcher theme, generates colors from your wallpaper, and starts GlazeWM (which starts Zebar).

Re-running `install.ps1` updates everything and keeps your `config.json` settings.
`uninstall.ps1` restores the backed-up configs, turns night light off, and removes everything it added.

## Keybinds

`Mod` is Alt by default, or Win with `-Mod win`.

| Keys | Action |
|---|---|
| `Mod+A` | Left sidebar: Home (player, lyrics), AI, Translator, Anime, Football |
| `Mod+N` | Right sidebar: toggles, sliders, notifications, calendar, to-do, pomodoro and stopwatch |
| `Mod+I` | Settings (colors, presets gallery, API keys, options) |
| `Mod+/` | Keybinds cheatsheet (also Settings → Keybinds) |
| ``Alt+` `` (or `Win+Tab`) | Overview |
| `Alt+Space` | Launcher (Flow Launcher) |
| `Mod+Ctrl+T` | Pick a wallpaper (image or video) and regenerate colors |
| `Mod+Ctrl+D` | Toggle dark / light |
| `Mod+Ctrl+R` | Restart the bar and reload GlazeWM |
| `Mod+Enter` or `Mod+T` | Terminal |
| `Mod+E` / `Mod+W` / `Mod+C` | File Explorer / Edge / VS Code |
| `Mod+Shift+S` | Screen snip |
| `Mod+Q` / `Mod+F` / `Mod+Shift+Space` | Close / fullscreen / float |
| `Mod+1…0` / `Mod+Shift+1…0` | Go to workspace / move window there |
| `Mod+Arrows` / `Mod+Shift+Arrows` | Focus / move |
| `Mod+R` | Resize mode (arrows, Esc to leave) |

In the sidebars, `Ctrl+PageUp/PageDown` switches tabs, the pin button keeps a sidebar open, and `Esc` closes it.

## Using the sidebar features

**AI chat.** Add a key once, either in Settings → Services or by typing `/key gemini YOUR_KEY` in the chat
(also `anthropic`, `openai`, `mistral` or `openrouter`). Keys are encrypted with Windows DPAPI for your
account, and the widgets never see them. If [Ollama](https://ollama.com) is running, its models
show up automatically. Other commands: `/model`, `/prompt`, `/temp`, `/clear`, `/help`.

**Anime and wallpapers.** Pick a source and search by tags. Hover an image to open its post, download it
to `Pictures\Wallpapers`, or set it as your wallpaper (which also re-themes everything). Results are
safe-rated unless you turn on **Allow NSFW** in Settings → Services.

**Football.** Fixtures for the next week, live scores (refreshed every minute), win/draw/loss
probabilities and league tables. Click a match for expected goals and corners. Choose leagues in Settings.

**Timer.** The right sidebar's Timer tab has a pomodoro (25/5/15 minutes by default, set in Settings → Bar &
desktop) and a stopwatch with laps. Both keep running when the sidebar closes. While one runs, the bar shows
its time. When a pomodoro phase ends, Windows shows a notification and the bar chip turns into a reminder.
Click the chip to open the timer.

**Desktop widgets.** Settings → Bar & desktop picks the cards shown on the desktop. The choices are clock,
weather, week, greeting, media, calendar, to-do (the same list as the sidebar), notes, timer, world clock,
resources and custom text. The clock has four styles: cookie, analog, digital and pixel. Each card is its own
window: drag it anywhere, on either monitor, and it stays there. **Reset layout** in the same page puts every
card back in a column on the left.

**Right sidebar tools.** **App volume** under the sliders sets each app's volume and mute. **Screen record**
records the primary monitor (video only) to `Videos\Recordings`; a red chip in the bar shows the time and stops
it. **Screen translate** freezes the screen: drag over some text, and Windows' OCR reads it and the translation
(into the translator's language) is drawn on top and copied. Windows only reads languages added in Settings >
Time & language > Language. **Color picker** copies the hex code of any pixel, and **Power plan** switches
between Windows' power plans.

**Bar.** Hover the clock, CPU/RAM, weather, network speed or system icons for details. When the volume or
microphone changes, the media pill shows the new level for a moment (end4's Dynamic Island).

**Overview.** Type `;` (or press the paste button) to see clipboard history (`Win+V` must be turned on) and
copy an item back.

**Dock.** Settings > Bar & desktop > Dock adds end4's dock along the bottom: pinned apps, then every app
with an open window. Click focuses an app (again cycles its windows) or starts a pinned one; right-click pins
or unpins it. It's off by default because Windows' taskbar does the same job.

**Wallpaper Engine.** Settings → Wallpapers lists everything you've subscribed to on the Workshop, plus
your own projects. Search by name or tag, filter by type, and click one to set it. end4 takes its colors from
the wallpaper: a frame for video wallpapers, the preview image for scenes and web wallpapers. With **Follow
Wallpaper Engine** on (the default), end4 also re-themes when you change wallpaper in Wallpaper Engine's own
window or a playlist moves on. **Stop live wallpaper** returns to your normal wallpaper, and so does picking an
image with `Mod+Ctrl+T`. Video files you pick with `Mod+Ctrl+T` play in Wallpaper Engine too.

**Presets.** Settings → Presets lists the community gallery. **Apply** downloads the preset's wallpaper and
applies its color scheme, workspace labels (for example 一 二 三) and transparency. You can also save your own setup and switch back later.

## How some of these work on Windows

- **Notifications** come from Windows' notification history, the same data Action Center shows.
  Dismissing one hides it in the sidebar. To clear it in Windows too, use **Notification center**.
- **Night light** warms the screen through the display gamma, the way f.lux does. If a display or HDR
  mode refuses it, the toggle opens Windows' own Night light settings instead.
- **Lock screen.** Windows' secure lock screen can't be replaced by another app, so end4's custom lock
  screen isn't possible. Instead, your wallpaper is copied to the Windows lock screen whenever it changes.
  You can turn this off in Settings, and some managed PCs block it.
- **Live wallpapers** use Wallpaper Engine's command line (`openWallpaper`, `closeWallpaper`, `getWallpaper`),
  so end4 works alongside it rather than replacing it. For scene and web wallpapers, only the preview image is
  available as a still, so the colors change but the Windows wallpaper (and lock screen) stays as it is. Video
  wallpapers give a full-size frame, which is also used for both. Without Wallpaper Engine, videos play in Lively.
  If Wallpaper Engine is installed outside your Steam libraries, set `wallpaperEngine.path` in `config.json`.
- **Visualizer** captures what your speakers are playing (WASAPI loopback). It's off-limits to apps that
  use exclusive mode. Toggle it, or add a small one in the bar, in Settings → Bar & desktop.

## Customize

Everything is in **Settings** (`Mod+I`). It edits `~\.glzr\zebar\end4-pC\config.json`, which you can
also open from Settings → Config file. Command-line equivalents:

- `switchwall.ps1 -Scheme expressive` (also `tonal-spot`, `vibrant`, `content`, `fidelity`, `fruit-salad`, `monochrome`, `neutral`, `rainbow`, `auto`)
- `switchwall.ps1 -Color '#6750A4'` themes from a color and keeps your wallpaper
- In Windows Terminal, pick the **end4-pC** color scheme once. It updates every time the colors change.

## Still not ported

- end4's AI **tools**: there, the AI can run shell commands and edit the shell config. Here it only chats, on purpose.
- Uploading your own preset to the gallery. The gallery stores Linux configs, so Windows setups are saved locally instead.
- Hyprland-only settings pages (animations, decorations, monitor layout). GlazeWM's equivalents live in its `config.yaml`.
- A do-not-disturb toggle. Windows 11 has no API for it.
- Wi-Fi and Bluetooth device lists. Use the toggle's right-click, which opens Windows Settings.
- Recording sound with the screen. The recorder captures video only.

## Requirements and notes

- Windows 10 or 11 with WebView2 (built into Windows 11; if Zebar fails to start on Windows 10, install the WebView2 runtime).
- Widgets load Zebar's client library from esm.sh the first time they start. After that, Zebar caches it for a week.
- Widgets can only run four scripts: `toggles.ps1`, `switchwall.ps1`, `media-art.ps1` and `end4_bridge.py`, and only from the paths `install.ps1` writes into `zpack.json`.

## License

GPL-3.0, the same as end4-pC. `scripts/football.py` is end4-pC's, and the color generation is adapted from its
`generate_colors_material.py`. Google Sans Flex is under the SIL Open Font License, and Material Symbols is under Apache 2.0 (license files are in `assets/fonts`).
