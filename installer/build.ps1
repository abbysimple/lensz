# Builds Lensz and its installer: installer\output\LenszSetup-<version>.exe
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$here = $PSScriptRoot
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$msbuild = 'C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe'
$iscc = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
$redist = 'C:\Program Files\Microsoft Visual Studio\18\Community\VC\Redist\MSVC\v145\vc_redist.x64.exe'
$target = Join-Path $root 'target-installer'

# 1. App + Windows 11 camera DLL (own target dir, so a running Lensz doesn't lock the files).
& $cargo build --release --workspace --manifest-path (Join-Path $root 'Cargo.toml') --target-dir $target
if ($LASTEXITCODE) { throw 'cargo build failed' }

# 2. DirectShow camera DLL (modified Softcam).
& $msbuild (Join-Path $root 'third_party\softcam\softcam.sln') /t:softcam /p:Configuration=Release /p:Platform=x64 /m /v:minimal /nologo
if ($LASTEXITCODE) { throw 'softcam build failed' }

# 3. Stage the files.
$stage = Join-Path $here 'stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force (Join-Path $stage 'models'), (Join-Path $stage 'assets') | Out-Null
$rel = Join-Path $target 'release'
Copy-Item (Join-Path $rel 'lensz.exe'), (Join-Path $rel 'lens_vcam.dll') $stage
Copy-Item (Join-Path $root 'third_party\softcam\x64\Release\softcam.dll') (Join-Path $stage 'lensz_dshow.dll')
# DirectML.dll next to the exe is a link into ort's download cache: copy the real file.
$dml = Get-Item (Join-Path $rel 'DirectML.dll')
$dmlSrc = if ($dml.LinkType) { @($dml.Target)[0] } else { $dml.FullName }
Copy-Item $dmlSrc (Join-Path $stage 'DirectML.dll')
Copy-Item (Join-Path $root 'models\*.onnx'), (Join-Path $root 'models\NOTICE.md') (Join-Path $stage 'models')
$photo = Join-Path $root 'assets\preview_face.png'
if (Test-Path $photo) { Copy-Item $photo (Join-Path $stage 'assets') }
Copy-Item $redist $stage

# 4. Installer.
& $iscc /Qp (Join-Path $here 'lensz.iss')
if ($LASTEXITCODE) { throw 'Inno Setup failed' }
Get-ChildItem (Join-Path $here 'output') | Select-Object Name, Length
