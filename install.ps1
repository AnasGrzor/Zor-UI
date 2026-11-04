<#
.SYNOPSIS
  Installs end4-pC for Windows (GlazeWM + Zebar + Flow Launcher + Material You theming).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1
  powershell -ExecutionPolicy Bypass -File .\install.ps1 -Mod win -HideTaskbar -Wallpaper C:\Pictures\wall.jpg

.PARAMETER Mod
  Modifier for all keybinds. 'alt' (default, never clashes with Windows) or
  'win' (closest to end4's Super key; Windows may still grab a few combos like Win+L).
.PARAMETER SkipDeps
  Don't install/upgrade apps with winget (use when they're already installed).
.PARAMETER HideTaskbar
  Turn on taskbar auto-hide so only the end4 bar is visible.
.PARAMETER Wallpaper
  Image to theme from. Defaults to your current Windows wallpaper.
.PARAMETER UserName
  Name shown in the greeting and sidebar. Defaults to your Windows user name.
.PARAMETER WithLively
  Also install Lively Wallpaper for video (live) wallpapers. Not needed if you have Wallpaper Engine:
  end4 uses Wallpaper Engine automatically when it's installed through Steam.
#>
param(
  [ValidateSet('alt', 'win')][string]$Mod = 'alt',
  [switch]$SkipDeps,
  [switch]$HideTaskbar,
  [string]$Wallpaper = '',
  [string]$UserName = '',
  [int]$Workspaces = 10,
  [switch]$WithLively
)

$ErrorActionPreference = 'Stop'
$Repo = $PSScriptRoot
$Root = Join-Path $env:LOCALAPPDATA 'end4-pC'
$Scripts = Join-Path $Root 'scripts'
$Glzr = Join-Path $env:USERPROFILE '.glzr'
$Pack = Join-Path $Glzr 'zebar\end4-pC'
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Utf8 = New-Object System.Text.UTF8Encoding $false   # no BOM: Zebar's JSON parser rejects it

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Info($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }
function Write-Text($path, $text) {
  New-Item -ItemType Directory -Force -Path (Split-Path $path) | Out-Null
  [IO.File]::WriteAllText($path, $text, $Utf8)
}
function Backup($path) {
  if (Test-Path $path) { Copy-Item $path "$path.bak-$Stamp" -Force; Info "backed up $(Split-Path $path -Leaf) -> $(Split-Path $path -Leaf).bak-$Stamp" }
}
# Windows PowerShell 5.1 turns native-command stderr into terminating errors
# under 'Stop'; run external tools with 'Continue' and check exit codes instead.
function Invoke-Native([scriptblock]$block) {
  $prev = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  try { & $block } finally { $ErrorActionPreference = $prev }
}
function Refresh-Path {
  $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
}
# Escape a path for a Rust regex (Zebar's shell privileges). Spaces and '-' stay literal.
function ConvertTo-RustRegex([string]$s) {
  ($s.ToCharArray() | ForEach-Object { if ('\.^$|?*+()[]{}'.Contains([string]$_)) { '\' + $_ } else { [string]$_ } }) -join ''
}

if ([Environment]::OSVersion.Version.Major -lt 10) { throw 'end4-pC for Windows needs Windows 10 or 11.' }

# ------------------------------------------------------------------ 1. apps
if (-not $SkipDeps) {
  Step 'Installing apps with winget'
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw 'winget not found. Install "App Installer" from the Microsoft Store, or re-run with -SkipDeps after installing the apps yourself.'
  }
  $apps = [ordered]@{
    'glzr-io.glazewm'             = 'GlazeWM (tiling window manager)'
    'glzr-io.zebar'               = 'Zebar (bar and widgets)'
    'Flow-Launcher.Flow-Launcher' = 'Flow Launcher (app launcher)'
    'Python.Python.3.12'          = 'Python 3.12 (wallpaper colors)'
    'DEVCOM.JetBrainsMonoNerdFont' = 'JetBrains Mono Nerd Font'
  }
  if ($WithLively) { $apps['rocksdanister.LivelyWallpaper'] = 'Lively Wallpaper (live/video wallpapers)' }
  foreach ($id in $apps.Keys) {
    Info $apps[$id]
    Invoke-Native { winget list --id $id -e --accept-source-agreements 2>&1 | Out-Null }
    if ($LASTEXITCODE -eq 0) { Info '  already installed'; continue }
    Invoke-Native { winget install --id $id -e --silent --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null }
    if ($LASTEXITCODE -ne 0) { Write-Warning "winget couldn't install $id (exit $LASTEXITCODE). Install it manually, then re-run with -SkipDeps." }
  }
  Refresh-Path
}

# ------------------------------------------------------------------ 2. scripts + python
Step "Copying scripts to $Root"
New-Item -ItemType Directory -Force -Path $Scripts | Out-Null
Copy-Item -Recurse -Force (Join-Path $Repo 'scripts\*') $Scripts
Copy-Item -Recurse -Force (Join-Path $Repo 'flowlauncher') $Root

Step 'Setting up the Python environment for Material You colors'
$py = $null
foreach ($cand in @('py', 'python')) {
  $cmd = Get-Command $cand -ErrorAction SilentlyContinue
  if ($cmd -and $cmd.Source -notlike '*WindowsApps*') { $py = $cmd.Source; break }
}
if (-not $py) {
  $guess = Get-ChildItem "$env:LOCALAPPDATA\Programs\Python\Python3*\python.exe" -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
  if ($guess) { $py = $guess.FullName }
}
if (-not $py) { throw 'Python 3 not found. Install it (winget install Python.Python.3.12) and re-run.' }
$venv = Join-Path $Root 'venv'
$venvPy = Join-Path $venv 'Scripts\python.exe'
if (-not (Test-Path $venvPy)) {
  if ((Split-Path $py -Leaf) -eq 'py.exe') { Invoke-Native { & $py -3 -m venv $venv } } else { Invoke-Native { & $py -m venv $venv } }
  if (-not (Test-Path $venvPy)) { throw "Couldn't create the Python venv at $venv." }
}
# pillow + materialyoucolor: colors. numpy + soundcard: visualizer. imageio-ffmpeg: live wallpaper frames,
# screen recording. pycaw: per-app volume.
Invoke-Native { & $venvPy -m pip install --quiet --disable-pip-version-check --upgrade pillow materialyoucolor numpy 'soundcard>=0.4.3' imageio-ffmpeg psutil pycaw 2>&1 | ForEach-Object { Info $_ } }
if ($LASTEXITCODE -ne 0) { throw 'pip install failed (pillow, materialyoucolor, numpy, soundcard, imageio-ffmpeg, psutil, pycaw).' }

# ------------------------------------------------------------------ 3. zebar widget pack
Step "Installing the Zebar widget pack to $Pack"
New-Item -ItemType Directory -Force -Path $Pack | Out-Null
Copy-Item -Recurse -Force (Join-Path $Repo 'zebar\end4-pC\*') $Pack

# Fonts for offline use (Google Fonts is the fallback if this fails).
$fontDir = Join-Path $Pack 'assets\fonts'
New-Item -ItemType Directory -Force -Path $fontDir | Out-Null
$fonts = @{
  'MaterialSymbolsRounded.woff2' = 'https://raw.githubusercontent.com/google/material-design-icons/master/variablefont/MaterialSymbolsRounded%5BFILL,GRAD,opsz,wght%5D.woff2'
  'GoogleSansFlex.ttf'           = 'https://raw.githubusercontent.com/google/fonts/main/ofl/googlesansflex/GoogleSansFlex%5BROND,opsz,slnt,wdth,wght%5D.ttf'
}
foreach ($f in $fonts.Keys) {
  $dest = Join-Path $fontDir $f
  if (Test-Path $dest) { continue }
  try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri $fonts[$f] -OutFile $dest
    Info "downloaded $f"
  } catch { Write-Warning "Couldn't download $f; widgets will load it from Google Fonts instead." }
}

$scriptsRe = (ConvertTo-RustRegex $Scripts).Replace('\', '\\')   # JSON-escape too
$zpack = [IO.File]::ReadAllText((Join-Path $Repo 'zebar\end4-pC\zpack.json')).Replace('{{SCRIPTS_RE}}', $scriptsRe).Replace('{{PYTHON}}', $venvPy.Replace('\', '\\'))
Write-Text (Join-Path $Pack 'zpack.json') $zpack

$cfgPath = Join-Path $Pack 'config.json'
$modKey = if ($Mod -eq 'win') { 'lwin' } else { 'alt' }
# Non-ASCII as char codes: Windows PowerShell 5.1 reads BOM-less scripts as ANSI.
$cfg = [ordered]@{
  scriptsDir = $Scripts; pythonPath = $venvPy; bridgePath = (Join-Path $Scripts 'end4_bridge.py'); modKey = $modKey
  userName = $(if ($UserName) { $UserName } else { $env:USERNAME }); workspaces = $Workspaces
  clockFormat = "EEE, dd/MM '$([char]0x2022)' HH:mm"; tempUnit = 'C'; showKeyboardLayout = $false; lyrics = $true
  workspaceLabels = @(); alwaysShowNumbers = $false; transparency = 0; desktopWidgets = $true; syncLockScreen = $true
  ai = [ordered]@{ model = 'gemini-2.5-flash'; temperature = 0.5; systemPrompt = '' }
  translator = [ordered]@{ source = 'auto'; target = 'en' }
  booru = [ordered]@{ source = 'yandere'; allowNsfw = $false }
  football = [ordered]@{ leagues = @() }
  visualizer = [ordered]@{ desktop = $true; bar = $false; bars = 64 }
  nightLight = [ordered]@{ kelvin = 4500 }
  wallpaperEngine = [ordered]@{ follow = $true; path = '' }
}
$paths = @('scriptsDir', 'pythonPath', 'bridgePath', 'modKey')
if (Test-Path $cfgPath) {   # keep the user's edits, only refresh paths and add new options
  $old = Get-Content $cfgPath -Raw | ConvertFrom-Json
  foreach ($p in $old.PSObject.Properties) {
    if ($paths -contains $p.Name -or ($p.Name -eq 'userName' -and $UserName)) { continue }
    $cfg[$p.Name] = $p.Value
  }
}
Write-Text $cfgPath ($cfg | ConvertTo-Json -Depth 6)

$zebarSettings = Join-Path $Glzr 'zebar\settings.json'
Backup $zebarSettings
$settings = [ordered]@{
  '$schema' = 'https://github.com/glzr-io/zebar/raw/v3.0.0/resources/settings-schema.json'
  startupConfigs = @(
    [ordered]@{ pack = 'end4-pC'; widget = 'bar'; preset = 'default' },
    [ordered]@{ pack = 'end4-pC'; widget = 'desktop'; preset = 'default' },
    [ordered]@{ pack = 'end4-pC'; widget = 'visualizer'; preset = 'default' }
  ) + @(if ($cfg.dock -and $cfg.dock.enabled) { [ordered]@{ pack = 'end4-pC'; widget = 'dock'; preset = 'default' } })
}
Write-Text $zebarSettings ($settings | ConvertTo-Json -Depth 5)

# ------------------------------------------------------------------ 4. glazewm config
Step 'Writing the GlazeWM config'
$glazeCfg = Join-Path $Glzr 'glazewm\config.yaml'
Backup $glazeCfg
$overview = if ($Mod -eq 'win') { 'lwin+tab' } else { 'alt+oem_tilde' }   # keep Alt+Tab for Windows
$yaml = [IO.File]::ReadAllText((Join-Path $Repo 'glazewm\config.template.yaml'))
# Workspaces 1-5 go on the main monitor, 6-10 on another. GlazeWM numbers monitors from the left;
# with one monitor, index 1 doesn't exist and GlazeWM puts 6-10 on the only one.
Add-Type -AssemblyName System.Windows.Forms
$screens = @([System.Windows.Forms.Screen]::AllScreens | Sort-Object { $_.Bounds.X }, { $_.Bounds.Y })
$mainIdx = [Math]::Max(0, [array]::IndexOf($screens, ($screens | Where-Object Primary | Select-Object -First 1)))
$secondIdx = if ($screens.Count -gt 1) { @(0..($screens.Count - 1) | Where-Object { $_ -ne $mainIdx })[0] } else { 1 }
$yaml = $yaml.Replace('{{MOD}}', $modKey).Replace('{{OVERVIEW}}', $overview).Replace('{{SCRIPTS}}', $Scripts)
$yaml = $yaml.Replace('{{MAIN}}', $mainIdx).Replace('{{SECOND}}', $secondIdx)
Write-Text $glazeCfg $yaml
Info "modifier key: $modKey"

# ------------------------------------------------------------------ 5. flow launcher
Step 'Selecting the end4-pC Flow Launcher theme'
$flowSettings = Join-Path $env:APPDATA 'FlowLauncher\Settings\Settings.json'
if (Test-Path $flowSettings) {
  if (Get-Process Flow.Launcher -ErrorAction SilentlyContinue) {
    Info 'Flow Launcher is running: pick "end4-pC" in Flow Settings > Theme (it will not be overwritten on exit).'
  } else {
    Backup $flowSettings
    $raw = [IO.File]::ReadAllText($flowSettings)
    $raw = [regex]::Replace($raw, '"Theme"\s*:\s*"[^"]*"', '"Theme": "end4-pC"')
    Write-Text $flowSettings $raw
  }
} else {
  Write-Text $flowSettings '{ "Theme": "end4-pC" }'
}

# ------------------------------------------------------------------ 6. colors
Step 'Generating Material You colors'
if (-not $Wallpaper) {
  $current = (Get-ItemProperty 'HKCU:\Control Panel\Desktop' -ErrorAction SilentlyContinue).WallPaper
  if ($current -and (Test-Path $current)) { $Wallpaper = $current }
}
$switch = Join-Path $Scripts 'switchwall.ps1'
$r = Invoke-Native { if ($Wallpaper) { & $switch -Image $Wallpaper } else { & $switch -Color '#6750A4' -Mode dark } }
Info ($r | Out-String).Trim()

# ------------------------------------------------------------------ 6b. Wallpaper Engine
$weStatus = Invoke-Native { & $venvPy -X utf8 (Join-Path $Scripts 'end4_bridge.py') we-status 2>$null | Select-Object -Last 1 }
try { $we = $weStatus | ConvertFrom-Json } catch { $we = $null }
if ($we -and $we.installed) {
  Info "Wallpaper Engine found: $($we.exe)"
  Info '  live wallpapers use it, and end4 re-themes when you change wallpaper there (Settings > Wallpapers).'
}

# ------------------------------------------------------------------ 6c. AI agents -> Dynamic Island
# Hooks (and Claude Code's status line, for usage limits) report to end4_agent.py. Other agents can
# pipe the same JSON into it; see the top of scripts\end4_agent.py.
Info 'AI agents (Claude Code, Codex): adding end4 hooks'
Invoke-Native { & $venvPy -X utf8 (Join-Path $Scripts 'end4_agent.py') --install 2>&1 | ForEach-Object { Info "  $_" } }

# ------------------------------------------------------------------ 7. taskbar
if ($HideTaskbar) {
  Step 'Turning on taskbar auto-hide'
  $k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StuckRects3'
  $v = (Get-ItemProperty $k).Settings
  $v[8] = 3
  Set-ItemProperty $k -Name Settings -Value $v
  Stop-Process -Name explorer -Force   # explorer restarts itself
}

# ------------------------------------------------------------------ 8. start
Step 'Starting GlazeWM (it launches Zebar)'
$pf = Join-Path $env:ProgramFiles 'glzr.io\GlazeWM'
$glaze = @((Join-Path $pf 'glazewm.exe'), (Get-Command glazewm -ErrorAction SilentlyContinue).Source) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $glaze) {
  Write-Warning 'GlazeWM not found. Install it, then start it from the Start menu.'
} else {
  $startup = Join-Path ([Environment]::GetFolderPath('Startup')) 'GlazeWM.lnk'
  if (-not (Test-Path $startup)) {
    $sc = (New-Object -ComObject WScript.Shell).CreateShortcut($startup)
    $sc.TargetPath = $glaze; $sc.Save()
    Info 'added GlazeWM to startup'
  }
  if (Get-Process glazewm -ErrorAction SilentlyContinue) {
    Get-Process zebar -ErrorAction SilentlyContinue | Stop-Process -Force
    Invoke-Native { & $glaze command wm-reload-config 2>&1 | Out-Null }
    Start-Process zebar
  } else {
    Start-Process $glaze   # its startup_commands also start Flow Launcher
  }
  # GlazeWM's startup_commands only run when GlazeWM starts, so start Flow Launcher now if needed.
  $flow = Join-Path $env:LOCALAPPDATA 'FlowLauncher\Flow.Launcher.exe'
  if ((Test-Path $flow) -and -not (Get-Process Flow.Launcher -ErrorAction SilentlyContinue)) { Start-Process $flow }
}

$m = if ($Mod -eq 'win') { 'Win' } else { 'Alt' }
$o = if ($Mod -eq 'win') { 'Win+Tab' } else { 'Alt+`' }
Write-Host @"

end4-pC for Windows is installed.

  $m+A          left sidebar (media, AI chat, translator, anime, football)
  $m+N          right sidebar (toggles, notifications, calendar, to-do)
  $m+I          settings (colors, Wallpaper Engine library, presets gallery, API keys)
  $o        overview (search and move windows)
  Alt+Space      launcher (Flow Launcher)
  $m+Ctrl+T     change wallpaper + colors
  $m+Ctrl+D     toggle dark / light
  $m+1..0       workspaces, $m+Shift+1..0 to move a window
  $m+Ctrl+R     restart the bar and reload GlazeWM

Edit $cfgPath for your name, clock format and workspace count.
"@ -ForegroundColor Green
