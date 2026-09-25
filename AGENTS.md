<!-- GENERATED FILE - do not edit directly.

     This file is a verbatim copy of CLAUDE.md with the self-references
     renamed. The two had drifted 634 lines apart by 2026-07-27 (four days and
     roughly twenty milestones), and then 3,061 lines apart by 2026-08-07
     (thirty-five milestones, still calling the Rewo work branch "unmerged, the
     largest non-code risk"). A duplicated source of truth invites exactly
     this; regenerating is the whole maintenance burden.

     Edit CLAUDE.md, then regenerate:
         python -c "import io; c=io.open('CLAUDE.md',encoding='utf-8',newline='').read(); io.open('AGENTS.md','w',encoding='utf-8',newline='').write(c.replace('CLAUDE.md','AGENTS.md'))"
     and re-add this header.

     The generator is a blind whole-file rename, so prose that names BOTH
     files reads as nonsense on one side. Say "the mirror". -->

# EwoClient + Rewo

Reference for anyone (human or agent) working in this repo. It is deliberately
short: invariants, commands, traps, and where to look. Milestone history does
not belong here — it lives in `git log` and `docs/history/`.

**Rule for this file:** no hand-typed status numbers (test counts, gate counts,
"verified" claims). They rot within days and previous versions of this file
were wrong in both directions. Measure instead: `cargo test --workspace` and
`python tools/gates.py`.

---

## What this repo is

Two projects in one Cargo workspace (`git remote`: `lewlone/ewoclient`):

1. **EwoClient** — a Minecraft launcher (`crates/ewo-launcher`, Skia UI in
   `crates/ewo-render`) plus an in-game HUD: a Rust cdylib (`crates/ewo-jni`)
   loaded by a Fabric mod (`ingame-mod/`) running on **EwoLoader**, a friendly
   fork of fabric-loader in the sibling repo `C:\Users\valtteri\Desktop\EwoLoaderV1`
   (`lewlone/ewo-loader`). "Velvet & Pearl" visual theme.
2. **Rewo** — a from-scratch native Minecraft Java client (`crates/rewo-*`,
   binary `rewo`): vanilla protocol 26.2 / 776, raw Vulkan via `ash`. The
   launcher starts it as the `Native` instance kind. It is not a JVM/mod
   project; the `ewo-jni`/mixin machinery does not apply to it.

Targets: Windows 11 (primary) and CachyOS/Hyprland (Wayland only; the Linux
paths are written but have never been run). macOS and X11 are out of scope.

Dead sibling directories on the Desktop (`EwoClient`, `EwoClient-v2`) are
abandoned; this one (`EwoClientV3`) is live.

Author identity for commits in both repos: `lewlone <valtteri.e.saarinen@gmail.com>`.

---

## Crate map

| Crate | What it is |
|---|---|
| `ewo-core` | Theme tokens, easing, color (oklch), module/pvp catalogs shared by launcher + HUD. No graphics deps. |
| `ewo-render` | Skia renderer for the launcher: backdrop layers, glass panel, widgets, screens, text, the D3D12/GL backends. |
| `ewo-launcher` | Launcher binary: winit window + custom frame, auth, versions, downloads, launch, JRE fetch, loaders, profiles, social. |
| `ewo-jni` | cdylib loaded into Minecraft: second WGL context, Skia HUD, overlay dashboard, modules buffer, media/audio. |
| `rewo-proto` | Wire primitives: varints, frame codec, NBT, packet reader/writer. |
| `rewo-data` | Asset bake (block/item models, textures), datagen-derived and generated tables (`*_table.rs`). |
| `rewo-world` | Client world state: chunks, light engine, physics, entities, inventory, and (mis-filed) UI screen models. |
| `rewo-net` | Protocol state machine: login/config/play, `PlaySession`, packet handlers, crypto, chat signing, sound engine. |
| `rewo-mesh` | Chunk mesher (greedy cubes + model quads + fluids) and the rayon mesh pool. |
| `rewo-gpu` | Vulkan renderer: device/swapchain, world pass (GPU cull + indirect), entities, HUD, screens, particles, sky. |
| `rewo-audio` | Mixer, decode (symphonia), cpal sink — behind the `audio` feature of `rewo-app`. |
| `rewo-app` | The `rewo` binary: `live` (the client), `play`/`view`/`net`/`bench`/`demo`, and the `*shot` verification gates. |

---

## Commands

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast       # unit tests (some skip without local assets)
python tools/gates.py                        # every `rewo *shot --check` gate + demo PNG hash
python tools/gates.py --only mobshot,itemshot
python tools/render_check.py                 # stages a fresh vanilla server, runs `rewo live --render-check`
cargo run -p ewo-launcher                    # launcher (add --dev for the tweaks overlay)
cargo run -p rewo-app -- live --host HOST:PORT
powershell ingame-mod/build.ps1 [-Pvp]       # builds ewo-jni + the mod jar AND deploys it (see traps)
powershell package.ps1                       # release bundle into dist/EwoClient
python tools/regen_agents_mirror.py          # after editing this file: regenerate AGENTS.md
```

**Local-only inputs.** Most gates and several tests need
`%APPDATA%/EwoClient/rewo/26.2/` (datagen reports + a Vineflower decompile of the
user's own client jar) and a Vulkan device with the SDK's validation layers.
CI (`.github/workflows/ci.yml`) runs only build + `cargo test` + clippy.
A test that silently returns when assets are missing is a false pass — prefer
`#[ignore]` or an explicit skip message.

**Gate rules** (enforced by `tools/gates.py`): a gate passes only if it exits 0
**and** its output has no Vulkan validation error. The demo PNG's expected
SHA-256 prefix lives in `tools/demo_hash.txt`; change it only in a commit that
intentionally changes rendering, and say why. `rewo live` validation is only on
in **debug** builds, so `render_check.py` insists on the debug binary.

---

## Design non-negotiables (launcher + HUD visuals)

1. Motion is load-bearing: every prototype surface animates continuously.
2. Never transform (scale) a container that holds text; animate adjacent layers.
3. No blur during entrance animations on text-bearing surfaces.
4. `--silk` = `cubic-bezier(0.22, 1, 0.36, 1)` is the default easing.
5. Pearl dust, velvet folds, caustics and bokeh are on every screen.
6. Honor reduced-motion (`SPI_GETCLIENTAREAANIMATION` on Windows).
7. The backdrop is its own compositing group (`isolation: isolate` semantics).

Visual source of truth: `StyleSheet1`, `StyleSheet2`,
`EwoClient · Velvet & Pearl prototype.htm`, `style/*.png` (don't move them).
Canonical palette: `ewo_core::theme::Theme::VELVET`. Velvet is the only
implemented theme.

---

## Architecture decisions

- **Launcher rendering:** Skia. Windows = D3D12 + DirectComposition swapchain
  (the only way to get per-pixel-alpha rounded corners on Win11); Linux = GL via
  glutin. Both behind `ewo_render::gl_backend::GlBackend`. On Windows the frame
  rate is vsynced (composition swapchains can't tear).
- **Launcher window** is a transparent floating card that fills the window
  (`CARD_INSET = 0`, must match `CARD_INSET_LP` in the launcher),
  `DWMWCP_DONOTROUND`, custom minimize/close. Layout is hand-rolled
  (`taffy` is a workspace dependency but is not used).
- **No async runtimes** (no tokio/smol). Long work = `std::thread` + `mpsc`,
  polled once per frame. HTTP via `ureq` (sync).
- **Offline-first intent:** no telemetry, analytics or auto-update. Network use
  is Microsoft/Mojang auth, version manifests + asset CDN, and (for signed-in
  users) the social API. Keep new network calls behind sign-in / opt-in.
- **Fonts** are bundled in `assets/fonts/` (Fraunces, Newsreader, JetBrains Mono
  variable TTFs); resolved next to the exe first, then the source tree.
- **Rewo rendering:** raw Vulkan 1.2+ (dynamic rendering, sync2), reversed-Z
  depth, GLSL compiled by `glslc` in `rewo-gpu/build.rs` (needs `VULKAN_SDK`).
- **Rewo threading:** one network reader thread (frames bytes into a channel),
  a rayon mesh pool, audio/decoder threads; everything else — packet handling,
  the 20 Hz tick, rendering — on the winit main thread.

### Disk layout (`%APPDATA%/EwoClient` or `$XDG_CONFIG_HOME/EwoClient`)

```
shared/{versions,libraries,assets}/   Mojang-compatible; vanilla launchers can read it. Don't break it.
instances/<id>/                       per-instance game dir, natives, logs
runtime/<major>/jre/                  Adoptium JREs fetched on demand
auth.toml                             accounts (plaintext refresh tokens — encrypt before distribution)
profiles.toml, profiles/<name>/       client profiles: client.toml, hud.toml, modules.toml, crosshair.toml, pvp.toml
settings.toml, instances.toml         global settings, instance list
rewo/26.2/                            Rewo's local datagen + decompile (not redistributable)
```

---

## Coupling you must keep in sync

- **Adding a bundled mod** is a three-place change, and a mismatch fails at
  launch: `crates/ewo-launcher/src/bundled.rs::CATALOG`, **both**
  `EwoLoaderV1/manifest/0.1.0/26.1.json` and `26.2.json` `libraries[]`, and
  `EwoLoaderV1/.../BundledMods.java::BUNDLED_MODS` (then rebuild the fat jar).
- **Shared buffers** between the Java mod and `ewo-jni`: `EwoHudData.java` ↔
  `hud.rs` and `EwoModuleData.java` ↔ `modules.rs` mirror byte layouts and each
  carries a `SCHEMA_VERSION`. Bump both sides together (a test pins the pair).
- **Module registry** (`ewo_core::modules::REGISTRY`) is shared by launcher and
  HUD; legit modules are slots 0..11 and stable; assist (PvP) modules exist only
  under `--features pvp` / `build.ps1 -Pvp`, and their Java lives in
  `dev.lewlone.ewohud.assist.*` so it is absent from the legit jar entirely.
- **Packet coverage:** `REWO_PACKET_COVERAGE.md`'s table is parsed by a unit
  test in `rewo-net/src/ids.rs`; keep it in sync when resolving packet ids.
- **Generated files** (`*_table.rs`, `anim_defs.rs`, etc.) say "do not edit";
  regenerate with the matching `tools/gen_*` script (needs the local decompile).

---

## Traps (each has cost real time)

**Protocol (Rewo)**
- `holder` codec = `id + 1` with 0 meaning inline; `holderRegistry` /
  `registry(...)` = raw 0-based. Mixing them yields a *plausible wrong* value.
- Several fields are fixed-width big-endian (`writeInt`, `readShort`) in a
  mostly-varint protocol. Read the codec, not the field type.
- `serde_json`'s default `Map` is sorted: never derive registry ids from
  iteration order — read `protocol_id`. Built-in registries resolve **by name
  from the report**; only datapack registries arrive in `registry_data`, whose
  wire order *is* the id.
- NBT lists are homogeneous: a mixed list is written as compounds with
  non-compound elements boxed as `{"": value}`; unwrap on read.
- A `DataComponentPatch` has no length prefix — an untranscribed component
  cannot be skipped.
- If a handler lives on `ClientCommonPacketListener`, the packet also exists
  in the **configuration** state.
- The local player is not in `EntityTable`; metadata/attributes addressed to it
  need their own path.

**Rendering (Rewo)**
- The mesher emits **world-space** vertices; shaders must not add a column origin.
- Collision uses `baked.solid`, not `RenderKind::Cube`; and `RenderKind::Cube`
  is not an opacity proxy either (glass/leaves/ice bake as `Cube`).
- A vertex ring written before `render` needs ≥ frames-in-flight + 1 slots;
  written inside `draw`, ≥ frames-in-flight. Frames in flight can be up to 3.
- Core validation cannot see host-write or cross-frame hazards; use
  synchronization validation before claiming a pass is race-free.
- The Velvet passes must be built with `world::unorm_of(target_format)` and
  drawn inside `with_gamma_space` (gamma-space blending, like Skia).
- The glint needs `VK_KHR_swapchain_mutable_format`; without it no glint draws.
- Java math that matters for exactness: `Mth.sin` is a 65,536-entry table;
  `(float)` casts happen before widening; JOML `fma` is non-fused.

**Launcher / in-game**
- The launcher caches `file://` libraries under `shared/libraries/` and does
  **not** refresh them when the source jar changes. Always build the mod with
  `ingame-mod/build.ps1` (it copies over the cache). If a change "doesn't take",
  compare `certutil -hashfile` of build output vs the cached copy first.
- The mod loads `ewo_jni.dll` from the workspace `target/` dir; check which
  build (debug/release) it logged as loaded.
- `wglSwapBuffers` on a fully obscured window queues presents in the NVIDIA
  driver forever; the launcher skips rendering when it isn't the foreground
  window (`window::is_foreground`). Don't remove that.
- Mojmap: 26.x ships deobfuscated; mods compile with plain `javac` against the
  on-disk jar with JDK 25. No Loom, no remapping. Read authlib properties
  reflectively (build vs runtime classpath skew).

**Process**
- Read a command's **exit code**, not a substring of its output. A mutation
  battery run against an already-failing command reports every mutant killed —
  include a no-op control that must survive.
- `cargo build` passing says nothing about whether tests compile; after a
  mutation battery the binary on disk is the last mutant — rebuild.
- A gate that constructs an input production derives does not test the
  derivation. A gate that reimplements app setup misses what the app adds.
- A frame-diff control must hold everything constant except the subject.
- Line endings: most files are LF; `rewo-gpu/src/cem.rs` is CRLF and a few
  files are mixed. Check `git diff --stat` for whole-file rewrites before
  committing. Measure by bytes, never with a shell pattern containing `\r`.

---

## Known issues and open work

See `docs/REVIEW-2026-09.md` for the independent review (2026-09-25) and what
has been fixed since. Anything listed there as open is open.

---

## Docs map

| File | Use |
|---|---|
| `REWO_PLAN.md` | Rewo plan of record. §0.0 is the current-state handoff; §15 is milestone history (long). |
| `REWO_PACKET_COVERAGE.md` | Which clientbound packets Rewo consumes (machine-checked table). |
| `REWO_AUDIO_PLAN.md` | Audio architecture and what no gate can verify (the listening pass). |
| `REWO_FEATURE_SURVEY.md` | Feature roadmap from a Modrinth survey (counts carry ~25% error). |
| `REWO_VELVET_UI_PLAN.md` | Velvet type stack for Rewo's UI; records the HUD visual freeze. |
| `REWO_HEALTH_BAR_SPEC.md`, `REWO_WAVY_CAPE_SPEC.md` | Specs for features with no vanilla oracle. |
| `docs/REVIEW-2026-09.md` | Independent code review + fix status. |
| `docs/history/` | Retired plans, handoffs, and the previous 7,000-line AGENTS.md (milestone narrative M0–M180). |
| `AGENTS.md` | Generated mirror of this file for non-Claude agents. |

---

## Working conventions

- Branch new work from `main`; don't push without being asked.
- Commit messages end with the `Co-Authored-By:` trailer the session specifies.
- Treat any "verified/exact/0 errors" claim in older docs as scoped to what was
  actually measured until you re-check it.
- Legit/PvP split: the default build ships only legit (non-packet-synthesizing)
  modules; do not move assist code into legit packages.
- Don't add a fifth `ewo-*` crate or a new `rewo-*` crate without a structural
  reason, and don't add tests that need a headless browser.
