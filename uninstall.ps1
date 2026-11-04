<#
.SYNOPSIS
  Removes Zor UI and restores your previous GlazeWM / Zebar configs.
  Apps installed by install.ps1 (GlazeWM, Zebar, Flow Launcher, Python) are left alone;
  remove them with winget if you want.
#>
$ErrorActionPreference = 'Continue'
$Glzr = Join-Path $env:USERPROFILE '.glzr'

function Restore-Latest($path) {
  $bak = Get-ChildItem "$path.bak-*" -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
  if ($bak) { Copy-Item $bak.FullName $path -Force; Write-Host "restored $path from $($bak.Name)" }
  elseif (Test-Path $path) { Remove-Item $path; Write-Host "removed $path" }
}

# Undo night light (gamma) and close live wallpapers before removing anything.
$py = Join-Path $env:LOCALAPPDATA 'zor-ui\venv\Scripts\python.exe'
$bridge = Join-Path $env:LOCALAPPDATA 'zor-ui\scripts\end4_bridge.py'
if ((Test-Path $py) -and (Test-Path $bridge)) { & $py -X utf8 $bridge nightlight off 2>$null | Out-Null }
$lively = Join-Path $env:LOCALAPPDATA 'Programs\Lively Wallpaper\Livelycu.exe'
if (Test-Path $lively) { & $lively closewp --monitor -1 2>$null | Out-Null }
Get-Process zebar, end4 -ErrorAction SilentlyContinue | Stop-Process -Force
$pid0 = Join-Path $env:LOCALAPPDATA 'zor-ui\caffeine.pid'
if (Test-Path $pid0) { Stop-Process -Id (Get-Content $pid0) -Force -ErrorAction SilentlyContinue }

Restore-Latest (Join-Path $Glzr 'glazewm\config.yaml')
Restore-Latest (Join-Path $Glzr 'zebar\settings.json')
Restore-Latest (Join-Path $env:APPDATA 'FlowLauncher\Settings\Settings.json')

Remove-Item -Recurse -Force (Join-Path $Glzr 'zebar\zor-ui') -ErrorAction SilentlyContinue
Remove-Item -Force (Join-Path $env:APPDATA 'FlowLauncher\Themes\zor-ui.xaml') -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'Microsoft\Windows Terminal\Fragments\zor-ui') -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'zor-ui') -ErrorAction SilentlyContinue

$startup = Join-Path ([Environment]::GetFolderPath('Startup')) 'GlazeWM.lnk'
if (Test-Path $startup) { Remove-Item $startup; Write-Host 'removed GlazeWM from startup' }

if (Get-Process glazewm -ErrorAction SilentlyContinue) { glazewm command wm-reload-config 2>$null | Out-Null; Start-Process zebar }
Write-Host "`nzor-ui removed. Taskbar auto-hide (if you enabled it) is in Settings > Personalization > Taskbar." -ForegroundColor Green
