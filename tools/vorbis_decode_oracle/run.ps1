# Compiles and runs VorbisOracle.java against the real Minecraft 26.2 client jar
# (JOrbisAudioStream, the vanilla Ogg Vorbis decoder) over a fixed sample of the
# local asset store, capturing exact frame counts and a PCM hash to vectors.tsv.
#
#   pwsh tools/vorbis_decode_oracle/run.ps1 [-Out vectors.tsv]
#
# The classpath is built from 26.2.json and never from a glob, for the reason
# tools/openal_loopback_oracle/run.ps1 records (several versions of one artefact
# sit in shared/libraries).

param([string]$Out = "$PSScriptRoot\vectors.tsv")

$ErrorActionPreference = "Stop"

$jdk       = "$env:APPDATA\EwoClient\jdks\temurin-25\bin"
$libRoot   = "$env:APPDATA\EwoClient\shared\libraries"
$verJson   = "$env:APPDATA\EwoClient\shared\versions\26.2\26.2.json"
$clientJar = "$env:APPDATA\EwoClient\shared\versions\26.2\26.2.jar"
$assets    = "$env:APPDATA\EwoClient\shared\assets"

foreach ($p in @("$jdk\javac.exe", "$jdk\java.exe", $verJson, $clientJar, $assets)) {
    if (-not (Test-Path $p)) { throw "missing prerequisite: $p" }
}

$manifest = Get-Content $verJson -Raw | ConvertFrom-Json
$cpEntries = New-Object System.Collections.Generic.List[string]
$cpEntries.Add($clientJar)
foreach ($l in $manifest.libraries) {
    $rel = $l.downloads.artifact.path
    if (-not $rel) { continue }
    $full = Join-Path $libRoot ($rel -replace '/', '\')
    if (Test-Path $full) { $cpEntries.Add($full) }
}
$cp = [string]::Join(";", $cpEntries)

$outDir = Join-Path $env:TEMP "vorbis_decode_oracle_build"
if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }

& "$jdk\javac.exe" -nowarn -cp $cp -d $outDir "$PSScriptRoot\VorbisOracle.java"
if ($LASTEXITCODE -ne 0) { throw "javac failed ($LASTEXITCODE)" }

$captured = & "$jdk\java.exe" --enable-native-access=ALL-UNNAMED -cp "$outDir;$cp" VorbisOracle $assets $manifest.assetIndex.id
if ($LASTEXITCODE -ne 0) { throw "oracle run failed ($LASTEXITCODE)" }

# LF endings, no BOM: the file is read by a Rust test via include_str!.
[System.IO.File]::WriteAllText(
    $Out,
    ([string]::Join("`n", $captured) + "`n"),
    (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $Out"
