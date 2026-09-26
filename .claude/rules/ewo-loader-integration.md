---
paths:
  - "crates/ewo-launcher/src/loaders/**"
  - "crates/ewo-launcher/src/downloads/**"
  - "crates/ewo-launcher/src/launch/**"
  - "crates/ewo-launcher/src/bundled.rs"
  - "crates/ewo-launcher/src/overlay_mods.rs"
---

# Loader integration — EwoLoader, the bundled mods, per-instance toggles

Open the sibling repo **EwoLoader** (`C:\Users\valtteri\Desktop\EwoLoaderV1`,
private `lewlone/ewo-loader`) in a second window when touching this. The two
repos are coupled at the loader-manifest contract. Full history:
`docs/history/ewoclient-v2-phases.md` (Phase D + Bundle phase).

## How a launch gets its mods

1. The loader manifest `EwoLoaderV1/manifest/0.1.0/<version>.json` lists every
   loader, ASM, mixin, infrastructure and mod jar in Mojang's `libraries[]`
   schema. **There are two manifests, `26.1.json` and `26.2.json`**, picked per
   Minecraft version line.
2. `loaders::merge::merge` prepends loader libraries ahead of vanilla's,
   overrides `mainClass` (`net.fabricmc.loader.impl.launch.knot.KnotClient`),
   concatenates args, and **keeps `id: vanilla.id`** so the client-jar path
   still resolves.
3. Phase B is loader-aware (`Stage::LoaderManifest`).
   `downloads::ensure_libraries` also runs in `try_real_launch` as a safety net
   for manifests edited between setup and launch. In steady state it's a no-op.
4. A loader-fetch failure falls back to a vanilla launch with a warning. It's
   non-fatal.
5. EwoLoader's `BundledMods.BUNDLED_MODS` verifies every expected mod id minus
   the user-disabled set, and fails loud otherwise.

## Per-instance mod toggles

`bundled::CATALOG` is the source of truth (name, category, version,
`fabric.mod.json` id, library name, `default_on`, `toggleable`). It has 17
toggleable mods, and infrastructure (Fabric API, fabric-language-kotlin, YACL,
placeholder-api, Cloth Config) is `toggleable: false`. At launch,
`disabled_mod_ids` strips those libraries from the classpath and appends
`-Dfabric.debug.disableModIds=<csv>`. Disabled jars stay on disk.
`sync_mods_with_catalog` migrates existing instances. In-game toggles write
back through `overlay_mods.rs` (per-instance `overlay-mods.toml` /
`overlay-mod-overrides.toml`). Dependency cascades aren't pre-detected: the
resolver fails loud.

## Adding a bundled mod: three places, and both manifests

1. `crates/ewo-launcher/src/bundled.rs::CATALOG`
2. `EwoLoaderV1/manifest/0.1.0/26.1.json` **and** `26.2.json` `libraries[]`.
   A mod in only one is missing on the other with no error until `BundledMods`
   verification fires.
3. `EwoLoaderV1/src/main/java/.../BundledMods.java::BUNDLED_MODS`, then rebuild
   the fat jar.

Mod jars come from Modrinth's Maven (`maven.modrinth:<slug>:<version_number>`).
ASM and sponge-mixin come from `maven.fabricmc.net`.

## Iteration loop

```
cd C:\Users\valtteri\Desktop\EwoLoaderV1
GITHUB_ACTIONS=true ./gradlew fatJar     # ~10 s, clean version string
# then click Launch — the manifest is re-read every launch
```

The fat jar is read from a `file://` URL during dev. Off-box snapshots live on
the private GitHub Releases (`gh release upload v0.19.2-bundle.<N> … --clobber`).
The launcher's `Authorization: Bearer $EWO_LOADER_TOKEN` wiring for GitHub asset
API URLs is in place and dormant.

## Stale `file://` jar gotcha (debugging trap)

The launcher caches `file://` libraries under `shared/libraries/<path>` and
**does not refresh them when the source changes**. A multi-hour debugging
session once ran a stale jar the whole time. If a change "doesn't take effect",
compare sha1s (`certutil -hashfile`) of the build output against the
`shared/libraries` copy **first**. `ingame-mod/build.ps1` copies its jar past
the cache. The same trap applies to the EwoLoader fat jar.
