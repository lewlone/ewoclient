# bundle-loader.ps1 — stage the EwoLoader manifests as a movable bundle.
#
#   .\tools\bundle-loader.ps1 -OutDir dist\EwoClient\loader\0.1.0 [-LoaderRepo ..\EwoLoaderV1]
#
# Reads <LoaderRepo>\manifest\0.1.0\*.json (the loader-manifest contract, see
# .claude/rules/ewo-loader-integration.md). Those manifests name some jars by
# absolute file:// URL (the loader fat jar, the ewo-hud jar, local mods). For
# each such library this script:
#   - copies the jar to <OutDir>\libs\<filename>,
#   - rewrites its url to the relative "libs/<filename>" (the launcher resolves
#     scheme-less urls against the manifest's folder),
#   - recomputes its sha1 and size from the copied file.
# https entries are left untouched. The rest of the manifest text is preserved
# byte for byte (written as UTF-8 without a BOM). A referenced file:// jar that
# does not exist fails the whole run (non-zero exit, nothing half-written is
# worth shipping).
#
# package.ps1 calls this; it is separate so it can be run (and tested) alone.

param(
    [Parameter(Mandatory = $true)][string]$OutDir,
    [string]$LoaderRepo = (Join-Path (Split-Path -Parent $PSScriptRoot) "..\EwoLoaderV1")
)

$ErrorActionPreference = "Stop"

# file:///C:/a%20b/x.jar -> C:\a b\x.jar
function ConvertFrom-FileUrl([string]$url) {
    $rest = $url.Substring("file://".Length)
    if ($rest -match '^/[A-Za-z]:') { $rest = $rest.Substring(1) }
    return ([Uri]::UnescapeDataString($rest)) -replace '/', '\'
}

$srcDir = Join-Path $LoaderRepo "manifest\0.1.0"
if (-not (Test-Path $srcDir)) {
    throw "loader manifests not found: $srcDir (pass -LoaderRepo <path to EwoLoaderV1>)"
}
$manifests = @(Get-ChildItem -Path $srcDir -Filter "*.json" -File)
if ($manifests.Count -eq 0) { throw "no *.json manifests in $srcDir" }

$libsDir = Join-Path $OutDir "libs"
New-Item -ItemType Directory -Force $OutDir | Out-Null
New-Item -ItemType Directory -Force $libsDir | Out-Null

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$copied = @{}   # filename -> sha1, to catch two different jars sharing a name
$urlRe = [regex]'"url"\s*:\s*"(file://[^"]*)"'

foreach ($m in $manifests) {
    $text = [IO.File]::ReadAllText($m.FullName)
    $matches_ = @($urlRe.Matches($text))
    # Back to front so earlier offsets stay valid while we splice.
    for ($i = $matches_.Count - 1; $i -ge 0; $i--) {
        $hit = $matches_[$i]
        $url = $hit.Groups[1].Value
        $src = ConvertFrom-FileUrl $url
        if (-not (Test-Path -LiteralPath $src -PathType Leaf)) {
            throw "$($m.Name): referenced jar is missing: $src (from $url)"
        }
        $name = Split-Path -Leaf $src
        $sha1 = (Get-FileHash -LiteralPath $src -Algorithm SHA1).Hash.ToLowerInvariant()
        $size = (Get-Item -LiteralPath $src).Length
        if ($copied.ContainsKey($name) -and $copied[$name] -ne $sha1) {
            throw "$($m.Name): two different jars are both named $name; rename one"
        }
        Copy-Item -LiteralPath $src -Destination (Join-Path $libsDir $name) -Force
        $copied[$name] = $sha1

        # The artifact object enclosing this url: nearest '{' before, '}' after.
        $objStart = $text.LastIndexOf('{', $hit.Index)
        $objEnd = $text.IndexOf('}', $hit.Index)
        if ($objStart -lt 0 -or $objEnd -lt 0) { throw "$($m.Name): cannot locate the artifact object for $url" }
        $block = $text.Substring($objStart, $objEnd - $objStart + 1)
        $block = [regex]::Replace($block, '("sha1"\s*:\s*")[0-9a-fA-F]*(")', "`${1}$sha1`${2}")
        $block = [regex]::Replace($block, '("size"\s*:\s*)\d+', "`${1}$size")
        $block = [regex]::Replace($block, '("url"\s*:\s*")[^"]*(")', "`${1}libs/$name`${2}")
        $text = $text.Substring(0, $objStart) + $block + $text.Substring($objEnd + 1)
    }

    # Sanity: still valid JSON, no local urls left, every relative url resolves.
    $parsed = $text | ConvertFrom-Json
    foreach ($lib in $parsed.libraries) {
        $a = $lib.downloads.artifact
        if ($null -eq $a) { continue }
        if ($a.url -like "file:*") { throw "$($m.Name): $($lib.name) still has a file:// url" }
        if ($a.url -notlike "https://*") {
            $f = Join-Path $OutDir ($a.url -replace '/', '\')
            if (-not (Test-Path -LiteralPath $f)) { throw "$($m.Name): $($lib.name) -> $($a.url) not found in bundle" }
            if ((Get-FileHash -LiteralPath $f -Algorithm SHA1).Hash.ToLowerInvariant() -ne $a.sha1.ToLowerInvariant()) {
                throw "$($m.Name): $($lib.name) sha1 mismatch after rewrite"
            }
        }
    }

    [IO.File]::WriteAllText((Join-Path $OutDir $m.Name), $text, $utf8NoBom)
    Write-Host "[bundle-loader] $($m.Name): $($matches_.Count) local jar(s) bundled" -ForegroundColor Cyan
}

Write-Host "[bundle-loader] done: $OutDir ($($copied.Count) jar(s) in libs\)" -ForegroundColor Green
