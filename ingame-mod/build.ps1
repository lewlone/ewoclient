# build.ps1 - rebuild both halves of the in-game HUD: the Rust cdylib
# (target/debug/ewo_jni.dll) and the Fabric mod jar (ewo-hud-0.1.0.jar).
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
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$ewo  = Join-Path $env:APPDATA "EwoClient"
$jdk  = Join-Path $ewo "jdks\temurin-25\bin"

if (-not (Test-Path (Join-Path $jdk "javac.exe"))) {
    throw "JDK 25 not found at $jdk - see PHASE_E_PLAN.md E2 (install a JDK 25)."
}

# Rebuild the Rust cdylib first. Cargo skips work when nothing depends on
# the change, so this is essentially free when only Java sources moved. If
# Minecraft is running it'll fail to write the dll - close the game first.
$repoRoot = Split-Path -Parent $here
Write-Host "cargo build -p ewo-jni..."
Push-Location $repoRoot
try {
    # Drop ErrorActionPreference for the cargo call: PowerShell 5.1 wraps
    # every stderr line from a native exe as an ErrorRecord, and cargo prints
    # its progress ("Compiling...", "Finished") on stderr. With "Stop" set
    # the first progress line throws even though cargo exits 0. Check
    # $LASTEXITCODE explicitly instead.
    $prevPref = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    # --quiet drops cargo's "Compiling..." / "Finished" progress lines so
    # PowerShell 5.1 doesn't surface them as fake errors. Real compile
    # errors still print + the non-zero exit code is checked below.
    & cargo build -p ewo-jni --quiet
    $cargoExit = $LASTEXITCODE
    $ErrorActionPreference = $prevPref
    if ($cargoExit -ne 0) {
        throw "cargo build -p ewo-jni failed (is Minecraft still running? it holds ewo_jni.dll open)"
    }
} finally {
    Pop-Location
}

# Classpath: the Minecraft jar (Mojmap-named - Mojang ships 26.x deobfuscated)
# plus every shared library. The libraries cover Minecraft's transitive
# supertypes, the loader (ClientModInitializer) and sponge-mixin (the mixin
# annotations) - all of which live under shared/libraries.
$mc = Join-Path $ewo "shared\versions\26.1.1\26.1.1.jar"
if (-not (Test-Path $mc)) {
    throw "Minecraft 26.1.1 jar not found at $mc - launch 26.1.1 once to download it."
}
$libs = (Get-ChildItem -Recurse (Join-Path $ewo "shared\libraries") -Filter *.jar).FullName
$cp = (@($mc) + $libs) -join ";"

$build = Join-Path $here "build"
$out   = Join-Path $build "classes"
if (Test-Path $build) { Remove-Item -Recurse -Force $build }
New-Item -ItemType Directory -Force $out | Out-Null

$srcs = (Get-ChildItem -Recurse (Join-Path $here "src\main\java") -Filter *.java).FullName
& (Join-Path $jdk "javac.exe") --release 21 -proc:none -cp $cp -d $out $srcs
if ($LASTEXITCODE -ne 0) { throw "javac failed" }

Copy-Item -Recurse -Force (Join-Path $here "src\main\resources\*") $out
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
