# build.ps1 - compile the ewo-hud mod with plain javac (no Loom).
#
# This toolchain runs mods in Minecraft's mapped namespace with no remapping
# step, so a Fabric mod here is just javac + jar. From E2 the mod reads
# Minecraft classes directly (Minecraft.getFps(), RenderSystem, ...), so the
# Minecraft jar is on the build classpath. That jar is Java-25 bytecode, which
# only a JDK 25 javac can read - hence the temurin-25 toolchain below.
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
