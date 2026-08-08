# Runs the brigadier oracle for `rewo_world::suggestions` (M114).
#
# Prints the vectors pinned by `suggestions.rs`'s
# `the_port_agrees_with_the_brigadier_jar`. Re-run after a Minecraft version
# bump: brigadier's version is pinned by the client jar, and
# `Suggestions.create`'s sort is a user-visible order.
#
# Both paths are Phase B's own download layout, so no network is needed.

$ErrorActionPreference = 'Stop'

$jdk = Join-Path $env:APPDATA 'EwoClient\jdks\temurin-25\bin\java.exe'
$brigadier = Join-Path $env:APPDATA 'EwoClient\shared\libraries\com\mojang\brigadier\1.3.10\brigadier-1.3.10.jar'

foreach ($p in @($jdk, $brigadier)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

& $jdk -cp $brigadier (Join-Path $PSScriptRoot 'Oracle.java')
