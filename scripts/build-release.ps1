<#
.SYNOPSIS
  Builds ZorUI-Setup.exe: one double-click installer with the native app prebuilt (no Rust needed
  on the target) and without the private buddies.
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
& cargo build --release --manifest-path (Join-Path $Repo 'native\end4\Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$pay = Join-Path $Out 'payload'
New-Item -ItemType Directory -Force $pay | Out-Null
foreach ($f in 'install.ps1', 'uninstall.ps1', 'LICENSE') { Copy-Item (Join-Path $Repo $f) $pay }
foreach ($d in 'scripts', 'flowlauncher', 'glazewm', 'zebar') { Copy-Item -Recurse (Join-Path $Repo $d) $pay }
Get-ChildItem $pay -Recurse -Directory -Filter __pycache__ | Remove-Item -Recurse -Force
Get-ChildItem (Join-Path $pay 'scripts') -Filter 'test_*.py' | Remove-Item -Force
Copy-Item (Join-Path $Repo 'native\end4\target\release\zorui.exe') $pay
Compress-Archive -Path "$pay\*" -DestinationPath (Join-Path $Stage 'payload.zip')
Copy-Item (Join-Path $Stage 'payload.zip') $Out   # upload this too: the in-app updater downloads it

# Runs the installer from a temp folder; the window stays open so errors can be read.
Set-Content (Join-Path $Stage 'setup.cmd') -Encoding ascii -Value @'
@echo off
set "D=%TEMP%\zorui-setup"
rmdir /s /q "%D%" 2>nul
powershell -NoProfile -ExecutionPolicy Bypass -Command "Expand-Archive -Force payload.zip $env:TEMP\zorui-setup"
powershell -NoProfile -ExecutionPolicy Bypass -File "%D%\install.ps1"
pause
'@

$exe = Join-Path $Out 'ZorUI-Setup.exe'
$sed = Join-Path $Stage 'zorui.sed'
Set-Content $sed -Encoding ascii -Value @"
[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
PackagePurpose=InstallApp
ShowInstallProgressWindow=0
HideExtractAnimation=1
UseLongFileName=1
InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
InstallPrompt=
DisplayLicense=
FinishMessage=
TargetName=$exe
FriendlyName=Zor UI Setup
AppLaunched=cmd /c setup.cmd
PostInstallCmd=<None>
AdminQuietInstCmd=
UserQuietInstCmd=
SourceFiles=SourceFiles
[SourceFiles]
SourceFiles0=$Stage\
[SourceFiles0]
%FILE0%=
%FILE1%=
[Strings]
FILE0="setup.cmd"
FILE1="payload.zip"
"@
Start-Process iexpress -ArgumentList '/N', '/Q', $sed -Wait
if (-not (Test-Path $exe)) { throw 'iexpress did not produce the exe' }
Get-Item $exe | Select-Object FullName, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}
