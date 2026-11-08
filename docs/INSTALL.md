# Install and run Zor UI

Zor UI is a Windows desktop shell built on GlazeWM (tiling) and a native Rust
process, `zorui.exe`, that draws the bar, Dynamic Island, sidebars, overview, settings and desktop cards.
Its look takes inspiration from [end4-pC](https://github.com/pctrade/end4-pC) by pctrade, which is based on
[illogical-impulse](https://github.com/end-4/dots-hyprland) by end-4. Some parts are ported from end4-pC; the README
lists them under Credits.

If you installed it under its old name, `end4-pC`, re-running `install.ps1` moves your settings and saved state
to the new `zor-ui` folders and updates the AI agent hooks.

## What you need

| Requirement | Why | Notes |
|---|---|---|
| Windows 10 or 11 | | |
| winget | Installs the apps below | Comes with "App Installer" in the Microsoft Store. Skip it with `-SkipDeps` if you install the apps yourself. |
| Windows PowerShell 5.1 | Runs `install.ps1` | Already part of Windows. |
| Rust (rustup) | Builds `zorui.exe` | Only when you install from a clone. The one-click release ships `zorui.exe` prebuilt. |
| Wallpaper Engine | Live wallpapers | Optional. Zor UI finds it in your Steam libraries. Without it, use `-WithLively` for video wallpapers. |

The installer uses winget to install GlazeWM, Flow Launcher and the JetBrains Mono Nerd Font.

## Install

1. **Get the code.**

   ```powershell
   git clone https://github.com/AnasGrzor/Zor-UI.git
   cd Zor-UI
   ```

   Or download the zip from GitHub and unzip it.

2. **Install Rust** (skip if `cargo --version` already works).

   ```powershell
   winget install Rustlang.Rustup
   ```

   Open a new PowerShell window afterwards so `cargo` is on the PATH. Either toolchain works:
   `stable-x86_64-pc-windows-gnu` (what the project is developed on, no Visual Studio needed) or
   `stable-x86_64-pc-windows-msvc` (needs the Visual Studio Build Tools).
   Do not run `cargo update`: `Cargo.lock` pins versions that build on the GNU toolchain.

3. **Run the installer** from the folder that contains `install.ps1`.

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   Useful flags:

   | Flag | What it does |
   |---|---|
   | `-Mod win` | Use the Windows key as the main modifier. The default is `alt`, because Windows reserves combinations such as Win+L. |
   | `-WithLively` | Also install Lively Wallpaper, for video wallpapers without Wallpaper Engine. |
   | `-HideTaskbar` | Auto-hide the Windows taskbar so only the Zor UI bar shows. |
   | `-Wallpaper C:\path\wall.jpg` | Image or video to take colors from. The default is your current wallpaper. |
   | `-UserName Anas` | Name shown in the greeting card. |
   | `-Workspaces 10` | Number of workspace dots in the bar. |
   | `-SkipDeps` | Do not use winget (GlazeWM and Flow Launcher are already installed). |

   The first run takes a few minutes: winget downloads and, from a clone,
   `cargo build --release` compiles `zorui.exe` (about a minute on a recent PC).

4. **What the installer changed.**

   | Where | What |
   |---|---|
   | `%LOCALAPPDATA%\zor-ui\` | `zorui.exe`, your saved state. |
   | `~\.glzr\zor-ui\` | `config.json` (your settings), `theme.json` (generated colors) and the fonts. |
   | `~\.glzr\glazewm\config.yaml` | The GlazeWM config with Zor UI's keybinds. Your old file is saved as `config.yaml.bak-<date>`. |
   | Flow Launcher, Windows Terminal | A color theme named `zor-ui` for each, regenerated whenever the wallpaper changes. |

Running `install.ps1` again updates everything and keeps your `config.json` settings.

## Run it

The installer starts GlazeWM, which starts `zorui.exe`. It also starts them at every login, so
after installing there is nothing to launch.

If you need to start or restart the shell by hand (or press `alt+ctrl+r`):

```powershell
# restart only the native part (bar, island, sidebars, overview, settings, desktop cards)
Stop-Process -Name zorui -Force -ErrorAction SilentlyContinue
Start-Process "$env:LOCALAPPDATA\zor-ui\zorui.exe"

# start the whole stack
glazewm start
```

You should now see a bar across the top of each monitor, desktop cards on the left, and a small Gojo
sitting on the bar. Press the keys below to open the rest.

| Keys (default `alt`) | Action |
|---|---|
| `alt+a` | Left sidebar: player, AI chat, translator, anime, football |
| `alt+n` | Right sidebar: toggles, sliders, notifications, calendar, to-do, timer |
| `alt+i` | Settings |
| `` alt+` `` (`win+tab` with `-Mod win`) | Overview |
| `alt+space` | Flow Launcher |
| `alt+ctrl+t` | Pick a wallpaper and regenerate the colors |
| `alt+ctrl+d` | Toggle dark and light mode |
| `alt+ctrl+r` | Restart the shell and reload GlazeWM |

The full list is in the [README](../README.md#keybinds). Click the Dynamic Island in the bar to expand it.

## Set up the optional parts

- **AI chat.** Settings > Services: paste an API key for Gemini, OpenAI, Anthropic, Mistral or OpenRouter, or run
  [Ollama](https://ollama.com) locally. Keys are stored encrypted with Windows DPAPI.
- **Wallpaper Engine.** Settings > Wallpapers lists your library. Pick one and the colors follow it.
- **Football, weather, anime.** These need internet and work without keys.
- **Dock.** Off by default: Settings > Bar & desktop > Dock.

## Update

```powershell
cd Zor-UI
git pull
powershell -ExecutionPolicy Bypass -File .\install.ps1 -SkipDeps
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| No bar after install | Check that `zorui.exe` is running (`Get-Process zorui`). If not, start it as shown above. If GlazeWM is not running, run `glazewm start`. |
| The installer says there is no prebuilt `zorui.exe` and Rust is not installed | You are installing from a clone. Install Rust (rustup.rs), open a new PowerShell window and run `install.ps1 -SkipDeps` again. |
| Build fails with `dlltool ... CreateProcess` | You are on the GNU toolchain and ran `cargo update`. Restore `Cargo.lock` with `git checkout Cargo.lock` and rebuild. |
| The bar overlaps windows, or the work area does not shrink | Reload GlazeWM's config (`glazewm command wm-reload-config`) so its `zorui` ignore rule is loaded, then restart `zorui.exe`. |
| The Wallpaper Engine wallpaper disappears after a restart | Update to the current version. Older builds closed Wallpaper Engine's wallpaper at start-up when the saved state was empty. |
| Colors look wrong after changing wallpaper | Run `zorui.exe --script switchwall` from a terminal to see the error, and report the wallpaper. |
| A widget shows an empty list or an error | Run the same helper from a terminal. `zorui.exe --bridge <command>` prints the JSON it returns (for example `zorui.exe --bridge we-status`). |

## Uninstall

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
```

This restores your backed-up GlazeWM config, turns night light off, and removes the folders and
the GlazeWM start-up shortcut the installer created. GlazeWM and Flow Launcher stay installed; remove them with winget if you
want them gone.
