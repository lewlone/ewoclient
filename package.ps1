# package.ps1 — build a self-contained, movable EwoClient bundle.
#
#   .\package.ps1
#
# Produces dist\EwoClient\ containing EwoClient.exe + assets\fonts (+ icon).
# The exe resolves its fonts next to itself (see ewo-render text.rs), so the
# whole dist\EwoClient folder can be moved/copied anywhere and still run.
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
    & cargo build --release -p ewo-launcher
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) { throw "cargo build failed ($code)" }
} finally { Pop-Location }

$exe = Join-Path $root "target\release\ewolauncher.exe"
if (-not (Test-Path $exe)) { throw "exe not found: $exe" }

Write-Host "[package] staging $dist ..." -ForegroundColor Cyan
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force (Join-Path $dist "assets\fonts") | Out-Null
Copy-Item -Force $exe (Join-Path $dist "EwoClient.exe")
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
Write-Host "  bundle  : $dist  (self-contained; move it anywhere)"
Write-Host "  shortcut: $desktop\EwoClient.lnk -> the bundle"
