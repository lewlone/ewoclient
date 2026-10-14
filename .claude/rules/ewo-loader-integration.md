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
4. **A loader manifest resolve/fetch failure fails the launch** with a visible
   error on the launching screen (`launch/prepare.rs`, `LOADER_ERR_PREFIX`). An
   Ewo instance never degrades to vanilla; that silent fallback is what hid the
   hard-coded dev base on every other machine (2026-10 audit H9).
5. EwoLoader's `BundledMods.BUNDLED_MODS` verifies every expected mod id minus
   the user-disabled set, and fails loud otherwise.

## Where the manifests come from (`loaders/base.rs`)

Instances store a neutral marker, `ewo-loader:<line>` (e.g. `ewo-loader:26.2`);
older `instances.toml` files hold a full URL there, which is never fetched,
only its version read. The base is resolved at launch, in order:

1. `EWO_LOADER_BASE` (a `file://` or `https://` base URL);
2. `<exe_dir>/loader/0.1.0/` if it exists: the bundle `package.ps1` stages
   through `tools/bundle-loader.ps1`, which copies every `file://` jar a
   manifest names into `libs/`, rewrites its url to the relative
   `libs/<jar>` and recomputes sha1/size (`package.ps1 -LoaderRepo <path>`
   picks the EwoLoader checkout, default the sibling `..\EwoLoaderV1`;
   from a worktree pass it explicitly). The bundle's `ewo-hud` jar is the one
   `package.ps1` builds from the same checkout (`build.ps1 -NoDeploy`, passed
   as `bundle-loader.ps1 -Override`), never the dev drop the manifests name,
   so the jar and `ewo_jni.dll` always share a commit;
3. debug builds only: the developer's `EwoLoaderV1/manifest/0.1.0`;
4. otherwise a hard launch error.

A `file://` manifest may list scheme-less library urls (`libs/x.jar`), resolved
against the manifest's folder (no absolute paths, no `..`); an https manifest
may not, nor may it list `file://` libraries.

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

## Stale jar gotcha (debugging trap) — corrected 2026-10

The old note here said `file://` libraries are cached and never refreshed.
**That is wrong**: `downloads/job.rs::ensure_file` re-copies every `file://`
source on every launch, with no sha1/size check. The real trap is a manifest
whose `file://` URL points at a **stale source**: the manifests once named
`EwoClientV3/ingame-mod/build/ewo-hud-0.1.0.jar` in a main checkout that was
months behind, so each launch overwrote a freshly deployed HUD jar with an old
one. The manifests now point the HUD jar at
`%APPDATA%\EwoClient\dev\ewo-hud-0.1.0.jar`, which a deploying
`ingame-mod/build.ps1` writes from whichever worktree built it. If a change
"doesn't take effect", check which path the manifest names, then compare sha1s
(`certutil -hashfile`) of that file and the `shared/libraries` copy. The same
applies to the EwoLoader fat jar.
