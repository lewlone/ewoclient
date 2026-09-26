# EwoClient v2 + Phases D–H, the leak hunt, the legit/pvp split, the floating-card window, and the 2026-05-31 perf pass

<!-- Moved verbatim from CLAUDE.md lines 631–1500 on 2026-09-26, when CLAUDE.md
     was cut from 7,192 lines to an always-loaded index plus path-scoped
     rules in .claude/rules/. Everything below the rule is the original text,
     unedited. It is history: read it for the reasoning behind a rule. -->

> **Superseded statements in this file** (kept verbatim below; the current truth is in `CLAUDE.md` or `.claude/rules/`):
>
> - Phase D's `DEV_EWO_LOADER_URL` constant no longer exists; there are two loader manifests (`26.1.json` and `26.2.json`) picked per Minecraft version line.
>
> - "16 user-toggleable mods" and "28 manifest entries": the catalog has 17 toggleable mods (Iris Shaders and Force Crawl joined later).
>
> - Phase E says the mod builds against the 26.1 jar. It now builds against 26.2, with `EwoMixinPlugin` choosing 26.1 vs 26.2 mixin variants.
>
> - Phase G says "seven shipped" modules. After the legit/pvp split the legit build has 12 modules and the pvp build 26.
>
> - The floating-card section's "Transparent GL windows can be driver-finicky… alpha_size 8" describes the GL path. On Windows the launcher presents through D3D12 + DirectComposition (`with_no_redirection_bitmap`), and the Architecture section's point that WGL alpha always composites opaque holds.
>
> - Phase H names "chickenedin" domains and API bases. The network was renamed **Frogsy** in mid-2026; verify what's live before relying on any of them.

---

## v2 status — A–E all shipped

**v2 turns the Launch button into an actually-functional Minecraft launcher.** Phases A–C are shipped and verified end-to-end (Minecraft 26.1 launches, plays, and exits cleanly from a cold start). Phase D blocks on the user's custom Fabric fork existing as a separate project. Phase E is the original v2 ambition (in-game GUI) and lives further out.

### Phase A — Microsoft authentication ✅ shipped + approved

Code is complete in [`crates/ewo-launcher/src/auth/`](crates/ewo-launcher/src/auth/):

- `chain.rs` — the four-step token exchange (Microsoft OAuth + PKCE → Xbox Live → XSTS → Minecraft Services) + profile fetch
- `loopback.rs` — `tiny_http` listener bound to a random `127.0.0.1:PORT` that catches the OAuth redirect at the bare `http://localhost` registered URI
- `pkce.rs` — code verifier/challenge generation
- `persistence.rs` — refresh-token persistence at `<config>/EwoClient/auth.toml` (plaintext for now; DPAPI/keychain encryption is a TODO before wide distribution)
- `service.rs` — background-thread auth runner + mpsc UI events

UI lives on the **Account** tab in Settings (first tab; uses a custom layout, not the row-grid system). Sign-in / sign-out / try-again button changes label by `AccountView` state.

**The Entra app `f901fc74-7e36-439d-80a8-c2e548f47fdc` is on Mojang's allowlist** (Launcher Program approval came through ~1 week after submission). Sign-in completes the full chain end-to-end: Microsoft OAuth → Xbox Live → XSTS → `login_with_xbox` → profile fetch → token persisted to `auth.toml`. Verified live: signed in as **Vwyla**, joined Hypixel, sent chat, played a Bedwars game end-to-end.

`App::try_real_launch` reads `AuthState::SignedIn(account)` and slots the live `MinecraftAccount.minecraft_token` into the `LaunchProfile` — online multiplayer works without further code changes. Falls back to `LaunchProfile::offline(name)` (synthetic UUID + placeholder `"0"` token) when no signed-in account is available; offline mode is still useful for LAN or singleplayer-only sessions.

### Phase B — Version manifest + downloads ✅ shipped

- [`crates/ewo-launcher/src/versions/`](crates/ewo-launcher/src/versions/) — master `version_manifest_v2.json` fetch + 6h disk cache; per-version manifest fetch + sha1 verify + permanent cache (manifests are immutable per Mojang)
  - `manifest.rs::is_supported` is the **curated allowlist**: 1.21.x + 26.x release line + 1.8.9 + `26.2-snapshot-5`. Add other snapshots via `SNAPSHOT_ALLOWLIST`. The launcher intentionally doesn't surface every Mojang version.
- [`crates/ewo-launcher/src/downloads/`](crates/ewo-launcher/src/downloads/) — orchestrates the per-stage download (`PerVersion → Client → Libraries → AssetIndex → Assets`), sha1-verifies every file, drops into Mojang's official disk layout under `<config>/EwoClient/shared/{versions,libraries,assets}/`. One worker thread per active job; mpsc events to the UI.

UI surfaces:
- New-instance modal Version dropdown is live (filtered from `version_manifest_v2.json`)
- Instance row shows real percentage badge: `DOWNLOADING · 47%`
- On completion the instance flips from `Pending` → `Ready` and persists

### Phase C — JVM spawn + game launch ✅ shipped

[`crates/ewo-launcher/src/launch/`](crates/ewo-launcher/src/launch/):

- `plan.rs` — `LaunchPlan` builder. Substitutes every documented Mojang token (`${auth_player_name}`, `${classpath}`, `${natives_directory}`, etc.) into both modern `arguments` blocks and legacy `minecraftArguments` strings. `LaunchProfile::offline(name)` synthesizes an offline-mode profile.
- `natives.rs` — extracts native-classifier JAR contents into per-instance `natives/` dir (deletes + recreates each launch). META-INF skipping, basic path-traversal guard.
- `spawn.rs` — `std::process::Command` child + two reader threads piping stdout/stderr line-by-line back to the UI through mpsc.
- `jre.rs` — JRE detector. Scans Oracle javapath, Adoptium, Microsoft, Zulu, Liberica, JAVA_HOME, PATH, plus the bundled-runtime dir (see Runtime below). Probes each via `java -version`. Cached + manually invalidatable. `pick_for_major(required)` returns exact match else lowest installed major ≥ requirement.

[`crates/ewo-launcher/src/runtime/`](crates/ewo-launcher/src/runtime/):

- **Bundled-JRE auto-fetch.** When `pick_for_major` returns `None`, `try_real_launch` triggers an Adoptium download for the missing major. Archive lands at `<config>/EwoClient/runtime/<major>/`; extracted into `<runtime_dir>/jre/`. JRE detector picks it up automatically (cache invalidated by `jre::invalidate_cache` after extraction completes). Currently Windows-only — `.zip` extraction; macOS/Linux `.tar.gz` is the next polish item.

**Real launches** are spawned with the right JRE, full classpath, extracted natives, working dir = per-instance dir. JVM stdout/stderr stream into the launching screen's log panel (replaces the synthetic `LOG_SCRIPT`). On clean exit the screen handoffs back to Instances; on non-zero exit the pbar flips to `ErrorRose`, the screen sticks, and a Retry / Back button pair appears in the footer.

**Per-launch logs** are dumped to `<config>/EwoClient/instances/<name>/logs/launch_<unix_ts>.log` on JVM exit, tagged `[OUT]` / `[ERR]` per source.

**Verified end-to-end on a fresh box:** create instance → Phase B downloads (~3 min for 26.1 full asset set) → click Launch → JRE auto-fetch via Adoptium (~2s for Java 25) → JVM spawn → Minecraft renders → clean exit → handoff. Logs in `tasks/b8ycimnb5.output` (smoke-test session 2026-05-03).

### Phase D — Custom Fabric-fork loader (EwoLoader) ✅ shipped + verified live

**EwoLoader** is a friendly fork of fabric-loader living in a sibling repo at `C:\Users\valtteri\Desktop\EwoLoaderV1` (`lewlone/ewo-loader`, private). Friendly-fork meaning package names stay `net.fabricmc.loader.*` for binary compat with the existing Fabric mod ecosystem — only artifact identity (`dev.lewlone:ewo-loader`) + Maven group + the `STRIP_PLAN.md`-driven leanness differ. Built from upstream `fabric-loader` 0.19.2; eight strip passes shipped (see `STRIP_PLAN.md` in that repo) removing ~17k LoC: ProGuard pipeline, LaunchWrapper/Applet/FML125, legacyJava source set, dev-mode helpers, `mods/` folder discovery, Swing crash GUI, all test infrastructure, V0 metadata schema. Build time: 6s clean on JDK 21.

**Launcher integration is fully wired:**

- [`crates/ewo-launcher/src/loaders/`](crates/ewo-launcher/src/loaders/) — `manifest.rs` (`LoaderManifest` shape), `merge.rs` (`merge(vanilla_pv, loader_manifest) -> PerVersion`), `fetch.rs` (`get_or_fetch` with HTTP + `file://` support).
- `merge::merge` prepends loader libraries ahead of vanilla's on the classpath, overrides `mainClass` when present, concatenates JVM/game args. Critical detail: keeps `id: vanilla.id.clone()` so client.jar path resolution still works (`versions/<vanilla_id>/<vanilla_id>.jar`).
- `try_real_launch` ([main.rs:254](crates/ewo-launcher/src/main.rs#L254)) reads `Instance::loader`, dispatches: `Vanilla` → vanilla PerVersion as-is; `Ewo { manifest_url }` → fetch loader manifest, merge, hand merged PerVersion downstream. Loader-fetch failures fall back to vanilla launch with a warn (non-fatal — won't block a flaky local manifest).
- Loader manifest URL is currently the in-development `file:///C:/Users/valtteri/Desktop/EwoLoaderV1/manifest/0.1.0/26.1.json` (per `DEV_EWO_LOADER_URL` const). Becomes a config knob (or public HTTPS URL) once the loader publishes a meta endpoint.

**The new-instance modal's Loader dropdown** is now `["Vanilla", "Ewo (development)"]` only — the prototype's `["Fabric", "Forge", "NeoForge", "Quilt"]` entries got removed during Phase D wiring since they were never going to be wired (we ship one loader, ours).

### Bundle phase — 16 user-toggleable mods + 5 infrastructure libs ✅ shipped

The bundle ships the curated set originally planned (Sodium, Lithium, Iris) plus 13 more — Simple Voice Chat, Distant Horizons (default-off, heavy), plus the optimization + QoL set the user picked (ImmediatelyFast, FerriteCore, EntityCulling, More Culling, Mod Menu, Reese's Sodium Options, BetterF3, AppleSkin, Zoomify, LambDynamicLights, Continuity), each toggleable per-instance from the Instances UI's mod list. Infrastructure (Fabric API + 4 transitive lib mods: fabric-language-kotlin for Zoomify, YACL for Zoomify, placeholder-api for Mod Menu, Cloth Config for BetterF3 + More Culling) is bundled but hidden from the toggle UI.

**How bundling works:**

1. The launcher reads `manifest/0.1.0/26.1.json` (in the EwoLoader repo, served via `file://` for active dev — see "GitHub Release snapshots" below) which lists every artifact the launch needs in its `libraries[]` array — Mojang's standard library schema (`name`, `downloads.artifact.{path, sha1, size, url}`). Currently 28 entries: 1 EwoLoader fat jar + 5 ASM jars + sponge-mixin + 5 infrastructure mods (Fabric API + fabric-language-kotlin + YACL + placeholder-api + Cloth Config) + 16 user-toggleable mods.
2. Phase D's `merge` prepends every loader library onto vanilla's, so the final `PerVersion` going to Phase B/C contains them all.
3. **Phase B is loader-aware as of `64c6fe1`** ([downloads/job.rs](crates/ewo-launcher/src/downloads/job.rs)). `DownloadService::start(entry, Some(LoaderSpec { id, url }))` fetches the loader manifest in a new `Stage::LoaderManifest` between PerVersion and Client, merges in-memory, and counts + downloads the merged library set through the same progress bar. **`downloads::ensure_libraries`** still runs from `try_real_launch` after the merge as a safety net for the iteration loop where the user edits the loader manifest between instance-setup and launch — Phase B's snapshot then misses the new entries and ensure_libraries picks them up. In the steady state it's a fast no-op.
4. JVM spawns with `mainClass = net.fabricmc.loader.impl.launch.knot.KnotClient` (from the manifest's mainClass override) and `-cp` containing all the loader+mod jars ahead of vanilla's libs + the client.jar.
5. EwoLoader's `ClasspathModCandidateFinder` scans `fabric.mod.json` resources across the classpath → finds every bundled mod that wasn't stripped → registers them with the mod resolver. Then `BundledMods.BUNDLED_MODS` verification fires: every expected modId not already in the user-disabled set must appear in the discovered set, else throw `ModResolutionException`. The expected list lives at `BundledMods.java`; the disabled-mods subtraction reads `fabric.debug.disableModIds` (upstream system property the launcher repurposes for per-instance toggles — see below).

**Per-instance mod toggles** are wired end-to-end as of `bcc3ea6` (launcher) + `99a38df` (loader):

- Launcher's `bundled::CATALOG` ([crates/ewo-launcher/src/bundled.rs](crates/ewo-launcher/src/bundled.rs)) is the source of truth: each row has the display name + category + version + `fabric.mod.json` id + loader-manifest library name + `default_on` + `toggleable`. Infrastructure rows (FAPI, language-kotlin, YACL, placeholder-api) are `toggleable: false` so they don't appear in the UI.
- New Ewo instances get their `Instance.mods` seeded from `bundled::seed_instance_mods()`. Existing instances are migrated on launcher startup via `bundled::sync_mods_with_catalog` (called from `persistence::load_instances`), which adds missing catalog entries with their default-on and preserves user-flipped state.
- At launch time, `try_real_launch` runs `bundled::disabled_mod_ids(&inst.mods)` → strips the matching libraries from the merged `PerVersion.libraries` (so the classpath excludes them) → appends `-Dfabric.debug.disableModIds=<csv>` to `plan.jvm_args`.
- The loader's `ModDiscoverer.findDisabledModIds` is upstream's already-wired filter at discovery time. `FabricLoaderImpl.setup()` was extended (`parseDisabledModIds` + verification subtraction) so `BundledMods.BUNDLED_MODS` checks don't fire on intentionally-absent mods.
- Disabled mods stay on disk after `ensure_libraries` — re-enabling a mod in the UI doesn't trigger a re-download.
- If the user disables a mod whose required deps are still enabled, the resolver fails loud at launch with the usual upstream "X requires Y" error. We don't pre-detect cascades.

**Bundled-mod sourcing:**
- Mod jars come from Modrinth's Maven (`https://api.modrinth.com/maven/...` → 307 redirects to `cdn.modrinth.com`). Coordinate form: `maven.modrinth:<slug>:<version_number>`. Modrinth uses the human-readable version string as the path segment.
- ASM + sponge-mixin come from `maven.fabricmc.net`. EwoLoader's Gradle `installer` configuration declares them but doesn't bundle them — they're expected to come from the installer's manifest, which in our case IS our loader manifest.
- The EwoLoader fat jar itself comes from `file:///C:/Users/valtteri/Desktop/EwoLoaderV1/build/libs/ewo-loader-0.19.2-fat.jar` for now. **Hosting it publicly is a follow-up** (see Known gaps).

**Live `BundledMods` declaration:** [`src/main/java/net/fabricmc/loader/impl/discovery/BundledMods.java`](https://github.com/lewlone/ewo-loader/blob/main/src/main/java/net/fabricmc/loader/impl/discovery/BundledMods.java) in the EwoLoader repo. Adding a new bundled mod = three places: (a) the loader manifest's `libraries[]`, (b) `BundledMods.BUNDLED_MODS`, (c) one `./gradlew fatJar` to bake the new BUNDLED_MODS into the fat jar. Manifest sha1 doesn't need updating for the EwoLoader fat jar (`file://` skips sha1 verification).

**Iteration loop (concrete commands):**
```
1. cd C:\Users\valtteri\Desktop\EwoLoaderV1
2. # edit src/main/java/.../BundledMods.java + manifest/0.1.0/26.1.json
3. GITHUB_ACTIONS=true ./gradlew fatJar       # 10s; clean version, no +local suffix
4. # click Launch in EwoClient
```
The launcher re-reads the loader manifest on every launch (no TTL cache in `loaders::fetch`). On a fresh instance Phase B already downloaded the merged library set through its progress bar; on the iteration loop where the user edits the manifest between setup and launch, the safety-net `ensure_libraries` hot-downloads new entries to `<config>/EwoClient/shared/libraries/...` before JVM spawn.

**Adding a new bundled mod:** three-place change (the BundledMods verification fails loud if any two drift):
1. `crates/ewo-launcher/src/bundled.rs::CATALOG` — adds the UI row + library-name → mod-id mapping the launcher uses for classpath stripping.
2. `EwoLoaderV1/manifest/0.1.0/<version>.json::libraries[]` — adds the download
   artifact entry. **There are TWO manifests now**, `26.1.json` and `26.2.json`,
   and the launcher picks one per Minecraft version line, so a mod added to only
   one of them is missing on the other with no error until `BundledMods`
   verification fires at launch.
3. `EwoLoaderV1/src/main/java/.../BundledMods.java::BUNDLED_MODS` — adds the post-discovery verification entry.

**GitHub Release snapshots (off-box backup of the fat jar):** versioned snapshots of the fat jar live on `lewlone/ewo-loader` GitHub Releases (private). `v0.19.2-bundle.1` was the first one. The loader manifest's fat-jar URL stays `file://` for active dev (per the iteration loop above — fast rebuild + no upload step), so the launcher reads the local jar on every launch. The Release exists as a versioned backup + a stepping-stone if the loader ever needs to ship to a second machine. To cut a new snapshot:
```
./gradlew fatJar
gh release upload v0.19.2-bundle.<N> build/libs/ewo-loader-0.19.2-fat.jar --repo lewlone/ewo-loader --clobber
```
The launcher already has the auth wiring for private-repo asset URLs: when an HTTP URL matches `api.github.com/repos/<owner>/<repo>/releases/assets/<id>`, [`downloads::job::github_auth_headers_for`](crates/ewo-launcher/src/downloads/job.rs) attaches `Authorization: Bearer <EWO_LOADER_TOKEN>` + `Accept: application/octet-stream`. The token comes from the env var (fine-grained PAT, Contents: read on `lewlone/ewo-loader`). When the env var is unset, no headers are sent and the request goes through unauthenticated — which is correct behavior for the file:// path (the helper is never called for file URLs at all). To flip to the hosted jar, just point the manifest's `dev.lewlone:ewo-loader:0.19.2` library URL at the GitHub asset API URL and re-launch.

### Phase E — In-game HUD ✅ E0–E7 shipped — Phase E complete (full HUD + editor + dashboard + cached glass refract)

**The spike is done (2026-05-20): `ewo-render`'s Skia pipeline paints over a running Minecraft, verified live on the 26.1 title screen** — a rotating glass panel composites over the game, stable (no flicker, no driver crash). Phase E's core question — can the launcher's Skia stack render over Minecraft at all — is answered: yes.

Three pieces, all in the EwoClientV3 repo:

- [`crates/ewo-jni/`](crates/ewo-jni/) — a `cdylib` loaded into the Minecraft JVM. Creates a **dedicated GL context** on Minecraft's window, builds a Skia `DirectContext` against it, and paints `ewo-render`'s glass-panel widget onto the window framebuffer.
- [`ingame-mod/`](ingame-mod/) — Fabric mod `ewo-hud`. Loads the cdylib; `EwoHudMixin` injects `RenderSystem.flipFrame` HEAD and calls into Rust once per frame. Plain `javac` + `jar` build ([`ingame-mod/build.ps1`](ingame-mod/build.ps1)) — no Loom, no Gradle.
- `EwoLoaderV1/manifest/0.1.0/26.1.json` — a `file://` `libraries[]` entry (`dev.lewlone:ewo-hud:0.1.0`) so EwoLoader puts the mod on the classpath.

Decisions + findings from the spike (load-bearing for the real HUD):

1. **Dedicated GL context, never shared.** Skia and Minecraft both drive the OpenGL state machine and corrupt each other if they share a context — observed live as flickering UI, then a fatal `EXCEPTION_ACCESS_VIOLATION` in `nvoglv64.dll`. The fix: a second `wglCreateContext` on MC's window; each frame `wglMakeCurrent`s to it, draws, and hands the thread's context back to Minecraft untouched. Two GL state machines, one shared window framebuffer. This is the HUD's permanent isolation model — do not regress to a shared context.
2. **Frame hook is `RenderSystem.flipFrame` HEAD** — a universal end-of-frame point (title screen, menus, in-game). Fabric API's `ScreenEvents.afterExtract` fires mid-pipeline in MC 26.x's deferred GUI rendering, too early to composite over.
3. **The toolchain runs mods in Minecraft's Mojmap namespace — no intermediary, no Loom remapping.** Fabric mods here build with plain `javac` against the on-disk jars. `shared/versions/26.1/26.1.jar` ships **Mojmap-named** (Mojang distributes 26.x deobfuscated — confirmed E2: `net.minecraft.client.Minecraft` + ~10.7k readable classes), and EwoLoader logs `Mappings not present!` — it does no remapping, so the on-disk jar *is* the runtime namespace. The jar is Java-25 bytecode, so building against it needs a **JDK 25** (installed E2 at `%APPDATA%/EwoClient/jdks/temurin-25/`). The spike's string-target mixin + `TracyFrameCapture` compile-only stub were a JDK-21 workaround; E2 removed them for plain class-literal mixins.
4. **Draw-direct in the spike; two-clock as of E1.** The spike painted `fbo 0` directly every frame. **E1 (done 2026-05-20)** replaced that with the decoupled model: `paint` renders the HUD to an offscreen GPU surface (rate-gated by `HudPaintRate`), `composite` blits it onto `fbo 0` every frame so it never tears. Verified live with the rate forced to 30 — chunky panel, game still 400–500 fps. The known tradeoff — offscreen painting drops the glass panels' live-game backdrop blur — was resolved in E7 with a cached frosted backdrop for the overlay views (see below). Details in `PHASE_E_PLAN.md`.

E2 shipped the first real widget (a live FPS readout). E3 finished the read-only widget set: **FPS, Coords, Ping, Keystrokes, Armor, PotionHUD, TargetHUD** — all Velvet re-skins of `hud.jsx` elements, in `crates/ewo-jni/src/hud.rs`. The data pipeline is a shared direct `ByteBuffer`: the mod allocates it once (`EwoHudData`), fills it each frame, and Rust reads it through its address (`GetDirectBufferAddress` via the `jni-sys` crate) — `nativeRender()` takes no args, zero per-frame JNI marshaling. A `SCHEMA_VERSION` guards the byte-for-byte layout mirror between `EwoHudData.java` and `hud.rs`. E4 added overlay input: Right Shift opens a custom `Screen` (`EwoOverlayScreen`) that frees the cursor and forwards mouse input to Rust. E5 made the HUD editable — drag widgets, snap-to-align, toggle them, anchor them via a side panel; the layout persists to `<config>/EwoClient/hud.toml`. E6 turned the overlay into a 3-tab dashboard — **HUD · MODS · SETTINGS** — with in-game bundled-mod toggles (write-back via `crates/ewo-launcher/src/overlay_mods.rs` — a per-instance `overlay-mods.toml`/`overlay-mod-overrides.toml` pair) and the `HudPaintRate` cap as a real setting.

E7 closed Phase E with the **glass-refract decision**: the MODS/SETTINGS overlay views frost the live game behind them; the HUD editor view leaves it sharp. The frost is a *genuine* blur but **cached on a third clock** — `refresh_frost` recomputes it ~10×/sec into a quarter-resolution surface via a clean two-step 2× downscale + a small gaussian, and `composite` upscales that cache every frame with a cubic resampler (cheap) plus a faint Velvet wine wash. A first cut that blurred `fbo 0` directly every composite looked chunky and was wasteful; the cached downscale→blur→cubic-upscale chain is smooth and nearly free per frame. Verified live at ≈500 fps with the overlay open.

**Phase E is complete (E0–E7, all 2026-05-20).** [`PHASE_E_PLAN.md`](PHASE_E_PLAN.md) holds the per-step detail and the locked architecture decisions — it is now a record, not a forward plan. The in-game HUD ships: full read-only widget set, draggable editor, 3-tab dashboard, in-game mod toggles, cached glass refract.

### Known gaps + small follow-ups

- **EwoLoader fat jar — local dev uses `file://`, snapshots live on GitHub Releases.** Active iteration reads the local build output via `file:///C:/.../ewo-loader-0.19.2-fat.jar` so the rebuild loop stays one step (per the "GitHub Release snapshots" section above). `v0.19.2-bundle.1` is the first off-box backup; cut new snapshots with `gh release upload --clobber` when shipping is meaningful. The launcher's `Authorization: Bearer $EWO_LOADER_TOKEN` wiring is in place + dormant — flip the manifest URL to the GitHub asset API URL and set the PAT to use the hosted jar.
- **Indium — upstream-blocked.** Latest release `1.0.36+mc1.20.1` published 2025-02-25, no MC 26.x build. Continuity 3.x ships without it for 26.1 (the original "Continuity needs Indium" assumption was wrong); Indium would unblock other render-API-extension mods if/when it returns to active maintenance.
- **macOS/Linux JRE bundling** — `.tar.gz` extraction shipped; not exercised on a real Linux/macOS box yet.
- **Hyprland verification** — still not run on actual Linux.
- **Pixel-parity pass** — never formally walked through every screen vs `style/*.png` with side-by-side screenshots.
- **Refresh-token at-rest encryption** — `auth.toml` is plaintext. Fine for single-developer dev box; would want DPAPI / keychain / libsecret before binary distribution.
- **Settings → Java runtime dropdown** — decorative; `pick_for_major` does the right thing automatically. Wiring is low-priority.
- **Duplicate `file_url_to_path`** in `loaders/fetch.rs` + `downloads/job.rs`. Both work; factoring into a shared module is a clean-up follow-up.

### Useful runtime conventions

- Disk: `<config>/EwoClient/` is `%APPDATA%/EwoClient` on Windows, `$XDG_CONFIG_HOME/EwoClient` (or `~/.config/EwoClient`) on Linux. Layout under it:
  ```
  shared/{versions,libraries,assets}/  ← Mojang-compatible; vanilla launchers can read these
  instances/<name>/                    ← per-instance: worlds, screenshots, mods, natives, logs
  runtime/<major>/jre/                 ← bundled JREs auto-fetched from Adoptium
  auth.toml                            ← AccountStore { active, accounts[] } (plaintext) — Phase F0
  profiles.toml                        ← client-profile registry { active, profiles[] } — Phase F2
  profiles/<name>/client.toml          ← profile-scoped config (tweak tokens, theme, audio, keybinds)
  profiles/<name>/hud.toml             ← in-game HUD layout, per profile — Phase F5a
  profiles/<name>/modules.toml         ← per-module config (enabled + settings) — Phase G
  versions_cache.json                  ← master manifest cache (6h TTL)
  settings.toml                        ← GLOBAL-only config (paths, window mode, log level) — Phase F2
  instances.toml                       ← persisted instance list
  ```
- Threading: every long-running task spawns a `std::thread` and reports back via `mpsc`. Polled by `App` once per frame in `RedrawRequested`. No tokio/smol — see CLAUDE.md non-negotiables.
- All HTTP via `ureq` (sync). User-Agent: `EwoClient/0.1 (+https://github.com/lewlone/ewoclient)`.

### What NOT to do in v2

- Don't prefetch Microsoft credentials at app startup if a user might never sign in. Lazy-fetch on launch.
- Don't add a real-time launcher protocol (Discord rich presence, telemetry, news). The "OFFLINE FIRST. NOTHING PHONES HOME." invariant from v1 stays — auth + version manifest + asset CDN are the only network calls.
- Don't write your own auth lib; the Microsoft-auth chain is well-documented but the failure modes are subtle. Reference https://wiki.vg/Microsoft_Authentication_Scheme religiously.
- Don't expand the curated version allowlist (`is_supported`) without a reason. The launcher targets specific versions deliberately.
- Don't break the disk layout — vanilla-launcher interop is a real feature.

---

## Phase F — Profiles, Dashboard & Keybinds ✅ shipped (F0–F6, 2026-05-21 → 2026-05-22)

The first feature phase past v1 + v2. `PHASE_F_PLAN.md` (repo root) holds the
per-step detail and is now a record, not a forward plan. Three pillars:
multi-account, client profiles, and an in-game dashboard.

### Accounts (F0–F1)

`auth.toml` went from a single account to an `AccountStore { active, accounts[] }`
(transparent migration on first F-build launch). `AuthService` owns the store
plus an `AuthOp` (Idle / Working / Failed) — the single source of truth. The
**Settings → Account tab** is a list: add / remove / set-active, with monogram
avatars (Velvet-tinted disc + initial; real skin-head avatars deferred — the
in-game 3D viewer covers the skin-display need).

### Client profiles (F2–F3, rename in F6)

A *client profile* is a named, hot-swappable bundle of cosmetic + perf config —
**global, not per-instance**. Orthogonal to accounts (any account × any profile).
Disk: `profiles.toml` (registry), `profiles/<name>/client.toml` (profile-scoped:
the 5 tweak tokens + theme / vsync / max-fps / audio / **keybinds**), and
`settings.toml` is now **global-only** (paths, window mode, auto-backup, log
level, telemetry). The **Settings → Profiles tab** manages them — switch / new /
duplicate / delete / **rename** (an inline text field with a blinking caret;
`profile::rename` moves the `profiles/<name>/` directory + updates `profiles.toml`,
`profile::is_valid_name` rejects path-unsafe names). Switching re-applies the
config live via `App::apply_loaded_config`.

The launcher-side `profile` module (`crates/ewo-launcher/src/profile.rs`) owns
all of this — `load` / `save` (split/merge the unified `SettingsConfig` ↔ the
on-disk pair), `list` / `active_name` / `switch` / `create` / `duplicate` /
`delete` / `rename`, and `load_keybinds` / `save_keybinds`.

### In-game dashboard (F4–F5)

**"The dashboard" is an in-game overlay tab, NOT a launcher home screen** — the
launcher main menu is unchanged (this was a scope correction mid-phase). The
overlay tab strip is **HOME · HUD · MODS · SETTINGS**. HOME (`draw_home` in
`crates/ewo-jni/src/hud.rs`) is the overview: session stat cards (FPS / ping /
playtime / coords / server), account + active-profile line, per-HUD-widget
quick-toggles, and a drag-rotatable **3D skin viewer**. The data pipeline is
`SCHEMA_VERSION` 3 (`EwoHudData.java` ↔ `hud.rs`). `hud.toml` moved under
`profiles/<name>/` (F5a) so HUD layout is per-profile; the overlay SETTINGS tab
has an in-game profile switcher (F5b) that hot-swaps the layout live.

### 3D skin viewer

`crates/ewo-jni/src/skin.rs` — a Skia software renderer for the Minecraft
player model: 12 textured-quad cuboids + cape, box-UV unwrap, back-face cull,
painter's-sort, per-face shade, drag-to-rotate. **Slim + wide models.** The mod's
`EwoSkinExport` downloads the skin/cape PNGs from the player's GameProfile
`textures` property and writes an `ewo-skin-slim` marker.
- **Gotcha:** the mod must read the `textures` property **reflectively** — the
  build-classpath authlib skews from the runtime one (record `properties()` /
  `value()` vs. class `getProperties()` / `getValue()`); a direct call compiles
  but throws `NoSuchMethodError` at runtime.
- **Gotcha:** the viewer reloads the skin on `ewo-skin.png` **mtime change**,
  not just once — else a stale png from an earlier launch freezes the slim flag.

### Keybinds (F5c)

A **module-extensible keybind registry** — launcher `keybind` module
(`crates/ewo-launcher/src/keybind.rs`): `KeyChord` (a GLFW key code + modifier
bitmask — GLFW is Minecraft's own key namespace, so the in-game side compares
the integer with no translation), `KeybindAction`, the static `REGISTRY` (F
ships one: `overlay.open` → Right Shift), a winit→GLFW key table, label
formatting. Keybinds are **per client profile** (`client.toml` `[keybinds]`
table). The **Settings → Keybinds tab** is a remap row per action — click the
chord button to arm a rebind, the next key press is captured. The active
profile's keybinds resolve to `<instance>/ewo-keybinds.txt` before each launch;
the mod's `EwoKeybinds` reads it so the overlay-open key (`KeyboardHandlerMixin`
+ `EwoOverlayScreen`) is rebindable. **The registry is the seam future
EwoClient *modules* plug their bindable actions into** — modules (EwoClient's
own legit client features) are out of scope for F; F built only the seam.

### Phase F — verification still pending (the user does these)

- Launcher-side: the Account / Profiles / Keybinds Settings tabs + profile
  rename — built + committed, not yet eyeballed.
- In-game: the F4 HOME tab, F5 profile hot-swap, the 3D skin viewer — build
  clean; in-game testing is crash-prone until the user disables NVIDIA
  Threaded Optimization in NVCP (it fights the HUD's 2nd GL context — a
  `nvoglv64.dll` access violation under heavy GPU load).

---

## Phase G — EwoClient Modules ✅ shipped (G0–G8, 2026-05-22)

The second feature phase past v1 + v2. `PHASE_G_PLAN.md` (repo root) holds the
per-step detail and is now a record, not a forward plan. Phase G builds the
**modules** the Phase F keybind registry was the seam for: in-game,
legit-client quality-of-life features with an on/off state, optional settings,
and an optional keybind. Seven shipped — see the table below.

**Constraint (Phase E #4) holds: legit-client features only.** No hacked-client
modules. The MODULES UIs are clean Velvet feature lists, not ClickGUI grids.

### The architecture

The pre-G in-game data flow was one-way — Java→Rust (`EwoHudData`: game state
in, Rust paints). Modules must *change the running game*, which only the Java
mod can do, so Phase G adds the missing direction — a **live Rust→Java
channel**:

- **`ewo_core::modules`** — the module catalog (`REGISTRY`): pure `&'static`
  data, the single source of truth shared by the launcher and `ewo-jni`. The
  launcher's `keybind::REGISTRY` is now *generated* from it — each module
  contributes a `KeybindAction`.
- **`modules.toml`** — per client profile (`profiles/<name>/modules.toml`,
  sibling of `hud.toml`): each module's `enabled` + settings. Both the launcher
  and `ewo-jni` read/write it; modules apply *live*, so there is no
  overrides-dance (unlike bundled mods).
- **`EwoModuleData`** — a second shared `ByteBuffer`, the mirror image of
  `EwoHudData`: Rust writes every module's state each frame, the mod reads it
  to drive the effect mixins. `crates/ewo-jni/src/modules.rs` owns the Rust
  side; `EwoModuleData.java` mirrors the layout (its own `SCHEMA_VERSION`).
- Two new JNI methods: `nativeInitModules` (register the buffer) and
  `nativeModuleToggle` (a keybind press round-trips a toggle through Rust,
  which owns module state).

### The module set

All seven are **non-destructive** — each overrides a *computed* value via a
mixin; nothing writes Minecraft's `options.txt`, so toggling a module off
restores vanilla behavior exactly.

| Module | Effect | Hook (26.1.1 Mojmap) |
|---|---|---|
| Full Bright | World renders fully lit | `@Inject` cranks `LightmapRenderState.brightness` after `LightmapRenderStateExtractor.extract` |
| FOV Control | FOV past the 110° cap | `@Redirect` the `options.fov()` read in `Camera.calculateFov` |
| Toggle Sprint | Sprint held for you | force the `keySprint` `KeyMapping` from the frame hook |
| Toggle Sneak | Sneak held for you | force the `keyShift` `KeyMapping` from the frame hook |
| No Damage Tilt | No hit camera-lurch | `@Inject` cancel on `GameRenderer.bobHurt` |
| No View Bob | No walk view-bob | `@Inject` cancel on `GameRenderer.bobView` |
| FreeLook | Spectator-style flying freecam — hold to detach; WASD flies, mouse looks, body frozen, player model visible, snaps back on release | `@Redirect` `LocalPlayer.turn` in `MouseHandler.turnPlayer` (mouse → freecam) + `@ModifyVariable` on `Camera.setRotation` (rotation) + `@Inject` at `Camera.alignWithEntity` RETURN with `@Shadow`'d `setPosition` (position) + `@Redirect` `CameraType.isFirstPerson()` in `alignWithEntity` (force detached so the body renders). Body is frozen by forcing the six movement `KeyMapping`s `setDown(false)` while active; flight uses raw `glfwGetKey` on default WASD/Space/Shift/Ctrl. No-clip in multiplayer = line-of-sight advantage; a leash or block collision would be the legit-client compromise. |

**26.x rendering moved** — load-bearing for future mixin work: `LightTexture` →
`net.minecraft.client.renderer.Lightmap`; the lightmap is GPU-driven (a
`LightmapRenderState` UBO, `Lightmap.getBrightness` has no callers); FOV is no
longer `GameRenderer.getFov` — it's `Camera.calculateFov`, and the projection
matrix is built in `Camera.extractRenderState`.

### The UIs

- **In-game** — a 5th overlay tab: `HOME · HUD · MODULES · MODS · SETTINGS`.
  `draw_modules` in `crates/ewo-jni/src/hud.rs` is a Velvet feature list — a
  toggle per module + a slider for FOV. A toggle writes `modules.toml` and
  flows live through the buffer.
- **Launcher** — an 8th `SettingsTab::Modules`, modelled on the Keybinds tab:
  a toggle per module + FOV's slider, editing the active profile's
  `modules.toml` (`profile::load_modules` / `save_modules`).
- **Keybinds** — each module contributes a `KeybindAction` (default unbound),
  so module hotkeys appear in the launcher Keybinds tab and resolve through
  `ewo-keybinds.txt` for free. In-game, `KeyboardHandlerMixin` toggles a module
  on its key; FreeLook's key is a hold, polled by `EwoFreeLook`.

### Phase G — verified live (2026-05-23)

All seven modules built, deployed and smoke-tested in-game. FreeLook was then
reworked into the flying spectator-style freecam described in the table row
above — the original "look around with body frozen" implementation worked but
was nearly invisible in first person, so the user asked for a real spectator
view.

Two non-blocking notes carry forward:
- Full Bright uses `brightness = 15.0` — tune in
  `LightmapRenderStateExtractorMixin` if it reads too dim or washed.
- In-game testing stays crash-prone until NVIDIA Threaded Optimization is off
  in NVCP (the HUD's 2nd GL context — see Phase E).

### Stale `file://` jar gotcha (debugging trap)

The launcher caches `file://` libraries under `shared/libraries/<path>` and
does **not** refresh them when the source jar changes. A multi-hour FreeLook
debugging session in 2026-05-22 was caused by exactly this: every "fresh
build + launch" was secretly running a several-hours-old jar; diagnostics
looked impossible until `certutil -hashfile` on the build output vs the
`shared/libraries` copy showed different sha1s.

**Fix in `ingame-mod/build.ps1`**: after `jar.exe --create`, the script copies
the freshly-built jar into
`%APPDATA%\EwoClient\shared\libraries\dev\lewlone\ewo-hud\0.1.0\ewo-hud-0.1.0.jar`,
bypassing the cache. **Always run `build.ps1` for mod changes; never `javac`
by hand** or you skip the deploy. If a change ever "doesn't take effect",
compare sha1s before anything else. The same caching almost certainly applies
to the EwoLoader fat jar (also a `file://` library entry); it has not bitten
in practice because the loader is rebuilt infrequently.

---

## Unfocused-swap memory leak (fixed 2026-05-26)

The launcher used to leak ≈6 KB / frame (≈3 MB/s at ~500 fps) of
C++-side memory whenever the window wasn't the foreground — by the time
you'd been in YouTube fullscreen for half an hour, the launcher could be
sitting on 6 GB. The hunt is logged in [[leak_hunt]] memory; the short
version: `wglSwapBuffers` on a fully-obscured window queues presentations
in the NVIDIA GL driver indefinitely because the compositor never
consumes them. Skia's tracked caches were all bounded the whole time;
nothing in Rust grew. winit's `WindowEvent::Focused(false)` /
`Occluded(true)` / `is_minimized()` are all **unreliable on Win11 when
another app takes fullscreen** — they kept reporting the launcher as
visible, focused, and not minimised even when YouTube fullscreen sat on
top of it.

**Fix:** skip `RedrawRequested` entirely (no Skia work, no
`swap_buffers`) when the launcher isn't the OS foreground window. The
foreground check polls `GetForegroundWindow()` via Win32 (lives in
`crates/ewo-launcher/src/window/win32.rs::is_foreground`, wrapped by
`window::is_foreground`). On non-Windows it returns `true` always, so
the cross-platform path falls back to winit's `Occluded` +
`is_minimized` signals — those work fine on Hyprland/wlroots because
Wayland's frame-callback model makes occlusion explicit. The render
loop also still skips on `WindowEvent::Occluded(true)` and
`window.is_minimized() == Some(true)` for the platforms / scenarios
where those do fire.

**`!self.focused` is intentionally NOT a skip signal** — the user might
have a chat or browser focused on a second monitor while the launcher
animates on their primary. Foreground covers the actually-leak-causing
case (no compositor presentations possible) without sabotaging that.

### Leak-hunt instrumentation — strip before release

Various leak-hunt diagnostics + Skia cache caps were left in place
after the fix landed. They're harmless but add noise + a tiny per-alloc
atomic overhead, and should be removed when the project is ready to
ship binaries. **Every diagnostic site is tagged with the comment
marker `LEAK_HUNT_INSTRUMENT`** — `git grep LEAK_HUNT_INSTRUMENT`
turns them all up. Removing the lot:

- `crates/ewo-launcher/src/main.rs` — the `CountingAllocator` block +
  `alloc_stats()` + `#[global_allocator]`, the periodic `mem:` / `alloc:`
  log block in `RedrawRequested`, and the `cap_skia_global_caches()`
  call in `main()`.
- `crates/ewo-render/src/gl_backend.rs` — `cap_skia_global_caches`,
  `log_skia_global_cache_state`, `format_bytes`, the
  `set_resource_cache_limit(192 MB)` call in `GlBackend::new`, the
  `frames` field, the periodic `perform_deferred_cleanup` +
  `skia gpu:` log block at the bottom of `render()`.
- `crates/ewo-launcher/src/window/win32.rs::process_memory` and the
  `window::process_memory` wrapper in `window/mod.rs`.
- `Cargo.toml` — `Win32_System_Threading` + `Win32_System_ProcessStatus`
  features on the `windows` workspace dep (only used by
  `process_memory`).

The actual fix (`is_foreground`, the `WindowEvent::Occluded` handler,
the `self.occluded` field, the render-skip check using all three
visibility signals) is NOT tagged — it stays forever.

---

## Post-ban refactor: legit / pvp split (2026-05-26)

The "legit-client only" rule that Phase G stated drifted hard during the
post-G iteration sprints — what was originally CLAUDE.md non-negotiable
#4 (Phase E) got walked across a series of incremental PvP / macro
modules (Auto Tool, Auto Totem, Hand Restock, Auto Eat, Sprint Tap, Auto
Mace Swap, Auto Jump Reset, Auto Pearl, Riptide Boost, Auto Hit Timing,
Mace Combo, Wind Charge MLG, Triggerbot, …) that shipped without docs
updates. An anticheat ban landed on CatPvP **with the macros switched
off**, which is the giveaway that class-name fingerprinting (not
behavior detection) was the surface. **This refactor splits the catalog
into a legit set that ships by default and an assist set behind a build
flag.**

### What's where after the refactor

- **Legit module catalog** (always ships, slots 0..11 of REGISTRY):
  Full Bright, FOV Control, Toggle Sprint, Toggle Sneak, No Damage Tilt,
  No View Bob, FreeLook, No Fire Overlay, Crosshair on Reach, No Pumpkin
  Overlay, Hit Color, Hit Indicator. Pure rendering / read-only / universal
  QoL — zero packet synthesis.
- **Assist module catalog** (slots 12..25 of REGISTRY, only present under
  `--features pvp` / `build.ps1 -Pvp`): Auto Tool, Auto Totem, Legit
  Elytra Swap, Hand Restock, Auto Eat, Auto Jump Reset, Sprint Tap,
  Knockback Maximizer, Auto Mace Swap, Auto Pearl, Riptide Boost, Reach
  Lock, Auto Hit Timing, **Swing Cadence**. All synthesize input packets
  (inventory clicks, hotbar swaps, sprint state, attack packets) — that's
  the user's "touching packets isn't fine" red line, and that's why these
  classes must not exist in the legit jar at all.
- **Deleted outright** (gone from both legit + pvp catalogs): `auto_crit`
  (was already a no-op since the bunny-hop tell was too obvious),
  `mace_combo` (tick-perfect kill chain — beyond the line even for
  semi-anarchy), `wind_charge_mlg` (snap-pitch mode was literal aim
  assist).
- **Renamed** with humanization: `triggerbot` → `swing_cadence`. Same
  core behaviour (auto-fires the next swing when attack-strength is
  ready and the crosshair sits on a living target), plus three
  humanization knobs on top — minimum inter-fire interval (default
  200 ms ≈ 5 hits/sec cap), ±ms jitter (default 30), and a
  target-acquired reaction delay (default 80 ms). The class identity
  changed (`EwoTriggerbot` → `EwoSwingCadence`) specifically to drop the
  obvious class-name fingerprint a class-scan AC would flag on.

### Build mechanics

- Rust: each crate (`ewo-core`, `ewo-jni`, `ewo-launcher`) has a `pvp`
  feature. `ewo-core` is the one that gates the registry; `ewo-jni` and
  `ewo-launcher` just propagate it. `cargo build` is the legit build;
  `cargo build --features pvp` (or `-p ewo-jni --features pvp`) is the
  pvp build.
- `crates/ewo-core/src/modules.rs` uses `#[cfg(feature = "pvp")]` on
  individual REGISTRY entries — the legit-build registry is a 12-entry
  prefix of the 26-entry pvp registry. Slot indices for legit modules
  are stable across builds, so Java's legit slot constants
  (`EwoModuleData.FULLBRIGHT = 0`, etc.) never change.
- Java: gated classes live in `dev.lewlone.ewohud.assist.*`
  (modules + `EwoActionMotor` + `EwoSwingCadence`) and their mixin in
  `dev.lewlone.ewohud.assist.mixin.PlayerAttackAssistMixin`. Assist slot
  constants live in `assist.AssistSlots` (slots 12..25). The legit
  `EwoModuleData.java` carries only legit slot constants — assist names
  don't enter the legit jar's constant pool at all.
- The legit driver `EwoModules.java` resolves
  `dev.lewlone.ewohud.assist.EwoAssist` via reflection at class-load:
  present → `tick()` + `handleKeyPress()` delegate to it; absent (legit
  build) → both are no-ops with zero per-frame cost.
- The pvp-only `PlayerAttackAssistMixin` injects HEAD on
  `Player.attack(Entity)` alongside the legit `PlayerAttackMixin`
  (multiple HEAD injects coexist). The legit `PlayerAttackMixin` was
  trimmed to only the legit handoffs (`EwoHitRange.onAttack` +
  `EwoComboTracker.onAttack`); the assist mixin adds Knockback Max +
  `EwoSprintTap.onAttack`.
- Two mixin configs: `ewohud.mixins.json` (legit, always shipped) and
  `ewohud-pvp.mixins.json` (additive, pvp jar only). Two `fabric.mod.json`
  variants: legit references only the legit config; `fabric-pvp.mod.json`
  references both (`build.ps1 -Pvp` renames it to `fabric.mod.json`
  inside the jar).
- `EwoModuleData.SCHEMA_VERSION` bumped 2 → 3. The new layout: legit
  slots 0..11 in stable order; assist slots 12..25 only written when
  Rust is built with `--features pvp`. `MODULE_COUNT` is now dynamic —
  read from buffer offset 4, where Rust writes the live registry length
  per build. `enabled(slot)` returns false for any slot at or past that
  value, so legit-build code asking about an assist slot is safe.
- `ingame-mod/build.ps1` gains a `-Pvp` switch. Without it: cargo build
  is plain, assist Java sources are filtered out of `javac`,
  `ewohud-pvp.mixins.json` is excluded from the jar, legit
  `fabric.mod.json` ships. With it: cargo build adds `--features pvp`,
  assist sources are compiled, both mixin configs ship, and
  `fabric-pvp.mod.json` ships as the in-jar `fabric.mod.json`.

### Verification (2026-05-26)

- `cargo test -p ewo-core --lib` passes 8 tests (legit). Same with
  `--features pvp` (one extra test covers the assist slot ordering).
  Two regression-guard tests block re-introduction: one fails if any
  deleted id reappears in REGISTRY; one fails if the legit-build
  REGISTRY ever exceeds 12 entries.
- `build.ps1` (legit) produces a jar whose `jar --list` shows zero
  classes under `dev/lewlone/ewohud/assist/` and exactly one mixin
  config (`ewohud.mixins.json`). `build.ps1 -Pvp` produces a jar with
  all 14 assist classes + the pvp mixin + both mixin configs.
- **In-game verification of both builds still pending** — the user does
  this. Likely smoke test: launch a legit-build session, open the
  overlay's MODULES tab, confirm only the 12 legit modules render;
  launch a pvp-build session, confirm all 26 show + Swing Cadence
  toggles + fires with the humanized cadence.

### Known follow-ups

- The launcher's profile-level `modules.toml` is id-keyed so settings
  for the deleted modules linger as inert sections. Cosmetic; the
  modules are no longer in REGISTRY so the launcher Settings → Modules
  tab won't surface them. A future cleanup pass on `profile::load_modules`
  could drop unknown ids on save.
- Keybinds for `triggerbot` (now `swing_cadence`) need a rebind because
  the id changed — the launcher Keybinds tab will show `swing_cadence`
  unbound on profiles where `triggerbot` was bound.

---

## Phase H — Social: Friends, Presence, Launcher-Link (built; deploy + live test pending)

The third feature phase past v1 + v2. [`PHASE_H_PLAN.md`](PHASE_H_PLAN.md)
holds the original forward plan (now partly stale — see below). Phase H
plugs the launcher into the user's existing **chickenedin** Minecraft
network (**renamed Frogsy in mid-2026** — this section predates the rename;
domains/API bases below still say chickenedin.com, verify what's live before
relying on them) so the launcher knows your friends, their presence, and
(future H6) lets you Roblox-style join them. **Offline-first holds**: signed-out
launcher makes zero network calls; signed-in-but-unlinked makes MS-auth
calls only; social calls happen only once the launcher has a per-user
`social_token`.

### Three repos, all built — the contract is verified-aligned

Phase H spans three repos. As of 2026-05-30 **all three are implemented
and the wire contract matches across them** (verified handler-by-handler
against [`crates/ewo-launcher/src/social/mod.rs`](crates/ewo-launcher/src/social/mod.rs)):

1. **chickenbot (Python)** — `C:/Users/valtteri/Desktop/FULLSTACK/chickenbot/`,
   branch `chickenbot-mod-tools`. **Committed + clean.** `database.py`
   declares the Phase H tables (`launcher_link_codes`, `social_tokens`,
   `presence`, `friendships`, `mc_name_cache`) and helpers
   (`mint_social_token`, `validate_social_token`, `list_friendships_for`,
   `upsert_friendship_request`, `respond_to_friendship`,
   `remove_friendship`, `upsert_presence`, `consume_launcher_link_code`,
   `lookup_uuid_by_mc_name`). `api.py` registers + implements every
   endpoint below. Auth: `check_auth` (system `API_SECRET`) for
   plugin/website calls; `check_user_token` (per-row `social_tokens`
   bearer) for launcher calls.
2. **ChickenLink (Paper plugin)** — `FULLSTACK/NETWORK/ChickenLink/`.
   The `/launcher-link` command (`LauncherLinkCommand.java`) mints a
   6-digit code via `POST /api/launcher-link-code` and shows it to the
   player. **Uncommitted as of 2026-05-30** (command + `APIClient` +
   `ChickenLink.java` registration + `plugin.yml`).
3. **EwoClient launcher (this repo)** — `social/mod.rs` (the HTTP +
   state machine), `screens/friends.rs` (`Screen::Friends`),
   `screens/launcher_link_modal.rs` (6-digit redeem modal). Wired into
   `App` as `SocialState`. **Uncommitted** (part of the big checkpoint).

### The wire contract (launcher ↔ bot)

Base URL = `https://chickenedin.com/bot` (override with env
`EWO_BOT_API_BASE`); endpoints hang under `{base}/api/...`, reverse-proxied
to the bot's `:8080`. **The bot itself listens on `/api/...` directly** —
the `/bot` prefix is the nginx route.

```
GET    /api/links/by-uuid?minecraft_uuid=<dashed>   PUBLIC  → {linked: bool}
POST   /api/launcher-link-code                      system  body {minecraft_uuid} → {code, expires_at} | 404 not-linked
POST   /api/launcher/link                           PUBLIC  body {code} → {social_token, discord_id} | 404 code-invalid-or-expired
POST   /api/presence/heartbeat                       user    body {minecraft_uuid, location, screen?, server_addr?, visibility?} → {ok}
GET    /api/friends                                  user    → {friends[], incoming[], outgoing[]} (discord_id as STRING, presence nested)
POST   /api/friends/request                          user    body {target_mc_name} → {status} | 404 {status: not-found|not-linked}
POST   /api/friends/respond                          user    body {request_from_discord_id:int, action} → {status}
DELETE /api/friends/{discord_id}                     user    → {status: removed}
```

Contract gotchas (already correct on both sides — recorded so they don't
regress): `discord_id` is a **string** in `/api/friends` entries (the
launcher parses it with `.as_str()`; a numeric value would silently drop
every friend) but a **number** in `/api/friends/respond`'s body. UUIDs go
to the bot **dashed** (`social::uuid_with_dashes`). Two cosmetic-only
mismatches remain unfixed: a self-request returns HTTP 400 so the launcher
shows "http 400" instead of "you can't friend yourself", and an
`already_*`/`reciprocal` status renders as generic text. Neither breaks
anything.

### What's done vs. open

- **Launcher H1–H5 built**: link probe on MS sign-in (Account tab shows
  linked status), `/launcher-link` redeem modal, 30s presence heartbeat,
  friends list + request/respond/remove, `Screen::Friends`. Some loose
  ends (the `friend_action` toast, `refresh_friends_now`, and the
  `InGame` presence variant are coded but not all call-sites wired —
  these surface as dead-code warnings).
- **H6 (live server-status widget + Roblox-style join) BUILT (2026-05-30)**:
  - Launch-into-server plumbing — `App::active_server` + a shared
    `start_launch(idx, server, time)` helper (the Launch button and both
    join paths funnel through it). When set, `try_real_launch` appends
    `--quickPlayMultiplayer <addr>` (the 1.20+ replacement for the removed
    `--server`/`--port` pair) and the presence heartbeat reports
    `in_game · <addr>` while the JVM is alive (gated on `launch_rx.is_some()`).
  - `social::ServerStatus` poller (`maybe_refresh_server_status`, 15s,
    main-menu only) against the now-public `GET /api/server-status`.
  - Main-menu network widget (`main_menu::draw_server_widget`, lower-left
    Velvet card) — "X / Y online · TPS Z", click joins
    `play.chickenedin.com`. Render-side `ServerWidgetView` mirrors the
    `FriendRowView` cross-crate pattern.
  - Friend "Join" button — `server_addr` added to `FriendRowView`; the
    button draws only for in-game friends and joins their `server_addr`.
  - **Bot change**: `GET /api/server-status` made public (POST stays
    system-authed) — needs a redeploy. **Visual placement of the
    main-menu widget is unverified** (lower-left; may want tuning vs the
    right-column menu items on narrow windows) — eyeball pass pending.
- **H7 (WebSocket push) not done** — polling-only, by design until lag
  justifies it.
- **In-game FRIENDS overlay tab BUILT (2026-05-31, read-only)** — an 8th
  overlay tab; the strip is now HOME · HUD · CROSSHAIR · MODULES · PVP ·
  MODS · FRIENDS · SETTINGS. **File-bridge, no HTTP in the cdylib**: the
  launcher writes a per-profile `ewo-friends.txt` snapshot
  (`<online>\t<name>\t<presence>\t<server_addr>` per accepted friend) on
  each friends-list change via `profile::active_dir()`; the cdylib's
  `social::read_friends()` reads it fresh each frame the tab is visible and
  `hud::draw_friends_view` renders the Velvet list. View-only — mutations +
  join stay launcher-side. **Freshness caveat**: the launcher only rewrites
  the snapshot while it's foreground (its leak-fix skips per-frame work when
  backgrounded), so during active play the list reflects the last
  launcher-foreground refresh (≈launch time); live-during-play needs an
  in-game poller or a launcher background tick — deferred. Also fixed a
  latent bug: `hud::tab_layout` was hardcoded to 6 slots while
  `OverlayView::ALL` had 7 (PVP), silently clipping the SETTINGS tab off
  the strip — it now sizes to `ALL.len()`.
- **THE real remaining blocker is ops, not code**: the bot must be
  *deployed/running* on the VPS with nginx routing `/bot/api/*` → bot
  `:8080`, and a live end-to-end test (sign in → `/launcher-link`
  in-game → paste code → see a friend's presence) has not been run. H0's
  SSH key (`ssh ewo-vps`) was generated 2026-05-27; the public key may
  still need deploying to the VPS.

## Two undocumented in-game features (shipped, in the big checkpoint)

Built during the post-G / Phase H sprints and absent from every plan doc
until now. Both live in `crates/ewo-jni` (the in-game HUD cdylib) and the
overlay editor:

- **Custom crosshair** — [`crates/ewo-jni/src/crosshair.rs`](crates/ewo-jni/src/crosshair.rs)
  + `ingame-mod/.../mixin/GuiCrosshairMixin.java`. A per-profile
  `crosshair.toml` (sibling of `hud.toml`), an in-overlay **CROSSHAIR**
  editor tab (sliders + toggles + colour swatches + live preview), and an
  in-world crosshair drawn at screen center. When enabled, the Java mixin
  reads `nativeIsCustomCrosshairEnabled` and cancels the vanilla
  `Gui.extractCrosshair` so only ours shows. `MouseHandlerInputMixin` was
  added alongside.
- **Media controller** — [`crates/ewo-jni/src/media.rs`](crates/ewo-jni/src/media.rs).
  Reads Windows **SMTC** (System Media Transport Controls — the global
  feed Spotify/browsers/Apple Music write to) on a background thread;
  paints a HUD "now playing" widget (title/artist/scrub/transport/thumb)
  plus a large card on the HOME overlay tab. Transport clicks
  (play/pause/skip) flow back through `MediaService::act` →
  `TryPlayAsync`/`TryPauseAsync`. `EwoHudData` schema is at version 10.

---

## Launcher window: transparent floating card (2026-05-31 — supersedes the Step 1/2 "app-window chrome" notes)

The launcher window changed from an **opaque card painted inside a 28px
margin with a 3-layer berry box-shadow** to a **real per-pixel-alpha window
where the rounded card IS the window**. The old Step 1/Step 2 notes
(`CARD_INSET = 28`, `draw_chrome_outer` berry-glow + black drop shadows,
`DWMWCP_ROUND`) are historical — here's the current state.

- **`with_transparent(true)`** on the winit window (the GL config already
  carries `alpha_size 8`). `draw_chrome_outer` clears the frame to
  `Color::TRANSPARENT`; the desktop shows through anywhere the card isn't.
  Transparent GL windows can be driver-finicky on Windows — it composited
  fine here (NVIDIA), but that's the thing to re-check if it ever renders
  black instead of the desktop.
- **`CARD_INSET = 0`** (both `app_window::CARD_INSET` *and* the launcher's
  mirror `CARD_INSET_LP` used for cursor→card mapping — they MUST match or
  the cursor drifts from the widgets). The 22px-rounded card fills the
  window edge-to-edge, so **the window's edges are the card's edges** and
  the OS resize/drag hit zones (which key off the window rect) line up. This
  fixed a long-standing UX bug where the invisible 28px margin still counted
  as the window, putting the resize edge out in empty space.
- **No outer drop shadow** — there's no margin to render one into.
  `draw_outer_shadow` is kept (`#[allow(dead_code)]`) for a future
  shadow-with-margin variant. Edge definition is the inset rose hairline rim
  in `draw_chrome_inner`. (An interim transparent-window build *did* keep a
  margin + shadow; the berry glow bled a coloured halo onto apps behind the
  window, so it was first neutralised, then dropped with the margin.)
- **`win32.rs`: `DWMWCP_DONOTROUND`** (was `DWMWCP_ROUND`) — we paint our own
  22px corners, so DWM must not also round the window rect (its ~8px would
  clip our corners + cast a competing rectangular shadow).
- **Minimize + close buttons** (top-right) — `app_window::window_button_bounds`
  + `draw_window_buttons`: vector — / × icons, hover fills a soft rounded bg
  (rose / ember) + brightens. `hit_test` excludes the button rects from the
  drag caption so clicks land; the launcher handles them before modals so
  they always work. Close → `event_loop.exit()`; minimize →
  `Window::set_minimized(true)`.

### Freeze-on-exit — fixed (was a bug since the start)

On quit (Quit-to-desktop / taskbar close / the close button) the GL-context +
window teardown could **deadlock the main thread**, leaving the window "Not
Responding" until killed via Task Manager. Fix: `main()` calls
**`std::process::exit(0)` right after `event_loop.run_app` returns**, skipping
the hanging `App`/`GlBackend` `Drop`. Nothing in `Drop` needs to run —
settings / instances persist on change, not on exit — and the OS reclaims the
window, GL context, and detached threads.

### Packaging — shortcut + icon + portability

- **Release builds are GUI apps** — `#![cfg_attr(not(debug_assertions),
  windows_subsystem = "windows")]` in `main.rs`, so no console window pops up
  from a shortcut. Debug keeps the console for `cargo run`.
- **App icon**: `assets/icon.ico` (multi-res 16/32/48/256, 32-bit RGBA) is
  embedded into the exe by `crates/ewo-launcher/build.rs` via the
  `winresource` build-dep (no-op until the `.ico` exists, so the build never
  blocks on art). Shows in taskbar / Explorer / shortcut. The borderless
  window's own taskbar/alt-tab icon may still need a runtime
  `with_window_icon` if it shows a default — not yet wired.
- **Portable assets**: `ewo-render` `text.rs::workspace_assets_dir` resolves
  `assets/fonts` **next to the executable first**, falling back to the
  compile-time workspace path for `cargo run`. Fonts are the only
  runtime-loaded asset (shaders are built-in SkSL).
- **`package.ps1`** (repo root): release-builds + stages a self-contained
  `dist/EwoClient/` (EwoClient.exe + assets/fonts + icon), then points the
  Desktop "EwoClient" shortcut at it. `dist/` is gitignored. Run it after code
  changes to refresh the bundle the shortcut launches; `cargo run` is the dev
  path (uses the in-repo assets).

### Launcher visual polish (same session)

- **Hover glow** — `text::draw_glow_str` / `draw_tracked_glow` (the hero
  title's two-halo treatment, gated by an intensity) now lights up: main-menu
  items, the top tab bar (hovered tab), the Settings sidebar tabs, instance
  list rows, and the "‹ Main menu" back-link — all on hover, with a pearl /
  warm-white brighten.
- **Tofu fixes** — the menu caret, server-widget "JOIN", Instances back/sort,
  and Settings back-link used arrow glyphs the serif fonts lack (rendered as
  boxes). Replaced with vector chevrons (`draw_chevron_right`) / en-dashes.
- **Dropdown** — flip-up is now bias-toward-down (only flips when < ~1.5 rows
  fit below); the "square corner" artifact was finally diagnosed by rendering
  the dropdown to a PNG (`crates/ewo-render/examples/dropdown_shot.rs`) — it
  was the drop shadow pooling below the rounded corners, fixed with a faint
  symmetric halo + opaque body. **That render harness is the tool for
  verifying visual changes without a full launcher run.**
- **Sliders** glow/grow the handle on hover; the **scrollbar** moved into the
  panel's right gutter (was overlapping widgets); the **main menu** dropped
  the SETTINGS link + footer + disturb-hint texts and gained a fade+slide
  entrance; **Friends** heading/subtitle overlap fixed; the back-link
  navigates to MainMenu.

---

## Memory + performance pass (2026-05-31)

A dedicated pass to kill the focused-RSS leak and cut per-frame GPU cost.
Verified live: RSS now **stable at ~108 MB** (was climbing ~2.6 GB/hour while
focused + idle), and the app runs nicely at the 500fps target.

### The focused-idle leak — root-caused + fixed

The H5-session leak (RSS → ~20.9 GB over 8h *while focused*; foreign
C++/FreeType memory not in any Skia cache) was two per-frame foreign
allocations:

1. **`backdrop/velvet_folds.rs` rebuilt a Perlin-noise filter chain every
   frame** — `shaders::fractal_noise` + `image_filters::shader` +
   `displacement_map` + `blur`, all *logically static* (fixed seed/freq/octaves;
   the module comment already said so), constructed fresh ~500×/sec. Each
   `SkPerlinNoiseShader` carries precomputed noise tables in plain C++ heap,
   invisible to Skia's resource/font caches — the exact "foreign, untracked,
   leaks while idle" profile. Runs on **every** screen (the backdrop draws
   unconditionally), which is why it leaked at idle. **Fix:** build the chain
   once into a `thread_local`, clone the refcounted handle per frame. This was
   the dominant leak.
2. **`screens/main_menu.rs::newsreader_italic_axes` called
   `clone_with_arguments` per frame** (main menu only) — the same
   variable-font-clone hazard `text.rs`'s `fraunces_cache` already documents
   (it measured ≈4 MB/s @ 500fps). **Fix:** added
   `FontStore::newsreader_italic_axes` with a quantized typeface cache mirroring
   `fraunces_cache`.

Both fixes are **zero pixel change**. The load-bearing principle: *never
construct a Skia shader / image-filter / variable-font `Typeface` inside a
per-frame draw — they allocate foreign C++/FreeType state that Skia's tracked
caches don't bound. Build once, clone the handle.* The constant-sigma blur
filters in `caustics.rs` / `bokeh.rs` were hoisted the same way.

### The backdrop render graph now has a "slow clock"

`backdrop::Backdrop` caches the four **slow** layers — wine → velvet folds →
caustics → bokeh, i.e. the three full-screen Gaussian blurs (σ 20/15/20) + the
fractal-noise displacement, by far the heaviest GPU work — into an offscreen
surface refreshed at **`CACHE_REFRESH_HZ` (20 Hz)**, blitted 1:1 every frame.
The **fast** layers (pearl dust, petals) + the cheap vignette draw live on top
each frame. Heavy blur work now runs ~20×/sec instead of ~500×/sec (~25× less
per-frame backdrop cost) with no perceptible change — those layers drift on
8–60 s periods. Mechanism: `canvas.new_surface(...)` → `image_snapshot()` →
`draw_image`; the cache is invalidated on resize. Same "cache the slow clock"
trick the in-game HUD frost uses (`ewo-jni::refresh_frost`).

Layering is exact: wine fills the offscreen opaquely, folds/caustics/bokeh
screen-blend on top inside it, the opaque result blits over the black card body
under the same rrect clip → identical pixels, rounded corners and all. The
offscreen has no MSAA (soft gradient content doesn't need it).

### Other wins

- **Pearl-dust halos batched** (`backdrop/pearl_dust.rs`) — the 110 airborne
  motes each allocated a fresh 3-stop radial-gradient shader per frame (which
  also churned Skia's gradient-LUT cache). Now the halo is **baked once into a
  64px GPU sprite** at reference alpha 1.0 and stamped per mote with a reused
  `set_alpha_f` paint — mathematically identical pixels, zero per-frame shader
  allocation. Cores + settled motes stay as solid `draw_circle` (no alloc).
  (Counts are 110 airborne + 80 settled, above the docs' 90/60.)
- **Inner berry glow baked** (`app_window.rs`) — was a σ40 mask-blur of an
  unchanging stroke every frame; now baked once per window size into a cached
  image (`GLOW_CACHE` thread_local) and blitted.
- **Idle frame throttle** (`main.rs::about_to_wait`) — after
  `IDLE_THRESHOLD_SECS` (0.5 s) of no input the focused loop drops to `IDLE_FPS`
  (120) **even with vsync on**; any cursor/key/scroll resets `last_activity` and
  snaps back to full rate; never throttles during the Launching screen. Cuts
  idle GPU/heat/power without touching the interactive 500fps target. (This
  softens the "always 500fps" line, but only when the launcher is untouched.
  `request_redraw` only marks the window dirty; `ControlFlow` decides the wake
  cadence, so the WaitUntil caps the rate even with a redraw pending — the
  existing `max_fps` cap relied on this too.)

### Still open

- **`LEAK_HUNT_INSTRUMENT` diagnostics are still in** (the counting global
  allocator + periodic mem/cache logs + Skia cache caps) — deliberately kept
  through the verification of this fix. Now that RSS is confirmed flat they can
  be stripped per the markers (`git grep LEAK_HUNT_INSTRUMENT`).

---

