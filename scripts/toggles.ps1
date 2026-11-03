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

function Out-Json($obj) { $obj | ConvertTo-Json -Compress -Depth 4 }

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
    'brightness' { Out-Json @{ brightness = (Set-Brightness ([int]$Value)) } }
    'osk'        { Start-Process osk; Out-Json @{ ok = $true } }
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
