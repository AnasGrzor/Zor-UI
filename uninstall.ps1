<#
.SYNOPSIS
  Removes Zor UI and restores your previous GlazeWM configs.
  Apps installed by install.ps1 (GlazeWM, Flow Launcher) are left alone;
  remove them with winget if you want.
#>
$ErrorActionPreference = 'Continue'
$Glzr = Join-Path $env:USERPROFILE '.glzr'

function Restore-Latest($path) {
  # Oldest backup first: the stamp is yyyyMMdd-HHmmss, so name order is time order, and a reinstall backs up Zor's own config.
  $bak = Get-ChildItem "$path.bak-*" -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -First 1
  if ($bak) { Copy-Item $bak.FullName $path -Force; Write-Host "restored $path from $($bak.Name)" }
  elseif (Test-Path $path) { Remove-Item $path; Write-Host "removed $path" }
}

# Undo night light (gamma) and close live wallpapers before removing anything.
$exe = Join-Path $env:LOCALAPPDATA 'zor-ui\zorui.exe'
if (Test-Path $exe) { & $exe --bridge nightlight off 2>$null | Out-Null }
$lively = Join-Path $env:LOCALAPPDATA 'Programs\Lively Wallpaper\Livelycu.exe'
if (Test-Path $lively) { & $lively closewp --monitor -1 2>$null | Out-Null }
if (Get-ScheduledTask -TaskName 'ZorShellLogon' -ErrorAction SilentlyContinue) { Unregister-ScheduledTask -TaskName 'ZorShellLogon' -Confirm:$false -ErrorAction SilentlyContinue }
# The boot service would restart zorui, so remove it first (removing a service needs admin).
if (Get-Service ZorShell -ErrorAction SilentlyContinue) {
  try {
    Stop-Service ZorShell -Force -ErrorAction Stop
    sc.exe delete ZorShell | Out-Null
    if ($LASTEXITCODE) { throw "sc.exe delete exited with code $LASTEXITCODE" }
    Remove-Item -Recurse -Force (Join-Path $env:ProgramFiles 'zor-ui'), (Join-Path $env:ProgramData 'zor-ui') -ErrorAction SilentlyContinue
    Write-Host 'removed the ZorShell service'
  } catch { Write-Warning "couldn't remove the ZorShell service ($($_.Exception.Message)); run uninstall.ps1 from an elevated PowerShell" }
}
Get-Process zorui, end4 -ErrorAction SilentlyContinue | Stop-Process -Force
$pid0 = Join-Path $env:LOCALAPPDATA 'zor-ui\caffeine.pid'
if (Test-Path $pid0) {
  $owner = $null
  try { $owner = Get-Process -Id ([int](Get-Content $pid0 -Raw)) -ErrorAction Stop } catch {}
  # After a reboot the pid can belong to another process: kill only our keep-awake PowerShell (see toggles.rs caffeine_on).
  if ($owner -and $owner.ProcessName -eq 'powershell') { Stop-Process -Id $owner.Id -Force -ErrorAction SilentlyContinue }
  Remove-Item $pid0 -Force -ErrorAction SilentlyContinue
}

Restore-Latest (Join-Path $Glzr 'glazewm\config.yaml')
Restore-Latest (Join-Path $env:APPDATA 'FlowLauncher\Settings\Settings.json')

Remove-Item -Recurse -Force 'HKCU:\Software\Classes\Directory\Background\shell\ZorUISettings' -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force (Join-Path $Glzr 'zor-ui') -ErrorAction SilentlyContinue
Remove-Item -Force (Join-Path $env:APPDATA 'FlowLauncher\Themes\zor-ui.xaml') -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'Microsoft\Windows Terminal\Fragments\zor-ui') -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA 'zor-ui') -ErrorAction SilentlyContinue

$startup = Join-Path ([Environment]::GetFolderPath('Startup')) 'GlazeWM.lnk'
if (Test-Path $startup) { Remove-Item $startup; Write-Host 'removed GlazeWM from startup' }
if (Get-ScheduledTask -TaskName 'GlazeWM' -ErrorAction SilentlyContinue) { Unregister-ScheduledTask -TaskName 'GlazeWM' -Confirm:$false; Write-Host 'removed GlazeWM login task' }

if (Get-Process glazewm -ErrorAction SilentlyContinue) { glazewm command wm-reload-config 2>$null | Out-Null }

# Non-admin runs cannot remove the elevated pieces: report them and fail instead of claiming success.
$left = @()
if (Get-Service ZorShell -ErrorAction SilentlyContinue) { $left += 'the ZorShell service' }
if (Get-ScheduledTask -TaskName 'ZorShellLogon' -ErrorAction SilentlyContinue) { $left += 'the ZorShellLogon task' }
if ($left) {
  Write-Error "zor-ui is not fully removed: $($left -join ' and ') still exist. Run uninstall.ps1 from an elevated (Administrator) PowerShell."
  exit 1
}
Write-Host "`nzor-ui removed. Taskbar auto-hide (if you enabled it) is in Settings > Personalization > Taskbar." -ForegroundColor Green
