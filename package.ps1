# package.ps1 — build a self-contained, movable EwoClient bundle.
#
#   .\package.ps1
#
# Produces dist\EwoClient\ containing EwoClient.exe, rewo.exe, ewo_jni.dll
# (the in-game HUD native) and assets\fonts (+ icon). The exe resolves its
# fonts next to itself (see ewo-render text.rs) and passes the bundled
# ewo_jni.dll to the game (-Dewo.hud.nativePath), so the launcher UI, Rewo
# and the HUD native all run from wherever the folder is moved.
#
# NOT bundled: the EwoLoader manifests + fat jar (Ewo instances read them via
# the EWO_LOADER_BASE env var, default = the dev checkout under the author's
# Desktop) and the ewo-hud mod jar (listed in those manifests). Vanilla and
# Native instances need neither.
#
# Also (re)creates the Desktop "EwoClient" shortcut pointing at the bundle.
#
# Run this after code changes to refresh the bundle the shortcut launches.
# (For quick dev iteration use `cargo run -p ewo-launcher` instead — that path
# falls back to the in-repo assets, no packaging needed.)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$dist = Join-Path $root "dist\EwoClient"

Write-Host "[package] building release..." -ForegroundColor Cyan
Push-Location $root
try {
    $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    & cargo build --release -p ewo-launcher -p rewo-app -p ewo-jni
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) { throw "cargo build failed ($code)" }
} finally { Pop-Location }

$exe = Join-Path $root "target\release\ewolauncher.exe"
if (-not (Test-Path $exe)) { throw "exe not found: $exe" }
# Rewo rides along: find_rewo_binary() looks next to the launcher exe, so
# Native instances launch from the dist bundle too.
$rewo = Join-Path $root "target\release\rewo.exe"
if (-not (Test-Path $rewo)) { throw "exe not found: $rewo" }
$jni = Join-Path $root "target\release\ewo_jni.dll"
if (-not (Test-Path $jni)) { throw "dll not found: $jni" }

Write-Host "[package] staging $dist ..." -ForegroundColor Cyan
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force (Join-Path $dist "assets\fonts") | Out-Null
Copy-Item -Force $exe (Join-Path $dist "EwoClient.exe")
Copy-Item -Force $rewo (Join-Path $dist "rewo.exe")
Copy-Item -Force $jni (Join-Path $dist "ewo_jni.dll")
Copy-Item -Force (Join-Path $root "assets\fonts\*") (Join-Path $dist "assets\fonts")
$icon = Join-Path $root "assets\icon.ico"
if (Test-Path $icon) { Copy-Item -Force $icon (Join-Path $dist "assets\icon.ico") }

$bundleExe = Join-Path $dist "EwoClient.exe"
$desktop = [Environment]::GetFolderPath('Desktop')
$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $desktop 'EwoClient.lnk'))
$lnk.TargetPath = $bundleExe
$lnk.WorkingDirectory = $dist
$lnk.IconLocation = "$bundleExe,0"
$lnk.Description = "EwoClient launcher"
$lnk.Save()

Write-Host "[package] done." -ForegroundColor Green
Write-Host "  bundle  : $dist  (movable; Ewo instances still need EWO_LOADER_BASE)"
Write-Host "  shortcut: $desktop\EwoClient.lnk -> the bundle"
