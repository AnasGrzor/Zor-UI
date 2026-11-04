<#
  Zor UI - album art, playback position and seeking for the current media session.

  Zebar's media provider has no artwork, and its position is a stale snapshot without the time
  Windows took it, so the widgets call this. It asks Windows' media session manager (the same
  source the volume flyout uses).

    media-art.ps1               saves the thumbnail as cover.jpg: {"ok":true,"title":...} or {"ok":false}
    media-art.ps1 -Timeline     {"ok":true,"position":s,"end":s,"updated":unix ms,"playing":bool,"canSeek":bool}
    media-art.ps1 -Seek 93.5    jumps to 93.5 s, then prints the timeline
#>
param([string]$Out = '', [switch]$Timeline, [string]$Seek = '')

$ErrorActionPreference = 'Stop'
if (-not $Out) { $Out = Join-Path $env:USERPROFILE '.glzr\zebar\zor-ui\cover.jpg' }

function Out-Json($obj) { $obj | ConvertTo-Json -Compress }

try {
  Add-Type -AssemblyName System.Runtime.WindowsRuntime
  $methods = [System.WindowsRuntimeSystemExtensions].GetMethods()
  $asTaskOp = $methods | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } | Select-Object -First 1
  function Await($op, [Type]$t) { $task = $asTaskOp.MakeGenericMethod($t).Invoke($null, @($op)); $task.Wait(-1) | Out-Null; $task.Result }

  $mgrType = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
  $propsType = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties, Windows.Media.Control, ContentType = WindowsRuntime]
  $streamType = [Windows.Storage.Streams.IRandomAccessStreamWithContentType, Windows.Storage.Streams, ContentType = WindowsRuntime]

  $mgr = Await ($mgrType::RequestAsync()) $mgrType
  $session = $mgr.GetCurrentSession()
  if ($null -eq $session) { Out-Json @{ ok = $false }; exit 0 }

  if ($Seek -or $Timeline) {
    if ($Seek) {
      $ticks = [long]([double]::Parse($Seek, [Globalization.CultureInfo]::InvariantCulture) * 10000000)
      [void](Await ($session.TryChangePlaybackPositionAsync($ticks)) ([bool]))
      Start-Sleep -Milliseconds 150   # let the player publish its new timeline
    }
    $t = $session.GetTimelineProperties(); $p = $session.GetPlaybackInfo()
    Out-Json @{
      ok = $true; position = $t.Position.TotalSeconds; end = $t.EndTime.TotalSeconds
      updated = $t.LastUpdatedTime.ToUnixTimeMilliseconds()
      playing = ($p.PlaybackStatus -eq 'Playing'); canSeek = [bool]$p.Controls.IsPlaybackPositionEnabled
    }
    exit 0
  }

  $props = Await ($session.TryGetMediaPropertiesAsync()) $propsType
  if ($null -eq $props.Thumbnail) { Out-Json @{ ok = $false; title = $props.Title }; exit 0 }

  $winrtStream = Await ($props.Thumbnail.OpenReadAsync()) $streamType
  # PowerShell 5.1 can't cast the WinRT stream to IInputStream itself ("no overload"); reflection's
  # Invoke does the COM interface lookup, the same way Await does for IAsyncOperation.
  $asStream = [System.IO.WindowsRuntimeStreamExtensions].GetMethod('AsStreamForRead', [Type[]]@([Windows.Storage.Streams.IInputStream]))
  $src = $asStream.Invoke($null, @($winrtStream))
  $tmp = "$Out.$PID.tmp"
  $dst = [System.IO.File]::Create($tmp)
  try { $src.CopyTo($dst) } finally { $dst.Dispose(); $src.Dispose() }
  Move-Item -Force $tmp $Out
  Out-Json @{ ok = $true; title = $props.Title; artist = $props.Artist }
}
catch {
  Out-Json @{ ok = $false; error = $_.Exception.Message }
}
