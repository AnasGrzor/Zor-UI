# Install and run Zor UI

Zor UI is a Windows desktop shell built on GlazeWM (tiling), Zebar (widget host) and a native Rust
process, `end4.exe`, that draws the bar, Dynamic Island, sidebars, overview, settings and desktop cards.
It began as a port of [end4-pC](https://github.com/pctrade/end4-pC) by pctrade, itself based on
[illogical-impulse](https://github.com/end-4/dots-hyprland) by end-4.

If you installed it under its old name, `end4-pC`, re-running `install.ps1` moves your settings, saved state and
Python environment to the new `zor-ui` folders and updates the AI agent hooks that point at the scripts.

## What you need

| Requirement | Why | Notes |
|---|---|---|
| Windows 10 or 11 | | Windows 10 also needs the WebView2 runtime (built into Windows 11). |
| winget | Installs the apps below | Comes with "App Installer" in the Microsoft Store. Skip it with `-SkipDeps` if you install the apps yourself. |
| Windows PowerShell 5.1 | Runs `install.ps1` | Already part of Windows. |
| Rust (rustup) | Builds `end4.exe` | Optional but strongly recommended. Without it, Zebar draws every widget in its own browser window and uses about 1 GB instead of about 35 MB. |
| Wallpaper Engine | Live wallpapers | Optional. Zor UI finds it in your Steam libraries. Without it, use `-WithLively` for video wallpapers. |

The installer uses winget to install GlazeWM, Zebar, Flow Launcher, Python 3.12 and the JetBrains Mono
Nerd Font. It creates a Python virtual environment for the helper scripts that are not ported to Rust yet.

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
   | `-SkipDeps` | Do not use winget (GlazeWM, Zebar, Flow Launcher and Python are already installed). |

   The first run takes a few minutes: winget downloads, `pip` installs the Python packages, and
   `cargo build --release` compiles `end4.exe` (about a minute on a recent PC).

4. **What the installer changed.**

   | Where | What |
   |---|---|
   | `%LOCALAPPDATA%\zor-ui\` | `end4.exe`, the helper scripts, the Python virtual environment, your saved state. |
   | `~\.glzr\zebar\zor-ui\` | The widget pack, `config.json` (your settings) and `theme.json` (generated colors). |
   | `~\.glzr\glazewm\config.yaml` | The GlazeWM config with Zor UI's keybinds. Your old file is saved as `config.yaml.bak-<date>`. |
   | `~\.glzr\zebar\settings.json` | Which widgets Zebar starts with Windows. |
   | Flow Launcher, Windows Terminal | A color theme named `zor-ui` for each, regenerated whenever the wallpaper changes. |

Running `install.ps1` again updates everything and keeps your `config.json` settings.

## Run it

The installer starts GlazeWM, which starts Zebar and `end4.exe`. It also starts them at every login, so
after installing there is nothing to launch.

If you need to start or restart the shell by hand (or press `alt+ctrl+r`):

```powershell
# restart only the native part (bar, island, sidebars, overview, settings, desktop cards)
Stop-Process -Name end4 -Force -ErrorAction SilentlyContinue
Start-Process "$env:LOCALAPPDATA\zor-ui\end4.exe"

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
| `alt+ctrl+r` | Restart Zebar and reload GlazeWM |

The full list is in the [README](../README.md#keybinds). Click the Dynamic Island in the bar to expand it.

## Set up the optional parts

- **AI chat.** Settings > Services: paste an API key for Gemini, OpenAI, Anthropic, Mistral or OpenRouter, or run
  [Ollama](https://ollama.com) locally. Keys are stored encrypted with Windows DPAPI.
- **Wallpaper Engine.** Settings > Wallpapers lists your library. Pick one and the colors follow it.
- **Football, weather, anime.** These need internet and work without keys.
- **Dock.** Off by default: Settings > Bar & desktop > Dock.

## Turning native parts on and off

`~\.glzr\zebar\zor-ui\config.json` has a `native` section. A part set to `true` is drawn by `end4.exe`;
a part set to `false` or missing is drawn by Zebar instead.

```json
"native": { "bar": true, "desktop": true, "gojo": true, "island-panel": true, "overview": true,
            "sidebar-left": true, "sidebar-right": true, "settings": true, "dock": true,
            "visualizer": true, "theme": true }
```

`"theme": true` makes wallpaper and color changes run in Rust. Set it to `false` to use the original
PowerShell and Python scripts for theming. To force every helper command back to the Python scripts for
a test, set the environment variable `END4_PYTHON_BRIDGE=1` before starting `end4.exe`.

## Update

```powershell
cd Zor-UI
git pull
powershell -ExecutionPolicy Bypass -File .\install.ps1 -SkipDeps
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| No bar after install | Check that `end4.exe` is running (`Get-Process end4`). If not, start it as shown above. If GlazeWM is not running, run `glazewm start`. |
| `cargo` not found, or the installer prints "Zebar draws every widget" | Rust is missing or not on the PATH. Install it, open a new PowerShell window and run `install.ps1 -SkipDeps` again. |
| Build fails with `dlltool ... CreateProcess` | You are on the GNU toolchain and ran `cargo update`. Restore `Cargo.lock` with `git checkout Cargo.lock` and rebuild. |
| The bar overlaps windows, or the work area does not shrink | Reload GlazeWM's config (`glazewm command wm-reload-config`) so its `end4` ignore rule is loaded, then restart `end4.exe`. |
| The Wallpaper Engine wallpaper disappears after a restart | Update to the current version. Older builds closed Wallpaper Engine's wallpaper at start-up when the saved state was empty. |
| Colors look wrong after changing wallpaper | Set `"theme": false` in `config.json` to use the original scripts, and report the wallpaper. |
| A widget shows an empty list or an error | Run the same helper from a terminal. `end4.exe --bridge <command>` prints the JSON it returns (for example `end4.exe --bridge we-status`). |

## Uninstall

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
```

This restores your backed-up GlazeWM and Zebar configs, turns night light off, and removes the folders and
the GlazeWM start-up shortcut the installer created. GlazeWM, Zebar, Flow Launcher and Python stay installed; remove them with winget if you
want them gone.
