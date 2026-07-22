# REWO_PLAN.md — Rewo: the from-scratch native Minecraft client

**Rewo** (from "rewolution", as Ewo came from "ewolution") is a from-scratch
Rust Minecraft: Java Edition client speaking the vanilla protocol, rendered
with raw Vulkan. This file is the plan of record. It supersedes both the
hand-off design doc (`~/Downloads/rust-mc-client-design.md`, drafted under
codename "Ferric") and the interim `FERRIC_PLAN.md` (deleted). The design
doc's reasoning was pressure-tested against the live repo and the on-disk
26.2 jar on 2026-07-21; its four product decisions are kept, a set of factual
errors is corrected (§2), and several missing workstreams are added (§3).

**Status: M0–M6 shipped + headlessly verified (2026-07-21), all pushed.**
See §0.0 for the fresh-session handoff and §15 for the per-milestone log.

---

## 0.0 HANDOFF — read this first (fresh session, 2026-07-21)

This section exists because the project is being handed to a session with no
prior context. **Read §0.0 → §0.1 → skim §2 (corrections) → §15 (status
log), then critique before continuing.** The rest of the plan is reference.

### What Rewo is, in one paragraph

A from-scratch Rust Minecraft: Java Edition **client** — it speaks the
vanilla network protocol (pinned **26.2 / protocol 776**), decodes the world
the server sends, and renders it with **raw Vulkan via `ash`**. Multiplayer
only (no singleplayer/worldgen). It lives in the `EwoClientV3` Cargo
workspace as `crates/rewo-*` (~11k LoC) and plugs into the EwoClient launcher
as a future `Native` instance kind. The four **fixed product decisions**
(don't silently pivot; raise a genuine objection with the user instead):
(1) from-scratch vanilla-protocol client, (2) performance = frame-time
consistency + input latency first, (3) raw Vulkan not wgpu, (4) integrates
into EwoClient reusing its MS auth. Everything else is open to revision.

### Where it is: M0–M6 all shipped, verified, pushed

- **M0** skeleton: ash 1.3 device + swapchain + frame-time overlay + GPU
  timestamps.
- **M1** protocol: full Handshake→Login(offline)→Config→Play, chunk/light/
  entity decode, record/replay. Verified vs a live vanilla 26.2 server:
  329 chunks, 0 decode failures, 10-min soak no kick, replay digest matches.
- **M2** first pixels: asset bake from the client jar, face-culled cube
  mesher, textured world pass.
- **M3** be a player: vanilla 20 Hz physics port, live play session
  (walk/sprint/jump/dig/place/chat), and **`rewo live`** — a real windowed
  client. Verified: **0 server corrections over 3,000 ticks** of movement.
- **M4** real meshing: full block-model path (stairs/slabs/fences/glass/
  torches/plants/logs), 26-neighbor AO, colormap tint.
- **M5** GPU-driven: mega-buffer arena + compute frustum cull +
  `drawIndexedIndirectCount` (one draw call; 216/329 culled on GPU).
- **M6** latency/measurement: `rewo bench` (deterministic render benchmark),
  1%/0.1% low reporting, frames-in-flight knob. GPU render 0.198 ms avg /
  0.367 ms 0.1%-low on the 5080.

### The verification toolkit (how to check things yourself — USE THESE)

The user hates manual testing (§0.1). Everything is headlessly verifiable:
- `rewo net soak --host … --seconds N [--record x.rewo] [--query X Y Z]` —
  protocol soak + block queries.
- `rewo net replay --file x.rewo [--expect-digest N]` — deterministic replay.
- `rewo view --replay FILE|--host … --out png` — snapshot render (M2).
- `rewo demo --out png` — synthetic showcase of every block-model family (M4,
  no server).
- `rewo play --host … --seconds N` — **the headless bot**; reports
  `CORRECTIONS` (the physics-parity meter — must stay ~0) + place/dig
  world-state verification + chat round-trip.
- `rewo live --host … [--out png] [--run-seconds N] [--fif 1|2]` — the real
  windowed client; `--out` renders the eye view headless.
- `rewo bench --replay FILE --frames N` — **the regression gate**: run it
  before/after any render change; a change that worsens the 0.1% low is a
  regression even if avg fps rises.
- **The test server**: a local offline flat-world vanilla 26.2 server the
  assistant sets up + runs at `%APPDATA%/EwoClient/rewo/26.2/testserver/`
  (online-mode=false, flat, port 25599). Wipe `world/` for a clean run; the
  same username respawns at its last logout position (repeat runs drift —
  not a bug).
- Ground truth for wire formats / physics constants is the **decompiled
  26.2 jar** at `%APPDATA%/EwoClient/rewo/26.2/decompiled/` (Mojmap) + the
  datagen reports under `…/datagen/generated/reports/` — NOT community docs
  (§11). Both are git-ignored (derived from the user's own Mojang download).

### Load-bearing gotchas (each cost real debugging time — internalize them)

1. **World-space vertices + no shader origin.** The mesher emits world-space
   positions (`cx*16+lx+corner`); the vertex shader must NOT add a column
   origin. A double-add here was THE cause of the M4 "far-field holes"
   (wrongly blamed on depth for a while). Keep vertices world-space.
2. **Collision uses `baked.solid`, NOT `matches!(RenderKind::Cube)`.** Many
   full-cube blocks (grass_block!) render as `Model` (cube + overlay
   element). Keying collision off the render fast-path made the bot fall
   through grass every tick → 258 corrections. `baked.solid` = Cube OR a
   Model with a full-16³ element. Any new collision code must use it.
3. **26.x model textures can be objects** `{sprite, force_translucent}`, not
   just string refs (glass baked invisible until handled).
4. **Paletted long array is FIXED-size (no length prefix); section = TWO
   leading shorts** (non-empty + fluid count). These are the two chunk-decode
   details that break everything if wrong.
5. **Packet ids resolved BY NAME** from the datagen report, so a version bump
   fails loud instead of misfiring.
6. **Uploads are async slot-ring submissions on the graphics queue** —
   the CPU never waits on the copy; same-queue FIFO ordering is what makes
   the frame's draws safe. Do not re-add a per-frame `wait_idle`, and do
   not move uploads to another queue without adding real cross-queue
   sync (timeline semaphores + ownership/sharing) — the FIFO guarantee is
   load-bearing.

### Known issues, gaps, and deviations from the plan — CRITIQUE THESE

Grouped by severity. The assistant is encouraged to challenge any of these,
find more, and propose better approaches — you are a stronger reviewer than
the code's author. Nothing below is settled truth.

**Architectural deviations from the plan (§4) worth reconsidering:**
- ~~Meshing runs on the MAIN thread~~ — **RESOLVED 2026-07-21.** Meshing
  now runs on a rayon worker pool (`rewo_mesh::pool::MeshPool`) fed by
  `World::snapshot_3x3` Arc-clone snapshots (§4's copy-on-write model,
  implemented); the frame only uploads finished meshes (6/frame budget).
  See the §15 entry for the design + verification.
- ~~Column uploads are synchronous~~ — **RESOLVED 2026-07-21** (the last
  §4 deviation): uploads are now an async 4-slot staging ring — the CPU
  submits and returns, never waiting the fence (gotcha #6 has the new
  contract). Scope call: the ring stays on the graphics queue — same-queue
  FIFO ordering preserves every draw-safety guarantee for free, and a
  dedicated-DMA-queue only overlaps ~µs of copy time today, so it's
  deferred measure-first like `VK_NV_low_latency2`.
- ~~The launcher `Native` arm spawns `rewo view`~~ — **RESOLVED
  2026-07-21.** It spawns `rewo live` (`launch::native_client_args`, argv[0]
  pinned by a regression test); `package.ps1` stages `rewo.exe` into dist;
  `EWO_DEV_SERVER=host:port` points a plain Launch at a server (the only
  way to reach the local offline test server from the UI until M7). The
  **UI eyeball itself is still pending** (user).

**Correctness / completeness gaps:**
- ~~Entities are decoded into a table but NOT rendered~~ — **RESOLVED
  2026-07-21.** Full entity track: movement/teleport/position-sync/
  player-info decode, vanilla 3-tick lerp + partial-tick blend, capsule
  render pass + bitmap-font nametags — and **players render as the real
  textured model** (12-cuboid wide model incl. overlay layers, Steve
  default skin from the jar, whole-body yaw + head pitch + **walk-cycle
  limb swing**), and **slimes** (green cube), **cows** + **pigs** + **sheep**
  (quadruped, walking legs — pig has short legs + a snout, sheep has its own
  body dims + an inflated white wool overlay), and **zombies/husks/drowned**
  (humanoid, arms-forward pose) render as real models — humanoids also **turn
  their heads** toward nearby players (server-driven `rotate_head`, verified
  via a fixed-body/cranked-head A/B). See the §15 entries. Still open within
  it: entity *collision* is ignored (walk through mobs), some mobs are still
  capsules (chicken/… — each needs its vanilla model dims + UVs; the entity
  atlas grew to 256×256 so there's room, no texture-array refactor needed),
  sheep wool dye-tint is deferred (white only), slime face/size need
  the translucent pass + entity metadata, no real per-player skins (needs
  online-mode profile textures — M7), tags are depth-tested (vanilla shows
  them through walls).
- **Collision is full-cube only** — slabs/stairs/fences have no collision
  (you walk through them). "Expected" for the M3 subset, but a real gap.
- **Physics parity verified only for the on-foot flat-world subset.** Water,
  cobwebs, stairs-vs-sneak, ladders, ice, slabs-as-steps, etc. untested —
  each will likely need decompile-derived constants (§13 risk).
- **Light decode beyond flat-world-full-skylight is unverified.** The M1
  Y-mask light distribution renders the flat world correctly, but caves/
  overhangs/mixed light are untested (the earlier "black patches" were the
  vertex double-add, not light — so light itself is *plausibly* fine but
  unproven).
- **Only the overworld dimension is tested.** Nether/end (different
  min_y/height via the registry parse) is coded but unexercised.
- **No online-mode / encryption / chat signing** — offline servers only
  (M7). Unsigned chat is kicked on `enforce-secure-profile` servers.
- **Reversed-Z frustum-plane math** (Gribb–Hartmann on the reversed-Z
  matrix) culls correctly *empirically* (216/329, render is correct) but a
  reviewer should verify the near/far plane semantics are exactly right, not
  just approximately — it's used on both CPU (M4 legacy) and GPU (M5 cull).

**Visual/perf follow-ons (all now guarded by `rewo bench`):**
- No **greedy meshing** (3.7M verts for 329 flat chunks — high; conflicts
  with per-vertex AO, hence deferred).
- **AO only on cube faces**, not model quads.
- **Per-biome tint** is a fixed plains color (no biome variation; water
  tint likewise fixed plains #3F76E4).
- ~~No fluids + no translucent pass~~ — **RESOLVED 2026-07-21**: water
  (translucent, corner-height surfaces) + lava (opaque fullbright) with a
  CPU-sorted back-to-front translucent pass. See §15. Still open within
  it: waterlogged blocks don't render their water, no flowing-texture
  UV rotation.
- ~~No texture animation ticking~~ — **RESOLVED 2026-07-21**: `.mcmeta`
  frame order + frametime drive per-layer re-uploads on the 20 Hz tick
  (water ripples, lava churns; `demo --anim-tick N` is the deterministic
  check). Frame *interpolation* (lava's `interpolate` flag) not done.
- **36-byte vertices**, not the packed 8–12 B the plan targets.
- **Grazing-angle far-field slivers** on flat ground at near-edge-on angles
  (candidate: MSAA / back-face cull once model-quad winding is guaranteed
  CCW). Cosmetic; the demo + normal angles are clean. (Much less visible
  now — the horizon fog covers the far field.)
- **Sky is gradient + distance fog only** — no sun/moon/clouds/stars, no
  time-of-day or per-biome sky color (§7 lists those; shipped the gradient
  + fog, deferred the rest).
- **HUD is crosshair + hotbar + hearts + hunger only** — no item icons in
  the hotbar slots (needs item models), no XP bar, no armor/air, no
  effect icons, no gamemode-awareness (creative shows hearts/hunger).
- **Mega-buffer has no resize** — over-cap columns are dropped with a log
  (4M verts / 6M indices; high RD could hit this).
- `VK_NV_low_latency2` deferred **measure-first** — the benchmark shows GPU
  render (~0.2 ms) is far below the frame budget, so submission pacing isn't
  the current bottleneck. Revisit at high RD/complexity.

**Minor / cleanup:**
- Dev env knobs `REWO_SETTLE` / `REWO_PITCH` linger in `live_cmd.rs` (one-
  shot, headless-only, zero-cost when unset).
- An `impl Renderer { frames_in_flight() }` block sits awkwardly right after
  the `use` line in `renderer.rs`.
- The `bench` orbit is a regression gate, not a worst-case stress test (the
  render is too cheap to stress the pipeline; a higher-RD scene would).

### Suggested next moves (the user will choose — don't assume)

Strongest candidates, roughly by value: (a) **M7 online-mode + chat
signing** — needed to play on the user's Frogsy network; (b) **async
transfer queue + staging ring** — the remaining §4 deviation; (c) the
visual follow-ons (greedy, fluids, biome tint, packed vertices, texture
animation); (d) the player-model port (capsules → real player model —
`skin.rs` cuboid geometry is the reference). Shipped 2026-07-21:
meshing-off-thread, the Native `live` arm (UI eyeball still the user's),
and entity rendering (capsules + nametags). Confirm direction with the
user before diving in.

---

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

**2026-07-21 — M4 real meshing shipped (model path + AO + tint) + a
reversed-Z depth fix.**

The world went from cubes-only to the full block-model variety — verified
by a synthetic showcase PNG showing stairs, slabs, fences, glass, torches,
cross-plants, logs, and directional cubes all rendering correctly.

- **Block-model parser** (`rewo-data/assets.rs`, ~750 lines): blockstate
  variants (x/y rotation) OR multipart (with `when`-condition eval against
  state properties) → model parent chains → arbitrary `elements` (from/to
  boxes) → per-face uv (explicit + derived) / texture / cullface / tintindex,
  element rotation (origin/axis/angle/rescale for cross-plants),
  `shade`/`ambientocclusion` flags. Output is a fast-path `Cube` (full
  opaque 16³, for cheap face-cull + AO) or a baked `Model` quad list. Bake
  result: **2,320 cubes + 26,555 models** (was 2,649 cube-only in M2).
  Gotcha fixed: 26.x textures can be an **object** `{sprite,
  force_translucent}` not just a string ref — glass + many blocks baked
  invisible until the parser accepted both forms.
- **Mesher** (`rewo-mesh`): full-cube fast path with **26-neighbor ambient
  occlusion** (vanilla 0–3 corner darkening) + the general model-quad path
  (cullface culling against full-cube neighbors, per-quad shade/light).
  Vertex format → pos/uv/layer/**color** (shade × light × AO); tint is baked
  into the texture layers (grayscale grass/foliage × colormap-center plains
  color) so the mesher doesn't re-tint. Greedy meshing deliberately skipped
  (conflicts with per-vertex AO — the plan's own tension, resolved toward
  the vanilla look).
- **`rewo demo`**: synthetic in-memory showcase (grass platform + a row of
  10 varied blocks) rendered headless — no server. The deterministic M4
  artifact. Inspected: **every model family correct**, AO visible on the
  terrain, grass green from the colormap tint, glass transparent.
- **Reversed-Z depth (`world::perspective_reverse_z`)** — while verifying
  M4 on the *real* flat world, distant terrain showed holes. Instrumented
  the mesher (face tally proved all top faces emit — geometry was correct)
  and isolated it to **depth precision**: standard [0,1] `LESS` with a
  0.05→2000 range z-fights a flat plane into holes at distance. Switched the
  world pass to reversed-Z (infinite far, `GREATER`, 0.0 clear) across all
  three cameras — the mid-field solidified. A **big win that also helps
  M2/M3**.
- **Verified:** demo PNG (all models) inspected; real-world live PNG much
  improved; 14 rewo tests green; debug instrumentation removed.
- **Known follow-up (cosmetic, not M4-scope):** the real flat world still
  shows some grazing-angle far-field slivers + dark patches at the horizon
  (the world-bottom bedrock down-faces showing through, amplified by 1×
  sampling at near-edge-on angles). Candidate fixes: MSAA (the plan's AA
  item) and/or back-face culling once model-quad winding is guaranteed CCW.
  It does not affect the demo or normal-angle play; the interesting content
  (models/AO/tint) is correct.
- **Deferred to a later M4 pass (tracked):** binary greedy meshing, fluids
  (water/lava neighbor-height geometry + translucent pass), per-biome tint
  (fixed plains color baked for now), animated-texture ticking, packed
  8–12 B vertices (36 B now), uvlock, AO on model quads (cubes only today).
- Next: the M4 follow-ons above + the M5 GPU-driven path (mega-buffer arena,
  compute cull, drawIndirectCount) which removes the live-remesh `wait_idle`
  stall and is where greedy/packing pay off.

**2026-07-21 — M5 GPU-driven rendering shipped (+ two bug fixes found while
verifying).**

The renderer went from one draw call *per column* with CPU frustum culling
to a **single `vkCmdDrawIndexedIndirectCount`** fed by a **GPU compute cull**
— the CPU draw cost stops scaling with column count.

- **Mega-buffer arena** (`rewo-gpu/src/world.rs`, full rewrite): all chunk
  geometry lives in two device-local buffers (4M verts + 6M indices), each
  column a **free-list-suballocated** region (first-fit + coalescing).
  Uploads go through a one-shot staging copy with its own fence, so the
  per-FRAME path never stalls — M4's per-frame `wait_idle` in the live
  client is gone.
- **Per-column metadata SSBO** (world-space AABB + draw params), rebuilt +
  map-written only when the column set changes.
- **Compute cull** (`shaders/cull.comp`): one invocation per column
  frustum-tests its AABB and, if visible, appends a
  `VkDrawIndexedIndirectCommand` + `atomicAdd`s a draw count. Runs BEFORE
  dynamic rendering (compute can't run in a render pass), so the frame is
  `cull()` (pre-pass) → begin_rendering → `draw()` (indirect) → overlay.
- **Device features**: enabled Vulkan 1.2 `draw_indirect_count` +
  `multiDrawIndirect`.
- **Verified:** demo + flat-world PNGs render **identically to M4**,
  validation-clean; a count readback confirms the GPU cull works —
  **113 of 329 columns drawn, 216 culled on the GPU**. Windowed soak
  ~974 fps, cpu p99 2.68 ms, 0 corrections.
- **Two bugs found + fixed while verifying M5** (both predated it):
  1. **World-space vertex double-add** (since M2): the mesher emits
     world-space positions but the vertex shader ALSO added the column
     origin, pushing distant chunks progressively out of place — THE actual
     cause of the M4 "far-field holes" (not depth precision). Dropped the
     origin add; the flat world now renders as a solid plane to a clean
     horizon. (Committed separately, `680622a`.)
  2. **grass_block collided as non-solid** (M4 regression): grass renders as
     a `Model` (its model has a cube element + an overlay element, so the
     single-element cube fast-path rejects it), and the bot's collision
     table was `matches!(Cube)` → the bot fell through the grass surface
     every tick → 258 server corrections. Added a proper `solid` flag to the
     bake (`Cube` OR a `Model` with a full-16³ element) so collision is
     independent of the render fast-path. 0 corrections restored.
- **Deferred to M5 follow-ons (tracked):** dedicated async transfer queue +
  staging ring (uploads still fence-sync, fine for a mostly-static world);
  visibility-graph ("cave") culling; HZB occlusion (M8); mega-buffer resize
  (over-cap columns are dropped with a log). Plus the M4 carryovers (greedy,
  fluids, per-biome tint, animation, packed vertices) — greedy + packing pay
  off most now that the arena exists.
- Next: M6 (latency pass — `VK_NV_low_latency2`, frames-in-flight tuning,
  click-to-photon, the replay-benchmark regression gate) or the M4/M5
  follow-ons.

**2026-07-21 — M6 latency/measurement pass shipped.**

The milestone that makes the project's stated goals *measurable*: frame-time
consistency (1%/0.1% lows) and the merge-gate benchmark.

- **1% / 0.1% low reporting** (`stats.rs`): the gaming "low" = the **mean of
  the slowest N% of frames** (not a percentile edge — that hides how bad the
  worst frames are). Unit-tested. Plus a compact ASCII frame-time histogram.
- **`rewo bench`** — the **deterministic render benchmark** (the plan's
  "metric that governs merges"). Loads a replay world, meshes it into the
  GPU-driven renderer, renders N frames from a **deterministic camera orbit**
  (same scene + path every run → trustworthy A/B), captures per-frame GPU
  timestamps + wall time, reports avg / p50 / p99 / p99.9 / 1% low / 0.1% low
  / max + histogram. Headless, machine-runnable, warmup-excluded.
  - **Measured** (329-chunk flat-world replay, 3.7M verts, 2000 frames, RTX
    5080): **GPU frame time avg 0.198 ms, p99 0.359, 1% low 0.362, 0.1% low
    0.367, max 0.369 ms** — the GPU-driven path is *extremely* consistent
    (the 0.1% low is barely above the average). GPU cull culled 220/329 on
    the orbit.
- **Frames-in-flight knob** (`Renderer::with_frames_in_flight`, `--fif`): the
  latency lever (1 = shortest CPU→GPU→present queue, 2 = default). Measured
  on the windowed live path (6 s soak each): **fif=1** frame time avg 1.02 ms,
  1% low 3.12, 0.1% low 4.81, max 5.76; **fif=2** avg 1.03, 1% low 3.43, 0.1%
  low 5.56, max 5.99 — fif=1 gives **tighter lows + lower latency at identical
  fps** (~975), exactly the plan's latency-first bias. Default stays 2 (safe
  under GPU-bound load); fif=1 is the measured latency-optimal option.
- **Windowed soak stats** upgraded to report 1%/0.1% lows (the real pipelined
  frame-consistency number: avg ~1 ms, 0.1% low ~5 ms — well within budget).
- **DoD met:** 1%/0.1% lows + latency numbers documented on the benchmark;
  frame-consistency (goal #1) measured and excellent; the latency knob (goal
  #2) measured in the predicted direction.
- **`VK_NV_low_latency2` deferred — measure-first finding** (per the plan's
  own guidance): the benchmark shows GPU render is ~0.2 ms, *nowhere near*
  the frame budget, so submission pacing (low_latency2's job) is not the
  current bottleneck. It becomes worth integrating at high render distance /
  scene complexity where the GPU is actually loaded — the measurement infra
  (`rewo bench` + the fif knob) is now in place to know when. Full
  click-to-photon needs the windowed path + ideally external LDAT capture;
  the software input→submit path is the controllable proxy.
- Next: the deferred visual/perf follow-ons (M4: greedy meshing, fluids +
  translucent pass, per-biome tint, animation, packed vertices; M5: async
  transfer queue, visibility-graph cull) — the `rewo bench` regression gate
  now guards all of them — or M7 (online-mode + chat signing) / M8.

**2026-07-21 — meshing off the main thread + the launcher spawns the real
client (the two top fixes from §0.0).**

- **Mesh worker pool** (`rewo-mesh/src/pool.rs` + `rewo-world` CoW): `World`
  now stores `Arc<Column>` — writers go through `Arc::make_mut`, i.e. the
  copy-on-write sharing §4 specified. `World::snapshot_3x3(cx, cz)` hands a
  mesh job a self-contained 9-Arc-clone view (face culling reads ±1 block
  and AO reads diagonal corners at ±1, so 3×3 is sufficient; snapshot-edge
  reads behave exactly like today's unloaded-edge reads). `MeshPool` (rayon,
  `available_parallelism − 2` clamped to [1, 8] workers) runs `mesh_column`
  **unchanged** on those snapshots; results return over an mpsc channel.
  Ordering safety is one rule: a column already in flight is never
  resubmitted — it stays dirty and follows the stale result with a fresh
  snapshot. The live frame now only *uploads* finished meshes
  (`UPLOAD_BUDGET = 6`/frame); meshing itself is off the frame — the plan's
  own thesis. One-shot paths (`live --out`) use `pool::mesh_all`
  (order-preserving rayon `par_iter`): 329 columns in ~70 ms wall.
- **Verified headlessly:** demo PNG **byte-identical** pre/post (the Arc
  refactor + pool change zero pixels); `rewo bench` gate PASS — identical
  scene (329 chunks / 3,723,160 verts / cull 220 of 329), GPU 0.1% low
  1.208 → 1.097 ms; windowed soak vs the live test server: 8 workers,
  initial flood **488 uploads for 329 columns done in 2.0 s**, corrections
  0, 0 jobs in flight at exit, frame time avg 0.43 / 0.1% low 3.61 ms; 71
  tests green incl. 4 new pool tests (off-thread mesh, in-flight dedupe,
  snapshot isolation from later edits, `mesh_all` ordering).
- **Launcher Native arm → `rewo live`** (was `view`, the M2 snapshot): argv
  built by `launch::native_client_args`, with a regression test pinning
  `argv[0] == "live"`. `rewo live` now resolves its username from the
  launcher's `REWO_USERNAME` env handoff (arg > env > "RewoLive").
  `package.ps1` builds + stages `rewo.exe` next to `EwoClient.exe` (the
  `find_rewo_binary` contract — without this the fix was dead in the dist
  bundle). New dev affordance: `EWO_DEV_SERVER=host:port` makes a plain
  Launch click behave like a server join (real join flows still win) — the
  only way to point the UI at the local offline test server, since Frogsy
  is online-mode (M7) and the H6 widget / friend-Join are the only other
  `active_server` setters. **UI eyeball still pending (user):** start the
  test server, then `EWO_DEV_SERVER=127.0.0.1:25599 cargo run -p
  ewo-launcher`, Launch a Native instance.
- **Drive-by fix:** `rewo view` had been silently broken since M4 — an
  M2-era bake sanity check demanded grass_block bake as a `Cube`, but M4
  correctly bakes it as a `Model` (§0.0 gotcha #2, the same one that caused
  the collision bug). The check now asserts visible + `baked.solid`.
- Next: entity rendering (capsules + nametags), M7 online-mode + chat
  signing, or the async transfer queue — see §0.0 "Suggested next moves".

**2026-07-21 — entity rendering shipped: capsules + nametags (multiplayer
is visible).**

- **Protocol** (all wire shapes read from the decompile, §11):
  `move_entity_pos` / `move_entity_pos_rot` / `move_entity_rot` (short
  deltas ÷4096 — accumulated onto the synced target, the client mirror of
  `VecDeltaCodec`), `entity_position_sync` + `teleport_entity`
  (`PositionMoveRotation`; teleport honors the same Relative bitfield as
  the player teleport), `player_info_update` (1-byte fixed bitset over the
  8 actions, ADD_PLAYER name kept, every other field skipped byte-exactly —
  properties/chat-session/NBT display names) + `player_info_remove`. All
  resolved by name (fail-loud). **Latent M1 bug fixed:** `add_entity` read
  2 rotation bytes as yaw-then-pitch; the wire is pitch, yaw, head-yaw (3
  bytes).
- **Interpolation** (`rewo-world/entities.rs`): vanilla semantics — targets
  from packets, 3-step tick lerp (`(target−cur)/steps_left`, exact on the
  third tick), frames blend prev→cur by partial-tick alpha.
  `PlaySession::tick` steps all lerps at 20 Hz. Names in a UUID→name map
  that outlives entity unload. Unit-tested (convergence, delta
  accumulation, name lifetime).
- **Font**: the vanilla bitmap font baked from the client jar
  (`ascii.png` → atlas + per-glyph advances measured the way the legacy
  provider does: rightmost lit column + 2, space = 4; a white texel is
  patched into the blank space cell so solid quads ride the same pipeline).
- **Renderer** (`rewo-gpu/entities.rs`): a deliberately simple CPU-built
  world-space triangle soup, rebuilt per frame — capsule shells (12-segment
  profile sweep, ~500 verts each, per-vertex sun shade) + camera-
  billboarded glyph/background quads. One vertex format, two pipelines
  (solid: depth-write GREATER; text: blended, depth-write off), both
  alpha-masked + linear-color (the two render disciplines), 2-slot buffer
  ring matching frames-in-flight. Lives inside `WorldRenderer::draw` after
  the terrain — renderer/offscreen untouched; snapshots (view/demo/bench)
  never init it and pay nothing. Instancing is the escalation path if
  entity counts ever grow 100×.
- **Colors**: players = accent rose + nametag, mobs = mauve, sized by an
  `entity_types` registry table (new `rewo-data` module, exact dims for
  the common types).
- **Verified headlessly on the live test server** (which turned out to
  have a natural slime herd + horses + pig — free test entities):
  validation layers **clean**; `[rewo-entities]` printout lists every
  tracked entity (`REWO_LOOK_ENTITY=1` aims the headless camera at the
  nearest player); **position cross-check exact** — a stationary second
  client self-reported (6.5,−60.0,7.5) and the observer tracked it at
  (6.50,−60.00,7.50); PNGs inspected: mauve capsules across the plain
  (one mid-jump), and a close-range shot with the rose player capsule +
  **"RewoCap2" legible in the vanilla font** on its dimmed plate; windowed
  soak with **129 live entities**: ~1,170 fps, avg 0.89 ms, 0.1% low
  4.1 ms, corrections 0. `rewo bench` gate green (0.1% low 0.976 ms) and
  the demo PNG still byte-identical.
- **Known limits (tracked in §0.0):** no entity collision, capsules not
  player models, tags depth-tested (vanilla renders them through walls),
  AO/light not sampled for entities (fixed sun shade).

**2026-07-21 — fluids + translucent pass shipped (the biggest M4
carryover).**

- **Bake** (`rewo-data/assets.rs`): water/lava classified by NAME (vanilla
  hardcodes fluid rendering — their blockstates carry no usable model) into
  a new `RenderKind::Fluid { layer, level, lava }` keyed on the `level`
  property. Water texture = `water_still` first animation frame (ships
  grayscale, uniform alpha 180) × plains water #3F76E4 — the same
  fixed-plains tint approach as grass; lava = `lava_still`, opaque.
- **Mesher** (`rewo-mesh`): a fluid path alongside cubes/models — top face
  at per-corner heights (source 8/9, flowing (8−level)/9, falling 1.0;
  corner = max over the 4 touching same-fluid cells, a simpler take on
  vanilla's weighted average that still reads as a continuous sloped
  surface), trapezoid side faces, bottom face vs air. Water emits into a
  new **translucent set** on `ColumnMesh` (`tvertices`/`tindices`); lava
  emits opaque at fullbright. 3 new unit tests (source height, lava
  opacity, submerged columns full-height).
- **Renderer** (`rewo-gpu/world.rs`): each column now owns up to four
  mega-buffer regions (opaque + translucent verts/indices from the same
  free-lists, one staged upload). The indirect GPU-cull path stays
  opaque-only (cull.comp already skips `index_count == 0`); water draws as
  **per-column direct indexed draws, CPU frustum-culled (mirroring
  cull.comp's positive-vertex test) and sorted far→near** from a new
  `set_camera(eye)` — the plan's "per-section back-to-front CPU sort;
  intra-section artifacts accepted v1". New `water.frag` (texture alpha
  rides to the blender) + a blended, depth-write-off pipeline variant.
  Blend-correct frame order: opaque terrain → entity capsules →
  translucent water → nametag text (EntityPass::draw split into
  solid/text).
- **Demo**: the showcase gains a water pool (with one level-4 flowing
  cell) and a lava pool carved into a front apron; camera aim dropped to
  frame them. Inspected: translucent blue water with the dirt floor
  clearly visible through it, surface at 8/9 below the grass rim; opaque
  glowing lava; the whole M4 model row intact behind.
- **Verified:** validation layers clean on the new pass; `view --replay`
  PNG **byte-identical** (no fluids in the flat world — the dual-region
  upload changes nothing without water); bench gate green (avg 0.211 ms /
  0.1% low 1.131 — noise-level vs every prior run); live soak vs the test
  server: corrections 0, 129 entities, frame times unchanged. 49 rewo
  tests green.

**2026-07-21 — texture animation + the player model (Steve replaces the
player capsule).**

- **Texture animation** (`.mcmeta`-driven): the bake slices animation
  strips into 16×16 frames (tint applied to all — water's 32 sequential
  frames at frametime 2, lava's explicit ping-pong order) and
  `WorldRenderer::anim_tick(gpu, tick)` re-uploads a layer (+ regenerated
  mips, a few KB) whenever its frame changes on the 20 Hz game tick.
  `rewo live` drives it from `session.ticks`; `rewo demo --anim-tick N`
  is the deterministic check — PNGs at tick 0 vs 20 differ in 168,873
  pixels **all confined to the fluid pools** (sky + model row
  byte-identical). Frame interpolation (lava's `interpolate`) deferred.
- **Player model**: players now render as the real textured wide model —
  the 12-cuboid set (6 base + 6 inflated overlay layers) with the
  standard box-UV unwrap, ported from `ewo-jni/src/skin.rs` (Phase F's
  battle-tested viewer) into `EntityPass` as pre-unwrapped quads. The
  **Steve default skin** bakes from the client jar (offline servers carry
  no profile textures; real skins arrive with M7 online-mode). The entity
  atlas grew to a combined 256×128 (font at (0,0), skin at (128,0)) so
  glyphs, capsule fills, and skin texels share one pipeline family;
  transparent overlay texels ride the existing alpha-test discard.
  Whole-body yaw rotation + head pitch about the neck pivot; vanilla's
  0.9375 render scale. Mobs stay capsules.
- **Verified:** validation clean; the close-range harness PNG shows a
  pixel-correct Steve from behind (hair, shirt, jeans — yaw 0 facing away
  from the observer, exactly right) under the "RewoCap2" nametag;
  position cross-check still exact; windowed soak with 137 live entities:
  corrections 0, frame times comfortable. New unit test pins the head's
  front-face UV rect + the 72-quad model + hat-top bound.

**2026-07-21 — async upload ring (the last §4 deviation closed).**

- `upload_column` / `upload_layer_frame` no longer block the CPU on their
  fence: a **4-slot ring** (per-slot command buffer + fence + growable
  staging) submits on the graphics queue and returns. Same-queue FIFO
  ordering keeps every existing draw-safety guarantee (the copy executes
  before any later-submitted frame that could read the regions — exactly
  what the old blocking path relied on, minus the stall); a trailing
  TRANSFER→VERTEX/INDEX memory barrier in the upload cb makes visibility
  locally airtight. Retired fences are harvested opportunistically; the
  CPU only waits when all 4 slots are in flight (sustained burst) or to
  grow a slot's staging. `read_draw_count` keeps its blocking readback
  (stats path). Texture animation (10 uploads/s) rides the same ring.
- **Scope call, documented in §0.0:** the dedicated transfer queue + DMA
  overlap stays deferred measure-first — a ~100 KB copy is microseconds
  of GPU time; the plan's real goal ("meshing/uploads happen off the
  frame") is met by pool + ring. Moving to another queue later requires
  real cross-queue sync (see the rewritten gotcha #6).
- **Verified:** demo + view PNGs **byte-identical** through the async
  path; bench gate green (avg 0.209 / 0.1% low 0.920 ms — best yet);
  validation 0 VUIDs on demo + live streaming (debug); live soak: flood
  488 uploads in 2.0 s, corrections 0, lows tighter than the pre-async
  run at the same 138-entity count.

**2026-07-21 — player limb-swing animation (the model walks).**

- **Derived client-side from motion** (the server never sends limb
  angles): `EntityState::tick` now updates vanilla's `animationSpeed`
  (smoothed horizontal speed 0..1, `+= (min(1, dist·4) − speed)·0.4`) and
  `animationPosition` (phase, `+= speed` each tick) from the entity's own
  per-tick displacement — so a standing entity's limbs freeze and a
  walker's build up, exactly as `LivingEntity.aiStep` does. Exposed via
  `EntityState::limb() → (swing, amount)`.
- **The model articulates** (`emit_player`): each of the 12 cuboids is
  tagged with a `LimbPart` (Body/Head/Arm{R,L}/Leg{R,L}); arms/legs rotate
  about their shoulder (y=24) / hip (y=12) pivots by
  `HumanoidModel.setupAnim`'s constants — arms ±2.0 rad·amount, legs
  ±1.4 rad·amount at the 0.6662 walk frequency, opposite-phase diagonal
  gait. Head keeps its look-pitch about the neck (unified into the same
  per-part X-rotation). Verified the gait direction from the decompile
  math: `+xRot` on a leg swings the foot to −Z (behind); right leg back ⇔
  right arm forward.
- **Verified:** unit test pins still→no-swing and sustained-walk→amount>0.5
  + phase advance; a `REWO_FORCE_LIMB=swing,amount` headless knob (one-shot,
  dev-only) pins a player's pose so a still-target PNG proves the mechanism
  deterministically — inspected a live second client mid-stride: legs
  clearly split (one forward, one back), arms swung in opposite phase, torso
  + head correct. Entity-only change → demo PNG **byte-identical**, bench
  untouched. 45 rewo tests green.

**2026-07-21 — gradient sky + distance fog (the world gained a horizon).**

- **Gradient sky** (`sky.vert`/`sky.frag`, a new fullscreen pipeline in
  `WorldRenderer`, drawn first with no depth test/write): reconstructs each
  pixel's world-space view ray from `inverse(view_proj)` (computed on the
  GPU-side via glam, no app plumbing) and blends a pale-blue horizon →
  deeper zenith by the ray's up-component. Colors linear (SRGB store).
- **Distance fog** in `world.frag` + `water.frag`: the vertex shader passes
  world-space position through; the fragment fades toward the **sky horizon
  color** over a `[start, end]` band (default 80–180, `REWO_FOG=a,b` to
  tune). Fog color = sky horizon, so terrain **melts into the sky** at the
  render-distance edge — the hard chunk-boundary silhouette that showed in
  every prior screenshot is gone. The world push grew
  (`view_proj` + camera + fog band; range now VERTEX|FRAGMENT).
- **Verified:** 0 VUIDs (the sky pipeline must declare the pass's depth
  *format* though it disables depth test/write — the one gotcha); flat-world
  `view --replay` + live eye-view PNGs show grass dissolving into a soft
  foggy horizon under the gradient sky (no visible edge); demo unchanged in
  substance (close scene, gradient sky behind); live soak corrections 0,
  155 entities, 0 VUIDs. Bench rose a hair (avg 0.209 → 0.243 ms, 0.1% low
  0.920 → 1.052) — the honest cost of a fullscreen pass + per-fragment fog,
  still deep under budget. Sun/moon/clouds/stars/time-of-day deferred (§7).

**2026-07-21 — in-game HUD (crosshair, hotbar, hearts, hunger).**

- **The live client now looks like you're playing.** A new screen-space
  `HudPass` (`rewo-gpu/hud.rs`, drawn last in `WorldRenderer::draw` with a
  positive viewport, alpha blend, no depth) renders the vanilla HUD from
  the jar's `gui/sprites/hud/` sprites: crosshair (centered), hotbar +
  selection frame (bottom-center, over the active slot), 10 health hearts
  (left) + 10 hunger drumsticks (right). Nine sprites packed into one
  256×64 atlas, one alpha-blended pipeline, a per-frame CPU quad list at
  the auto GUI scale (`floor(min(h/240, w/320))`).
- **Real data**: `rewo-data` extracts the sprites (`decode_png_any` — a
  size-agnostic decoder handling the P/LA/RGBA sprite modes); the session
  now tracks `food` (Set Health carries it after health); the HUD reflects
  live health/food. Number keys 1–9 select the hotbar slot (HUD frame +
  the `set_carried_item` packet so the held item matches). Hearts + hunger
  share one fill rule (full/half/empty at ≥(j+1)·2 / >j·2 / else),
  mirrored origins.
- **Live-only**: `set_hud` is called only by `rewo live`; view/demo/bench
  never draw a HUD (they aren't "playing") — so the demo PNG is
  **byte-identical** to the post-sky baseline, bench flat (avg 0.244 ms).
- **Verified:** headless live PNG shows the full HUD — white crosshair,
  9-slot hotbar with the slot-0 selection frame, 10 red hearts, 10
  drumsticks — pixel-correct vanilla layout; 0 VUIDs; live soak corrections
  0, 153 entities. (Debugging note: the first render was blank because a
  stale *debug* binary ran after a release-only build — a solid-red frag
  test proved the geometry/UVs/alpha were correct all along. Same
  build-profile gotcha bit twice this session; always rebuild the profile
  you run.)

**2026-07-21 — slime mob model (first real mob; capsules start retiring).**

- **The entity pass gained a mob-model registry.** `EntityDraw.player: bool`
  became `EntityDraw.kind: EntityModelKind {Player, Slime, Capsule}`;
  `emit_player` generalized to `emit_model(quads, scale)` (per-part pivot
  rotation is a no-op for non-articulated parts), and the player box-UV
  builder became a reusable `cuboid_quads(cuboids, atlas_off_x, off_y)`.
  Adding a mob is now: a texture in the atlas + a `*_model_quads()` + a
  dispatch arm.
- **Slime**: the vanilla `SlimeModel` 8³ outer body cube, converted to this
  crate's convention (feet-up y, front +Z; `my = (-vx, 24-vy, -vz)`). Its
  64×32 texture bakes into the entity atlas at (SKIN_X, 64). Rendered opaque
  at a fixed ~1-block (size-2) scale — real slime size lives in entity
  metadata (`set_entity_data`, not decoded), and the eyes/mouth sit inside
  a *translucent* outer shell in vanilla, so **face + size are follow-ups**
  (need the entity translucent pass + metadata). A green cube reads clearly
  as a slime.
- **Dispatch** by entity-type name (`minecraft:slime` / `magma_cube`);
  slime capsule dims overridden to (1,1). `REWO_LOOK=slime` headless knob
  aims the verification camera at the nearest slime.
- **Verified:** the test world's ~15 slimes now render as green slime cubes
  (close-up inspected — slime-green textured cube); the player model still
  renders correctly (regression check on the shared `emit_model` refactor —
  Steve + nametag intact); 0 VUIDs; entity-pass change is live-only so the
  demo PNG is **byte-identical**, bench flat (avg 0.240 ms); live soak
  corrections 0, 154 entities. 45 rewo tests green.

**2026-07-21 — block targeting: selection outline + interactive dig/place
(the windowed client can now mine and build).**

- **Raycast** (`rewo-world/raycast.rs`): Amanatides–Woo voxel DDA from the
  eye along the look dir, testing each cell against `baked.solid` (the same
  flag collision uses), up to a 4.5-block reach; returns the hit block + the
  entered-face normal (the placement side). 5 unit tests (floor-from-above,
  wall-near-face, miss, max-distance, diagonal). Exposed as
  `PlaySession::target_block(eye, dir, reach)`.
- **Selection outline** (`rewo-gpu`): a `LINE_LIST` pipeline (depth-tested
  against terrain — reversed-Z GREATER, depth-write off, alpha-blended)
  draws the 12 edges of the targeted block, inflated 0.002 to dodge
  z-fighting. `WorldRenderer::set_selection(Option<[i32;3]>)`; `None` →
  nothing (so view/demo/bench, which never set it, stay byte-identical).
  Black at 0.7 alpha — vanilla's look.
- **Interactive** (`rewo live`, windowed): each frame raycasts and sets the
  outline; **left-click digs** the targeted block (creative = instant break),
  **right-click places** against the hit face. The client gives itself a
  stack of dirt in slot 0 on spawn (creative) so placing has something to
  place; the existing dirty-set → mesh-pool loop remeshes the edit live.
- **Verified:** headless render looking down logs `targeting block
  [-3,-61,6] face [0,1,0]` (correct — the grass block under the crosshair
  via its top face) and the PNG shows the black diamond outline on that
  block; 0 VUIDs; windowed soak (per-frame raycast in the hot loop)
  corrections 0, frame avg 0.71 ms; demo/view **byte-identical**, bench flat
  (best-yet 0.1% low 0.810 ms). Dig/place packet mechanics were already
  proven by `rewo play`; this wires them to the mouse + the live remesh.
  50 rewo tests green.

**2026-07-22 — zombie mob (humanoid mobs reuse the player model) + the
command packet.**

- **Zombie**: reuses the player model geometry *verbatim* — the 64×64
  zombie skin has the same box-UV layout as the player skin, so
  `player_model_quads` became `humanoid_cuboids()` (shared 12 cuboids) and
  both player + zombie build quads from it via `cuboid_quads` with different
  atlas offsets (skin at (128,0), zombie at (192,0), no atlas growth). Near
  zero new geometry. `emit_model` gained an `arm_forward` param; zombies get
  −1.3 rad (arms held out — the iconic pose), players 0. Dispatched by name
  (`zombie`/`husk`/`drowned`/`zombie_villager`). Skeleton deferred (its
  64×32 texture uses the older single-layer humanoid UV — a variant).
- **Command packet**: `chat_command` (unsigned, just the command string) →
  `PlaySession::send_command`. Verification tool + a real feature.
- **Verification tooling**: a `REWO_SUMMON=<mob>` headless knob `/summon`s a
  mob 3 blocks ahead once spawned (the client is op'd via a generated
  `ops.json` with the offline UUID); `REWO_LOOK=zombie` aims at it.
- **Verified:** summoned a zombie and rendered it — green head, teal shirt,
  **arms held forward** (pose correct), from behind at yaw 45; 0 VUIDs; the
  player still renders (shared-geometry regression — unit test + arm_forward
  0 is a player no-op); demo **byte-identical**, bench flat (0.1% low
  0.583 ms), live soak corrections 0. 50 rewo tests green.

**2026-07-22 — cow mob (the quadruped body plan; third model family).**

- **Quadruped model** (`quadruped_model_quads`), from vanilla
  `QuadrupedModel::createBodyMesh` (cow `legSize=12`): head + a **body box
  rotated 90° about X** (lies horizontal) + 4 legs. The rotated body is why
  this can't use plain `cuboid_quads` — so the box-UV face generation was
  extracted into `box_uv_faces`, and the quadruped builds each part's faces
  in vanilla-local coords, applies the part's X-rotation + pose, then
  converts to this crate's convention (`(-x, 24-y, -z)`). Static for v1 (no
  leg walk-swing, no head look — both follow-ups). Cow texture
  `cow_temperate.png` (64×64) bakes into the atlas at (192,64); a `blit_64`
  helper now shares the entity-skin blit. `EntityModelKind::Cow`, mob 1/16
  scale, dispatched on `minecraft:cow` (pig/sheep share the shape but need
  their own textures — follow-up).
- **Verified:** summoned a cow (`REWO_SUMMON=cow`, `REWO_LOOK=cow`) and
  rendered it — brown/white body, **pink ears**, blocky head, 4 legs, from
  the front-left; the rotated-body UVs land correctly (coloring in the right
  places). 0 VUIDs; player + zombie still render (shared `box_uv_faces`
  refactor — unit test intact); demo **byte-identical**, bench flat, live
  soak corrections 0. 50 rewo tests green. The mob registry now spans three
  body plans: humanoid (player/zombie), cube (slime), quadruped (cow).

**2026-07-22 — screen-space text: coordinates + chat overlay (a new
capability).**

- **`TextPass`** (`rewo-gpu/text.rs`): the vanilla bitmap font rendered as
  2D screen-space glyph quads — its own 128×128 font texture + a 2D
  pipeline (alpha-blended, no depth, top-left pixel origin), a per-frame
  glyph-quad ring. Each glyph draws **twice**: a darkened copy offset +1
  font-px (the vanilla drop shadow), then the tinted glyph, so text stays
  readable on any background. Layout uses the font's per-glyph advances.
  Drawn last (over the HUD). `OwnedTextLine` lets the app fill lines each
  frame; the renderer borrows them into `TextLine` at draw time.
- **Uses**: a coordinates + facing line top-left (`XYZ x y z   facing NE`,
  compass from yaw) and a **chat overlay** (the last 8 `chat_log` messages,
  above the hotbar). Both at the auto GUI scale.
- **This is the client's first on-screen text** — nametags were world-space
  billboards; now chat, coords, and any debug line have a home.
- **Verified:** headless PNG shows the coords line (with drop shadow) +
  "hello from rewo!" in the chat area (sent via a `REWO_CHAT` knob); the
  HUD + world render underneath; 0 VUIDs; text is live-only so the demo PNG
  is **byte-identical**, bench flat; live soak corrections 0. 50 rewo tests
  green.

**2026-07-22 — entity metadata decode → custom nametags on mobs.**

- **`set_entity_data` was undecoded** — the `SynchedEntityData` delta stream
  (`u8 index` [0xFF end] + `VarInt serializer type` + a type-dependent
  value). New `rewo-net/metadata.rs` parses it with a **serializer skip
  table** (the `EntityDataSerializers` registration order, read from the
  decompile): it extracts the reliably-indexed **Entity base** fields —
  shared flags (index 0, BYTE) and **custom name** (index 2,
  OPTIONAL_COMPONENT → flattened text) — skipping the rest by type, and
  bails cleanly on a complex type (item stack / particle / …) that never
  precedes index 2. 3 unit tests (name-after-skips, bail-on-complex, empty).
- **Custom nametags**: `EntityTable` gains an id→name map
  (`set_custom_name` / `custom_name`, cleared on removal); the live client
  renders it as the entity's nametag (players still show their profile
  name). Slime size / baby flags live at *entity-specific* indices
  (fragile) — deferred; the parser + flags byte are the reusable base.
- **Verified:** summoned `cow {CustomName:'"Bessie"'}` — **"Bessie" floats
  above the cow** (metadata decoded → nametag rendered); the same shot also
  shows `commands.summon.success` in the chat overlay (the server's command
  feedback — end-to-end chat + metadata + the `chat_command` packet all in
  one frame). 0 VUIDs; decode-side + live-only render → demo
  **byte-identical**, bench flat; soak corrections 0. 53 rewo tests green.

**2026-07-22 — quadruped leg animation (the cow walks).**

- The 4 cow legs now swing in vanilla's **diagonal gait** (back-right +
  front-left in phase, back-left + front-right opposite; ±1.2 rad · amount
  at the 0.6662 walk frequency) — driven by the same client-derived
  `limb_swing`/`limb_amount` the humanoid mobs use. Head + body stay static
  (head-look is a follow-up).
- **`emit_model`'s rotation generalized from `pivot_y` to `(pivot_y,
  pivot_z)`** — the crux: humanoid legs sit at z≈0 (rotating about z=0 is
  fine), but the quadruped's front/back legs sit at different z, so each
  must swing about *its own* top (front z≈+5, back z≈−7). 4 new `LimbPart`
  variants (`QuadBackRight/Left`, `QuadFrontRight/Left`) carry the pivot-z +
  phase; humanoid parts pass pivot_z 0 (unchanged behavior — player/zombie
  render identically).
- **Verified:** summoned a cow, forced a mid-stride pose (`REWO_FORCE_LIMB`)
  — the legs splay front/back in the diagonal gait (a side-view cow shows it
  cleanly); the persisted "Bessie" nametag from the prior run confirms
  custom names survive a world reload too. 0 VUIDs; entity-only + live-only
  → demo **byte-identical**, bench flat; soak corrections 0. 53 rewo tests
  green.

**2026-07-22 — mob head-look (humanoids watch you).**

- Humanoid mobs (player/zombie/husk/drowned) now **turn their heads** toward
  whatever the server steers them at (nearby players). Wire side: decode the
  `rotate_head` packet (entity id + packed-degree `yHeadRot`) + the
  `add_entity` head-yaw byte (previously dropped) → `EntityState.head_yaw`
  (defaults to body yaw). Render side: `EntityDraw.head_yaw` flows to
  `emit_model`, where `LimbPart::Head` quads yaw by the head's **own**
  absolute angle instead of the body's. This collapses to a one-line
  per-part angle swap because the humanoid neck pivot sits at x=z=0 — two
  Y-rotations about the same vertical axis compose, so "head-about-neck +
  body-about-origin" *is* "head-yaw-about-origin". The quadruped head is a
  rigid `Body` part, so cows are correctly excluded (a v1 simplification).
- **Caught + fixed a regression I introduced in the same change:** the new
  `let (c, s) = …` head/body-yaw binding **shadowed `emit_model`'s `s` model
  scale**, so the `d.pos + p*s` placement multiplied by `sin(yaw)` instead of
  1/16 — every `emit_model` mob silently scaled by `sin(yaw)`, vanishing near
  yaw 0/±180 and rendering at wrong sizes elsewhere. The visual verification
  is exactly what surfaced it (the humanoid was invisible when aimed at
  head-on); renamed to `(cyaw, syaw)`. Lesson: a render artifact that
  correlates with *orientation* is a rotation/scale term leaking.
- **Verified** headlessly: summoned a `husk` (humanoid model, doesn't burn in
  daylight — clean capture) with `NoAI` + a fixed body rotation facing the
  camera, then rendered a **fixed-framing A/B** — `REWO_FORCE_HEAD=0` (head
  aligned, face head-on, both eyes symmetric) vs `REWO_FORCE_HEAD=70` (same
  body/pose/framing, head cranked into profile). Only the head differs. New
  reusable verification knobs added to `rewo live`: `REWO_SUMMON_DIST` /
  `REWO_SUMMON_DY` (place a summon at range / floated into empty sky),
  `REWO_PRECMD` (one op command before the summon — e.g. clear a scene),
  `REWO_LOOK_AT="x,y,z"` (deterministic fixed-point camera aim),
  `REWO_LOOK_HIGH` (aim at the highest matching mob), `REWO_FORCE_HEAD`.
  0 VUIDs; decode-side + live-only render → demo **byte-identical**, bench
  flat (avg 0.25 ms); 24 rewo-gpu + 6 rewo-net + world tests green.

**2026-07-22 — F3 debug overlay.**

- The single top-left coords line grew into a vanilla-style **F3 block**:
  version header (+ `fps` in windowed), `XYZ`, `Block` world + in-chunk
  coords, `Chunk`, a `Facing` line (compass name + Towards-axis hint + yaw/
  pitch), and a `Loaded chunks / Entities / grounded` line. Chunk math uses
  `rem_euclid`/`div_euclid` so block-in-chunk + chunk index are correct in
  the negative hemisphere (plain `%`/`/` truncate toward zero — wrong across
  the origin). `F3` toggles it in the windowed client (default on); always on
  in headless so a verification PNG shows the state.
- Live-only (`demo`/`bench`/`view` never call `build_text`) → those gates
  unchanged. **Verified** in a headless PNG: all six lines correct at spawn
  (block 6/-60/4, in-chunk [6 4 4], chunk 0/0, facing S/+Z, 329 loaded). A
  bonus in the same shot: a leftover summoned husk renders at correct scale
  with its head **naturally** turned toward the client — real server
  `rotate_head`, confirming head-look under live data, not just the forced
  knob. 4 rewo-app tests green.

**2026-07-22 — the pig (fourth mob model; retiring more capsules).**

- **Grew the entity atlas 256×128 → 256×256** so more mob skins fit without a
  texture-array refactor (UVs already normalize by the atlas dims, so this is
  a one-const change + a blit; the bottom half is expansion room). Font +
  player/slime/zombie/cow stay in the top half; the pig sits at (0, 128).
  The font/nametag sampling is unaffected (UVs are atlas-relative), and the
  F3 text + HUD are separate passes with their own atlases.
- **`quadruped_model_quads` generalized** from cow-only to `(off_x, off_y,
  leg, snout)`: `leg` is the vanilla legSize (cow 12, pig 6) — it also drives
  the head/body pose height (18/17 − legSize, which is why `PigModel`'s head
  offset is `(0, 12, -6)`); `snout` appends the pig's nose box (vanilla
  `texOffs(16,16) addBox(-2,0,-9, 4,3,1)`, riding the head pose). The cow now
  calls it with `(…, 12, false)` — an identical parts list to before, so cows
  render unchanged. New `EntityModelKind::Pig` + `pig_tex`
  (`entity/pig/pig_temperate.png`) threaded through `bake` → `init_entities`
  → `EntityPass::new`; `minecraft:pig` maps to it in `collect_entities`.
- **Verified** with summon renders: a **pig** (pink, squat, short legs, snout
  clearly on the face) and a **cow** (taller legs, brown/white — visually
  unchanged) side by side confirm the generalized model serves both. Tests
  4+5+3+3+18 green; **demo byte-identical** (md5 match) + entities live-only
  so bench/view unaffected; 0 VUIDs.

**2026-07-22 — the sheep (fifth mob; the farm-animal trio complete).**

- The sheep has its **own body dims** (unlike the pig, which reused cow
  dims): head 6×6×8, body 8×16×6, legSize 12 (vanilla `SheepModel`), plus the
  iconic **inflated wool overlay** (vanilla `SheepFurModel`: head 6×6×6 +0.6,
  body 8×16×6 +1.75, upper-legs 4×6×4 +0.5) sampling a second texture. So the
  cow-only `quadruped_model_quads` was refactored: extracted `build_quad_parts
  (parts, off_x, off_y)` (the xform + box-UV loop) as the shared builder;
  cow/pig call it with their `(leg, snout)` parts (identical output — cow
  renders byte-for-byte the same, re-verified), and `sheep_model_quads` builds
  the sheep body (sheep slot) **plus** the 6-box wool overlay (wool slot) and
  concatenates. The wool renders on top; transparent wool texels alpha-discard
  to show the body, and the inflation puts the wool just outside the body so
  reversed-Z sorts it in front — vanilla's fleece look. Wool dye-tint deferred
  (white for v1).
- Infra: **grew the atlas usage** (two 64×32 sheep slots in the lower half;
  generalized `blit_64` → `blit_tex(…, w, h)` for the 64×32 sheep textures),
  and **replaced `EntityPass::new`'s 8-positional-`Option<&[u8]>` texture
  list with an `EntityTextures` struct** (one named field per skin — call
  sites can't transpose them; `entity_textures(&baked)` builds it). New
  `EntityModelKind::Sheep`; `minecraft:sheep` maps to it.
- **Verified** with a summon render: a fluffy white sheep (wool body/head +
  the tan lower legs showing beneath the wool upper-legs), and a re-rendered
  **cow unchanged** by the refactor. Tests 4+5+3 green; **demo byte-identical**
  (full md5 `ee6e26f4…`); entities live-only so bench/view unaffected; 0 VUIDs.
