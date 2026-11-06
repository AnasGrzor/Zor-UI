<#
.SYNOPSIS
  Installs Zor UI (GlazeWM + Flow Launcher + the native zorui.exe shell + Material You theming).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1
  powershell -ExecutionPolicy Bypass -File .\install.ps1 -Mod win -HideTaskbar -Wallpaper C:\Pictures\wall.jpg

.PARAMETER Mod
  Modifier for all keybinds. 'alt' (default, never clashes with Windows) or
  'win' (closest to Zor UI's Super key; Windows may still grab a few combos like Win+L).
.PARAMETER SkipDeps
  Don't install/upgrade apps with winget (use when they're already installed).
.PARAMETER HideTaskbar
  Turn on taskbar auto-hide so only the Zor UI bar is visible.
.PARAMETER Wallpaper
  Image to theme from. Defaults to your current Windows wallpaper.
.PARAMETER UserName
  Name shown in the greeting and sidebar. Defaults to your Windows user name.
.PARAMETER WithLively
  Also install Lively Wallpaper for video (live) wallpapers. Not needed if you have Wallpaper Engine:
  Zor UI uses Wallpaper Engine automatically when it's installed through Steam.
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
$Root = Join-Path $env:LOCALAPPDATA 'zor-ui'
$Scripts = Join-Path $Root 'scripts'
$Glzr = Join-Path $env:USERPROFILE '.glzr'
$Pack = Join-Path $Glzr 'zebar\zor-ui'
$Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$Utf8 = New-Object System.Text.UTF8Encoding $false   # no BOM

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
if ([Environment]::OSVersion.Version.Major -lt 10) { throw 'Zor UI needs Windows 10 or 11.' }

# ------------------------------------------------------------------ 0. the old name
# Zor UI was called end4-pC. Move its folders and theme files so settings, saved state and the
# Python environment carry over. Runs once: it does nothing when the old folders are gone.
$oldRoot = Join-Path $env:LOCALAPPDATA 'end4-pC'
$oldPack = Join-Path $Glzr 'zebar\end4-pC'
$oldFlow = Join-Path $env:APPDATA 'FlowLauncher\Themes\end4-pC.xaml'
$oldFrag = Join-Path $env:LOCALAPPDATA 'Microsoft\Windows Terminal\Fragments\end4-pC'
if ((Test-Path $oldRoot) -or (Test-Path $oldPack) -or (Test-Path $oldFlow) -or (Test-Path $oldFrag)) {
  Step 'Moving settings from the old end4-pC name'
  Get-Process end4, zorui, zebar -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 1
  if ((Test-Path $oldRoot) -and -not (Test-Path $Root)) { Move-Item $oldRoot $Root; Info "moved $oldRoot" }
  if ((Test-Path $oldPack) -and -not (Test-Path $Pack)) { New-Item -ItemType Directory -Force (Split-Path $Pack) | Out-Null; Move-Item $oldPack $Pack; Info "moved $oldPack" }
  if ((Test-Path $oldFlow) -and -not (Test-Path ($oldFlow -replace 'end4-pC', 'zor-ui'))) { Move-Item $oldFlow ($oldFlow -replace 'end4-pC', 'zor-ui') }
  if ((Test-Path $oldFrag) -and -not (Test-Path ($oldFrag -replace 'end4-pC', 'zor-ui'))) {
    Move-Item $oldFrag ($oldFrag -replace 'end4-pC', 'zor-ui')
    $frag = $oldFrag -replace 'end4-pC', 'zor-ui'
    if (Test-Path "$frag\end4-pC.json") { Move-Item "$frag\end4-pC.json" "$frag\zor-ui.json" }
  }
}
# Paths and keys inside the moved files, and the AI agent hooks other tools point at the scripts with.
# Checked on every run, so a move that was interrupted still gets its paths fixed.
$stale = @("$Pack\config.json", "$Pack\theme.json", "$Root\state.json", "$Root\shared.json",
    "$env:USERPROFILE\.claude\settings.json", "$env:USERPROFILE\.codex\config.toml", "$env:USERPROFILE\.codex\hooks.json") |
  Where-Object { (Test-Path $_) -and ([IO.File]::ReadAllText($_) -match 'end4-pC') }
foreach ($f in @($stale)) {
  if ($f -match '\\\.(claude|codex)\\') { Backup $f }
  [IO.File]::WriteAllText($f, ([IO.File]::ReadAllText($f) -replace 'end4-pC', 'zor-ui'), $Utf8)
  Info "updated $f"
}

# ------------------------------------------------------------------ 1. apps
if (-not $SkipDeps) {
  Step 'Installing apps with winget'
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw 'winget not found. Install "App Installer" from the Microsoft Store, or re-run with -SkipDeps after installing the apps yourself.'
  }
  $apps = [ordered]@{
    'glzr-io.glazewm'             = 'GlazeWM (tiling window manager)'
    'Flow-Launcher.Flow-Launcher' = 'Flow Launcher (app launcher)'
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

# ------------------------------------------------------------------ 2. files
Step "Copying files to $Root"
New-Item -ItemType Directory -Force -Path $Scripts | Out-Null
Copy-Item -Recurse -Force (Join-Path $Repo 'scripts\terminal') $Scripts
Copy-Item -Recurse -Force (Join-Path $Repo 'flowlauncher') $Root
New-Item -ItemType Directory -Force -Path $Pack | Out-Null
Copy-Item -Recurse -Force (Join-Path $Repo 'assets') $Pack
# The Zebar widget pack and Python helpers of older versions are gone: remove what they left behind.
Get-Process zebar -ErrorAction SilentlyContinue | Stop-Process -Force
foreach ($old in 'lib', 'we-thumbs') { Remove-Item -Recurse -Force (Join-Path $Pack $old) -ErrorAction SilentlyContinue }
foreach ($pat in '*.html', '*.js', 'zpack.json') { Remove-Item -Force (Join-Path $Pack $pat) -ErrorAction SilentlyContinue }
Remove-Item -Recurse -Force (Join-Path $Root 'venv') -ErrorAction SilentlyContinue
Get-ChildItem (Join-Path $Scripts '*') -Include *.py, *.ps1 -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

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
  } catch { Write-Warning "Couldn't download $f; the shell falls back to Segoe UI." }
}

# The native shell (native/zor-ui): one small process draws the bar, sidebars, overview and desktop cards.
$native = Join-Path $Root 'zorui.exe'
$prebuilt = Join-Path $Repo 'zorui.exe'   # the one-click release ships it built; no Rust needed
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (Test-Path $prebuilt) {
  Step 'Installing the shell'
  Get-Process end4, zorui -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 1
  Copy-Item -Force $prebuilt $native
} elseif ($cargo) {
  Step 'Building the shell (native\zor-ui)'
  Invoke-Native { & $cargo build --release --manifest-path (Join-Path $Repo 'native\zor-ui\Cargo.toml') 2>&1 | Out-Null }
  $built = Join-Path $Repo 'native\zor-ui\target\release\zorui.exe'
  if (-not (Test-Path $built)) { throw 'The shell failed to build.' }
  Get-Process end4, zorui -ErrorAction SilentlyContinue | Stop-Process -Force
  Copy-Item -Force $built $native
} else { throw 'No prebuilt zorui.exe next to install.ps1 and Rust is not installed (rustup.rs).' }

$cfgPath = Join-Path $Pack 'config.json'
$modKey = if ($Mod -eq 'win') { 'lwin' } else { 'alt' }
# Non-ASCII as char codes: Windows PowerShell 5.1 reads BOM-less scripts as ANSI.
$cfg = [ordered]@{
  scriptsDir = $Scripts; modKey = $modKey
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
$paths = @('scriptsDir', 'pythonPath', 'bridgePath', 'modKey')   # the Python paths are dropped from old configs
if (Test-Path $cfgPath) {   # keep the user's edits, only refresh paths and add new options
  $old = Get-Content $cfgPath -Raw | ConvertFrom-Json
  foreach ($p in $old.PSObject.Properties) {
    if ($paths -contains $p.Name -or ($p.Name -eq 'userName' -and $UserName)) { continue }
    $cfg[$p.Name] = $p.Value
  }
}
Write-Text $cfgPath ($cfg | ConvertTo-Json -Depth 6)

# Zebar (if still installed) must not start the removed widgets: drop zor-ui from its startup list.
$zebarSettings = Join-Path $Glzr 'zebar\settings.json'
if (Test-Path $zebarSettings) {
  try {
    $zs = Get-Content $zebarSettings -Raw | ConvertFrom-Json
    if ($zs.startupConfigs) {
      $zs.startupConfigs = @($zs.startupConfigs | Where-Object { $_.pack -ne 'zor-ui' })
      Write-Text $zebarSettings ($zs | ConvertTo-Json -Depth 6)
    }
  } catch { Info "couldn't update $zebarSettings" }
}

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
$yaml = $yaml.Replace('{{MOD}}', $modKey).Replace('{{OVERVIEW}}', $overview)
$yaml = $yaml.Replace('{{MAIN}}', $mainIdx).Replace('{{SECOND}}', $secondIdx)
Write-Text $glazeCfg $yaml
Info "modifier key: $modKey"

if (Test-Path $native) {   # desktop / folder background right-click: "Open ZorUI settings" (Win11: under Show more options)
  $ctx = 'HKCU:\Software\Classes\Directory\Background\shell\ZorUISettings'
  New-Item -Path "$ctx\command" -Force | Out-Null
  Set-ItemProperty -Path $ctx -Name '(default)' -Value 'Open ZorUI settings'
  Set-ItemProperty -Path $ctx -Name Icon -Value $native
  Set-ItemProperty -Path "$ctx\command" -Name '(default)' -Value "`"$native`" --open settings"
}

# ------------------------------------------------------------------ 5. flow launcher
Step 'Selecting the zor-ui Flow Launcher theme'
$flowSettings = Join-Path $env:APPDATA 'FlowLauncher\Settings\Settings.json'
if (Test-Path $flowSettings) {
  if (Get-Process Flow.Launcher -ErrorAction SilentlyContinue) {
    Info 'Flow Launcher is running: pick "zor-ui" in Flow Settings > Theme (it will not be overwritten on exit).'
  } else {
    Backup $flowSettings
    $raw = [IO.File]::ReadAllText($flowSettings)
    $raw = [regex]::Replace($raw, '"Theme"\s*:\s*"[^"]*"', '"Theme": "zor-ui"')
    Write-Text $flowSettings $raw
  }
} else {
  Write-Text $flowSettings '{ "Theme": "zor-ui" }'
}

# ------------------------------------------------------------------ 6. colors
Step 'Generating Material You colors'
if (-not $Wallpaper) {
  $current = (Get-ItemProperty 'HKCU:\Control Panel\Desktop' -ErrorAction SilentlyContinue).WallPaper
  if ($current -and (Test-Path $current)) { $Wallpaper = $current }
}
$r = Invoke-Native { if ($Wallpaper) { & $native --script switchwall -Image $Wallpaper } else { & $native --script switchwall -Color '#6750A4' -Mode dark } }
Info ($r | Out-String).Trim()

# ------------------------------------------------------------------ 6b. Wallpaper Engine
$weStatus = Invoke-Native { & $native --bridge we-status 2>$null | Select-Object -Last 1 }
try { $we = $weStatus | ConvertFrom-Json } catch { $we = $null }
if ($we -and $we.installed) {
  Info "Wallpaper Engine found: $($we.exe)"
  Info '  live wallpapers use it, and Zor UI re-themes when you change wallpaper there (Settings > Wallpapers).'
}

# ------------------------------------------------------------------ 6c. AI agents -> Dynamic Island
# Hooks (and Claude Code's status line, for usage limits) report to `zorui.exe --agent <name>`. Other
# agents can pipe the same JSON into it; see the top of native\zor-ui\src\bridge\agent_hook.rs.
Info 'AI agents (Claude Code, Codex): adding Zor UI hooks'
Invoke-Native { & $native --install-agents 2>&1 | ForEach-Object { Info "  $_" } }

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
Step 'Starting GlazeWM (it launches the shell)'
$pf = Join-Path $env:ProgramFiles 'glzr.io\GlazeWM'
$glaze = @((Join-Path $pf 'glazewm.exe'), (Get-Command glazewm -ErrorAction SilentlyContinue).Source) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $glaze) {
  Write-Warning 'GlazeWM not found. Install it, then start it from the Start menu.'
} else {
  # A logon task starts GlazeWM at login. The Startup folder waits for every Run-key app first (about 80 s here).
  try {
    $act = New-ScheduledTaskAction -Execute $glaze
    $trg = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
    $set = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
    Register-ScheduledTask -TaskName 'GlazeWM' -Action $act -Trigger $trg -Settings $set -Force | Out-Null
    $old = Join-Path ([Environment]::GetFolderPath('Startup')) 'GlazeWM.lnk'
    if (Test-Path $old) { Remove-Item $old }
    Info 'GlazeWM starts at login (scheduled task)'
  } catch {
    Info "couldn't add the login task ($($_.Exception.Message)); falling back to the Startup folder"
    $startup = Join-Path ([Environment]::GetFolderPath('Startup')) 'GlazeWM.lnk'
    if (-not (Test-Path $startup)) {
      $sc = (New-Object -ComObject WScript.Shell).CreateShortcut($startup)
      $sc.TargetPath = $glaze; $sc.Save()
    }
  }
  if (Get-Process glazewm -ErrorAction SilentlyContinue) {
    Invoke-Native { & $glaze command wm-reload-config 2>&1 | Out-Null }
  } else {
    Start-Process $glaze   # its startup_commands also start Flow Launcher
  }
  # GlazeWM's startup_commands only run when GlazeWM starts, so start Flow Launcher now if needed.
  $flow = Join-Path $env:LOCALAPPDATA 'FlowLauncher\Flow.Launcher.exe'
  if ((Test-Path $flow) -and -not (Get-Process Flow.Launcher -ErrorAction SilentlyContinue)) { Start-Process $flow }
  if ((Test-Path $native) -and -not (Get-Process zorui -ErrorAction SilentlyContinue)) { Start-Process $native }
}

$m = if ($Mod -eq 'win') { 'Win' } else { 'Alt' }
$o = if ($Mod -eq 'win') { 'Win+Tab' } else { 'Alt+`' }
Write-Host @"

Zor UI is installed.

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
