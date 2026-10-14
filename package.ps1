# package.ps1 — build a self-contained, movable EwoClient bundle.
#
#   .\package.ps1                      # dist\EwoClient + the Desktop shortcut
#   .\package.ps1 -NoShortcut -Zip     # a release: no shortcut, plus a zip
#
# Produces dist\EwoClient\ containing EwoClient.exe, rewo.exe, ewo_jni.dll
# (the in-game HUD native) and assets\fonts (+ icon). The exe resolves its
# fonts next to itself (see ewo-render text.rs) and passes the bundled
# ewo_jni.dll to the game (-Dewo.hud.nativePath), so the launcher UI, Rewo
# and the HUD native all run from wherever the folder is moved.
#
# Also bundled: the EwoLoader manifests as dist\EwoClient\loader\0.1.0\*.json
# with every file:// jar they list (loader fat jar, ewo-hud, local mods) copied
# to loader\0.1.0\libs\ and referenced as the relative "libs/<jar>" (see
# tools\bundle-loader.ps1). The launcher finds that folder next to the exe, so
# Ewo instances work wherever the bundle is moved. Packaging FAILS if a listed
# jar is missing (build EwoLoaderV1 first: `./gradlew fatJar`). Pass
# -LoaderRepo to use a checkout other than the sibling ..\EwoLoaderV1.
# EWO_LOADER_BASE still overrides at run time.
#
# **The HUD jar is built here, from this checkout** (`ingame-mod\build.ps1
# -NoDeploy`, the legit variant), and substituted for the manifests' ewo-hud
# entry, so the bundled jar and ewo_jni.dll always come from the same commit.
# Before 2026-10-15 the bundle took whatever jar was last deployed to the dev
# drop the manifests name, which could be another worktree's or weeks old; a
# jar and DLL that disagree fail the HUD's ABI handshake and the HUD stays off.
#
# dist\EwoClient\BUILD.txt records the version, commit and build time.
# tools\check_bundle.py then checks the staged bundle (every file present, no
# file:// left in a manifest, no gate code in rewo.exe) and the run fails if it
# does not pass.
#
# -Zip also writes dist\EwoClient-<version>-<commit>-windows-x64.zip.
# Without -NoShortcut it (re)creates the Desktop "EwoClient" shortcut pointing
# at the bundle, which is what the owner's machine launches.
#
# (For quick dev iteration use `cargo run -p ewo-launcher` instead — that path
# falls back to the in-repo assets, no packaging needed.)

param(
    [string]$LoaderRepo = (Join-Path $PSScriptRoot "..\EwoLoaderV1"),
    [switch]$NoShortcut,
    [switch]$Zip
)

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$dist = Join-Path $root "dist\EwoClient"

# The HUD jar first: it is the step most likely to fail (it needs JDK 25),
# and failing here costs nothing.
Write-Host "[package] building the HUD jar (legit, -NoDeploy) ..." -ForegroundColor Cyan
& (Join-Path $root "ingame-mod\build.ps1") -NoDeploy
$hudJar = Join-Path $root "ingame-mod\build\ewo-hud-0.1.0.jar"
if (-not (Test-Path $hudJar)) { throw "HUD jar not found after build: $hudJar" }

# Stage the loader bundle next so a missing jar fails before the long build
# and before the previous dist\ is wiped.
$loaderStage = Join-Path ([IO.Path]::GetTempPath()) ("ewo-loader-bundle-" + $PID)
if (Test-Path $loaderStage) { Remove-Item -Recurse -Force $loaderStage }
Write-Host "[package] bundling loader manifests from $LoaderRepo ..." -ForegroundColor Cyan
& (Join-Path $root "tools\bundle-loader.ps1") -OutDir $loaderStage -LoaderRepo $LoaderRepo `
    -Override @{ "ewo-hud-0.1.0.jar" = $hudJar }

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
Copy-Item -Force (Join-Path $root "LICENSE") (Join-Path $dist "LICENSE.txt")

$loaderDest = Join-Path $dist "loader\0.1.0"
New-Item -ItemType Directory -Force $loaderDest | Out-Null
Copy-Item -Recurse -Force (Join-Path $loaderStage "*") $loaderDest
Remove-Item -Recurse -Force $loaderStage

# Provenance. The version is the workspace's; a dirty tree is said so, because
# a bundle built from uncommitted work cannot be rebuilt from its commit.
$version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$commit = (& git -C $root rev-parse --short=12 HEAD).Trim()
$dirty = [bool](& git -C $root status --porcelain --untracked-files=no)
$built = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$buildTxt = @(
    "EwoClient $version",
    "commit  $commit$(if ($dirty) { ' (dirty tree)' } else { '' })",
    "built   $built",
    "hud     legit (ingame-mod\build.ps1 -NoDeploy)"
) -join "`n"
[IO.File]::WriteAllText((Join-Path $dist "BUILD.txt"), $buildTxt + "`n", (New-Object System.Text.UTF8Encoding($false)))

Write-Host "[package] checking the bundle ..." -ForegroundColor Cyan
& python (Join-Path $root "tools\check_bundle.py") $dist
if ($LASTEXITCODE -ne 0) { throw "tools\check_bundle.py failed ($LASTEXITCODE)" }

if ($Zip) {
    $zipPath = Join-Path $root "dist\EwoClient-$version-$commit-windows-x64.zip"
    if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
    Compress-Archive -Path $dist -DestinationPath $zipPath
    Write-Host "  zip     : $zipPath"
}

if (-not $NoShortcut) {
    $bundleExe = Join-Path $dist "EwoClient.exe"
    $desktop = [Environment]::GetFolderPath('Desktop')
    $lnk = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $desktop 'EwoClient.lnk'))
    $lnk.TargetPath = $bundleExe
    $lnk.WorkingDirectory = $dist
    $lnk.IconLocation = "$bundleExe,0"
    $lnk.Description = "EwoClient launcher"
    $lnk.Save()
    Write-Host "  shortcut: $desktop\EwoClient.lnk -> the bundle"
}

Write-Host "[package] done." -ForegroundColor Green
Write-Host "  bundle  : $dist  (movable; includes the EwoLoader manifests + jars)"
Write-Host "  version : $version ($commit$(if ($dirty) { ', dirty' } else { '' }))"
