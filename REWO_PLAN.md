# REWO_PLAN.md — Rewo: the from-scratch native Minecraft client

**Rewo** (from "rewolution", as Ewo came from "ewolution") is a from-scratch
Rust Minecraft: Java Edition client speaking the vanilla protocol, rendered
with raw Vulkan. This file is the plan of record. It supersedes both the
hand-off design doc (`~/Downloads/rust-mc-client-design.md`, drafted under
codename "Ferric") and the interim `FERRIC_PLAN.md` (deleted). The design
doc's reasoning was pressure-tested against the live repo and the on-disk
26.2 jar on 2026-07-21; its four product decisions are kept, a set of factual
errors is corrected (§2), and several missing workstreams are added (§3).

**Status: M0 shipped + headlessly verified (2026-07-21).** See §15 status log.

## 0.1 Verification policy (user mandate, 2026-07-21)

The user hates manual testing. **Everything that can be verified headlessly
must be** — this is a standing constraint on every milestone, not a
nice-to-have:

- `rewo --headless N [--chart-demo] --out x.png` renders offscreen with no
  window and writes a PNG an agent can inspect. Every visual feature grows a
  headless render path like this.
- `rewo --run-seconds N` soaks the windowed path and auto-exits, printing a
  parseable stats block (adapter, present mode, fps, cpu/gpu percentiles).
- M1+: packet record/replay is the offline fixture; protocol soak tests run
  against a **local offline Paper server that Claude sets up and runs**;
  unit tests pin codec/physics correctness.
- The user personally eyeballs only what genuinely needs human eyes (visual
  parity, input feel) — and gets asked rarely, in batches.

---

## 1. Product decisions (fixed — from the user)

1. **From-scratch Rust client speaking the vanilla protocol.** Multiplayer
   only. No singleplayer, no worldgen, no world persistence.
2. **Performance = frame-time consistency (1%/0.1% lows) + input latency
   first.** RAM/VRAM/CPU are budget to spend, not conserve.
3. **Raw Vulkan via `ash`** — control, mesh-shader option, low-latency exts.
4. **Plugs into EwoClient** as a new `Native` instance kind, reusing the
   launcher's Microsoft auth and spawn machinery (§9).

Reference machine: RTX 5080 · Ryzen 7 9800X3D · 20 GB+ RAM budget · **500 Hz
OLED** (the monitor matters: at 500 Hz, present-mode choice is worth ~2 ms
max — the real latency wins are elsewhere, §8).

## 1.1 Protocol pin (confirmed against disk)

Pinned to what the launcher bundles today, read from
`%APPDATA%/EwoClient/shared/versions/26.2/26.2.jar!/version.json`:

| Field | Value |
|---|---|
| Version id | **26.2** |
| Protocol version | **776** |
| World version (data version) | 4903 |
| Resource / data pack formats | 88.0 / 107.1 |

Single-version client. No multi-version machinery — when the launcher moves
past 26.2, Rewo re-pins (the `rewo-data` pipeline in §5 + decompile-diff
workflow in §11 is what makes re-pinning cheap).

---

## 2. Corrections to the original design doc (each changed the plan)

1. **Block-state registry is NOT sent over the wire.** Configuration-phase
   Registry Data covers *dynamic* registries only (biomes, dimension types,
   damage types, chat types…). Blocks, block states, items, entity types,
   packet ids are **compile-time constants per version**. Consequence: Rewo
   needs a generated data layer (§5) built from Mojang's own data generator
   (`--reports` → `blocks.json`, `registries.json`, `packets.json`). The
   doc's "built from the registry data the server sends" is impossible.
2. **The assets/models workstream was missing entirely.** Textures, blockstate
   JSONs, and block-model JSONs live inside the client jar. Rendering
   "vanilla-style" requires parsing blockstate→model mappings (variants +
   multipart), resolving model parent chains, and baking per-state quad lists.
   Binary greedy meshing only applies to **full opaque cubes**; stairs, slabs,
   fences, torches, crops, snow layers etc. go through a baked-model quad path
   (no greedy merge). Fluids are their own geometry (neighbor-height corners).
   Animated textures (water/lava/fire strips + `.mcmeta` frame timing) need
   per-tick texture-array layer updates. None of this was in the doc; it is
   the single largest underestimated work item, and it lands in M4 with a
   full-cube-only stopgap in M2.
3. **AO needs 26 neighbors, not 6.** The doc said mesh jobs snapshot the
   section "plus a one-block border from its six neighbors." Face culling
   needs 6; vanilla-style AO samples *diagonal* neighbors at face corners, so
   the snapshot is an 18³ block volume touching up to **27 sections**.
4. **Own-player prediction runs at 20 Hz, not per-frame.** Vanilla ticks
   player physics at 20 Hz and interpolates position between ticks; only
   *look* (mouse rotation) applies per-frame. Running dt-scaled physics at
   render rate would desync from server expectations and read as speed-hack
   jitter to anti-cheat. Rewo copies vanilla: fixed 20 Hz client tick for
   movement/collision/packets, per-frame camera = interpolated position +
   immediate rotation. The honest latency story (§8): **aim latency is
   per-frame (the thing that matters); movement latency is tick-quantized,
   same as vanilla.**
5. **The `Native` launch path must NOT skip all downloads.** The doc said
   "no client.jar / libraries / assets." Wrong in a load-bearing way: Rewo
   *needs* the client jar (textures/models/data extraction) and the asset
   index (lang files; later sounds) — and can't ship them in the binary
   (Mojang EULA: extract from the user's own download, don't redistribute).
   The Native instance runs a **reduced download profile**: PerVersion →
   Client → AssetIndex → Assets, skipping Libraries/natives/JRE (§9.2).
6. **"Zero auth code" needs one asterisk.** Rewo receives the Minecraft
   token from the launcher and runs no OAuth/XSTS code — but the **per-connect
   sessionserver join POST** (`/session/minecraft/join` with token + undashed
   UUID + server hash) happens inside Rewo's encryption handshake and is
   Rewo's job. One `ureq` HTTPS call. (Launcher's `MinecraftAccount.uuid`
   is already stored undashed — exactly what the join call wants.)
7. **The protocol liveness contract was missing** — everything a modern
   (1.20.2+ shape, 26.x) client must *answer* or get kicked/throttled. Full
   checklist in §6.2: Keep Alive, teleport confirm, **chunk batch ack**
   (throttles chunk streaming if unanswered), Ping/Pong, config-phase
   re-entry mid-play, bundle delimiter, resource-pack response, cookie
   request/response + transfer, known-packs, tick-rate state, client brand.
8. **Chat signing / secure profiles were missing.** Vanilla servers default
   `enforce-secure-profile=true`; unsigned chat gets the client kicked on
   send. v1 targets servers with enforcement off (our own network — Paper,
   we control it); M7 implements the real thing (player certificates +
   signed messages + last-seen chain). Without this, "connect to real
   servers" quietly means "connect to permissive servers."
9. **"Be a real player" (M3) was underspecified.** Movement alone isn't
   playing. M3 gains: dig/place/attack with the 1.19+ **sequence-number
   block-change ack** (predict locally, roll back on server disagreement),
   hotbar + held item, minimal container/inventory clicks (stateId desync
   handling), chat send/receive with text-component flattening, death/respawn.
10. **Server-authoritative light has a visible v1 cost:** placing/breaking a
    torch relights only after the server round-trip (~1 tick + RTT). Accepted
    for v1, documented; a local incremental relight pass is a later track.
11. **Entity rendering scoped honestly:** v1 renders entities as capsules +
    floating nametags; the player model port (12 textured cuboids, slim/wide
    — geometry already known from `crates/ewo-jni/src/skin.rs`) is its own
    later work item, then mobs. Vehicles: visible + rideable as a passenger;
    *driving* (client-authoritative vehicle move packets) is post-v1.
12. **Mesh shaders stay an M8 A/B experiment, expectations lowered** —
    greedy-meshed quads are already few and fat; meshlet culling wins less
    here than in high-poly scenes. `ash` remains justified by control +
    `VK_NV_low_latency2` regardless.
13. **Texture atlas → texture array stands, with two adds:** mip generation
    must be alpha-coverage-preserving for cutout mips (distant leaves), and
    the compressed vertex needs a **per-vertex tint slot** (grass/foliage/
    water biome tint from colormaps + wire-registry biome effects) the doc's
    8-byte budget didn't include. Budget is 8–12 bytes/vertex; exact split
    decided in M4 with real data.

---

## 3. Missing workstreams now in scope

- **`rewo-data` — the vanilla data & asset pipeline (§5).** The fix for
  corrections #1/#2/#5. Biggest de-risk item in the whole plan.
- **Text components + font.** Chat/nametags/titles arrive as NBT/JSON text
  components. v1 flattens to styled runs (color/bold/italic) and renders with
  the **vanilla bitmap font** extracted from assets (glyph-atlas quads — no
  Skia dependency in the core client). Velvet-themed UI text comes later with
  the overlay track (§9.4).
- **Record/replay harness (M1).** Raw decrypted+decompressed packet stream
  (both directions, timestamped, config phase included so registries replay
  too). It is simultaneously: the deterministic perf benchmark for every
  1%-low comparison, the offline dev fixture (renderer work without a
  server), and a bug-repro format.
- **Sound: explicitly post-v1.** Asset index already gives us the files;
  `rodio` when it happens. Silence is acceptable for the experiment phase.
- **Server resource packs: policy decision** (§10, D8).

---

## 4. Architecture (locked)

Thread domains as in the design doc, with the tick correction applied:

- **Net thread** — owns socket + authoritative world. Decrypt (AES-128-CFB8),
  frame, inflate (zlib), decode, apply. Single writer.
- **Mesh pool** — `rayon`, ~physical cores. Input: 27-section snapshot
  handles; output: compact vertex blobs into recycled buffers.
- **Main thread** — winit event pump + **render/predict loop** (latency
  critical): drain raw input → (on tick boundary: 20 Hz physics + send
  movement packets) → per-frame camera (interp pos + live look) → cull →
  submit → present. Zero allocation, zero pipeline creation, zero descriptor
  churn per frame.
- **Upload path** — dedicated transfer queue + staging ring + timeline
  semaphores; a mesh not uploaded yet just isn't drawn this frame.

**World-state sharing: `Arc<Section>` copy-on-write.** Sections are
palette-packed immutable-ish blobs in a column map keyed by chunk pos (height
= dynamic, from the wire dimension registry: `min_y`/`height`). The net
thread applies a block change by cloning-and-mutating (or `Arc::make_mut` on
unique) and swapping the slot. Mesh jobs and render-thread collision queries
clone Arcs — snapshot-free consistency, no epochs, no locks held across a
frame. Entities: per-tick double-buffered snapshot table with interpolation
history (~100 ms render delay, vanilla-style).

Dirty meshing is a **coalesced set, not a queue**: a MultiBlockChange with 30
edits in one section produces one re-mesh, tagged with the tick, deduped
until a worker picks it up. Border-touching edits also dirty the neighbor
section(s).

Channels: `crossbeam` MPSC. No tokio/smol (repo convention holds for Rewo).

## 4.1 Crate layout (same workspace as the launcher)

Members of the existing `EwoClientV3` workspace — sharing the pinned
`winit 0.30` / `glam` / `flate2` / `ureq` / `sha1` / `zip` workspace deps:

```
crates/
  rewo-data/     # §5: datagen runner, jar asset extraction, baked caches
  rewo-proto/    # packet types + codec (VarInt, NBT, text components)
  rewo-net/      # socket, crypto, compression, state machine, session join
  rewo-world/    # sections, registries-at-runtime, entities, physics/prediction
  rewo-mesh/     # greedy mesher, model-quad path, AO/light bake, vertex pack
  rewo-gpu/      # ash: device, swapchain, allocator, pipelines, culling   [M0 ✅]
  rewo-render/   # frame graph/passes once they outgrow rewo-app (M2+)
  rewo-app/      # binary `rewo`: winit, config, env handoff, metrics, glue [M0 ✅]
```

Naming caveat: `rewo-*` sits one letter from `ewo-*` (`rewo-render` vs
`ewo-render`) — inherent to the chosen name; be careful in import lines.

No `rewo-auth` crate — the launcher owns token acquisition (§9.1); the one
sessionserver join call lives in `rewo-net`. Metrics live in `rewo-app`
(+ `tracy-client` everywhere) until they earn a crate.

Third-party deps (M0 actuals): `ash 0.38`, `ash-window 0.13`,
`gpu-allocator 0.28`, `tracy-client 0.18`, `png 0.18` (headless harness).
Coming with M1+: `rayon`, `crossbeam-channel`, RustCrypto (`aes` +
`cfb-mode`, `rsa`). Crypto is RustCrypto, not openssl.

---

## 5. `rewo-data` — vanilla data & asset pipeline

Runs **once per (Rewo build, MC version)**, at instance setup or first
launch; everything caches under `<config>/EwoClient/rewo/<version>/`.

1. **Reports generation.** Run Mojang's data generator for the pinned
   version: `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar
   --reports`. Produces `blocks.json` (every block-state id ↔ properties),
   `registries.json` (items, entity types, particles, packets…). The
   launcher already bundles JREs (Temurin auto-fetch) and the per-version
   manifest carries the server-jar URL (add the `downloads.server` field to
   `PerVersion` parsing if absent — client-jar-only datagen is the fallback
   if the server bundler path is awkward).
2. **Asset extraction** from the already-downloaded client jar (`zip` crate):
   block/item textures, `blockstates/*.json`, `models/**.json`, font bitmaps
   (`ascii.png` / unifont), colormaps (`grass.png`, `foliage.png`), plus
   `en_us.json` from the asset-index objects (item/block display names).
3. **Bake step** → Rewo-native binary caches (versioned by a schema const):
   - `states.bin` — state id → {opaque, full-cube?, render layer, AO flag,
     emission, cullface behavior, model ref, tint kind}.
   - `models.bin` — resolved model parent chains → per-state quad lists
     (pos/uv/cullface/rotation/tintindex), variants + multipart applied.
   - `atlas.bin` + texture-array source — deduped texture list, animation
     metadata (frame strips + frametime), array layer assignments.
   - `font.bin` — glyph atlas + advances.
4. **Legality note:** nothing from the jar ships inside Rewo's binary or
   repo. Extraction happens on the user's machine from the user's own Mojang
   download — same model as every third-party launcher.

This crate also doubles as the **version-churn absorber**: re-pinning to a
new MC version = rerun datagen + rebake + fix whatever the protocol diff
demands (§11 workflow).

---

## 6. Protocol layer

### 6.1 Connection state machine

`Handshake → Login (encryption + compression) → Configuration → Play`, as in
the design doc — with the addition that **Play can re-enter Configuration**
(server-driven, e.g. datapack reload) and both directions must be handled.

Login encryption (online mode): RSA-encrypt the 16-byte shared secret +
verify token to the server's DER public key; server hash = Minecraft's
signed-hex SHA-1 over (server id ‖ secret ‖ public key); POST the join to
sessionserver with the launcher-provided token; enable AES-128-CFB8 both
ways (key = IV = secret). Compression: Set Compression threshold during
login; below-threshold packets ride uncompressed with a 0 marker.

### 6.2 Liveness contract (answer these or die)

The checklist the doc lacked — M1's soak test is literally this table:

| Server sends | Client must | Or else |
|---|---|---|
| Keep Alive (config + play) | echo id promptly | kicked (~30 s) |
| Synchronize Player Position | Confirm Teleportation (id) + snap | movement ignored, rubber-band loop |
| Chunk Batch Start/Finished | Chunk Batch Received (desired rate) | chunk streaming throttles to a crawl |
| Ping (play) | Pong | tick-sync tools break |
| Resource Pack Add/Remove | status responses (see D8) | kick if pack `required` |
| Cookie Request | Cookie Response (empty ok) | join stalls (also across Transfer) |
| Transfer | reconnect to new host (or clean error) | dead session |
| Select Known Packs (config) | respond (empty = send me everything) | config stalls |
| Bundle Delimiter | buffer + apply bundle atomically | entity pop-in artifacts |
| Set Ticking State / tick rate | scale client tick (`/tick rate` exists) | desync on modified servers |
| — (client-initiated) | `minecraft:brand` plugin message after login | many servers log/flag its absence |

Movement send cadence (per tick: Pos / PosRot / Rot / OnGround variant
selection + the periodic idle resend + the 1.21.2+ Player Input packet) is
**derived from decompiled vanilla, not guessed** — anti-cheats fingerprint
the pattern (§11).

### 6.3 Codec

Typed packet structs + derive-style codec over a scratch-buffer reader
(allocation-light; strings/NBT into arenas). Packet-id ↔ struct tables are
generated per state/direction from `packets.json` (datagen report) where
possible, hand-written where not. NBT: own minimal reader (network NBT is
unnamed-root since 1.20.2), plus the text-component subset.

### 6.4 Record/replay

Frame every decrypted+decompressed packet with `{dir, t_us, len}` into a
session file, config phase included. Replay drives `rewo-world` +
`rewo-mesh` + rendering with no socket — deterministic scene for frame-time
histogram A/Bs, and the fixture all renderer work runs against.

---

## 7. Renderer

Vulkan 1.3 via `ash` — dynamic rendering, timeline semaphores, descriptor
indexing/bindless, BDA, drawIndirectCount, `VK_NV_low_latency2`;
`gpu-allocator` for memory. All pipelines compiled at startup + pipeline
cache; shaders GLSL → SPIR-V via `glslc` at build time (D4; revisit Slang
if/when mesh shaders).

The staged path (matches milestones):

- **M2 baseline:** per-section vertex buffers, one draw per section, CPU
  frustum cull, full-cube geometry only, texture array + mips.
- **M5 target:** mega-buffer arena (free-list suballocation, regions
  recycled on unload) + per-section metadata SSBO + **compute cull →
  drawIndirectCount** + visibility-graph ("cave") culling flood-filled from
  the camera section + dedicated-transfer-queue uploads. HZB occlusion is
  M8, only after frustum+visgraph are measured.
- **Passes:** opaque → cutout → translucent (per-section back-to-front CPU
  sort; intra-section artifacts accepted v1) → entities (instanced capsules
  v1) → particles (minimal v1) → sky (gradient + sun/moon quads) → HUD
  (sprite/text batch from vanilla assets) → metrics overlay.
- **Vertex format:** packed 8–12 bytes (section-local pos+extent, normal,
  texture-array layer, AO, baked sky+block light, tint index, UV corner) —
  exact bit split locked in M4 against real content.
- **Textures:** one (or few, bucketed by size) texture array(s); animated
  strips update their layer per tick; alpha-coverage-preserving mips for
  cutout.
- **Lighting:** baked at mesh time from server light arrays (per-vertex
  4-sample smooth light + AO darkening, vanilla-look). No runtime light
  engine v1 (correction #10 caveat applies).

**Render disciplines established in M0 (apply everywhere):**
- SRGB attachments encode on store — **shader color constants authored in
  sRGB must be converted to linear in the shader** (see
  `overlay.frag::srgb_to_linear`), and clear colors passed pre-linearized.
  Writing sRGB values raw double-encodes and washes everything out.
- UI/overlay passes **mask alpha writes** (`color_write_mask = RGB`) so the
  attachment keeps alpha=1 — otherwise readback PNGs ghost against the
  viewer background and alpha-honoring present paths would misbehave.

---

## 8. Latency & frame-consistency model (honest version)

- **Aim (mouse look):** raw input drained immediately before frame build →
  camera matrix → render → present. This is the un-pipelined path we
  optimize: ≤2 frames in flight (tunable to 1), MAILBOX default / IMMEDIATE
  option, `VK_NV_low_latency2` submission pacing, late input sampling. At
  500 Hz the present-mode delta is small; queue depth + GPU time dominate.
- **Movement keys:** consumed by the next 20 Hz tick — identical to vanilla,
  cannot be "faster" without desyncing (correction #4). What Rewo removes
  vs. the JVM client is *jitter* on top of that floor: no GC, no allocation
  spikes, bounded per-frame work.
- **World updates:** 20 Hz server truth, hidden by prediction + entity
  interpolation. Fast decode just keeps us from falling behind the stream.
- **Hitch elimination checklist** (designed-in from M0, verified by the
  replay benchmark each milestone): async uploads off the graphics queue,
  startup-compiled pipelines, no frame allocation, bindless (no per-draw
  descriptor churn), bounded chunks-uploaded/meshes-applied per frame
  (teleport storms amortize), stable swapchain handling (no-op resizes
  skipped; forced recreate path for OUT_OF_DATE), thread pinning if
  profiling demands.
- **Merge gate:** captured frame-time histogram on the benchmark replay.
  A change that raises average FPS but worsens 0.1% lows is a regression.
  Click-to-photon instrumented in M6.

---

## 9. EwoClient integration (verified against the live repo, 2026-07-21)

### 9.1 Auth handoff

`MinecraftAccount` ([auth/mod.rs:51](crates/ewo-launcher/src/auth/mod.rs#L51))
holds `name`, `uuid` (32 hex, undashed — join-call-ready), `minecraft_token`
(serde-skipped, re-derived from the MS refresh token at launcher startup, so
freshness is already the launcher's job). Rewo receives via **env vars at
spawn** (matches the `EWO_LOADER_TOKEN` precedent): `REWO_ACCESS_TOKEN`,
`REWO_UUID`, `REWO_USERNAME`, `REWO_SERVER` (host:port), `REWO_DATA_DIR`.
Threat-model honesty: a same-user process can read another process's env on
Windows anyway; env vars buy "not in argv / process listings / logs," which
is the realistic bar. No tokens on the command line, ever. Token expiry
mid-session = reconnect concern; Rewo never refreshes.

### 9.2 `Native` instance kind

- `InstanceLoader` gains a third variant, `Native`, next to `Vanilla` /
  `Ewo { manifest_url }` (enum lives render-side in
  `ewo_render::screens::instances`; serde round-trip tests at
  [persistence.rs:144](crates/ewo-launcher/src/persistence.rs#L144)).
- **Downloads:** the Phase-B job runs a *reduced profile* for Native
  (correction #5): PerVersion → Client → AssetIndex → Assets; skip the
  Libraries stage, natives extraction, and JRE resolution. Instance flips
  `Ready` when the jar + assets are down, then `rewo-data` bakes on first
  launch (progress surfaced on the launching screen).
- **Launch dispatch:** new early arm in `try_real_launch`
  ([main.rs:448](crates/ewo-launcher/src/main.rs#L448), loader match at
  [main.rs:499](crates/ewo-launcher/src/main.rs#L499)) that bypasses the
  whole JVM pipeline (merge, ensure_libraries, extract_all, pick_jre) and
  spawns `rewo.exe` with the env contract. Ewo-mod plumbing
  (`overlay_mods::*`, mod seeding/sync) is skipped for Native instances.
  Server address rides the existing H6 `start_launch(idx, server, time)`
  plumbing — the main-menu server widget and friend-Join work for Rewo
  instances for free.
- **Spawn/exit:** reuse `launch/spawn.rs`'s `LaunchEvent` model unchanged
  (generalize `LaunchPlan.jvm_path` → program path); Rewo's stdout streams
  into the Velvet launching screen like the JVM's does. `launch/reaper.rs`
  records the PID the same way — extend its image-name guard
  (`java.exe`/`javaw.exe`) to include `rewo.exe` so a hung Rewo is reapable
  (it shouldn't hang — no loader-lock/JNI teardown hazards — but the
  backstop is one string).
- **UI:** new-instance modal's Loader dropdown gains "Native · Rewo
  (experimental)"; instance meta renders `NATIVE · 26.2`; download-status UI
  shows the reduced pipeline's stages.

### 9.3 What's reused vs. reimplemented

| Existing | In Rewo |
|---|---|
| `ewo-launcher/auth` | reused as-is (launcher-side; env handoff) |
| `launch/spawn.rs` + `reaper.rs` | reused (program-path generalization + image-name string) |
| Downloads pipeline | reused with reduced stage profile |
| `ewo-core` (theme/easing) | reused when the Velvet overlay lands (§9.4) |
| `ewo-render` (Skia widgets) | overlay track only — **not** in the core client |
| `ewo-jni` HUD/mixins | does not transfer (no JVM); module *effects* become native code |
| `skin.rs` cuboid geometry | reference for the player-model port |

### 9.4 Velvet overlay track (deferred, not load-bearing)

Core-client HUD (hotbar/hearts/chat/nametags) uses **vanilla assets +
bitmap font** — zero cross-tech risk, authentic look. The Velvet
dashboard/menus (Skia on Vulkan: `skia-safe` with the `vulkan` feature
wrapping Rewo's device/queue) is a separate later milestone. Check item
before committing: today's workspace pins `skia-safe 0.78 = ["gl",
"textlayout", "d3d"]` — confirm a prebuilt exists for the `vulkan` feature
combo on `x86_64-pc-windows-msvc`, else that track eats an LLVM source
build. Fallback if interop misbehaves: Skia renders UI to an image, Rewo
samples it as a texture (one copy, still fine at UI resolutions).

---

## 10. Decisions

Resolved:

- **D2** Protocol pin 26.2/776, single-version (§1.1).
- **D3** `winit 0.30` + raw device events; a raw Win32 input layer only if
  measurements demand it.
- **D4** ✅ **GLSL → glslc, confirmed**: Vulkan SDK 1.3.296 is installed on
  the dev box (`C:\VulkanSDK\1.3.296.0`, `VULKAN_SDK` set); `rewo-gpu`'s
  build.rs invokes it. Validation layers available and used in debug builds.
  Slang revisited with mesh shaders.
- **D5** No async runtime; threads + crossbeam + rayon.
- **D6** `gpu-allocator` (0.28); **D7** RustCrypto (no openssl).
- **D9** Chat: receive always; send unsigned until M7 signing lands.
- **D10** Sound post-v1. **D12** Same Cargo workspace, `crates/rewo-*`.
- **D13** Env-var token handoff (§9.1), `REWO_*` names.
- **D14** Vanilla-asset HUD first; Skia-Vulkan Velvet overlay later (§9.4).
- **D11** ✅ **Name: Rewo** (user, 2026-07-21). Binary `rewo.exe`, crates
  `rewo-*`, env `REWO_*`.

**DECISION (user) — D1: target-server policy.** Plan assumes: local offline
Paper for dev → **the user's own network, Frogsy** (ex-chickenedin; renamed
2026-07 — old `chickenedin.com` endpoints may still be live, verify before
relying on them) for staging (user controls anti-cheat +
`enforce-secure-profile`; can whitelist the account) → public servers
explicitly out of scope until the user says otherwise. A from-scratch client
on anti-cheat'd public servers risks bans on the *account* (CatPvP precedent
— and that was fingerprinting, not behavior). Related knob: the
`minecraft:brand` string — honest `"rewo"` on own infra; anything else is
the user's call and risk.

**DECISION (user) — D8: server resource packs.** Options when a server
pushes a pack: (a) honest **decline** — refuses entry to `required`-pack
servers (default in plan); (b) **claim accepted/loaded without applying** —
maximizes compatibility, misrepresents client state; (c) actually download +
hot-apply — real feature, big scope, post-v1 at best. Plan ships (a) with a
per-server override to (b). Say if you want a different default.

---

## 11. Version churn + ground truth workflow

The 26.x jars on disk are **Mojmap-named** (established in Phase E). Ground
truth for packet layouts, movement constants, and send cadence is the
decompiled bundled jar, not community docs: Vineflower over
`shared/versions/26.2/26.2.jar` (JDK 25 already at
`%APPDATA%/EwoClient/jdks/temurin-25/`), kept as a local reference tree.
Re-pinning to a future version = rerun `rewo-data`, diff the decompiled
trees, patch the codec/physics deltas. Community protocol docs (former
wiki.vg) are the map; the decompile is the territory.

---

## 12. Milestones (each shippable, each measured, each headlessly checkable)

Sizes: S ≈ a session, M ≈ a few, L ≈ many, XL = the grind.

- **M0 — Skeleton + metrics.** ✅ **SHIPPED 2026-07-21** — see §15.
- **M1 — Protocol foundation + launcher handoff.** (XL, biggest de-risk)
  `rewo-data` end-to-end (datagen reports + asset extraction + baked
  caches); codec core (VarInt/NBT/text components, framing, zlib, AES);
  full Handshake→Login→Config→Play against **local offline Paper 26.2**
  (Claude sets up + runs the server headlessly); liveness subset (keepalive,
  brand, known packs, chunk-batch ack, teleport confirm, cookie stub);
  chunk + light decode into sections; entity tables; **record/replay**;
  `Native` instance kind + env handoff + spawn/reaper wiring so every later
  milestone launches from the real launcher UI.
  DoD: 10-minute soak without kick; replay re-decodes bit-identically;
  world queries correct vs known coordinates.
- **M2 — First pixels.** (M) Texture array + mips (+ animation ticking),
  full-cube face-culled mesher, per-section draws, CPU frustum cull, fly
  camera, depth. DoD: recognizable world at RD 16 on the replay + live
  server; headless PNG of a known scene; loading a fresh area doesn't spike
  the strip chart.
- **M3 — Be a player.** (XL) 20 Hz tick + prediction (walk/sprint/sneak/
  jump/basic swim) with constants from the decompile; collision; movement
  packet cadence parity; server-correction snap + teleport confirm; entity
  interpolation (capsules + nametags); **dig/place/attack with sequence-ack
  predict/rollback**; hotbar + held-item; minimal inventory + container
  clicks (stateId handling); chat send/receive (flattened components, bitmap
  font); death/respawn. DoD: a 30-minute survival session on the Frogsy
  staging server — mine, build, fight, chat — no kick, corrections rare.
- **M4 — Real meshing.** (L/XL) Binary greedy for full cubes + **baked-model
  quad path** for everything else, fluids, 26-neighbor AO + smooth light,
  packed vertices, biome tint, cutout layer, per-tick dirty coalescing,
  rayon pool + per-frame apply budget. DoD: side-by-side screenshots vs
  vanilla near-indistinguishable on a survival base scene (headless PNG on
  a replay fixture); meshing throughput + remesh p99 recorded.
- **M5 — GPU-driven.** (L) Mega-buffer arena + free-list, metadata SSBO,
  compute cull → drawIndirectCount, visibility-graph culling, dedicated
  transfer queue + staging ring + timeline semaphores, translucent sort.
  DoD: RD 32 with CPU submit cost flat vs. chunk count; teleport-storm test
  stays under frame budget; culled-count counters live.
- **M6 — Latency + lows pass.** (M) `VK_NV_low_latency2`, frames-in-flight
  tuning (2→1 experiments), late-sample audit, click-to-photon
  instrumentation, replay-benchmark histogram regression gate wired into the
  dev loop. DoD: documented 1%/0.1% lows + latency numbers on the benchmark;
  this is where goals #1/#2 are demonstrated, not asserted.
- **M7 — Online-mode + hardening.** (M/L) Encryption + sessionserver join
  with the launcher token (live on Frogsy); **chat signing** (player
  certificates + signed chain) for `enforce-secure-profile` servers;
  resource-pack policy (D8) implemented; config re-entry, transfer/cookies,
  reconnect UX; per-frame work caps re-audited under real network jitter.
  DoD: full session on an online-mode server with enforcement on.
- **M8 — Advanced (optional, A/B'd).** HZB occlusion; mesh-shader path vs
  M5 baseline; Velvet overlay (Skia-Vulkan, §9.4); player/mob model +
  animation port; local relight; shader/PBR track. Each lands only if the
  replay benchmark says it pays.

Sequencing note: launcher integration lands **in M1**, so all subsequent
work is exercised through the real launcher with a real account.

---

## 13. Risks

- **Anti-cheat / account bans** on non-owned servers — mitigated by D1
  (Frogsy staging) and decompile-derived packet cadence; never fully gone.
  The user's CatPvP ban history says treat this as real.
- **Protocol churn** every MC release — absorbed by `rewo-data` +
  decompile-diff (§11); still a recurring maintenance tax on the pin.
- **Movement-parity grind** (M3) — the constants are documented in decompiled
  source, but edge cases (stairs vs sneak, water exits, cobweb…) will eat
  sessions. Scope fence: on-foot survival subset v1.
- **skia-safe `vulkan` prebuilt availability** for the overlay track —
  checked before that milestone starts; fallback path defined (§9.4).
- **Entity/model scope creep** — fenced to capsules v1 (correction #11).
- **Stamina** — the honest risk. Counter-structure: every milestone is
  playable, launcher-launched, and measured; M2 already beats where most
  from-scratch clients die.

## 14. References

- Decompiled bundled 26.2 jar (Mojmap) — packet/physics ground truth (§11).
- Community protocol docs (former wiki.vg, now minecraft.wiki) for 776.
- **azalea** (Rust protocol + auth semantics), **Minosoft** (proof of
  finishability), Sodium writeups (visibility graph, meshing), 0fps binary
  greedy meshing articles, Vulkan samples for drawIndirectCount / dynamic
  rendering / `VK_NV_low_latency2` / `VK_EXT_mesh_shader`.

---

## 15. Status log

**2026-07-21 — plan v1 + M0 shipped.**

- Plan drafted (as FERRIC_PLAN.md), renamed to Rewo per user; network
  staging target renamed chickenedin → **Frogsy** per user.
- **M0 built + verified headlessly** on the reference machine:
  - `crates/rewo-gpu` — ash 0.38 bootstrap (instance w/ optional validation,
    RTX 5080 picked, Vulkan 1.3 features dynamic_rendering + sync2), MAILBOX
    swapchain (B8G8R8A8_SRGB, 3 images, no-op-resize guard + forced
    OUT_OF_DATE recreate path), frame driver with 2 frames in flight,
    per-frame GPU timestamps, overlay pipeline (fullscreen tri + SSBO ring,
    Velvet budget colors), offscreen target + PNG readback.
  - `crates/rewo-app` — binary `rewo`: winit 0.30 windowed loop with tracy
    zones/frame marks, `--run-seconds` auto-exit soak, `--headless N
    [--chart-demo] --out x.png` no-window verification, end-of-run stats
    block (avg/p50/p99/p99.9/max cpu ms + gpu ms), nearest-rank percentiles
    (unit-tested).
  - **Verified:** validation layers ON in debug headless runs — zero
    messages; headless PNG inspected (chart bars/gridlines/colors correct);
    release windowed soak: **MAILBOX, 21,424 frames / 5 s ≈ 4,284 fps, cpu
    p99 0.87 ms, p99.9 2.24 ms, max 3.35 ms, gpu avg 9 µs** on clear +
    overlay. Clean exit, no teardown hang.
  - Two render disciplines locked (§7): sRGB→linear shader constants;
    alpha-masked UI writes.
- Next: **M1** (rewo-data datagen + codec + local offline Paper 26.2 soak +
  record/replay + `Native` instance wiring).

**2026-07-21 — M1 protocol foundation shipped + headlessly verified.**

Connected to a real vanilla 26.2 server, walked the full protocol, decoded
the world, and proved replay equivalence — all without a human at a window.

- **Ground truth captured.** Decompiled the bundled `26.2.jar` with
  Vineflower 1.12 (Mojmap source at `<data>/rewo/26.2/decompiled/`, 39.7k
  log lines) and ran Mojang's data generator off `server.jar`
  (`--reports` → `blocks.json` 32,366 states, `packets.json`,
  `registries.json`) under `<data>/rewo/26.2/datagen/`. Every wire layout
  below was read from that source, not guessed.
- **`crates/rewo-proto`** — VarInt/VarLong (canonical-vector tested),
  primitive reader/writer, packed Position, network NBT (unnamed root,
  hostile-length-guarded), frame codec with zlib compression. 11 unit tests.
- **`crates/rewo-data`** — parses the datagen reports into runtime tables:
  `blocks` (state id → name, air=0 assertion, 15-bit global palette derived),
  `packets` (id ↔ name by (state,dir), resolved by *name* at connect so a
  version bump can't silently misfire — REWO_PLAN §11), server-jar
  ensure/sha1.
- **`crates/rewo-world`** — paletted container decode (single/indirect/
  direct, **fixed-size long array, no length prefix** — the format detail
  that had to be exactly right), 16³ sections with **two leading shorts
  (non-empty + fluid count)**, dimension shape from the wire
  `dimension_type` registry NBT (`min_y`/`height`), block-light/sky-light
  nibble arrays distributed by Y-mask, entity table, `block_state_at` query,
  commutative FNV world digest. Palette round-trip unit tests
  (single/indirect/direct).
- **`crates/rewo-net`** — the connection state machine:
  Handshake→Login(offline)→Configuration→Play with zlib compression,
  the liveness contract (keep-alive, ping/pong, teleport confirm,
  chunk-batch ack, known-packs empty-reply, brand + client-information,
  cookie stubs, config re-entry), chunk/block-update/add-entity/
  forget-chunk decode into `rewo-world`, and a packet **recorder + replay
  driver**. Packet ids resolved by name via `ids::Ids` (fails loud on a
  missing required packet).
- **`rewo net` subcommand** (`crates/rewo-app/src/net_cmd.rs`): `soak`
  (connect N seconds, decode, stats + digest, optional record + block
  query) and `replay` (re-decode a recording, compare digest).

  **Verified against a live vanilla 26.2 offline flat-world server** (set up
  + run headlessly by Claude on `127.0.0.1:25599`):
  - Reached Play, answered keep-alive + teleport, took **8k+ play packets /
    2.5 MB** over a 20 s soak.
  - **329 chunks decoded, zero decode failures.**
  - Block queries hit the flat-world layers exactly: `(0,-64,0)=bedrock`,
    `(5,-62,5)=dirt`, `(0,-61,0)=grass_block`, `(8,100,8)=air`.
  - **Replay equivalence:** replaying the recording reproduced the live
    world digest `0x194468be04d129e8` bit-for-bit (329 chunks both sides).
  - **10-minute endurance soak (the DoD's "no kick"):** stayed in Play the
    full 600 s — **237,678 packets / 5.7 MB, 39 keep-alives answered**
    (server sends one ~every 15 s and kicks after ~30 s of silence, so 39
    over 10 min = zero missed), exit 0, block query still correct.
  - 18 unit tests green across the M0+M1 crates.

- **How to reproduce the ground-truth + server** (one-time per version):
  ```
  # decompile (optional, for layouts): vineflower.jar over 26.2.jar
  # datagen reports:
  java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports
  # test server: server.properties online-mode=false, level-type=flat,
  #   enforce-secure-profile=false, server-port=25599; eula=true; java -jar
  ```
  Artifacts live under `%APPDATA%/EwoClient/rewo/26.2/` (git-ignored — they
  derive from the user's own Mojang download; see the EULA note in §5).

- **Carried to a follow-on (not blocking M2):**
  - **`Native` instance wiring in the launcher is NOT done yet** — the plan
    put it in M1, but the protocol core was the load-bearing risk and took
    priority. `rewo net` is driven directly for now; the `InstanceLoader::
    Native` arm + env handoff + reduced-download profile is the first M2-adjacent
    task so later milestones launch from the real UI.
  - Biome container uses a placeholder global-bit width (single-biome flat
    world never exercises direct biome palettes); registry-derived biome
    bits lands with M4 tint.
  - Datagen is run out-of-band; wiring it into Native-instance first-launch
    is part of the `rewo-data` §5 followon.
- Next: **M2** (first pixels) — `Native` launcher arm + a full-cube
  face-culled mesher fed by the replay fixture + live server, texture array,
  fly camera, headless PNG of a known scene.

**2026-07-21 — M2 first pixels shipped + headlessly verified.**

A recognizable Minecraft world renders from both the M1 replay fixture and a
live vanilla 26.2 server — verified by inspecting headless PNGs, no human at
a window.

- **Asset bake** (`rewo-data/src/assets.rs`): client.jar → texture-array
  layers + per-state render table. Minimal by design: blockstate variants →
  model parent chains → cube-family / first-full-size-element faces (that
  rule is what makes grass_block work — it's element-defined, not cube_all).
  **2,649 cube states across 460 blocks, 542 textures**; everything else is
  Invisible until M4's real model baker. Gotcha caught by the bake sanity
  assertions: vanilla textures are mostly **palette-indexed PNGs** —
  without `Transformations::normalize_to_color8()` only 39 blocks resolved.
  Grass-top tint is a hardcoded plains-green multiply (M4 does colormaps).
- **Mesher** (`crates/rewo-mesh`): face-culled full cubes, classic per-face
  shade × the neighbor cell's light (server light data finally visible).
  329 columns → **695k vertices in ~21 ms** single-threaded. Face-table
  orientation unit tests (texture-top-at-block-top on side faces).
- **GPU world pass** (`rewo-gpu/src/world.rs`): R8G8B8A8_SRGB texture array
  with CPU box-filter mips, NEAREST/mip-LINEAR sampler (vanilla look), D32
  depth target, two-pass frame (world clears color+depth → overlay loads),
  CPU frustum culling (Gribb–Hartmann, 219 of 329 columns culled in the DoD
  shot), negative-viewport Y-flip, per-column host-visible buffers (the
  device-local mega-buffer arena stays M5; cull-mode NONE until winding is
  regression-tested).
- **`rewo view`**: snapshot viewer — world from `--replay FILE` or a live
  `--host/--port` fetch → bake → mesh → render. `--out x.png` headless
  (the DoD artifact) or windowed WASD/mouse/Space/Ctrl fly camera with
  cursor grab + `--run-seconds` soak.
- **Launcher `Native` arm (carried M1 item, done):** `InstanceLoader::
  Native` (+ TOML round-trip test), "Native · Rewo" in the new-instance
  modal, `try_real_launch` early arm that skips the whole JVM pipeline and
  spawns `rewo.exe` (found next to the launcher exe — covers dev target dir
  + dist) with the `REWO_*` env contract; when a server join is active it
  passes `view --host … --port …` so Launch opens a native fly-over of that
  server. `spawn_native` shares the JVM path's supervision/log-streaming
  (`run_child` refactor); the reaper's image-name guard accepts `rewo.exe`.

  **Verified headlessly:**
  - Replay PNG + live-server PNG both inspected: grass plain with per-block
    texture detail, sky, chunk-edge silhouette at the loaded boundary,
    overlay chart composited. 110 columns drawn / 219 culled.
  - Windowed release soak (5 s auto-exit): **~1,000 fps, cpu p99 2.16 ms,
    max 5.72 ms**, clean exit.
  - 21 Rewo tests + 37 launcher tests green; debug validation layers clean.

- **Deferred, tracked:** texture animation ticking (no animated texture in
  the M2 scene to prove it against — lands with fluids in M4); biome
  tint/colormaps (M4); streaming-while-rendering + the chart-spike-on-load
  criterion (needs the M3 live loop; M2 is snapshot-only by design);
  reduced Native download profile (uses the vanilla profile — the extra
  libraries are shared with vanilla instances and cost nothing on this
  box); `package.ps1` doesn't yet copy `rewo.exe` into dist; the modal's
  Native option + Launch flow not yet eyeballed in the UI (machine-tested
  at the unit/spawn level only).
- Next: **M3** (be a player) — 20 Hz tick + prediction, movement packets,
  dig/place/attack with sequence-ack, hotbar, chat, live streaming into the
  renderer.

**2026-07-21 — M3 be a player shipped + headlessly verified.**

A headless bot connects, spawns, walks/sprints/jumps, looks, places, mines,
and chats on the live vanilla 26.2 server — and the physics-parity meter
reads **zero server corrections over 3,000 ticks** of continuous movement.

- **Vanilla physics port** (`rewo-world/src/physics.rs`): a faithful 20 Hz
  tick from the decompiled `LivingEntity.travel` / `Entity.move` /
  `LocalPlayer` — gravity 0.08, drag 0.91×0.98, ground accel
  `speed·0.216/friction³`, air accel 0.02/0.026, jump 0.42 (+0.2 forward on
  sprint-jump), input×0.98, sneak×0.3. Axis-separated AABB collision (Y→X→Z)
  with 0.6 step-up. **Unit tests lock walk (≈8.63 blk/2s), sprint (≈11.2),
  and jump apex (≈1.2522) to vanilla values**, plus wall-stop, step-up-fails
  / jump-succeeds on a full block.
- **Live play session** (`rewo-net/src/play.rs`): `Connection::into_play`
  does login+config synchronously, then splits the socket — a reader thread
  decodes frames into a channel; `PlaySession::tick` drains inbound, runs
  physics, and sends movement with the **exact decompiled
  `LocalPlayer.sendPosition` cadence** (Pos / PosRot / Rot / StatusOnly +
  20-tick reminder + `player_input` on change + `client_tick_end`). Handles
  teleport-correct (relative-bit aware, echoes accepted position), keep-
  alive/ping, live chunk/block-update/entity, set-health→respawn, and both
  system + player chat decode.
- **Gameplay verbs**: chat (unsigned — M7 signs), creative hotbar set +
  select, dig (`player_action` START + swing), place (`use_item_on` with a
  block-hit-result + click offset + sequence), attack (`interact` ATTACK).
  Sequence numbers on dig/place feed the server's block-changed-ack.
- **`rewo-data` items** (`items.rs`): parses the `minecraft:item` registry
  from registries.json (1,537 items) so the bot can hold real item ids.
- **`rewo play`** (`rewo-app/src/play_cmd.rs`): the headless DoD harness —
  scripted session on a real-time 20 Hz clock (settle → walk → sprint →
  jump → look → give+place → dig → chat → continuous wander), reporting the
  corrections meter + place/dig world-state verification + chat-received.

  **Verified against the live server:**
  - `RewoBot logged in … joined the game … <RewoBot> rewo bot online … left`
    — clean join/chat-broadcast/clean-disconnect, **no "moved too
    quickly/wrongly" warnings** (independent corroboration of 0 corrections).
  - Movement: **0 corrections over a 150 s / 3,000-tick continuous-wander
    run** (walk + curve + sprint-jump across fresh chunks).
  - Build: place → `block_update (…) = 10` (dirt) echoed + world query reads
    dirt ✓. Dig: `block_update (…) = 0` (air) echoed + query reads air ✓.
  - Chat send→receive round-trip ✓.
  - 73 workspace tests green.
  - **Bug found + fixed in the process**: `Column::block_state_at` ignored
    the `overrides` map that `set_block` writes, so queries returned stale
    chunk-snapshot state even though the block_update *was* applied — the
    server had done the right thing all along. Now the query is
    override-aware (the mesher benefits too: edits show on remesh).
- **Not exercised (honest):** "fight" — `attack_entity` is implemented but
  the flat creative test world has no mobs; it sends on a valid entity id
  but hasn't been hit against a live target. Prediction is **server-
  authoritative-apply**, not client-predict-with-rollback yet: we run
  physics locally and *reconcile* on the server's teleport corrections
  (which stay at 0), but don't yet pre-apply block changes before the ack —
  fine while corrections are rare; full predict/rollback is a refinement.
- **The live windowed client shipped too** (`rewo live`, `rewo-app/src/
  live_cmd.rs`) — the M3 capstone. The M1 protocol + M3 physics session feed
  the M2 renderer in **one single-threaded loop** (the socket reader is
  already its own thread): a 50 ms accumulator drives the 20 Hz tick with
  WASD/mouse input, every frame renders from the player's eye
  (`eye_y = y + 1.62`, MC-convention yaw/pitch camera). Live re-meshing uses
  a **per-frame budget** (`REMESH_BUDGET = 6`, nearest-column-first) fed by a
  `PlaySession` dirty set (block edits + new chunks mark the 3×3
  neighborhood stale; forgotten columns free their buffers) so the initial
  ~329-chunk flood amortizes instead of hitching. A `wait_idle` guards the
  host-visible column-buffer swap on any frame that re-uploaded (the
  device-local arena that removes the stall is M5).
  - Headless-verified: `rewo live --out PNG` pumps the session to spawn +
    settle, meshes everything, and renders the eye view — **inspected: a
    proper first-person shot standing on the flat world at the real spawn
    (7.5,-60,8.5)**, sky + grass + a persisted dirt block from a prior bot
    run visible on the horizon (real mutated server state).
  - Windowed release soak (`--run-seconds 6`): **~988 fps, cpu p99 2.18 ms,
    0 corrections**, 329 columns, tick loop running concurrently.
- Next: **M4** (real meshing) — binary greedy + model-quad path, fluids,
  26-neighbor AO, biome tint (colormaps), packed 8–12 B vertices. The live
  client makes M4's visual gains directly inspectable.
