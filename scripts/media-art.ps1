<#
  end4-pC for Windows - saves the current media session's album art.

  Zebar's media provider has no artwork, so the widgets call this when the
  track changes. It asks Windows' media session manager (the same source the
  volume flyout uses) for the thumbnail and writes it into the widget pack as
  cover.jpg. Prints {"ok":true,"title":...} or {"ok":false}.
#>
param([string]$Out = '')

$ErrorActionPreference = 'Stop'
if (-not $Out) { $Out = Join-Path $env:USERPROFILE '.glzr\zebar\end4-pC\cover.jpg' }

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
  $props = Await ($session.TryGetMediaPropertiesAsync()) $propsType
  if ($null -eq $props.Thumbnail) { Out-Json @{ ok = $false; title = $props.Title }; exit 0 }

  $winrtStream = Await ($props.Thumbnail.OpenReadAsync()) $streamType
  $src = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($winrtStream)
  $tmp = "$Out.$PID.tmp"
  $dst = [System.IO.File]::Create($tmp)
  try { $src.CopyTo($dst) } finally { $dst.Dispose(); $src.Dispose() }
  Move-Item -Force $tmp $Out
  Out-Json @{ ok = $true; title = $props.Title; artist = $props.Artist }
}
catch {
  Out-Json @{ ok = $false; error = $_.Exception.Message }
}
