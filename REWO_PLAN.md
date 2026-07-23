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

### Where it is: M0–M7 shipped + M9 (CEM/Fresh Animations), all pushed

**Latest (2026-07-22, one long session): M7 online-mode, real skins,
metadata mob detail, and the whole M9 CEM stack — see §15 for the blow-by-
blow.** Headlines:
- **M7 online-mode** — login encryption (RSA→sessionserver join→AES-128-
  CFB8) + **signed chat** (player cert → chat_session_update → SHA256withRSA
  chain). Verified with the user's REAL account on an `enforce-secure-
  profile` server (chat logged with no `[Not Secure]`). `crypt.rs` +
  `chat_sign.rs`; account handoff via `REWO_ACCESS_TOKEN/UUID/USERNAME`
  (`ewolauncher --mint-rewo-env` prints them). The offline path is
  unchanged (offline servers ignore the auth).
- **M7c real player skins** — decode the Player Info `textures` property →
  fetch the PNG → upload into a 32-slot atlas pool at runtime → relocate the
  player quads via a UV offset. Slim + wide models. `mobshot --skin
  <user|url>` verifies serverless.
- **Metadata mob detail** — slime/magma **size** + **baby** scaling, both at
  metadata **index 16** (polymorphic: INT=size, BOOLEAN=baby; serializer
  type disambiguates). `EntityDraw.scale_mul`.
- **M9 native CEM (the EMF/ETF-equivalent) — models AND animations run**
  from a real OptiFine resource pack, no mod. `rewo-data/src/cem.rs` (pack
  zip loader), `rewo-gpu/src/cem.rs` (JEM→Model, named bones), `cem_anim.rs`
  (the OptiFine expression interpreter). Verified on the user's **Fresh
  Animations** pack: all body plans render + **animate** (zombie strides,
  pig/cow walk). `mobshot --pack <zip> [--walk sw,amt --time t]` +
  `rewo live --pack`. **Two load-bearing CEM facts**: (1) a top-level
  `.jem` `part.translate` is the *rotation pivot* (`pivot = to_model(
  −translate)`), NOT static position — only *submodel* translates
  accumulate; (2) the model is baked through a 180° Z-rotation
  (`invertAxis:"xy"`), so the animation's X/Y rotations + translations are
  **negated**, Z passes through. **M9d (2026-07-23, §15) closed the polish
  list**: per-face UVs (creeper/zombie/pig eyes + pig snout), scale channels
  (+ bone-channel reads + file-order via `indexmap`), and **submodels-as-bones**
  (every `.jem` node is now a parented bone, so head-look/blink/feet animate;
  the "~1px foot pivot" is subsumed). Two more asymmetries surfaced there:
  a submodel's pivot is its *accumulated position* (`to_model(boxOff)`, not
  −translate), and OptiFine translation **replaces** a bone's translate
  (subtract a per-bone rest baseline, else the pig head flings off). CEM polish
  left: **ETF random/emissive textures (M9b)** + per-face `uvUp/uvDown` winding
  (visual-only-verified) + the `.jpm` `"model"` geometry ref.

Original M0–M6 (still the foundation):
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
- `rewo mobshot [--out png]` — serverless contact sheet of every mob model
  (no server); `rewo mobshot --check` is **the mob-texture gate**: facelabel
  textures (`REWO_MOB_DEBUG_TEX`) rendered from front/left/top per mob, each
  view's dominant color asserted against a perspective ray-cast of the same
  geometry's face labels (occlusion-exact). Run it after ANY mob/UV change
  (must stay 243/243). Also: `--only <mobs>` (closeups), `--walk sw,amt` +
  `--time t` (pose the animation), `--skin <user|url>` (real player skin,
  M7c), `--pack <zip>` (render OptiFine CEM models + animations, M9),
  `--gesture name[,age]` / `--shell` (gesture rigs).
- **The test servers**: local flat-world vanilla 26.2 servers the assistant
  sets up + runs. **Offline** at `%APPDATA%/EwoClient/rewo/26.2/testserver/`
  (online-mode=false, port 25599, bot is op'd as `RewoOp` for
  `REWO_SUMMON`); **online** at `…/testserver-online/` (online-mode=true,
  `enforce-secure-profile=true`, port 25600) for M7 verification (join with
  `ewolauncher --mint-rewo-env` env). Start with the temurin-25 JRE at
  `%APPDATA%/EwoClient/jdks/temurin-25/`. Wipe `world/` for a clean run.
  **Gotcha**: `Stop-Process` by `-like '*testserver*'` won't match — the
  java command line is just `java -jar server.jar`, no cwd; kill by PID.
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
  limb swing**), and **slimes**, **cows**, **pigs**, **sheep**, and
  **zombies/husks/drowned** render as **model-shaped** mobs with correct
  silhouettes + animation (walk-swing, humanoid head-look). The 2026-07-22
  scrambled-texture bug (`box_uv_faces`, a non-faithful box-UV approximation)
  was **fixed by the mob redo (2026-07-22)**: `rewo-gpu/src/mobs.rs` is now a
  **verbatim port of vanilla `ModelPart.Cube`/`Polygon`** (per-face vertex
  arrays, UV columns, UP-flip, mirror reversal) plus vanilla's exact entity
  transform (`rotY(180−yaw)·scale(−1,−1,1)·translate(0,−1.501,0)` — the old
  path also had the X sign mirrored), with every mob mesh transcribed from
  the 26.2 decompile (the 26.2 cow is its own mesh — horns/muzzle/udder —
  not the generic quadruped). **77 mob models ship** (2026-07-22 second
  pass, "all the mobs"): the full zombie/skeleton families (incl. bogged
  w/ head mushrooms + parched), creeper, spiders, enderman, slime + proper
  layered magma cube, farm set (cow/mooshroom/pig/sheep+wool/chicken),
  wolf, squids, rabbit, villager/wandering trader/witch (64×128 hat
  stack)/zombie villager, illager quartet (crossed-arm vindicator/evoker/
  illusioner, armed pillager), vex, phantom, guardian + elder (2.35×),
  shulker, silverfish/endermite segment chains, blaze (12 rods),
  ghast (4.5×, tentacle lengths from vanilla's seeded `Random(1660)` —
  a Java-LCG port), piglin family, hoglin/zoglin, strider, bat, cat/ocelot,
  fox, goat, bee, frog + tadpole, armadillo, axolotl, dolphin, turtle,
  cod/salmon/pufferfish/tropical fish, panda, polar bear (1.2×), camel,
  llama/trader llama, parrot, horse/donkey/mule/skeleton+zombie horse,
  snow/iron golem, allay — and (third pass, same day) **the rest**:
  warden, sniffer (192²), breeze (+its 128² wind-funnel texture),
  creaking, ravager, wither, **ender dragon** (256², full 30-cube mesh
  with membrane wings), happy ghast (4.0×), copper golem (mind its
  root's +24 translate), nautilus + zombie nautilus. **88 mob models
  total** — every living vanilla mob; capsules remain only for object
  entities (armor stands, boats, minecarts, projectiles). Entity atlas is
  1024² with a shelf packer (16²..256² textures). Verified by the
  **`rewo mobshot --check` facelabel gate** (246/246 mob-views; 6 mobs —
  guardian/elder, bee, pufferfish, sniffer, breeze — are auto-detected as
  color-check-N/A because their vanilla textures reuse the same texels
  across face labels, and are skipped with a notice) + closeup sheets
  (`--only`). **Animations (2026-07-22 pass): every procedural vanilla
  `setupAnim` is implemented formula-exact** — walk gaits + head-look plus
  spider leg waves, wolf tail wag, golem triangle-wave limbs, blaze rod
  orbits, ghast/squid tentacles, phantom/allay/vex/bee wing flaps, fish
  tail sways, silverfish wiggles, wither side-head tracking (parts carry
  base rotations, a parent hierarchy, and pivot-motion anims;
  `set_entities` takes a time param, `mobshot` has `--time`/`--walk`).
  **Keyframe rigs run too (same-day fourth pass)**: vanilla
  `AnimationDefinition`s machine-extracted from the decompile
  (`tools/gen_anim_defs.ps1` → generated `anim_defs.rs`) and played by a
  vanilla-exact evaluator in `part_transforms` — frog/camel/sniffer/
  armadillo/creaking/copper-golem walks, bat flight, breeze idle,
  nautilus swim, rabbit hop (jump-state approximation, documented).
  **Gesture rigs run too (fifth pass)**: entity Pose metadata (index 6)
  + sniffer/armadillo state enums (index 17) are decoded, a
  `GestureTracker` times each rig from the observed state change
  (vanilla clocks from the transition; wire carries only current state),
  and `KfGate::{During, Unless, NotShell}` + `KfDriver::GestureAge` +
  part `Show` rules play warden roar/sniff/emerge/dig, frog croak
  (+throat pouch visibility)/tongue, breeze shoot/slide/inhale/jump,
  sniffer dig/long-sniff/happy/rise + the SEARCHING walk-swap, and the
  armadillo roll/scared/unroll **with the shell-ball visibility swap**
  (`mobshot --gesture name[,age] [--shell]`,
  `REWO_FORCE_GESTURE=name[,age]`).
  Still open: entity *collision* is ignored (walk through mobs),
  entity-*event*-driven anims (armadillo re-peek event 64, warden
  attack/sonic-boom, creaking attack, allay dance) need the
  entity_event/jukebox packets, dragon flight is bespoke procedural code
  (not a rig) and stays posed, sheep wool dye-tint deferred (white),
  slime/magma **size now decoded** (metadata index 16 → linear model
  scale, §15) — their face detail still needs the translucent pass;
  texture variants are fixed picks (tabby cat, brown horse, creamy
  llama, lucy axolotl, temperate chicken/frog…), ~~no real per-player
  skins (M7)~~ **real per-player skins shipped (M7c, §15)** — slim + wide,
  fetched from the profile `textures` property, uploaded into a 32-slot
  atlas pool; tags are depth-tested.
- ~~**Collision is full-cube only**~~ / ~~**entity collision is ignored**~~ —
  **RESOLVED 2026-07-23 (§15)**: per-block collision *shapes* (slabs, stairs,
  fences, trapdoors, carpets, …) and vanilla entity pushing. Verified live:
  **0 corrections** walking on a slab floor and with a mob shoving the player.
- **Physics parity verified only for the on-foot flat-world subset.** Water,
  cobwebs, stairs-vs-sneak, ladders, ice, slabs-as-steps, etc. untested —
  each will likely need decompile-derived constants (§13 risk).
- **Entities are now world-lit** (2026-07-23, §15) — they were rendering at a
  fixed directional shade, identical in a cave and at noon.
- ~~**Light decode beyond flat-world-full-skylight is unverified**~~ —
  **VERIFIED CORRECT 2026-07-23 (§15)**: measured in a sealed torch-lit room
  (sky 0, block light peaking at the torch with exact 1-per-block falloff) and
  under open sky (sky 15). The real gap is different and still open: **there is
  no client-side light engine**, so lighting never changes after chunk load.
- **Only the overworld dimension is tested.** Nether/end (different
  min_y/height via the registry parse) is coded but unexercised.
- ~~**No online-mode / encryption / chat signing**~~ — **shipped M7
  (2026-07-22)**: AES-128-CFB8 login encryption + Mojang session join +
  SHA256withRSA signed chat, verified on an `enforce-secure-profile`
  server with the user's real account (§15). Leftovers: last-seen chat
  tracking (empty acknowledged set), mid-session key-expiry refresh.
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

Shipped 2026-07-22 (all §15): **M7 online-mode + signed chat** (Rewo joins
Frogsy), **M7c real player skins**, **metadata mob size/baby**, and the
**M9 CEM stack** (Fresh Animations models + animations run natively).
Remaining candidates, roughly by value: (a) **CEM polish** — foot-submodel
leg pivots ~1px off, per-face `uvNorth` (creeper-eye detail), scale
channels, ETF random/emissive textures (M9b); (b) **entity collision** (you
walk through mobs) — a real gameplay gap; (c) **async transfer queue +
staging ring** — the last perf micro-deviation (§4); (d) visual follow-ons
(greedy meshing, biome tint, packed vertices). Confirm direction with the
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
- **M7 — Online-mode + hardening.** ✅ **core shipped 2026-07-22** (§15):
  AES-128-CFB8 encryption + sessionserver join with the launcher token +
  **chat signing** (player certificates + signed chain) for
  `enforce-secure-profile` servers. DoD met: full session on an
  online-mode server with enforcement on, using the user's real account.
  Leftovers (additive): resource-pack policy (D8), transfer/cookies +
  reconnect UX, last-seen chat tracking, mid-session key-expiry refresh.
- **M8 — Advanced (optional, A/B'd).** HZB occlusion; mesh-shader path vs
  M5 baseline; Velvet overlay (Skia-Vulkan, §9.4); ~~player/mob model +
  animation port~~ (shipped 2026-07-21/22 — 88 mobs + full animation
  stack, see §15); local relight; shader/PBR track. Each lands only if
  the replay benchmark says it pays.
- **M9 — Native resource-pack entity models + textures (CEM/ETF).** (M)
  What EMF/ETF do as Fabric mods, built in as plain asset loading — no
  mod system. Rewo is unusually well-positioned: a `.jem` part tree
  (pivot/rotate/box-UV cubes) is structurally our `Model` IR, every box
  goes through the same verbatim `cube_faces` port the facelabel gate
  validates, and OptiFine's animation-expression variables
  (`limb_swing/limb_speed/age/head_yaw/head_pitch/hurt_time…`) are
  already on `AnimCtx` where `part_transforms` composes deltas.
  Ladder inside the milestone:
  - **M9a — pack plumbing + CEM static models**: resource-pack stack
    layered over the client jar in `assets::bake` (jar < pack₁ < pack₂);
    `.jem`/`.jpm` parser building `Model` at bake; per-entity override
    registry (`optifine/cem/<entity>.jem`). OptiFine axis/pivot quirks
    ("translate" semantics, attach, jpm includes) are the known jank —
    EMF's author documents them.
  - **M9b — ETF textures**: `optifine/random/entity/*.properties`
    variant lists + weights + conditions (name/biome/baby…), picked
    UUID-stable per entity (a `tex_variant` on `EntityDraw` + more atlas
    entries); emissive `_e.png` overlays as always-fullbright quads (a
    per-quad light-ignore flag — vertex color already carries light).
    This also subsumes the "texture variants are fixed picks" gap (cat/
    horse/llama/axolotl/frog/… variants become per-entity choices).
  - **M9c — animation expressions**: OptiFine's expression language
    (parser → AST, evaluated per part per frame in `part_transforms`
    alongside `Anim`/`KfAnim`) — the piece that makes Fresh
    Animations-class packs work.
  DoD: a real published CEM pack renders its custom models headlessly
  (`mobshot --only` closeups with the pack loaded); random-texture +
  emissive packs verified the same way; facelabel gate still 246/246
  with no pack loaded (pack content is additive, never a regression
  surface). Rough effort: M9a 1–2 days, M9b 1 day, M9c 2–3 days.

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

**2026-07-22 — mob textures are wrong; redo handed off.**

- User review of the shipped mobs: the textures are **scrambled** (the cow has
  no readable face). Correct: the mob models are *shape*-right but *UV*-wrong.
- Root cause: `box_uv_faces` (entities.rs) is a hand-rolled approximation of
  MC's box-UV unwrap that gets the face→sub-rect mapping and the per-face
  vertex→UV-corner ordering wrong (and has no mirror / UP-flip). Every mob
  shares it, so every mob is wrong. The geometry/animation layer is fine.
- **The verification failure that let this ship:** the mob passes checked
  silhouette + dominant colour ("pink & squat = pig") and called that
  "verified." That can't detect a scrambled UV map. Going forward, mob/texture
  work must verify **texture-face correspondence** (a face-labeled debug
  texture rendered from 3 angles), not just shape.
- Full redo brief written: [`REWO_MOB_REDO_HANDOFF.md`](REWO_MOB_REDO_HANDOFF.md)
  — the verbatim vanilla `Cube`/`Polygon` algorithm to port, the file/symbol
  inventory, the per-mob vanilla model sources, the mandatory verification
  method, and the keep/don't-touch + traps list. The redo itself is handed to
  a separate stronger pass (user's call).

**2026-07-22 — the mob redo shipped: faithful Cube port, 21 mobs, facelabel
gate green.**

- **`crates/rewo-gpu/src/mobs.rs` (new)** — verbatim port of vanilla
  `ModelPart.Cube` + `Polygon` from the 26.2 decompile: the 8 corners, the
  per-face vertex arrays, the UV column table, the UP-face v-flip, mirror =
  swap-X + reverse-vertex-array. Models are authored in **vanilla model
  space** (y-down px, front −Z) and rendered through vanilla's exact
  transform chain `rotY(180°−yaw) · scale(−1,−1,1) · translate(0,−1.501,0)`
  — the decompile of `LivingEntityRenderer.render` settled the order, and
  exposed that the old path's `(−x, 24−y, −z)` conversion had the **X sign
  wrong** (every mob was also left/right-mirrored on top of the scrambled
  UVs). Unit tests pin the humanoid head's 6 faces to hand-computed vanilla
  UV corners, the mirror/grow semantics, and the world orientation.
- **Part/animation model**: cubes attach to `Part`s (pivot + `Anim` kind +
  amplitude); static pose rotations (quadruped bodies, zombie arms, spider
  leg splay, villager arm cross, rabbit pose chains) fold into quad vertices
  at build via `Fold` chains — animated parts always rest at identity, so
  `emit_model` is just per-part `rotateZYX(0, netHeadYaw, pitch)` /
  `Rx(swing)` about the pivot. Head yaw is now vanilla's **net-of-body**
  rotation about the head's own pivot (was: absolute yaw about the model
  origin, which only worked because humanoid necks sit at x=z=0).
- **21 mob models** transcribed from the decompiled 26.2 meshes (each fn
  cites its class): player (wide + overlays), zombie/husk (1.0625×)/drowned
  (+outer layer), skeleton/stray (+overlay)/wither skeleton (1.2×), creeper,
  spider/cave spider (0.7×), enderman (half-amp swing), slime, **cow (26.2's
  own mesh: 8×8×6 head + muzzle + horns, 12×18×10 body + udder, legs at ±4
  — not the generic quadruped the first pass guessed)**, pig (quadruped
  legSize 6 + snout), sheep + wool (mirrorRight legs per source), chicken
  (beak + wattle), wolf (skull/ears/snout + two body segments + tail),
  squid (8 tentacles), rabbit (full static pose-chain transcription),
  villager (nose + hat rim + robe + crossed arms + half-swing legs).
- **Texture plumbing generalized**: rewo-data bakes a keyed
  `MobTexture` table (`MOB_TEXTURE_SPECS`, 24 entries incl. chicken_temperate
  / pig_temperate / cow_temperate 26.x names); the entity atlas grew to
  512×512 with fixed 64×64 slots around the font block; `EntityPass` builds
  every registry mob whose textures are present (missing → capsule,
  warn-once). `EntityModelKind` + `kind_for_entity_name` live in mobs.rs.
- **Verification (the part that failed last time)**: new **`rewo mobshot`**
  subcommand — serverless offscreen render. `--check` paints every box-UV
  face rect a per-label color, renders each mob front/left/top, and asserts
  the rendered dominant color equals the label predicted by a
  **perspective ray-cast of the same geometry** (same eye/fov → occlusion
  and projection match exactly; near-ties accepted only when the rendered
  label's predicted share is ≥80% of max). **63/63 mob-views pass.** The
  checker caught real subtleties along the way: a chicken from above
  correctly shows its rotated body's *South* rect; a villager's hat rim
  wins its top view; a zombie's hat beats its outstretched arms only under
  perspective. Contact sheet (`--out`) + live summon shots (cow with
  forced head-look 35° + walk gait; enderman; creeper; villager) verified
  by eye — the cow has its face back.
- **Gates**: all crate tests green (41 across rewo-*), `rewo demo` PNG
  **byte-identical** (`ee6e26f4…`), bench percentiles flat (GPU 0.25 ms avg
  / 1.1 ms 0.1%-low), 0 VUIDs with validation on (mobshot runs it), live
  smoke on the flat-world testserver (summons + a horizon full of
  naturally-spawned mobs rendering as models).
- Deleted: `box_uv_faces`, `cuboid_quads`, `humanoid_cuboids`,
  `quadruped_model_quads`, `build_quad_parts`, `sheep_model_quads`,
  `slime_model_quads`, the per-mob `EntityTextures` fields, and rewo-data's
  duplicate `bake_player_skin`.

**2026-07-22 — "all the mobs": the roster grows 21 → 77, gate 231/231.**

- Same-day second pass on the new builder. ~40 new model fns transcribed
  from the decompile (each cites its class), plus texture-variant reuses
  (mooshroom=cow, wandering trader=villager, parched=skeleton,
  zoglin=hoglin, ocelot=cat, trader llama=llama, elder=guardian×2.35,
  horse family shares `AbstractEquineModel` ± donkey ears).
- Infra: entity atlas 512² → **1024² with a shelf packer** (tallest-first,
  font block reserved; textures 16² tadpole → 192²-class); per-texture
  **UV clamping** (a few vanilla fin rects stray negative/out-of-texture —
  clamp-sampler behavior, else they'd bleed into atlas neighbors);
  zero-area faces of plate boxes (fins/wings/0-thick bristles) skipped at
  build. Vanilla mechanics captured: `MeshTransformer.scaling` is
  ground-anchored (`y'=k·y+24.016(1−k)`) ≡ our `Model.scale` (ghast 4.5,
  polar bear 1.2, elder 2.35 need no special casing);
  `PartDefinition.addOrReplaceChild` **preserves replaced children** (the
  witch keeps the villager nose/hat); ghast tentacle lengths reproduce
  vanilla's seeded `Random(1660)` via an exact Java-LCG port.
- `kind_for_entity_name` is now generated from the kind names (1:1 with
  wire ids; unit test asserts every registry def round-trips).
  `rewo mobshot --only <names>` renders closeup sheets for detail review.
- Gates: **231/231 facelabel views** (the perspective ray-cast reference
  scaled to 77 mobs unchanged), 41 crate tests, demo PNG byte-identical,
  bench flat, 0 VUIDs. Closeups eyeballed: guardian spikes/eye/tail,
  blaze rings, hoglin tusks+bristles, piglin snout/ears, horse, iron
  golem, witch hat stack, camel, strider, fox/cat/axolotl/turtle/frog/
  bee/dolphin/parrot/allay all read correctly.

**2026-07-22 — the rest: 77 → 88, every living vanilla mob modeled.**

- Third same-day pass: warden (tendril plates, ribcage overlays), sniffer
  (192² — six legs, moss-back plate), breeze (head + 3 rods on breeze.png
  **plus the wind funnel on breeze_wind.png as texture 1** — first
  two-texture mob beyond the overlay pattern), creaking (twig crown,
  0-thick foot fans), ravager, wither (3 skulls; the tail pose is
  *computed* in vanilla — `6.9 + cos(0.204)·10` — transcribed exactly),
  **ender dragon** (256²: head+jaw, 5+12 spine segments, 24×24×64 body,
  two-segment wings with 0-thick membrane skins at negative texOffs,
  3-segment legs ×2 sides), happy ghast (body + inner cube + 9 dangling
  legs, mesh-scale 4.0), copper golem (the whole mesh rides a
  `transformed(p → p.translated(0,24,0))` root — easy to miss), nautilus
  (+ zombie nautilus texture reuse).
- The facelabel checker gained **ambiguity auto-detection**: painting
  tracks per-texel labels, and a texture where two different labels hit
  the same texel (vanilla region reuse — breeze wind's concentric
  shells, bee antennae rows, pufferfish spikes, sniffer plates,
  guardian tail) marks its mobs color-check-N/A; the gate skips them
  loudly instead of failing on paint races. 82 of 88 mobs remain under
  the strict gate — **246/246 views green** — and all 88 share the same
  verbatim Cube port those 246 views validate.
- Gates re-run: 41 tests, demo PNG byte-identical, bench flat, 0 VUIDs.
  Closeups eyeballed: dragon (spine scales, wings, purple eyes), warden
  chest glow, sniffer moss back, breeze funnel, wither triple skull,
  nautilus spiral, ravager horns, creaking eyes, happy ghast all read.

**2026-07-22 — the animation pass: every procedural vanilla anim, exact.**

- The part system generalized twice: parts carry **base Euler rotations**
  (vanilla sums pose + setupAnim angles and composes ONE `rotateZYX` —
  spider-leg splay + walk-wave now share a part), and parts form a
  **runtime hierarchy** (`parent` links) so phantom wing *tips* bend on
  their wing *bases* and vex arms/wings ride the π/20-tilted body. Some
  anims also *move* pivots (vanilla repositions blaze rods and crawler
  segments per frame). `part_transforms` composes it all and is shared by
  `emit_model` AND the mobshot prediction, so the facelabel gate can never
  disagree with the renderer (checked at the gate's fixed t=0 inputs —
  still 246/246 green, still deterministic).
- `set_draws`/`set_entities` gained a `time` parameter (vanilla
  `ageInTicks` = seconds·20); `rewo live` passes real elapsed time,
  `rewo mobshot` passes `--time` (sheet) and 0 (gate). `--walk swing,amt`
  poses the walk-driven anims for stills.
- Implemented from the decompiled `setupAnim` bodies, formula-exact:
  spider 8-leg walk wave (yRot/zRot ± phases), wolf tail wag, iron golem
  triangle-wave arms+legs (`triangleWave(pos,13)`), blaze rod ring orbits
  (three counter-rotating rings + per-rod y-bob), ghast tentacle sway
  (`0.2·sin(age·0.3+i)+0.4`), phantom two-segment wing flap + tail bob
  (`flapTime·7.448451°`), allay hover-flap (`cos(age·20°+pos)·π·0.15+amt`
  with the fly/idle blend), vex arm bob + wing flutter (`45.836624°`
  rate), bee wing buzz (`120.32113°` rate, airborne pose with tucked π/4
  legs), cod/tropical tail + salmon back-half sway (`−0.45/−0.25·
  sin(0.6·age)`), pufferfish fin flutter, dolphin two-segment tail stroke
  (gated on movement), silverfish/endermite segment wiggle (yRot AND x
  displacement, per-species amplitudes), wither side heads now track the
  look. Squid tentacles get a gentle curl (the entity-driven
  `tentacleAngle` isn't on the wire — approximation noted).
- Still keyframe-rigged in vanilla (NOT procedural — needs an
  AnimationDefinition player, a separate follow-up): frog/camel/sniffer/
  armadillo/rabbit walk cycles, bat flight, creaking/copper-golem/warden
  gestures. Their poses stay static; their walk-gait approximations (where
  given) remain.
- Gates: 41 tests, `mobshot --check` 246/246 at t=0, demo byte-identical,
  bench flat, 0 VUIDs. Animated sheet (`--time 0.42 --walk 2.2,0.8`)
  eyeballed: golem mid-stride, spider leg wave, orbiting blaze rods,
  curled squid, swept ghast tentacles, flapping phantom all read.

**2026-07-22 — the keyframe player: vanilla AnimationDefinition rigs run.**

- The last animation gap closed. Vanilla's keyframe rigs are *Java code*,
  not assets — so **`tools/gen_anim_defs.ps1` machine-extracts them from
  the decompile** into `crates/rewo-gpu/src/anim_defs.rs` (generated,
  checked in; re-run after a version bump): 11 defs — FROG_WALK,
  CAMEL_WALK, SNIFFER_WALK, ARMADILLO_WALK, BAT_FLYING, CREAKING_WALK,
  COPPER_GOLEM_WALK + IDLE, NAUTILUS_SWIMMING, BREEZE_IDLE, RABBIT_HOP —
  with `degreeVec`→radians, `posVec` y-negation, and scale-channel
  omission baked at generation. Machine extraction, not hand transcription
  — hundreds of keyframes with zero typo surface.
- Runtime (`KfDef/KfChannel/KfFrame` in mobs.rs, evaluation in
  `part_transforms`): vanilla-exact — prev/next frame search, the *next*
  keyframe's interpolation mode, `Mth.catmullrom` over the surrounding 4,
  looping via elapsed-mod-length, values ADD onto the pose
  (`offsetRotation`/`offsetPos`). Drivers mirror the call sites:
  `applyWalk(pos·50·speed_f ms, min(amt·scale_f, 1))` per mob's exact
  params (sniffer's odd `(9, 100)`, nautilus's `pos + age/5, amt + 0.2`
  blend), `Age` for bat flight/breeze idle/copper-golem idle, and an
  `AgeGatedByWalk` approximation for RABBIT_HOP (vanilla triggers it per
  jump; jump state isn't on the wire — a moving rabbit hops continuously,
  documented).
- The 10 rigged mobs were remodeled from fold-chains into **named part
  trees** (`Part.name` + parent links) matching vanilla's bone names, so
  channels resolve at build (`registry_builds_clean` fails loud on a
  missing bone). Camel/sniffer/armadillo/creaking/copper-golem quad-gait
  approximations were *deleted* — the real rigs drive their legs now.
- One checker fix shaken out: 0-thick plates (bat wings) have coplanar
  N/S faces; the renderer resolves them deterministically (strict depth
  test → first-drawn wins) while the ray-cast tie-broke on float noise.
  The prediction now keeps the first-emitted quad within a 2e-5 epsilon —
  matching the renderer by construction. Gate: **246/246** with the rigs
  live (t=0 rest pose is part of the checked geometry).
- Gates: 41 tests, demo PNG byte-identical, 0 VUIDs; bench numbers this
  run were contaminated by a game running on the box (~69% CPU) — the
  bench path renders no entities and demo is byte-identical, so the
  render path is provably untouched. Animated closeup eyeballed: bat
  mid-flap, rabbit mid-hop (ears back), camel/creaking mid-stride,
  copper-golem arm swing, breeze funnel spun, sniffer head-dip.
- Still static (state-driven, not walk/idle): gesture rigs (warden
  roar/dig, creaking attack, sniffer dig/sniff, armadillo roll, frog
  tongue/croak, allay dance) — they need entity event/pose state off the
  wire, not more animation machinery.

**2026-07-22 — gesture rigs: pose/state one-shots live off the wire.**

- The state-driven animation gap closed. **Wire side**: metadata decode
  gained the Pose serializer (index 6, type 20 — the `Pose.java` id order:
  ROARING=11, SNIFFING=12, EMERGING=13, DIGGING=14, CROAKING=8,
  USING_TONGUE=9, SLIDING=15, SHOOTING=16, INHALING=17, LONG_JUMPING=6)
  and the mob-state enums at index 17 (SNIFFER_STATE=35 /
  ARMADILLO_STATE=36 / COPPER_GOLEM_STATE=37 varint ordinals);
  `EntityTable` stores both per entity, cleared on remove.
- **Timing**: the wire carries only the *current* state — vanilla times
  the rigs from the transition (`AnimationState.start(tickCount)`). A
  `GestureTracker` in `rewo live` records the instant each entity's
  pose/state changes; age-in-seconds feeds the rig clock. Entering
  armadillo SCARED pre-advances the clock 2.5 s — vanilla's
  `fastForward(SCARED.animationDuration())` — landing the non-looping
  PEEK rig on its held tucked-ball end pose.
- **Gate/driver split** in `KfAnim` (mirrors the `setupAnim` call
  patterns exactly): `KfGate::{Always, During(g), Unless(g), NotShell}` ×
  `KfDriver::GestureAge` (one-shot `apply(state, age)`, holds last frame
  past the end) — so the sniffer's SEARCHING is a *walk-driven swap*
  (`During(Searching)` + `Walk{9,100}` replacing the normal walk's
  `Unless(Searching)`), the armadillo walk yields while balled
  (`NotShell`), and dig/roar/croak/… are `During + GestureAge`. 19
  gesture defs extracted (WARDEN_ROAR/SNIFF/EMERGE/DIG, FROG_CROAK/
  TONGUE, BREEZE_SHOOT/SLIDE/INHALE/JUMP, SNIFFER_DIG/LONGSNIFF/HAPPY/
  STAND_UP/SNIFF_SEARCH, ARMADILLO_ROLL_UP/ROLL_OUT/PEEK; SNIFFSNIFF is
  scale-only → no rig, noted).
- **Visibility rules** (`Show` on parts, filtered in `emit_model` AND
  `neutral_quads` so the facelabel prediction can't disagree): armadillo
  shell swap — hiding shows the `cube` ball and hides body cubes/tail/
  hind legs exactly like vanilla's `skipDraw` (head + front legs keep
  rendering, tucked *inside* by the held rig — `Show::{ShellOnly,
  NotShell}`); frog `croaking_body` is `Show::During(FrogCroak)`
  (vanilla's `isStarted()` visibility). Shell timing per state:
  ROLLING balls after 5 ticks, SCARED always, UNROLLING opens at
  tick 26 (`shouldHideInShell` per-state overrides, transcribed).
  The frog's head became a real named part (FROG_TONGUE tilts it open) —
  rest-pose geometry identical.
- **Knobs**: `rewo mobshot --gesture name[,age] [--shell]` renders any
  rig at any clock (names via `Gesture::from_name`);
  `REWO_FORCE_GESTURE=name[,age]` pins live/headless sessions.
- Gates: 11 rewo-gpu tests, `mobshot --check` **246/246**, demo PNG
  byte-identical (`ee6e26f4…`), bench clean this run (GPU avg 0.227 ms /
  0.1%-low 0.533 ms — last session's spike confirmed as the reported
  machine contamination). Closeups eyeballed: warden roar (arms flung,
  mouth cavity), frog tongue-out + croak throat pouch, sniffer dig
  (head buried) + search walk, breeze shoot funnel whip, armadillo
  mid-tuck and full ball (body/legs/tail hidden, head tucked inside).
- Not wired (needs wire signals we don't decode yet): entity *events*
  (armadillo re-peek event 64, warden attack/sonic-boom, creaking
  attack), allay dance (jukebox proximity), breeze SLIDE_BACK (no pose).
  All are additive on the same gate/driver machinery.

**2026-07-22 — M7: online-mode login encryption + signed chat.**

- **The offline-only restriction is gone.** Rewo now joins `online-mode`
  and `enforce-secure-profile` servers with the launcher's real account,
  verified end-to-end against the user's own account (`lewlone`) on a
  local Paper 26.2 server.
- **M7a — login encryption** (`rewo-net/src/crypt.rs`). The clientbound
  login `hello` (encryption request) is answered: a 16-byte shared secret
  is generated, the Mojang session-join is POSTed
  (`sessionserver…/session/minecraft/join` with the Java-`BigInteger`
  server hash — the signed-hex quirk, KAT-tested against the Notch/jeb_/
  simon vectors), the secret + verify-token are RSA-PKCS1v15-encrypted
  into the `key` packet, and from the next byte on the whole stream is
  **AES-128-CFB8** both directions. The cipher is a hand-rolled CFB8 over
  the `aes` block core (NIST SP 800-38A F.3.7 KAT-tested), wired as a
  `NetStream` wrapper that ciphers transparently at the `Read`/`Write`
  seam and **splits into read/write halves each carrying its direction's
  state** (the play-phase reader thread just gets the decrypt half).
- **M7b — signed chat** (`rewo-net/src/chat_sign.rs`). On entering Play
  with an account, the client fetches its per-session key pair +
  certificate (`api.minecraftservices.com/player/certificates`), announces
  the public half in `chat_session_update`, and signs every message:
  SHA256withRSA over the verbatim `PlayerChatMessage.updateSignature`
  layout (header `int(1)` + link `sender/session/index` + body
  `salt/ts_secs/len/content` + `lastSeen`), with a strictly-incrementing
  chain index. **Proof**: on `enforce-secure-profile=true` the message
  logged `<lewlone> rewo signed chat works` with **no `[Not Secure]`
  prefix** — an unverified message gets tagged or kicked, so a clean
  prefix means the signature validated against the announced key. Cert
  gotcha (cost a round): Mojang labels the private key `RSA PRIVATE KEY`
  (PKCS#1) but ships PKCS#8 DER wrapped at 76 chars — both trip the rsa
  crate's strict RFC-7468 reader, so we strip the armor ourselves and
  parse the DER directly (PKCS#8 → PKCS#1 fallback). Fetch/parse failures
  are non-fatal (fall back to unsigned chat with a warning).
- **Account handoff**: `crypt::OnlineAuth::from_env` reads the launcher's
  existing `REWO_ACCESS_TOKEN` / `REWO_UUID` / `REWO_USERNAME` contract;
  `into_play` takes `Option<&OnlineAuth>`. The launcher grew a headless
  `ewolauncher --mint-rewo-env` (refreshes the active account's MC token
  via the existing auth chain, prints the three env lines) so the
  verification harness needs no live browser sign-in.
- **Verification server**: a second dir `testserver-online/`
  (online-mode=true, enforce-secure-profile=true, port 25600) beside the
  offline one. `rewo play` on it: **0 corrections over 520 ticks**,
  place/dig verified, encryption + session-join + signed chat all logged
  clean.
- Gates unaffected (render path untouched): 11 rewo-gpu + 10 rewo-net (2
  crypto KATs + the signed-layout guard) + 18 rewo-world tests,
  `mobshot --check` 246/246, demo PNG byte-identical (`ee6e26f4…`).
- Not yet wired (M7 leftovers, additive): last-seen message tracking (we
  send an empty acknowledged set — fine for a bot, a chatty client
  echoing others' messages needs the seen-signature cache), profile-key
  expiry refresh mid-session, and resource-pack policy (D8).

**2026-07-22 — M7c: real per-player skins.**

- Online play now shows each player's **actual skin**, not the default
  Steve. Verified headlessly against two real accounts.
- **Decode** (`rewo-net/src/skins.rs`): the Player Info ADD_PLAYER
  `textures` property (base64 JSON) → skin URL + `metadata.model=="slim"`.
  Unit-tested against a real captured value (slim) + classic + cape-only +
  garbage. `PlaySession` stores resolved skins by UUID and drains a
  pending-fetch queue (`take_pending_skins`).
- **Fetch** (`rewo-app/src/skin_fetch.rs`): a worker thread resolves a
  username→UUID→profile→URL (Mojang API) or takes a raw URL, downloads the
  PNG, and normalizes it to 64×64 RGBA (RGB/palette/grayscale via EXPAND;
  legacy 64×32 top-anchored with a warn).
- **Runtime atlas upload** (`rewo-gpu/src/entities.rs`): the entity atlas
  reserves a **32-slot 64×64 skin pool** in its bottom two rows (the mob
  packer is capped above it — still 243 facelabel views green).
  `upload_skin` region-copies a fetched skin into a free slot
  (`SHADER_READ_ONLY → TRANSFER_DST → SHADER_READ_ONLY`, `wait_idle`-fenced
  — skins arrive rarely, so the one-off stall is cheaper than per-frame
  fence tracking against the shared atlas) and returns a **normalized UV
  offset**. The player model's quads are baked against the default-Steve
  slot; adding the constant offset relocates sampling onto the player's
  slot (same 64² layout, so one offset covers every quad).
- **Slim + wide**: `PlayerModel.createMesh(_, slim)` transcribed exactly —
  `player_model(slim)` shares head/body/legs and branches only the arm/
  sleeve boxes (3-px vs 4-px). A new `EntityModelKind::PlayerSlim` (never
  wire-mapped; the caller picks it from the profile's model) renders slim
  skins on the narrow model. Overlay layers (hat/jacket/sleeves/pants) ride
  along for free.
- **Live wiring** (`rewo-app/src/live_cmd.rs`): a `SkinLoader` requests
  each newly-announced skin once, uploads finished fetches into the atlas,
  and `collect_entities` looks up each player's UUID → sets the draw's
  `skin_uv` + Player/PlayerSlim kind.
- **Verification** (`mobshot --skin <username|url>`): serverless, headless
  — fetch a real skin, upload it, render the player model with it. `--only
  player --skin lewlone` → the user's green-haired panda-hat slim skin;
  `--skin Notch` → the classic wide skin; both distinct from the default
  Steve, overlays + arm width correct. Gates: 25 crate tests (4 new skin-
  decode), `mobshot --check` 243/243 (Player/PlayerSlim share the "player"
  texture → the color checker marks it N/A, honest; geometry unchanged),
  demo PNG byte-identical (`ee6e26f4…`), bench flat.
- Leftover: the live per-player path is exercised by the same
  `upload_player_skin` the mobshot verification proves, but a two-real-
  players in-world capture (vs. the force-skin PNG) is still the user's to
  eyeball; legacy 64×32 skins render with transparent lower-body faces.

**2026-07-22 — metadata-driven slime / magma-cube size.**

- Slimes and magma cubes now render at their **actual size**, not a fixed
  medium — the "fragile entity-specific metadata index" the decoder
  deferred is now pinned exactly. `AbstractCubeMob.ID_SIZE` sits at
  **metadata index 16** (Entity defines fields 0..7, LivingEntity 8..14,
  Mob 15, AbstractCubeMob 16 — and the already-working `DATA_POSE=6`
  cross-checks the count), INT serializer. `metadata.rs` captures it,
  `EntityTable` stores per-entity size, `play.rs` applies it.
- Rendering: vanilla `AbstractCubeMobRenderer` scales the whole model
  uniformly by `size`. `EntityDraw` gains a `scale_mul` applied to the
  baked px→block scale in `emit_model`; the slime model stays baked at its
  size-2 look, so `scale_mul = size/2` (size 2 → 1.0, no default-case
  regression) and the bbox is `0.51 × size`. Non-cube mobs pass 1.0.
- Verified live on the flat-world test server (op'd summons): a `{Size:6}`
  slime renders ~3 blocks tall beside horizon-distant naturals; Size:1 vs
  Size:2 vs Size:4 scenes show the exact vanilla 1:2:4 ratio (a broken
  decode would render every slime identical). Plus a metadata unit test
  (index 16 → size). Gates: 26 crate tests, `mobshot --check` 243/243,
  demo byte-identical (`ee6e26f4…`), 0 VUIDs.
- The metadata delta stream can still bail before index 16 if a complex
  serializer (effect particles, index 10) precedes it in the same packet —
  plain cube mobs never send that, so it's reached in practice; a
  PARTICLES skip would harden it. Per-family variant enums
  (cat/horse/sheep-colour…) are the same machinery at other indices —
  still fixed picks, a documented follow-up.

**2026-07-22 — M9a: native CEM resource-pack models (foundation).**

- The EMF-equivalent, first slice: load an OptiFine **CEM** resource pack
  and render mobs with the pack's `.jem` models — no mod, just asset
  loading. Verified against the user's real **Fresh Animations** packs.
- **Pack loader** (`rewo-data/src/cem.rs`): reads a pack zip, pulls every
  `assets/minecraft/optifine/cem/*.jem` (+ `.jpm`) as raw strings. FA
  1.10.5 → 114 models, 89 jpm parts; 72 map to model kinds.
- **JEM parser** (`rewo-gpu/src/cem.rs`): parses a `.jem` into the same
  `Model` IR the built-ins use — `part`/`translate`/`invertAxis`/`boxes`
  (`coordinates`, `textureOffset`, `sizeAdd`) + nested `submodels`. The
  OptiFine map (negate X+Y for `invertAxis:"xy"`, fold Y about a baseline,
  Z through) is exactly a **180° Z-rotation + Y-translate**, expressed as a
  `Fold` so the existing `cube_f` box-UV emitter applies it per-vertex
  (rotating normals / re-deriving shade for free). Calibrated against the
  vanilla creeper.
- **Override pipeline**: `EntityPass::new_with_cem` swaps a kind's built-in
  model for its parsed CEM model, through the same UV-normalization + atlas
  bake. `mobshot --pack <zip>` is the inspection tool.
- **The convention, cracked (M9a.2, same day)**: OptiFine's top-level
  `part.translate` is the **rotation pivot** (used by animations), **not**
  static position — a top-level part's boxes sit at their raw coordinates;
  only **submodel** translates accumulate. So the map is
  `render = [−(box+Σsub).x, 24 − (box+Σsub).y, (box+Σsub).z]` with the
  top-level translate skipped. This reproduces the vanilla creeper
  right-hind-leg *exactly* (x[-4,0] y[18,24] z[2,6]) and the vanilla
  humanoid head/body/arms/legs — no per-mob pivot table needed; the pivots
  fall out of the box coordinates. The whole thing is still one `Fold`
  (180° Z-rotation + BASE_Y translate); the fix was a single line (don't
  accumulate the top-level translate).
- **Verified across body plans**: creeper, pig, cow, **zombie, skeleton,
  spider, enderman, piglin** all render correct + recognizable with
  textures mapped right — the humanoids that were stretched now have proper
  heads/arms/legs. Near-identical to their vanilla built-ins (FA replicates
  vanilla geometry — a real parser correctness check, not a proxy). The
  **no-pack facelabel gate stays 243/243** — pack overrides are cleanly
  additive, zero regression. Parser unit tests (top-level-pivot +
  submodel-accumulate) + demo byte-identical.
- **`--pack` is an explicit inspection flag**; `rewo live` doesn't load
  packs yet (no gameplay regression — wiring it in is a small follow-up).
  Not yet: per-face UV overrides (`uvNorth`…, e.g. creeper-eye detail —
  box-UV `textureOffset` works), the `.jpm` `"model"` geometry reference
  (FA's are pure animation containers), and rotated parts (`rotate` — none
  seen in FA statics). Animations (FA's actual payoff — the
  `_animations.jpm` OptiFine expression language) are **M9c**, and this
  parser is their prerequisite.

**2026-07-22 — M9c.1: the CEM animation expression interpreter.**

- The reusable core of Fresh Animations' `_animations.jpm`: a lexer +
  Pratt parser + evaluator for the OptiFine expression language
  (`rewo-gpu/src/cem_anim.rs`). Operators (arith/compare/bool, short-circuit
  `&&`/`||`), functions (`sin/cos/clamp/min/max/abs/floor/torad/todeg/pow/
  sqrt/random/between/equals/in` + variadic `if(c,a,b,…)`), constants
  (`pi/true/false`), and the built-in variable set (`head_yaw/limb_swing/
  limb_speed/age/is_on_ground/hurt_time/id/…` + user `var.*`/`varb.*` slots
  interned across a mob's whole program). Parse once → AST; eval per frame
  against an `AnimContext` the runtime fills from entity state.
- Verified: 5 unit tests + a **corpus test that parses all 284 real
  expressions** from the FA creeper/cow/allay animation files with zero
  failures — the grammar covers real FA data, not just hand-picked cases.
**2026-07-22 — M9c.2: Fresh Animations runs (the CEM animation runtime).**

- **FA mob animation works in Rewo.** The CEM parser now builds a **named
  part tree** (one bone per top-level `.jem` part, boxes pivot-relative),
  the `_animations.jpm` is parsed into an ordered program of
  `var.*`/`bone.channel` assignments, and each frame the interpreter
  evaluates it → per-bone `[rx,ry,rz,tx,ty,tz]` deltas that
  `part_transforms` applies alongside the built-in animation.
- **The pivot, solved**: OptiFine's `translate` IS the negated rotation
  pivot, so `pivot = to_model(−translate)` — verified exact against vanilla
  (zombie leg `[1.9,-12,0]`→hip `[1.9,12,0]`, body `[0,-24,0]`→`[0,0,0]`).
  Bones rotate about it.
- **The rotation convention**: the model is baked through a 180° Z-rotation
  (that's what `invertAxis:"xy"` is), so conjugating the animation by it
  **negates the X/Y rotation angles + translations**; Z passes through.
  This was the fix that turned flung-apart limbs into a cohesive walk — the
  FA zombie now strides (leg forward, body lean), arms attached at the
  shoulders, legs at the hips.
- `AnimContext` is filled from the entity state each frame (`limb_swing/
  limb_speed/age/head_yaw/…`, per-entity `id` for `random(id)`), so a herd
  doesn't animate in lockstep. `EntityDraw` gained `anim_id`.
- Gates: no-pack facelabel **243/243** (CEM adds a `cem` field + `Anim`
  marker but changes no built-in rendering), 22 rewo-gpu tests (incl. the
  pivot + program-parse cases), demo byte-identical.
- **Verified across body plans** (mobshot `--walk`/`--time`): zombie
  strides, pig + cow walk as cohesive quadrupeds, creeper cohesive — all
  limbs attached, swinging. **Live is wired** too: `rewo live --pack`
  threads the animation clock (`emit_model` gets `time`; `limb_swing` from
  entity motion), so mobs animate in actual play; the 72 CEM overrides load
  live.
- Known follow-ups (polish): **foot-submodel leg pivots** (creeper) are
  ~1 px off vs the flat-part rigs (zombie/humanoids, which are exact) — a
  small offset, not the gross detachment the rotation-convention fix cured;
  per-face `uvNorth` (creeper-eye detail), scale channels, and the jpm
  `"model"` geometry ref remain. The engine is done; these are finishing
  touches.

**2026-07-22 — baby mobs.**

- Baby zombies / animals render at ~half scale instead of adult-sized.
  `AgeableMob.DATA_BABY_ID` and `Zombie.DATA_BABY_ID` both sit at
  **metadata index 16** too — but as BOOLEAN, not INT. Index 16 is
  polymorphic: INT there = a cube-mob size, BOOLEAN there = baby, and the
  **serializer type disambiguates** in one decode (`(16,1)`→size,
  `(16,8)`→baby). No need to enumerate which entity kinds are ageable —
  any mob that sends BOOLEAN-at-16 is a baby.
- `metadata.rs` decodes it, `EntityTable` tracks a baby set, `collect_
  entities` multiplies `scale_mul` (and the bbox) by 0.5 for non-player
  babies — reusing the exact `scale_mul` machinery the slime-size work
  added. **Uniform** scale: vanilla keeps the head proportionally larger
  (a per-part transform), a documented approximation (like squid tentacles
  / rabbit-hop).
- Verified live: an adult zombie and a `{IsBaby:1b}` zombie side by side —
  the baby is unmistakably half height. The baby flag arrives in a
  follow-up `set_entity_data` packet, so a too-short settle can miss it;
  the debug run confirmed `baby=Some(true)` decoded (raw `…10 08 01 ff`,
  skipping the health FLOAT before it). Metadata unit tests for both the
  INT-size and BOOLEAN-baby readings at index 16. Gates: 27 crate tests,
  demo byte-identical, `mobshot --check` unaffected (mobshot passes 1.0).

**2026-07-23 — M9d: CEM polish — per-face UVs, scale channels, and
submodels-as-bones (Fresh Animations fully animates).**

The M9c "polish left" list (foot-submodel pivots ~1px off, per-face
`uvNorth`, scale channels) turned out to share **one** root cause and got
closed together: the parser only made *top-level* parts into bones, so every
FA rig's nested detail — the head, the eyes, the feet — was flattened onto
its parent and its animation channels silently skipped. The fix is a
bone-per-node tree. Three verified pieces (all headless via `mobshot --pack`
against the real FA creeper/pig/zombie; no-pack facelabel gate stays
**243/243**; 26 rewo-gpu tests, 8 new):

- **Per-face UVs** (`mobs::cube_f_faceuv` + `cem::emit_box`): OptiFine
  `uvNorth`/`uvSouth`/`uvEast`/`uvWest`/`uvUp`/`uvDown` texture-pixel rects
  override the box-UV unwrap per face, wound identically to the box-UV `face`
  closure (reversed rects → mirror, exactly like OptiFine). This is what the
  FA creeper/zombie/pig eyes (flat `uvNorth` plane boxes) and the pig snout
  (all sides) need — before, they sampled box-UV `(0,0)` garbage. `cube_f`
  now delegates to `cube_f_faceuv` with all-`None`, so the thousands of
  built-in callers are untouched.

- **Scale channels + bone-channel reads + file order** (`cem_anim`/`cem`):
  `sx/sy/sz` were parsed but dropped; now applied about the bone pivot as
  `R·S` (vanilla's scale-innermost), propagating through the parent chain.
  Two dependencies fell out of the real data: (1) FA expressions **read**
  other bones' channels as variables (`"l_eye_white.sy": "r_eye_white.sy"`,
  `"head2.sy": "head2.sx"`), so bone channels now intern a slot and publish
  their value each frame; (2) FA relies on **file order** of a JSON object's
  assignments, but `serde_json`'s default `Map` is a sorted `BTreeMap` — the
  `_animations.jpm` blocks are now deserialized into `indexmap::IndexMap`
  (added to rewo-gpu) so keys keep file order (a sorted map evaluated
  `l_eye_white.sy` before `r_eye_white.sy` → the mirror read 0 → collapsed).

- **Submodels-as-bones** (`cem::model_from_jem`/`add_node`): every `.jem`
  node (top-level `part` *and* every submodel `id`) becomes a named,
  parented bone in tree pre-order, so head-look (`head2`), eye blink, and
  foot articulation animate. Box **rest positions are unchanged** (the
  `submodel_translate_accumulates` test still holds); only the per-bone
  pivot is new. **Two load-bearing asymmetries**, both derived empirically
  from FA data + verified against vanilla, not guessed:
  - **Pivot**: a top-level part's `translate` is the negated pivot
    (`to_model(−translate)`, vanilla-exact); a submodel's `translate` is an
    accumulated *position*, so its pivot sits there (`to_model(boxOff)`) —
    creeper `head2` → the neck `[0,6,0]`, matching vanilla `head.offset(0,6,0)`.
  - **Translation is REPLACE, not add**: OptiFine `tx/ty/tz` overwrite a
    bone's position; FA authors them as `rest + sway`. Adding them flung the
    pig head ~12 units off the body. `eval_program` subtracts a per-bone rest
    baseline (`Model::cem_translate`) so a rest re-spec nets zero.
    **⚠️ M9d got the sign of this wrong — corrected in M9e below.** (FA's
    `-3.2` vs `-3` in a leg's `tx` is an *intentional* stance offset,
    correctly kept as a small delta.)

  Verified live: the FA creeper head sways naturally about the neck (smooth
  6-frame filmstrip) and stays attached, the zombie/pig render as coherent
  full-body FA mobs (head+snout+eyes+legs all attached), and an old-vs-new
  diff localizes the change exactly to the head/eyes. Built-in mobs
  (`cem = None`) are byte-identical (243/243 + demo unchanged).

  Verification pack: reconstructed from the extracted FA `creeper/pig/zombie`
  `.jem` + `_animations.jpm` under `%LOCALAPPDATA%/Temp/claude/fa-cem/` into a
  standard `assets/minecraft/optifine/cem/` zip (the original download wasn't
  on disk; the jems are genuine FA data).

  **Still open (M9b / follow-ups)**: ETF random + emissive textures (a
  separate subsystem — spider/enderman/blaze eyes, cow/pig random variants);
  per-face UV winding for `uvUp`/`uvDown` verified only visually (creeper eyes
  use `uvNorth`, which is exact); the `.jpm` `"model"` geometry reference
  (FA's are pure animation containers). Foot-submodel pivots, per-face
  `uvNorth`, and scale channels — the M9c leftover list — are now **done**.

**2026-07-23 — M9e: the CEM audit — five more bugs, found by measuring.**

M9d was signed off on *appearance* ("it reads like a pig"), and the user
correctly rejected that: several FA mobs were visibly malformed. The lesson
from [[feedback_verify_property_not_proxy]] applies to packs too, so the
verification was mechanized instead:

- **`rewo mobshot` audit** (`scratchpad/audit.py`, reusable): render each of
  the 71 FA-overridden mobs **twice** — vanilla built-in (independently
  gate-verified at 243/243) vs the FA CEM pack — measure the silhouette
  bounding box, and rank by divergence. FA replicates vanilla geometry
  closely, so a large divergence is a strong bug signal. `rest` mode
  (`REWO_CEM_NOANIM=1`, also a new runtime knob) compares the CEM **rest
  pose** to separate static-geometry bugs from animation bugs.
- **Corpus scans** over the real pack for (a) every identifier the animations
  reference vs the interpreter's builtin set, and (b) every JSON key the
  `.jem`/`.jpm` files use vs the keys the parser reads.

Five bugs surfaced, none of them visible by inspection:

1. **`rotate` ignored** — **71/119 models, 234 parts**. FA lays quadruped
   bodies flat with a dedicated `"rotation"` submodel carrying `[-90,0,0]`;
   dropping it left the pig/cow/sheep body standing *vertically*. This was
   the reported malformation. Applied as the bone's base pose (`Part::rot`),
   conjugated like the animation (X/Y negate, Z through).
2. **`mirrorTexture:"u"` ignored** — **174 parts** (`left_arm`, `left_leg`,
   `left_eye`…). It is exactly vanilla's cube `mirror` flag; the CEM path
   hardcoded `false`, so every left-side part sampled un-mirrored texels.
3. **CEM override discarded the kind's render scale** — `model_from_jem` ends
   at `finish(1.0)`, so ghast (4.5×), elder guardian (2.35×), slime (2×) and
   cave spider (0.7×) rendered 2–3× wrong. The override now inherits the
   built-in's scale.
4. **Ten builtins silently read 0** — `pos_x`/`pos_z` (95 mobs),
   `player_pos_x/z` (88), `rot_y` (50), `is_ridden` (34), `is_sitting`,
   `is_in_lava`, `is_tamed`, `is_on_shoulder`. Unknown identifiers fell
   through to a never-written user slot. **Systemic fix**: `parse_anim` now
   **warns** on any identifier that is neither a `var.*` slot nor a readable
   `bone.channel` — this whole class of silent corruption can no longer hide.
   Unknown identifiers across the pack: **23 → 0**.
5. **`bone.visible` unimplemented** — 13 sites (goat horns, bee stinger,
   bogged mushrooms, donkey chests, a sheared snow golem's face). Added as a
   `Channel`, returned per-bone in the new `CemFrame`, and propagated down the
   parent chain (hiding a bone hides its subtree).

**The translation sign error (the important one).** M9d's "subtract a rest
baseline" only *approximates* OptiFine's replace semantics — it holds when
FA's anim base equals the jem rest (the pig) and fails when FA deliberately
**repositions** a bone (the spider plants its legs on the ground with
`ty=23.5` against a rest pivot of `15`, and flew apart instead).

Root cause: **translation values are stated in model space — identity map, NO
X/Y negation**, unlike rotation angles, which the 180° Z bake *does* negate.
Proof: the pig leg's anim base `(-3.2, 24, 7)` equals its model pivot
`(-3, 24, 7)` on all three axes; a negating map would require `(3, -24, 7)`.
M9d negated *and* tuned a baseline to cancel at rest, so **rest looked right
while every sway moved the wrong way**, and repositioned bones were displaced
by twice the error.

The final rule — `delta = anim − baseline`, evaluated **identically for every
channel and every bone**, with the baseline recording the frame each node kind
states its position in:

| node kind | baseline | verified on |
|---|---|---|
| top-level `part` | `pivot_abs` (as-is) | pig `leg1` ty=24 → Δ0; spider `leg1` ty=23.5 → Δ+8.5 |
| submodel | `invertAxis(own_translate)` = `[-t.x, -t.y, t.z]` | pig `head2` ty=−12 → Δ0; pig `snout` ty=1 → Δ0 |

Two wrong turns are worth recording, because both *look* right at rest:
- Using `−box_off` instead of `invertAxis(own_translate)` for submodels.
  They coincide only for a **first-level** submodel, so deeper ones detached
  (the pig's snout floated below the body).
- Keeping M9d's negation for submodels while making top-level identity. That
  freezes rest correctly but **inverts every submodel sway**, so mobs whose
  limbs FA repositions flew apart (the axolotl went to 292% divergence).

Both are ruled out by the four calibration points above; a rest-only check
cannot distinguish them, which is exactly why the audit had to compare
*animated* renders too.

Gates: no-pack facelabel **243/243** throughout (pack overrides stay purely
additive), 26 rewo-gpu tests. Measured against vanilla, animated: **pig
1%/2%, cow 1%/3%, cave spider 0%/2%, spider 1%/13%, creeper 9%/1%,
chicken 2%/9%** (dW/dH). `Model` gained `cem_names`/`cem_top` for diagnostics
and `REWO_CEM_NOANIM=1` renders a pack's rest pose.

**Top-level parts inherit VANILLA's hierarchy (the last piece).** A `.jem`'s
top-level parts map onto *vanilla model parts*, and a pack states an animated
part's position relative to whatever parent **vanilla** gives it — not the
`.jem` nesting. The vex is the clean case: its `right_arm` is top-level in the
`.jem`, but `VexModel` makes it a **child of `body`**, so FA writes
`right_arm.ty = 0.6` against the body (whose pivot sits 0.5 away). Read as
absolute, the arms floated 18 px above the mob.

Fixed by machine-extracting vanilla's hierarchies from the decompile —
`tools/gen_vanilla_hierarchy.py` → generated `rewo-gpu/src/vanilla_hier.rs`
(**66 models with real nesting**), the same generate-from-ground-truth pattern
as `gen_anim_defs.ps1`; re-run it after a version bump. `model_from_jem_for`
takes the entity name, parents each top-level bone to its vanilla parent when
both ends are present in the `.jem` (topologically ordered, since
`part_transforms` composes in index order and `.jem` order doesn't guarantee
parents first), and the top-level baseline becomes **`rel_pivot`** — the
pivot relative to that parent bone. For a part vanilla keeps at root level
`rel_pivot == pivot_abs`, so the rule is unchanged for flat models.

That last property is why the result is clean: **3 improved, 0 worsened, 68
unchanged** — vex 166%→**10%**, frog 172%→**23%**, allay 36%→**19%**. Zero
regressions is the signature of a correct fix; the earlier per-node-type
attempt scored 18 improved / 20 worsened, which was the tell that it was
curve-fitting rather than modelling.

**Persistent variable state (the animation clock).** A pack's variables are
**integrators** — `var.run = var.run ± rate*frame_time`, plus `var.air`,
`var.t_jump`, `var.t_land`, `var.tr` — and every one is gated by
`varb.pfc = frame_counter == var.pre_frame_counter`, a "same frame" check that
holds them when the program is evaluated twice in one frame. `eval_program`
cleared `ctx.user` each frame, which was worse than losing the accumulator:
`var.pre_frame_counter` reset to 0 and `frame_counter` was never set, so the
guard compared `0 == 0` and **pinned every integrator at its initial value** —
all smoothing and transition behaviour was inert, not merely restarted.

Fixed by giving the state a home and the clock real values:
- `EntityPass` keeps `cem_state: HashMap<u64, CemVars>`, keyed by model kind +
  `EntityDraw::anim_id` (kind included so a recycled id can't inherit a slot
  layout that no longer matches). Slots are handed to the interpreter and taken
  back each frame; entries unseen for `CEM_STATE_TTL` (600) generations are
  pruned so despawns can't leak.
- `eval_program` **resizes without clearing**, so the carried values survive.
- `frame_counter` increments per `set_draws` and `frame_time` is the real
  inter-frame delta (clamped, with a 1/20 fallback for still renders) — FA
  integrates against both.
- `mobshot --settle <seconds>` steps a 20 Hz clock before the shot so a still
  renders the converged pose instead of frame 1. Verified: settling 3 s moves
  ~13.7k px on the pig and creeper (previously bit-identical, because the
  integrators were frozen), and the settled poses stay coherent.

Unit-tested directly: an integrator advances `1, 2, 3` across frames and
**holds** when re-evaluated at the same `frame_counter`.

**`player_pos_*` — the last hole in the data feed.** The entity pass never
received the camera position, so those builtins read 0. `WorldRenderer`
already tracked `camera_eye` (for the translucent sort + fog), so it is now
handed to `set_draws` and fed to the interpreter; `live_cmd` was reordered so
`set_camera` precedes `set_entities` (it ran after, which would have served a
frame-stale eye). Verified at runtime: a real `cam_pos` reaches the pass.

Worth recording so nobody re-hunts it: **65 FA expressions use `player_pos`,
and 39 are a z-fighting depth bias** — e.g. the pig's
`right_eye.tz = -8 − clamp(dist,0,128)/1000`, ~0.1 px at 128 blocks, i.e.
deliberately sub-pixel. So this fix is *invisible* on those mobs by design
(a still render is byte-identical), and expecting a visual delta there is a
mis-read of the pack. The other 26 (`var.distance` on enderman/evoker/fox/
goat) do drive real behaviour. Head/eye *tracking* comes from `head_yaw` off
the wire, not from `player_pos`.

**Outliers triaged — all FA design, no structural defects.** The mobs the
audit still ranks far from vanilla were checked one by one against their
built-in counterpart: **phantom** (wings attached + symmetric, a raised flap
phase vs vanilla's flat glide), **cod** and **salmon** (angled/curved swim
poses, body-tail-fins intact), **shulker** (rendered with the shell *open*
where vanilla shows it closed), **wither_skeleton** (arms out, from FA's
`var.bow` / `is_aggressive` branch — attached and symmetric; the plain
skeleton scores 0%/1%), and **bat** (spread wings). None shows detached or
exploded geometry. This is the expected end state: a large animated-audit
divergence on these mobs measures FA's *intent*, not a bug — which is why the
rest-pose audit is the geometry gate and the animated one is only a pointer.

**Methodology note (worth keeping).** The *rest-pose* audit is the valid
detector for **geometry** bugs, because FA replicates vanilla geometry. The
*animated* audit is NOT a quality metric on its own — Fresh Animations exists
to animate differently from vanilla, so divergence there mixes intent with
defect. Read them together: low rest + high animated = FA design; high rest =
geometry bug; and for a specific fix, compare the same mob before/after.

**2026-07-23 — collision: per-block shapes + entity pushing.**

Two long-standing §0.0 gaps, both verified against the live 26.2 server with
the `rewo play` corrections meter (the physics-parity DoD).

- **Per-block collision shapes.** Collision was a per-state `solid` bool, so
  every block was either a full cube or nothing — you fell through slabs and
  walked through fences. `BakedAssets` now carries `collide: Vec<Vec<[f32;6]>>`
  (block-local `0..1` boxes; empty = no collision) and `physics::tick` takes
  `shapes(x,y,z) -> &[[f32;6]]` instead of a bool, clipping against each box.
  **19,665 states** get a real shape.

  Vanilla keeps collision shapes in Java code — no datagen report has them, so
  unlike `vanilla_hier` there is nothing to generate from. Shapes are therefore
  taken from the **model** geometry, gated by a curated family list
  (`model_collision`): slabs, stairs, walls, fences (+gates), trapdoors, doors,
  carpets, snow, beds, chests, cauldrons, … Everything outside the list keeps
  the old behaviour, so the change can only *add* collision where the model is
  known to match. That gate is the load-bearing part: deriving shapes for
  *every* block would be wrong in the obvious direction — torches, plants and
  rails have models but no collision, and the player would bump into flowers.
  Model boxes are rotated by the blockstate's `x`/`y` (stairs pick a rotated
  model per facing), and fence-likes are raised to 1.5 as vanilla does so they
  can't be jumped.

  Spot-checked against vanilla's real shapes: `stone_slab` `[0,0,0,1,0.5,1]`,
  `oak_stairs` `[0,0,0,1,.5,1] + [0,.5,0,1,1,.5]`, `oak_fence`
  `[.375,0,.375,.625,1.5,.625]`, `oak_trapdoor` 3/16 thick, `white_carpet`
  1/16 — and `torch`/`dandelion` correctly **empty**.

- **Entity pushing.** A verbatim port of `Entity.push(Entity)`: entities whose
  bounding boxes overlap shove each other apart horizontally, applied before
  `travel` exactly as `LivingEntity.aiStep` does. Only the player is moved —
  the server owns every other entity, so pushing them client-side would just be
  corrected away. Vanilla's quirk is preserved literally: `dd` is
  `absMax(dx,dz)` and then **square-rooted** (the sqrt of the larger component,
  not the vector length), so the "obvious" normalize would give the wrong
  strength. Only *living* entities push — `Entity.isPushable()` is false by
  default and only `LivingEntity` overrides it — expressed as
  `EntityTypes::pushable`, an exclusion list over registry names (items,
  projectiles, displays, armor stands).

Verification: 5 new unit tests (standing on a slab settles at exactly y=−0.5;
a fence post blocks and can't be stepped over; the push math's threshold,
unit-separation value, antisymmetry and clamp), plus live runs — **0
corrections** over 800 ticks walking on a slab floor, with a fence wall, and
with a cow shoving the player; place/dig still verify. A probe confirmed the
shove actually fires in-session (`dv=(0.0395, 0.0395)` off a cow at ~0.62
separation, which is exactly `sqrt(0.624)×0.05`).

`rewo play --setup "<command>"` was added to run one server command after
spawn — how the slab floor / fence wall / mob were staged for those runs.
(A first attempt used `fill … hollow`, which fills the box's *floor and
ceiling* too and so buried the player in fences; the server then shoved it out
for 508 "corrections". A parity meter only means anything when the setup
doesn't corrupt the premise.)

**2026-07-23 — entity lighting.**

Entities rendered at a fixed directional shade — the same brightness in a
sealed cave as at noon — while blocks around them were lit. `EntityDraw` gains
`light: f32`, multiplied into the vertex colour on both the model and capsule
paths, and `live_cmd::entity_light` samples it from the world.

Two details make it match rather than merely darken:
- **Sample at the eye, not the feet.** Vanilla uses
  `BlockPos.containing(x, eyeY, z)`; the feet block is usually the floor the
  entity stands *in* (light 0), so sampling there renders every mob black.
- **Reuse the mesher's curve** — `0.25 + 0.75·max(block,sky)/15`, the exact
  expression the block mesher uses — so a mob reads as part of its
  surroundings instead of floating in its own lighting.

Nametags stay fullbright (vanilla does the same). Still renders default to
`light = 1.0`, so `mobshot` and the facelabel gate are untouched.

Verified: a probe in a live session read `raw=15 → 1.000` for a cow under open
sky and `raw=4 → 0.450` / `raw=11 → 0.800` for entities under a stone roof —
i.e. the value tracks the real world light per entity. New `mobshot --light
<0..1>` renders the same mobs at a chosen level without a server; at 0.45 the
cow and creeper are visibly darker while directional shading and readability
are preserved.

Note this is the *client-side* lightmap only — there is still no time-of-day
sky darkening (blocks don't have it either), so "night" doesn't dim anything
yet. Consistent with the block path, which is the point.

**2026-07-23 — cave light: the decode is correct; the missing piece is a
client light engine.**

The §0.0 "light decode beyond flat-world is unverified" item is now measured
rather than assumed, with two new headless diagnostics:

- `World::light_at(x,y,z) -> (block, sky)` — the two sources separately (the
  existing `brightness_at` is their max). Surfaced as a vanilla-style
  **`Light: N (S sky, B block)`** line in the F3 overlay.
- `rewo play --light-at "x,y,z"` reports light at a fixed world coordinate in
  the run summary, plus a short `+x` profile at the bot. Fixed coordinates
  matter: the bot wanders, so "light near the bot" is not reproducible.

**The decode is correct.** In a sealed stone room with one torch at
`(46,-59,46)`, sampled two blocks above it: sky **0** everywhere and block
light `8 9 10 11 12 11` across x=42..47 — the peak of 12 exactly over the
torch column (14 − 2 for the vertical distance) falling 1 per block either
side, i.e. textbook vanilla propagation. Under open sky, `sky 15, block 0`.
The Y-mask distribution, nibble indexing and sky-vs-block array ordering are
all right.

**The actual gap: lighting never changes after chunk load.** Place a torch
mid-session and the client's block light stays 0. Instrumenting the packet
stream shows **zero `light_update` packets** — because vanilla clients run
their **own light engine**: the server ships light with chunk data and then
leaves subsequent block edits to the client. So this was never a decode bug.

Shipped here: a `light_update` handler anyway (`chunk::apply_light_update` +
`World::column_mut`), since servers do send it in some situations (chunk-border
relight) and it re-meshes the neighbourhood on arrival. It is correct but inert
against a vanilla server for ordinary block edits.

**Still open — the client light engine.** Needs BFS block-light propagation
(add on placement, removal-then-repropagate on break) and sky-light re-flood on
column changes. Two prerequisites worth recording: per-state **luminance is not
in any datagen report** (blocks.json has no light field), so it has to come
from the decompile or a curated table, the same situation as collision shapes;
and light **opacity** would need more than the existing `solid` flag (vanilla
glass is a full cube but blocks no light). Until it lands, lighting is correct
as of chunk load and frozen thereafter.

### 2026-07-23 — M10: the client light engine shipped (server-exact)

**Done, and measured against vanilla's own light engine.** The previous entry's
two prerequisites both got resolved, neither by a curated table.

**The rules — transcribed, not guessed.** All four come from the decompile and
are worth keeping here, because every one of them is somewhere a naive
implementation goes wrong:

```
BlockBehaviour.getLightDampening:
    isSolidRender ? 15 : (propagatesSkylightDown ? 0 : 1)
    propagatesSkylightDown = !fullCubeShape && fluidState.isEmpty()
LightEngine:78          step cost into a cell = max(1, dampening)
LightEngine.getLightDampeningInto
                        a face passes NO light (16) when the two adjacent
                        occlusion shapes together cover it — binary, not graded
ChunkSkyLightSources.isEdgeOccluded
                        the sky column descends while dampening == 0 AND the
                        horizontal face between the two cells is not occluded
```

Note the middle two: a stair or slab has `dampening 0` yet still casts a real
shadow, purely through face occlusion. Vanilla's "cost 3 through a stair" that
showed up in the gate was not a graded cost at all — it was light **detouring
around** a blocked face and arriving two steps later.

**Opacity turned out to be mostly derivable.** `isShapeFullBlock` is the
collision-shape query already baked for M-collision, and fluid-ness is known,
so the rule collapses to needing exactly two imported facts:

```
if canOcclude && full_cube → 15    (stone)
elif !full_cube && !fluid  → 0     (torch, slab, air)
else                       → 1     (glass, leaves, water)
```

Note the `1` — glass/leaves/water **dampen by one**, neither 0 nor 15. A
curated "transparent set" gets this wrong in both directions. And
`RenderKind::Cube` is **not** a valid opacity proxy: glass, leaves and ice all
bake as `Cube`.

**`tools/gen_block_light.py`** is the extractor (the `gen_anim_defs.ps1`
precedent — re-run after a version bump). It pulls emission and `noOcclusion()`
out of the `Blocks.java` registrations, inlining helper factories like
`leavesProperties`. Two facts are *virtual method overrides* rather than
builder calls, so it also maps block → implementation class (from the
registration lambda) → override, resolved up the `extends` chain:
`propagatesSkylightDown` (**TransparentBlock returns true — sky passes glass at
full strength**, and `StainedGlassBlock extends TransparentBlock` inherits it)
and `getLightDampening` (leaves pin 1, tinted glass 15). All override bodies
reduce to three forms: `true`, `false`, and "not waterlogged"
(`getFluidState().isEmpty()` and `!getValue(WATERLOGGED)` are the same
predicate). Colour families registered through `ColorCollection` are not
fields, so they are expanded to their 16 dye names — and **every generated name
is validated against `blocks.json`**, which makes the naming convention checked
rather than assumed. Nine state-dependent emission forms (candles, cave vines,
vaults…) are approximated and **listed in the generated header**; nothing is
silently defaulted.

**Face coverage** is baked per state by rasterising each face of the collision
boxes at 16×16. Every vanilla shape lies on 1/16 boundaries, so this is exact
for the shapes that matter and avoids carrying a `VoxelShape` algebra.
`useShapeForLightOcclusion` is *derived* (`canOcclude && !full_cube`) rather
than scraped: its only observable effect is through face coverage, and a block
vanilla leaves out of that list essentially never covers a whole face (a fence
post is 6/16 wide), so a false positive is inert.

**A real decode bug fell out of the gate.** The chunk light payload's
`empty_sky` / `empty_block` masks were read and **discarded**, so a section in
neither mask stayed `None` and read as 0. Vanilla reads those as full-bright:
the server only sends sky arrays for sections near terrain (measured: `sky=[1,2]`,
`empty_sky=[0]`, sections 2..23 in neither), so **everything above the terrain
was silently sky-0**. `Column::sky_full_above` now tracks the boundary, set from
the masks and re-set by `light_update`. This had been invisible for a long time
because on the flat test world the player stands where real arrays exist — it
took a second, independent implementation to disagree with the decoder.

**The gate: `rewo play --light-check`.** Recompute the loaded columns from
scratch and diff against the server's authoritative light — the lighting
equivalent of the `CORRECTIONS` physics meter, and the "verify the property,
not a proxy" answer for lighting. **884,736 cells, both channels, 0 mismatches**
on: flat terrain, a village (natural stairs/slabs/panes), an enclosed shaft, and
a sealed stone room with torch + glowstone + glass skylight + leaves + stairs +
slab. The engine is wired into `PlaySession` (`set_light_tables` →
`relight` on every block update), so `rewo live` relights its own edits and
marks exactly the affected columns for remesh.

Two harness fixes came with it, both of which had silently faked results
earlier: `--setup` now accepts `;`-separated commands **paced one per 250 ms**
(firing them in one tick trips the server's chat rate limit and the tail is
dropped — which looks exactly like a light bug, because the structure never
appears), and `--still` suppresses the movement script so the gate stays inside
whatever was built. The op account is `RewoOp`, not `RewoBot`.

**M10 finished (same day).** Three gaps closed and one flaw in the gate.

*`section_blocks_update` was entirely unhandled* — a world-sync bug well beyond
lighting. Single-block edits arrive as `block_update` (wired); a `/fill`, an
explosion, a piston, a growing tree or another player building arrives as a
multi-block section update, and Rewo dropped it, so any edit to an
already-loaded chunk never appeared. **It hid behind the harness**: a structure
built right after a `tp` is already present when the chunks stream in. A run
padded with paced no-ops until the chunks are definitely loaded exposed it — a
4-block fill read `state 0` before and `state 1` after. The bit unpacking
(section x in bits 42..63, z in 20..41, y in 0..19; in-section **x is the high
nibble, z the middle, y the low**) is extracted into pure functions with tests,
because getting it wrong lands edits in the wrong chunk and reads as "some
blocks never update".

*Property-driven emission.* Nine blocks compute `lightLevel` from block-state
properties and were approximated by their max literal — an unlit candle emitted
12. The generator now carries the seven rules, each **keyed by a source
signature** so a rewritten expression stops matching rather than silently
keeping a stale rule, and emits `STATE_EMISSION`. Verified per state: candles
`lit ? 3 × candles : 0` (with the 16 dyed variants), glow berries 14 on
`berries`, sea pickles `3 + 3 × pickles` while waterlogged, respawn anchor
`floor(charges/4 × 15)` = 0/3/7/11/15, light blocks the `level` property,
vaults 6 inactive / 12 otherwise, trial spawners 0/4/8/8/8/0. One approximation
remains, documented: glow lichen's "any face" predicate held at 7 — a lichen
with no faces cannot be placed.

*Shape occlusion for the rest of `useShapeForLightOcclusion`.* Occlusion is
computed from the collision boxes, so a block with **no boxes has no occluding
faces**. `farmland`, `dirt_path` and `sculk_sensor` had none and let light fall
through where vanilla stops it at their full bottom face; they and the rest of
that set (sculk shrieker, shelf, pistons) joined the curated family list.
Physics parity unchanged at 0 corrections — they carry real vanilla collision
shapes, so this makes collision more correct too.

*The gate could grade itself.* `--light-check` diffs a recomputation against the
**stored** light, and incremental relighting *writes* that store — so a world
built during the session was compared against our own engine, not the server's.
New `--no-relight` keeps the store purely server-authoritative. This matters
because **a vanilla server sends no light packets for ordinary block edits at
all** (exactly why a client needs this engine), so the honest protocol is:
build in one run, join fresh in a second, and grade the chunk-load light.

Two artifacts worth remembering from the verification: farmland **reverts to
dirt** when unhydrated, and the server leaves stale block-light *inside* the
now-opaque cell — 17 phantom mismatches that are invisible in practice (light
inside a solid block is never sampled). `dirt_path` is the stable stand-in.

Verified **EXACT** — 884,736 cells, both channels, 0 mismatches — on the village
spawn, a pre-built sealed room graded from a fresh join, and a live session
whose room is built entirely through section updates with incremental
relighting active. 79 tests; demo, mobshot 243/243, bench unchanged.

**Still open (not M10):** the per-side vs true-shape-union face merge (differs
only for complementary partial faces meeting, which no vanilla pair produces).

### 2026-07-23 — M11: vanilla's lightmap + the day/night cycle

M10 produced correct light *values*; they were then rendered through an
invented formula. `max(block, sky) / 15` into `0.25 + 0.75 * l` is wrong three
ways against `assets/minecraft/shaders/core/lightmap.fsh` in the client jar:
vanilla **adds** the two channels rather than taking their max, the ramp is
`l / (4 - 3l)` (far darker at low levels than a straight line), and there is
**no floor** — an unlit cave is genuinely black, where ours bottomed out at 25%
so no interior could ever be dark.

`shaders/lightmap.glsl` transcribes it: each channel through the curve, sky
scaled by a time-of-day factor, block tinted warm (`BLOCK_LIGHT_TINT`
0xFFD86C) fading to white at both ends via the parabolic mix, summed and
clamped. The two levels ride in the **spare bits of the existing per-vertex
layer word** (`layer | block << 16 | sky << 20`), so the vertex does not grow
and the mesh carries no time-of-day state — the fragment masks the low 16 bits
for the texture index. Keeping the channels separate to the shader is what
makes a sunrise a uniform update instead of a remesh, and why a torch is as
bright at midnight as at noon.

**Day/night.** 26.x dropped the hard-coded `getSkyDarken` for a keyframed
timeline over a registered world clock. `rewo-world/src/daylight.rs`
transcribes `Timelines.OVERWORLD_DAY` — a 24000-tick period carrying
SKY_LIGHT_FACTOR (1.0 → 0.24), SKY_LIGHT_COLOR (white → blue), SKY_COLOR and
FOG_COLOR (→ near black), sampled with the linear easing `KeyframeTrack`
defaults to. The sky gradient and fog darken with the ground, or night would
be dark terrain under a noon-blue sky.

Three wire details that had to be measured, not assumed:
- `set_time` changed shape: `i64 gameTime` + a **map of per-clock states**
  (`Holder<WorldClock>` → `{VarLong totalTicks, f32 partial, f32 rate}`).
- A vanilla server sends **two** clocks (`overworld` and `the_end`). Taking the
  first entry picks whichever serialised first — here `the_end`, whose ticks
  track the game time closely enough to look plausible. Match by registry id;
  the overworld id is captured from the Configuration registry data rather than
  assumed from bootstrap order.
- **`ByteBufCodecs.holderRegistry` writes the id RAW.** The `id + 1` /
  direct-holder encoding belongs to `holder(...)`, a different codec.
- Clock states are sent only when they change (the join packet carries them,
  later ticks send an empty map), so the last value must be held.

**A real bug the old formula was hiding.** `emit_model` sampled the block's
**own** cell — the inside of a solid block, always dark. `grass_block` renders
as a Model (cube + overlay), so the entire ground plane of every overworld was
lighting at zero; the old 0.25 floor plus a `.max(1)` made it merely dim rather
than black, so it had never been noticed. It now samples the cell the quad
faces, as vanilla's `renderModelFaceFlat` does. Found by forcing the lightmap
to 1.0 in the shader and watching the ground reappear — the general trick when
a render looks wrong: disable one term and see which one owns the pixel.

Verified headlessly: noon vs midnight differ as expected — sky (144,178,242) →
(84,121,193), ground (75,110,66) → (22,36,30), mobs dimming with the world; and
a sealed torch-lit room shows warm falloff into real darkness. The demo PNG is
**byte-identical** across both commits, which is the proof the refactor is
neutral where it should be (under open sky the new curve returns exactly 1.0,
as the old one did). 104 tests, mobshot 243/243, bench 0.232 ms, 0 corrections,
M10 light gate still EXACT.

**Left open:** the block-light flicker (vanilla jitters `blockFactor` slightly
per frame), gamma/brightness and night-vision/darkness terms of the lightmap,
and the sun/moon/stars — the sky is still a gradient with no celestial bodies.
