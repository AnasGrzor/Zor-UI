<#
  end4-pC for Windows - quick toggle backend for the Zebar sidebars.

  Every action prints one line of JSON so the widgets can read the result.
  Runs under Windows PowerShell 5.1 (powershell.exe), which ships with Windows
  and can call the WinRT radio APIs used for Wi-Fi / Bluetooth.

  Usage: toggles.ps1 -Action <name> [-Value <arg>]
#>
param(
  [Parameter(Mandatory = $true)][string]$Action,
  [string]$Value = ''
)

$ErrorActionPreference = 'Stop'
$StateDir = Join-Path $env:LOCALAPPDATA 'end4-pC'
$CaffeinePid = Join-Path $StateDir 'caffeine.pid'
New-Item -ItemType Directory -Force -Path $StateDir | Out-Null

# Non-ASCII goes out as \uXXXX: Windows PowerShell writes stdout in the console code page, which
# would turn clipboard text, plan names (™) etc. into '?'.
function Out-Json($obj) {
  [regex]::Replace(($obj | ConvertTo-Json -Compress -Depth 4), '[^\x00-\x7F]', { param($m) '\u{0:x4}' -f [int][char]$m.Value })
}

# ---------------------------------------------------------------- WinRT radios
$script:asTask = $null
function Await($op, [Type]$resultType) {
  if (-not $script:asTask) {
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $script:asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
      $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
      $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
    } | Select-Object -First 1
  }
  $task = $script:asTask.MakeGenericMethod($resultType).Invoke($null, @($op))
  $task.Wait(-1) | Out-Null
  $task.Result
}

function Get-Radios {
  [Windows.Devices.Radios.Radio, Windows.System.Devices, ContentType = WindowsRuntime] | Out-Null
  [Windows.Devices.Radios.RadioAccessStatus, Windows.System.Devices, ContentType = WindowsRuntime] | Out-Null
  [Windows.Devices.Radios.RadioState, Windows.System.Devices, ContentType = WindowsRuntime] | Out-Null
  Await ([Windows.Devices.Radios.Radio]::RequestAccessAsync()) ([Windows.Devices.Radios.RadioAccessStatus]) | Out-Null
  Await ([Windows.Devices.Radios.Radio]::GetRadiosAsync()) ([System.Collections.Generic.IReadOnlyList[Windows.Devices.Radios.Radio]])
}

function Get-RadioState([string]$kind) {
  $r = Get-Radios | Where-Object { $_.Kind -eq $kind } | Select-Object -First 1
  if ($null -eq $r) { return $null }
  return ($r.State -eq 'On')
}

function Set-Radio([string]$kind, [string]$want) {
  $r = Get-Radios | Where-Object { $_.Kind -eq $kind } | Select-Object -First 1
  if ($null -eq $r) { return $null }
  $on = ($r.State -eq 'On')
  $target = switch ($want) { 'on' { $true } 'off' { $false } default { -not $on } }
  $state = if ($target) { 'On' } else { 'Off' }
  Await ($r.SetStateAsync($state)) ([Windows.Devices.Radios.RadioAccessStatus]) | Out-Null
  return $target
}

# ------------------------------------------------------------------ caffeine
function Test-Caffeine {
  if (-not (Test-Path $CaffeinePid)) { return $false }
  $procId = Get-Content $CaffeinePid -ErrorAction SilentlyContinue
  if (-not $procId) { return $false }
  return [bool](Get-Process -Id $procId -ErrorAction SilentlyContinue)
}

function Switch-Caffeine {
  if (Test-Caffeine) {
    Stop-Process -Id (Get-Content $CaffeinePid) -Force -ErrorAction SilentlyContinue
    Remove-Item $CaffeinePid -ErrorAction SilentlyContinue
    return $false
  }
  # ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED, held by a hidden process.
  $code = @'
Add-Type -Name P -Namespace W -MemberDefinition '[DllImport("kernel32.dll")] public static extern uint SetThreadExecutionState(uint f);'
[W.P]::SetThreadExecutionState([uint32]"0x80000003") | Out-Null
while ($true) { Start-Sleep -Seconds 3600 }
'@
  $enc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
  $p = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList '-NoProfile', '-EncodedCommand', $enc
  Set-Content -Path $CaffeinePid -Value $p.Id
  return $true
}

# ------------------------------------------------------------------ brightness
function Get-Brightness {
  try { return [int](Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness -ErrorAction Stop | Select-Object -First 1).CurrentBrightness }
  catch { return $null }
}

function Set-Brightness([int]$pct) {
  $pct = [Math]::Max(1, [Math]::Min(100, $pct))
  $m = Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods -ErrorAction Stop | Select-Object -First 1
  Invoke-CimMethod -InputObject $m -MethodName WmiSetBrightness -Arguments @{ Timeout = 1; Brightness = [byte]$pct } | Out-Null
  return $pct
}

# ------------------------------------------------------------------ dark mode
$PersonalizeKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
function Get-DarkMode { return ((Get-ItemProperty $PersonalizeKey -ErrorAction SilentlyContinue).AppsUseLightTheme -eq 0) }

# ------------------------------------------------------------------ power plans (end4's power profile toggle)
# Windows 11's power-mode slider only works on the Balanced plan, so this switches whole plans.
function Get-PowerPlans {
  powercfg /list | ForEach-Object {
    if ($_ -match '([0-9a-f-]{36})\s+\((.+)\)(\s*\*)?') { [ordered]@{ id = $Matches[1]; name = $Matches[2]; active = [bool]$Matches[3] } }
  }
}

# ------------------------------------------------------------------ clipboard history (Win+V's list)
function Get-ClipItems {
  [Windows.ApplicationModel.DataTransfer.Clipboard, Windows.ApplicationModel.DataTransfer, ContentType = WindowsRuntime] | Out-Null
  [Windows.ApplicationModel.DataTransfer.ClipboardHistoryItemsResult, Windows.ApplicationModel.DataTransfer, ContentType = WindowsRuntime] | Out-Null
  if (-not [Windows.ApplicationModel.DataTransfer.Clipboard]::IsHistoryEnabled()) { throw 'Clipboard history is off. Turn it on in Settings > System > Clipboard (or press Win+V).' }
  $r = Await ([Windows.ApplicationModel.DataTransfer.Clipboard]::GetHistoryItemsAsync()) ([Windows.ApplicationModel.DataTransfer.ClipboardHistoryItemsResult])
  return @($r.Items)
}

# ------------------------------------------------------------------ dispatch
try {
  switch ($Action) {
    'status' {
      Out-Json @{
        wifi       = (Get-RadioState 'WiFi')
        bluetooth  = (Get-RadioState 'Bluetooth')
        caffeine   = (Test-Caffeine)
        darkMode   = (Get-DarkMode)
        brightness = (Get-Brightness)
      }
    }
    'wifi'       { Out-Json @{ wifi = (Set-Radio 'WiFi' $Value) } }
    'bluetooth'  { Out-Json @{ bluetooth = (Set-Radio 'Bluetooth' $Value) } }
    'caffeine'   { Out-Json @{ caffeine = (Switch-Caffeine) } }
    'powerplan'  {
      # No value: list plans. 'next': switch to the plan after the active one.
      $plans = @(Get-PowerPlans)
      if ($Value -eq 'next' -and $plans.Count -gt 1) {
        $i = [array]::FindIndex($plans, [Predicate[object]]{ param($p) $p.active })
        $next = $plans[($i + 1) % $plans.Count]
        powercfg /setactive $next.id
        $plans = @(Get-PowerPlans)
      }
      Out-Json @{ plans = $plans }
    }
    'clipboard'  {
      $items = Get-ClipItems
      $list = for ($i = 0; $i -lt [Math]::Min($items.Count, 30); $i++) {
        $c = $items[$i].Content
        $text = if ($c.Contains('Text')) { Await ($c.GetTextAsync()) ([string]) } else { $null }
        if ($text -and $text.Length -gt 400) { $text = $text.Substring(0, 400) }
        [ordered]@{ i = $i; text = $text; image = [bool]$c.Contains('Bitmap'); time = $items[$i].Timestamp.ToUnixTimeSeconds() }
      }
      Out-Json @{ items = @($list) }
    }
    'clipboard-set' {
      # Value: index from 'clipboard'. Makes that history item the current clipboard content.
      $items = Get-ClipItems
      $item = $items[[int]$Value]
      if (-not $item) { throw 'That clipboard item is gone.' }
      Out-Json @{ ok = ([Windows.ApplicationModel.DataTransfer.Clipboard]::SetHistoryItemAsContent($item).ToString() -eq 'Success') }
    }
    'gamemode'   {
      # Frees RAM for games: stops Zebar (its widgets, webviews and bridge streams go with it), Flow
      # Launcher and Wallpaper Engine's wallpapers; run again to bring them back. GlazeWM keeps
      # running so the shortcut still works. "On" = Zebar not running, so no state file can go stale.
      $we = (Get-Process wallpaper64, wallpaper32 -ErrorAction SilentlyContinue | Select-Object -First 1).Path
      $flow = Join-Path $env:LOCALAPPDATA 'FlowLauncher\Flow.Launcher.exe'
      if (Get-Process zebar -ErrorAction SilentlyContinue) {
        Stop-Process -Name zebar, Flow.Launcher -Force -ErrorAction SilentlyContinue
        if ($we) { & $we -control stop }
        Out-Json @{ gamemode = $true }
      } else {
        if ($we) { & $we -control play }
        Start-Process zebar
        if (Test-Path $flow) { Start-Process $flow }
        Out-Json @{ gamemode = $false }
      }
    }
    'brightness' { Out-Json @{ brightness = (Set-Brightness ([int]$Value)) } }
    'osk'        { Start-Process osk; Out-Json @{ ok = $true } }
    { $_ -in 'colorpick', 'screentranslate' } {
      # Full-screen bridge tools, started detached so they outlive the sidebar that started them.
      $tool = @{ colorpick = 'color-pick'; screentranslate = 'screen-translate' }[$Action]
      $pyw = Join-Path $StateDir 'venv\Scripts\pythonw.exe'
      Start-Process $pyw -ArgumentList '-X', 'utf8', "`"$(Join-Path $PSScriptRoot 'end4_bridge.py')`"", $tool
      Out-Json @{ ok = $true }
    }
    'snip'       { Start-Process 'ms-screenclip:'; Out-Json @{ ok = $true } }
    'settings'   {
      # Value is a ms-settings page name, e.g. "nightlight", "network-wifi", "bluetooth".
      $page = if ($Value -match '^[a-z-]+$') { $Value } else { '' }
      Start-Process ("ms-settings:" + $page); Out-Json @{ ok = $true }
    }
    'taskmgr'    { Start-Process taskmgr; Out-Json @{ ok = $true } }
    'actioncenter' { Start-Process 'ms-actioncenter:'; Out-Json @{ ok = $true } }
    'editconfig' { Start-Process notepad (Join-Path $env:USERPROFILE '.glzr\zebar\end4-pC\config.json'); Out-Json @{ ok = $true } }
    'timer-done' {
      # Windows toast when a pomodoro phase ends (end4 uses notify-send). Value is the phase that ended.
      $msg = @{ focus = 'Focus session done. Take a break.'; short = 'Break is over. Back to focus.'; long = 'Long break is over. Back to focus.' }[$Value]
      if (-not $msg) { throw "unknown phase '$Value'" }
      [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
      [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
      $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
      $xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>Pomodoro</text><text>$msg</text></binding></visual><audio src='ms-winsoundevent:Notification.Reminder'/></toast>")
      $app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'   # PowerShell's registered AUMID
      [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show((New-Object Windows.UI.Notifications.ToastNotification $xml))
      Out-Json @{ ok = $true }
    }
    'lock'       { rundll32.exe user32.dll, LockWorkStation; Out-Json @{ ok = $true } }
    'sleep'      {
      Add-Type -AssemblyName System.Windows.Forms
      [System.Windows.Forms.Application]::SetSuspendState('Suspend', $false, $false) | Out-Null
      Out-Json @{ ok = $true }
    }
    'logout'     { shutdown.exe /l; Out-Json @{ ok = $true } }
    'restart'    { shutdown.exe /r /t 0; Out-Json @{ ok = $true } }
    'shutdown'   { shutdown.exe /s /t 0; Out-Json @{ ok = $true } }
    default      { Out-Json @{ error = "unknown action '$Action'" } }
  }
}
catch {
  Out-Json @{ error = $_.Exception.Message }
}
