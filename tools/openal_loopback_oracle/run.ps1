# Compiles and runs LoopbackOracle.java against the *real* Minecraft 26.2 jars
# and the *real* OpenAL Soft that ships with them, capturing PCM to vectors.tsv.
#
#   pwsh tools/openal_loopback_oracle/run.ps1 [-Out vectors.tsv]
#
# Re-run after a Minecraft version bump, an lwjgl bump, or an OpenAL Soft bump.
# The vector file records the DLL's identity in its own header precisely so a
# stale capture is visible rather than silent.
#
# WHY THE CLASSPATH IS BUILT FROM 26.2.json AND NEVER FROM A GLOB.
# `shared/libraries` holds several versions of the same artefact — lwjgl 3.3.3
# AND 3.4.1, guava 17.0/33.5.0/33.6.0, datafixerupper 9.0.19/10.0.21,
# netty-buffer 4.2.7/4.2.15. A `Get-ChildItem -Recurse -Filter *.jar` puts both
# versions of each on the path and the JVM takes the first, silently. 26.2.json
# names exactly one path per artefact; that list is the only correct answer, and
# one earlier survey in this project graded the wrong lwjgl jar by globbing.

param(
    [string]$Out = "$PSScriptRoot\vectors.tsv",
    # Frames of silence rendered between stimuli. Leave unset for the value the
    # checked-in capture used; pass one to re-tune it against the ctl.posx pair.
    [int]$Settle = 0
)

$ErrorActionPreference = "Stop"

$jdk     = "$env:APPDATA\EwoClient\jdks\temurin-25\bin"
$libRoot = "$env:APPDATA\EwoClient\shared\libraries"
$verJson = "$env:APPDATA\EwoClient\shared\versions\26.2\26.2.json"
$clientJar = "$env:APPDATA\EwoClient\shared\versions\26.2\26.2.jar"

foreach ($p in @("$jdk\javac.exe", "$jdk\java.exe", $verJson, $clientJar)) {
    if (-not (Test-Path $p)) { throw "missing prerequisite: $p" }
}

# Every declared artefact that is actually on disk. The macos/linux natives are
# not, which is why "exists" is a filter rather than an assertion.
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

$outDir = Join-Path $PSScriptRoot "build"
if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Path $outDir | Out-Null }

Write-Host "classpath entries: $($cpEntries.Count)"
& "$jdk\javac.exe" -nowarn -cp $cp -d $outDir "$PSScriptRoot\LoopbackOracle.java"
if ($LASTEXITCODE -ne 0) { throw "javac failed ($LASTEXITCODE)" }

# The natives jars are on the classpath, so lwjgl's SharedLibraryLoader
# extracts OpenAL.dll itself; no manual -Dorg.lwjgl.librarypath is needed.
#
# `--enable-native-access=ALL-UNNAMED` only silences lwjgl's restricted-method
# warning; without it the run still works but writes four WARNING lines to
# stderr, which is noise in a capture whose whole point is provenance.
$oracleArgs = @()
if ($Settle -gt 0) { $oracleArgs += "$Settle" }
$captured = & "$jdk\java.exe" --enable-native-access=ALL-UNNAMED -cp "$outDir;$cp" LoopbackOracle @oracleArgs
if ($LASTEXITCODE -ne 0) { throw "oracle run failed ($LASTEXITCODE)" }

# Written through .NET rather than Out-File because Windows PowerShell 5.1's
# `-Encoding utf8` emits a BOM, and a BOM at the head of a TSV is invisible in
# every viewer and breaks the first row for any consumer that tests the first
# character. Rust's include_str! keeps it, so the Rust parser saw a one-column
# row where a comment should have been. LF endings for the same reason: the
# file is read by a Rust test, not by Notepad.
[System.IO.File]::WriteAllText(
    $Out,
    ([string]::Join("`n", $captured) + "`n"),
    (New-Object System.Text.UTF8Encoding($false)))

Write-Host "wrote $Out"
