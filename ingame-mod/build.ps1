# build.ps1 - rebuild both halves of the in-game HUD: the Rust cdylib
# (target/{debug,release}/ewo_jni.dll) and the Fabric mod jar (ewo-hud-0.1.0.jar).
#
# Flags:
#   -Pvp    Build the PvP variant. Includes the dev.lewlone.ewohud.assist
#           package (Auto-action assist modules, Swing Cadence, KnockbackMax
#           mixin) and propagates --features pvp to the cargo build so the
#           Rust REGISTRY adds the assist module slots. Default (no flag) is
#           the legit build — assist sources are skipped, assist mixins are
#           excluded, and the cdylib's REGISTRY tops out at the 12 legit
#           slots. Class fingerprint of "Triggerbot" / "AutoTotem" / etc. is
#           not present in the legit jar at all. See CLAUDE.md "Legit / pvp
#           split (post-ban refactor)".
#
# The cdylib step exists so changes to ewo-core (e.g. modules::REGISTRY) or
# ewo-jni are picked up automatically. Skipping it stranded Auto Totem +
# Legit Elytra Swap behind a stale dll for a session - see CLAUDE.md "stale
# file:// jar gotcha" for the parallel jar-side cache trap.
#
# This toolchain runs mods in Minecraft's mapped namespace with no remapping
# step, so a Fabric mod here is just javac + jar. The mod reads Minecraft
# classes directly (Minecraft.getFps(), RenderSystem, ...), so the Minecraft
# jar is on the build classpath. That jar is Java-25 bytecode, which only a
# JDK 25 javac can read - hence the temurin-25 toolchain below.
#
# Output bytecode stays at --release 21 (v65): a Java-25 JVM runs it, JDK 25's
# javac still reads the v69 Minecraft classes off the classpath, and the mixin
# config can keep compatibilityLevel JAVA_21.
[CmdletBinding()]
param(
    [switch]$Pvp
)

$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$ewo  = Join-Path $env:APPDATA "EwoClient"
$jdk  = Join-Path $ewo "jdks\temurin-25\bin"

if (-not (Test-Path (Join-Path $jdk "javac.exe"))) {
    throw "JDK 25 not found at $jdk - see PHASE_E_PLAN.md E2 (install a JDK 25)."
}

if ($Pvp) {
    Write-Host "[ewo-hud] building PvP variant (assist module set included)"
} else {
    Write-Host "[ewo-hud] building legit variant (assist module set excluded)"
}

# Rebuild the Rust cdylib first. The pvp feature gates the assist half of
# REGISTRY; without it, Rust writes only 12 module records into the shared
# buffer and the Java assist code (excluded below) is never reached. With
# the feature, Rust writes 26 records and the assist classes are on the
# classpath to consume them.
$repoRoot = Split-Path -Parent $here
Push-Location $repoRoot
try {
    $prevPref = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    if ($Pvp) {
        Write-Host "cargo build -p ewo-jni --features pvp..."
        & cargo build -p ewo-jni --features pvp --quiet
    } else {
        Write-Host "cargo build -p ewo-jni..."
        & cargo build -p ewo-jni --quiet
    }
    $cargoExit = $LASTEXITCODE
    $ErrorActionPreference = $prevPref
    if ($cargoExit -ne 0) {
        throw "cargo build -p ewo-jni failed (is Minecraft still running? it holds ewo_jni.dll open)"
    }
} finally {
    Pop-Location
}

# Classpath: the Minecraft jar (Mojmap-named) plus every shared library.
# Compile against the 26.2 jar: it resolves every class the 26.1 mixins
# reference (Gui, RenderSystem, TracyFrameCapture, MultiBufferSource all
# still exist there) PLUS the 26.2-only class literals the 262 mixin
# variants need (Hud, SubmitNodeCollector). The reverse is not true — the
# 26.1 jar cannot compile the 262 variants.
$mc = Join-Path $ewo "shared\versions\26.2\26.2.jar"
if (-not (Test-Path $mc)) {
    throw "Minecraft 26.2 jar not found at $mc - launch (or start downloading) a 26.2 instance once, or copy the official client jar there."
}
$libs = (Get-ChildItem -Recurse (Join-Path $ewo "shared\libraries") -Filter *.jar).FullName
$cp = (@($mc) + $libs) -join ";"

$build = Join-Path $here "build"
$out   = Join-Path $build "classes"
if (Test-Path $build) { Remove-Item -Recurse -Force $build }
New-Item -ItemType Directory -Force $out | Out-Null

# Source filter. Legit build excludes dev/lewlone/ewohud/assist/ entirely —
# its class files never enter the jar, so a class-name scan can't see them.
$srcRoot = Join-Path $here "src\main\java"
$allSrcs = (Get-ChildItem -Recurse $srcRoot -Filter *.java).FullName
if ($Pvp) {
    $srcs = $allSrcs
} else {
    $assistDir = Join-Path $srcRoot "dev\lewlone\ewohud\assist"
    $srcs = @($allSrcs | Where-Object { -not $_.StartsWith($assistDir + [IO.Path]::DirectorySeparatorChar) })
}

# The classpath (every jar in shared/libraries) blew past the Windows
# command-line length limit once the 26.2 mod set doubled the library
# count — pass everything to javac via an @argfile instead. javac argfile
# quoting: backslashes must be escaped, values with spaces quoted.
$argFile = Join-Path $build "javac-args.txt"
$quote = { param($s) '"' + ($s -replace '\\', '\\') + '"' }
$argLines = @("--release", "21", "-proc:none", "-cp", (& $quote $cp), "-d", (& $quote $out))
$argLines += $srcs | ForEach-Object { & $quote $_ }
Set-Content -Path $argFile -Value $argLines -Encoding ascii
& (Join-Path $jdk "javac.exe") "@$argFile"
if ($LASTEXITCODE -ne 0) { throw "javac failed" }

# Resource layout. The legit jar ships exactly one mixin config
# (ewohud.mixins.json) and one fabric.mod.json. The pvp jar ships both
# mixin configs (legit + pvp) and a fabric.mod.json that references both.
# fabric-pvp.mod.json is the on-disk source for the pvp variant; we copy
# it into the jar as fabric.mod.json so Fabric finds it by its expected
# name. Either way the source file fabric-pvp.mod.json itself is never
# included in the jar.
$resRoot = Join-Path $here "src\main\resources"
$skipResources = @("fabric.mod.json", "fabric-pvp.mod.json")
if (-not $Pvp) {
    # Legit build: also skip the pvp mixin config so it isn't shipped.
    $skipResources += "ewohud-pvp.mixins.json"
}
Get-ChildItem $resRoot -File | Where-Object { $skipResources -notcontains $_.Name } | ForEach-Object {
    Copy-Item -Force $_.FullName $out
}
if ($Pvp) {
    Copy-Item -Force (Join-Path $resRoot "fabric-pvp.mod.json") (Join-Path $out "fabric.mod.json")
} else {
    Copy-Item -Force (Join-Path $resRoot "fabric.mod.json") (Join-Path $out "fabric.mod.json")
}

$jar = Join-Path $build "ewo-hud-0.1.0.jar"
& (Join-Path $jdk "jar.exe") --create --file $jar -C $out .
if ($LASTEXITCODE -ne 0) { throw "jar failed" }
Write-Host "built: $jar"

# Deploy into the launcher's shared library cache. The loader manifest points
# at this jar via a file:// URL, but the launcher copies file:// libraries into
# shared/libraries once and never refreshes them - so an in-place rebuild must
# redeploy here itself, or the next launch silently runs the stale jar.
$deploy = Join-Path $ewo "shared\libraries\dev\lewlone\ewo-hud\0.1.0\ewo-hud-0.1.0.jar"
New-Item -ItemType Directory -Force (Split-Path $deploy) | Out-Null
Copy-Item -Force $jar $deploy
Write-Host "deployed: $deploy"
