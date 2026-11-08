<#
.SYNOPSIS
  Builds ZorUI-Setup.exe: one double-click installer (a wizard window, not a terminal) with the native app
  prebuilt (no Rust needed on the target) and without the private buddies.
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\scripts\build-release.ps1
#>
$ErrorActionPreference = 'Stop'
$Repo = Split-Path $PSScriptRoot
$Out = Join-Path $Repo 'dist'
$Stage = Join-Path $Out 'stage'
Remove-Item $Out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage | Out-Null

$env:ZORUI_PUBLIC = '1'   # build.rs then leaves the private buddies (cast_personal.rs) out
& cargo build --release --manifest-path (Join-Path $Repo 'native\zor-ui\Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$pay = Join-Path $Out 'app'
New-Item -ItemType Directory -Force $pay | Out-Null
foreach ($f in 'install.ps1', 'uninstall.ps1', 'LICENSE') { Copy-Item (Join-Path $Repo $f) $pay }
foreach ($d in 'scripts', 'flowlauncher', 'glazewm', 'assets') { Copy-Item -Recurse (Join-Path $Repo $d) $pay }
Copy-Item (Join-Path $Repo 'native\zor-ui\target\release\zorui.exe') $pay
Compress-Archive -Path "$pay\*" -DestinationPath (Join-Path $Stage 'zorui-app.zip')
Copy-Item (Join-Path $Stage 'zorui-app.zip') $Out   # upload this too: the in-app updater downloads it
# Builds up to 0.1.2 look for an asset named payload.zip, so this release carries the same zip under that name.
# Delete this line (and "payload.zip" in ASSETS in native/zor-ui/src/bridge/update.rs) in the release after.
Copy-Item (Join-Path $Stage 'zorui-app.zip') (Join-Path $Out 'payload.zip')

# The installer: a wizard window (native/zor-setup) with the app zip inside it. build.rs embeds the zip named here.
$env:ZORSETUP_ZIP = Join-Path $Out 'zorui-app.zip'
& cargo build --release --manifest-path (Join-Path $Repo 'native\zor-setup\Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build (zor-setup) failed' }
$exe = Join-Path $Out 'ZorUI-Setup.exe'
Copy-Item (Join-Path $Repo 'native\zor-setup\target\release\ZorUI-Setup.exe') $exe
Get-Item $exe | Select-Object FullName, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}
