<#
  end4-pC for Windows - wallpaper + Material You switcher (port of end4's switchwall.sh).

    switchwall.ps1                       # pick an image with a file dialog
    switchwall.ps1 -Image C:\wall.jpg    # set wallpaper and theme from it
    switchwall.ps1 -Color '#6750A4'      # theme from a color, keep wallpaper
    switchwall.ps1 -ToggleMode           # flip dark/light, keep source
    switchwall.ps1 -Mode light -Scheme expressive
    switchwall.ps1 -Image C:\clip.mp4      # live wallpaper (Wallpaper Engine if installed, else Lively)

  Sets the Windows wallpaper, generates the palette (scripts/end4_theme.py) and
  pushes it to Zebar, GlazeWM, Flow Launcher and Windows Terminal, then switches
  Windows' own app/system theme to match dark/light. Videos become live
  wallpapers through Lively (end4 uses mpvpaper), themed from one frame, and the
  Windows lock screen picture follows the wallpaper.
#>
param(
  [string]$Image = '',
  [string]$Color = '',
  [ValidateSet('', 'dark', 'light')][string]$Mode = '',
  [ValidateSet('', 'auto', 'content', 'expressive', 'fidelity', 'fruit-salad', 'monochrome', 'neutral', 'rainbow', 'tonal-spot', 'vibrant')]
  [string]$Scheme = '',
  [switch]$ToggleMode,
  [switch]$Pick,
  [string]$Live = '',        # set by end4_bridge.py: 'we:<path>' for Wallpaper Engine wallpapers
  [switch]$ThemeOnly         # only regenerate colors; leave the Windows wallpaper and lock screen alone
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot          # %LOCALAPPDATA%\end4-pC
$Python = Join-Path $Root 'venv\Scripts\python.exe'
$Theme = Join-Path $PSScriptRoot 'end4_theme.py'
$StatePath = Join-Path $Root 'state.json'
$Bridge = Join-Path $PSScriptRoot 'end4_bridge.py'
$PackConfig = Join-Path $env:USERPROFILE '.glzr\zebar\end4-pC\config.json'
$VideoExt = @('.mp4', '.webm', '.mkv', '.mov', '.avi', '.wmv', '.m4v')

function Find-Lively {
  $cands = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Lively Wallpaper\Livelycu.exe'),
    (Join-Path $env:ProgramFiles 'Lively Wallpaper\Livelycu.exe'),
    (Get-Command Livelycu -ErrorAction SilentlyContinue).Source
  )
  $cands | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}

function Set-LockScreenImage([string]$path) {
  # Best effort: Windows may refuse this for desktop apps or when a policy controls the lock screen.
  try {
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $m = [System.WindowsRuntimeSystemExtensions].GetMethods()
    $asTaskOp = $m | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } | Select-Object -First 1
    $asTaskAction = $m | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and -not $_.IsGenericMethod -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncAction' } | Select-Object -First 1
    $fileType = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]
    [Windows.System.UserProfile.LockScreen, Windows.System.UserProfile, ContentType = WindowsRuntime] | Out-Null
    $t = $asTaskOp.MakeGenericMethod($fileType).Invoke($null, @($fileType::GetFileFromPathAsync($path))); $t.Wait(-1) | Out-Null
    $t2 = $asTaskAction.Invoke($null, @([Windows.System.UserProfile.LockScreen]::SetImageFileAsync($t.Result))); $t2.Wait(-1) | Out-Null
    return $true
  } catch { return $false }
}

function Out-Json($obj) { $obj | ConvertTo-Json -Compress -Depth 4 }

try {
  $state = if (Test-Path $StatePath) { Get-Content $StatePath -Raw | ConvertFrom-Json } else { $null }

  # No source given and not just re-theming -> open a picker (end4 does the same).
  $retheme = $ToggleMode -or $Mode -or $Scheme
  if ($Pick -or (-not $Image -and -not $Color -and -not $retheme)) {
    Add-Type -AssemblyName System.Windows.Forms
    $dlg = New-Object System.Windows.Forms.OpenFileDialog
    $dlg.Title = 'Choose a wallpaper'
    $dlg.Filter = 'Images and videos|*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.gif;*.mp4;*.webm;*.mkv;*.mov|Images|*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.gif|Videos (live wallpaper)|*.mp4;*.webm;*.mkv;*.mov;*.avi;*.wmv|All files|*.*'
    $pics = [Environment]::GetFolderPath('MyPictures')
    $dlg.InitialDirectory = if (Test-Path (Join-Path $pics 'Wallpapers')) { Join-Path $pics 'Wallpapers' } else { $pics }
    if ($dlg.ShowDialog() -ne 'OK') { Out-Json @{ ok = $false; cancelled = $true }; exit 0 }
    $Image = $dlg.FileName
  }

  if ($ToggleMode) {
    $current = if ($state -and $state.mode) { $state.mode } else { 'dark' }
    $Mode = if ($current -eq 'dark') { 'light' } else { 'dark' }
  }

  # 0. Live wallpaper: play the video with Wallpaper Engine (or Lively), theme from one of its frames.
  $lively = Find-Lively
  $isVideo = $Image -and ($VideoExt -contains ([IO.Path]::GetExtension($Image).ToLower()))
  if ($isVideo -and -not $Live) {
    $ErrorActionPreference = 'Continue'
    $weOut = & $Python -X utf8 $Bridge we-status 2>$null | Select-Object -Last 1
    $ErrorActionPreference = 'Stop'
    $we = $null; try { $we = $weOut | ConvertFrom-Json } catch { }
    if ($we -and $we.installed) {
      # end4_bridge.py opens it in Wallpaper Engine and calls back into this script with a frame.
      $ErrorActionPreference = 'Continue'
      $res = & $Python -X utf8 $Bridge we-apply (Resolve-Path $Image).Path 2>$null | Select-Object -Last 1
      $ErrorActionPreference = 'Stop'
      $r = $res | ConvertFrom-Json
      if ($r.theme) { $r.theme | ConvertTo-Json -Compress -Depth 4 } else { $res }
      exit 0
    }
  }
  if ($isVideo -and -not $Live) {
    if (-not $lively) { throw 'Live wallpapers need Wallpaper Engine or Lively Wallpaper (winget install rocksdanister.LivelyWallpaper, or re-run install.ps1 -WithLively).' }
    $Live = (Resolve-Path $Image).Path
    $frame = Join-Path $Root 'live-frame.png'
    $ErrorActionPreference = 'Continue'
    $null = & $Python -X utf8 $Bridge video-frame $Live $frame 2>$null
    & $lively setwp --file $Live 2>$null | Out-Null
    $ErrorActionPreference = 'Stop'
    if (-not (Test-Path $frame)) { throw "Couldn't read a frame from $Live" }
    $Image = $frame
  } elseif ($Image -and -not $Live -and (($state -and $state.live) -or ($weRunning = [bool](Get-Process wallpaper32, wallpaper64 -ErrorAction SilentlyContinue)))) {
    # Back to a static wallpaper: stop whichever app was playing the live one. Wallpaper Engine is
    # closed even when end4 didn't start it (state.live empty): it draws over the Windows wallpaper.
    $ErrorActionPreference = 'Continue'
    if ($weRunning -or "$($state.live)".StartsWith('we:')) { $null = & $Python -X utf8 $Bridge we-close 2>$null }
    elseif ($lively) { & $lively closewp --monitor -1 2>$null | Out-Null }
    $ErrorActionPreference = 'Stop'
  }

  # 1. Wallpaper. IDesktopWallpaper crossfades to the new picture (end4 animates its wallpaper
  #    changes); SPI_SETDESKWALLPAPER, which switches instantly, is the fallback.
  if ($Image) { $Image = (Resolve-Path $Image).Path }
  if ($Image -and -not $ThemeOnly) {
    if (-not ('W.Wall' -as [type])) {
      Add-Type -Namespace W -Name Wall -MemberDefinition '[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int SystemParametersInfo(int a, int b, string c, int d);'
      Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
namespace W {
  // Methods up to SetPosition, in vtable order; only SetWallpaper and SetPosition are called.
  [ComImport, Guid("B92B56A9-8B55-4E14-9A89-0199BBB6F93B"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  public interface IDesktopWallpaper {
    void SetWallpaper([MarshalAs(UnmanagedType.LPWStr)] string monitorId, [MarshalAs(UnmanagedType.LPWStr)] string path);
    IntPtr GetWallpaper([MarshalAs(UnmanagedType.LPWStr)] string monitorId);
    IntPtr GetMonitorDevicePathAt(uint index);
    uint GetMonitorDevicePathCount();
    void GetMonitorRECT([MarshalAs(UnmanagedType.LPWStr)] string monitorId, IntPtr rect);
    void SetBackgroundColor(uint color);
    uint GetBackgroundColor();
    void SetPosition(int position);
  }
  [ComImport, Guid("C2CF3110-460E-4fc1-B9D0-8A1C0C9CC4BD")] public class DesktopWallpaperClass { }
  public static class Fade {
    public static void Set(string path) {
      var dw = (IDesktopWallpaper)new DesktopWallpaperClass();
      dw.SetPosition(4);           // DWPOS_FILL
      dw.SetWallpaper(null, path); // every monitor
    }
  }
}
'@
    }
    Set-ItemProperty 'HKCU:\Control Panel\Desktop' -Name WallpaperStyle -Value '10'   # Fill
    Set-ItemProperty 'HKCU:\Control Panel\Desktop' -Name TileWallpaper -Value '0'
    try { [W.Fade]::Set($Image) }
    catch { [W.Wall]::SystemParametersInfo(0x0014, 0, $Image, 0x01 -bor 0x02) | Out-Null }
  }

  # 2. Palette -> Zebar / GlazeWM / Flow Launcher / Terminal.
  $pyArgs = @('-W', 'ignore', $Theme, '--state', $StatePath)
  if ($Image) { $pyArgs += @('--image', $Image) }
  if ($Live) { $pyArgs += @('--live', $Live) }
  if ($Color) { $pyArgs += @('--color', $Color) }
  if ($Mode) { $pyArgs += @('--mode', $Mode) }
  if ($Scheme) { $pyArgs += @('--scheme', $Scheme) }
  # Windows PowerShell 5.1 turns native stderr into terminating errors under
  # 'Stop', so relax it just for the Python call.
  $ErrorActionPreference = 'Continue'
  $result = & $Python @pyArgs 2>$null | Select-Object -Last 1
  $ErrorActionPreference = 'Stop'
  if (-not $result) { throw "end4_theme.py produced no output (is the venv at $Python?)" }
  $parsed = $result | ConvertFrom-Json

  # 3. Windows' own light/dark setting follows the palette.
  $dark = ($parsed.mode -eq 'dark')
  $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
  $v = if ($dark) { 0 } else { 1 }
  Set-ItemProperty $key -Name AppsUseLightTheme -Value $v -Type DWord
  Set-ItemProperty $key -Name SystemUsesLightTheme -Value $v -Type DWord
  if (-not ('W.Broadcast' -as [type])) {
    Add-Type -Name Broadcast -Namespace W -MemberDefinition '[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, UIntPtr w, string l, uint f, uint t, out UIntPtr r);'
  }
  $out = [UIntPtr]::Zero
  [W.Broadcast]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'ImmersiveColorSet', 0x0002, 1000, [ref]$out) | Out-Null

  # 4. Lock screen picture follows the wallpaper (config: syncLockScreen).
  if ($Image -and -not $ThemeOnly) {
    $sync = $true
    if (Test-Path $PackConfig) { $pc = Get-Content $PackConfig -Raw | ConvertFrom-Json; if ($pc.syncLockScreen -eq $false) { $sync = $false } }
    if ($sync) { $null = Set-LockScreenImage $Image }
  }

  $result
}
catch {
  Out-Json @{ ok = $false; error = $_.Exception.Message }
  exit 1
}
