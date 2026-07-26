# REWO_PLAN.md — Rewo: the from-scratch native Minecraft client

**Rewo** (from "rewolution", as Ewo came from "ewolution") is a from-scratch
Rust Minecraft: Java Edition client speaking the vanilla protocol, rendered
with raw Vulkan. This file is the plan of record. It supersedes both the
hand-off design doc (`~/Downloads/rust-mc-client-design.md`, drafted under
codename "Ferric") and the interim `FERRIC_PLAN.md` (deleted). The design
doc's reasoning was pressure-tested against the live repo and the on-disk
26.2 jar on 2026-07-21; its four product decisions are kept, a set of factual
errors is corrected (§2), and several missing workstreams are added (§3).

**Status: M0–M18 shipped + headlessly verified (2026-07-25). M0–M9 are
pushed (`origin/main` @ `973ea5e`); M10–M18 are reviewed local work,
intentionally not yet pushed. M18 is feature commit `bb8be20` on
`codex/rewo-m18-allay-dance`; the current documentation commit is local too.**
See §0.0 for the fresh-session handoff and §15 for the per-milestone log.

---

## 0.0 HANDOFF — read this first (fresh session, 2026-07-25)

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

### Where it is: M0–M30 shipped; M0–M9 pushed, M10–M30 reviewed local

Everything from M10 on is reviewed local work on branch
`codex/rewo-m19-combat-swings` — `origin/main` is still at the M0–M9 point.

**Latest (2026-07-26): M27/M28 — sign text, and the invisible block entities.**
Five commits took `blockentityshot` from 70 to **125** witnesses and the
still-invisible block-entity set from eleven types to **two**. M27 dyed and
glowing sign text plus the line break that keeps it on the board (glowing text
is the dye at FULL strength with the dim one demoted to its outline — not "the
same colour, brighter"). M28 skulls (7 types, 14 blocks — **entity** models,
authored y-down, so both transforms end in `scale(-1,-1,1)`) and the conduit
shell. M28b the decorated pot, the first block entity that is not one model.
M28c banners, the first whose texture carries no colour — a pattern sprite is a
greyscale mask and the dye is a per-layer tint. M28d the spawner's
`block_event`, the third meaning of `b0 == 1`. M28e/M28f then closed the last
two: the **copper golem statue** (four separate pose layers with nested rotated
hierarchies, machine-extracted by `tools/gen_copper_golem_poses.py` because
thirty-eight rotated boxes fail silently when hand-copied) and the **two end
portals** (whose geometry is an ordinary cube — only the render *type* is a
shader, approximated by one static layer). **M25's Invisible list is now
empty: eleven types measured, eleven rendering.** Five gate witnesses caught
real bugs before they shipped (a pot side baking six quads instead of one, a
banner base texture path that baked no pole at all, a statue weathering suffix
on the wrong end of the name, and an existing witness that had quietly started
measuring the wrong set). **M29 then shipped the per-block-entity animation
clock**, so a banner sways, a pot wobbles, a piglin head's ears move and a
dragon head's jaw opens — and it exposed **two rest poses that were already
wrong**, because `setupAnim` always runs and those parts were never at the
mesh's own pose. **M30 then built that world scan** and the conduit's
active cage, wind and eye came with it — a conduit decides its own activation
from the blocks around it, and the shell is **42 positions, which is also the
hunting threshold**, so its eye opens exactly when the frame is complete. Two
items remain: a spawner's caged mob needs an **entity-in-block draw**, and an
end portal's starfield a **shader**. See §15.

**Earlier (2026-07-26): M26 — `block_event` reaches the right block entity, and
a shulker box opens.** `b0 == 1` is not one opcode: it means a chest's viewer
count, a shulker box's open/close pair, and a bell's *click direction*,
selected by the block entity's type exactly as vanilla's virtual `triggerEvent`
call is. Reading it as "a chest lid" — which this client did — meant a rung
bell opened a phantom lid at its own position. Shipped with it: the shulker
box's four-state lid animation (whose rule is `b1 == 0` / `b1 == 1` with no
else, **not** the chest's `b1 > 0`), the animated part group becoming a matrix
so one emitter can express both a hinge and a slide-plus-spin, and the
block-entity classification catching up with the four types that had quietly
started rendering — including replacing the witness that guarded it, which
asserted a *moment* ("nothing is Rendered yet") and so passed happily while the
world moved underneath it. `BlockEntityRegistry` also runs in the client now
rather than only in the gate. Gate: `blockentityshot` 70 → **88**. See §15.

**M23–M25 and the four block-entity commits (2026-07-25/26)** — item-use state
and the eight `ArmPose`s; the death animation; block-entity decode plus a
fail-closed registry; then item entities, chests, chest lids + double chests,
shulker boxes, and world-space text so signs are legible. Their records live in
the §16 blockquote as well as §15.

**Earlier (2026-07-25): M18 — exact Allay dance, the first metadata-driven rig.**
The Allay's `DATA_DANCING` (SynchedEntityData **index 16, BOOLEAN serializer 8**)
is now a consumed metadata animation, distinct from M17's one-shot entity events.
It was being silently mis-decoded as `DATA_BABY_ID` (both live at slot 16 with the
same serializer; only the *kind* separates them — Allay extends `PathfinderMob`,
not `AgeableMob`). Shipped: the exact `Allay.tick()` client counters
(dance-tick % 55 < 15 spin window; bidirectional spin ramp clamped 0..15; false
resets next tick; repeated true does not restart) in `rewo-world` `EntityTable`;
the exact `AllayModel` root/head formulas (`Anim::AllayRoot`/`AllayHead`) with the
Allay model restructured into the real `root → {head, body → {arms, wings}}`
hierarchy (rest geometry neutral, mobshot 243/243 unchanged); vanilla
missing-entity inertness (`handleSetEntityData` drops metadata for an untracked
id); and `live_cmd::resolve_allay_dance` shared by the collector and the gate.
Gate: **`rewo danceshot --check`** (serverless, fail-closed **24/24**). See §15
for the blow-by-blow.

**Latest (2026-07-24): M16 dimensions (Nether/End/caves, the whole transition),
M14 per-biome color and M15 geometry performance. See §15 for the blow-by-blow.**
- **M16 dimensions** — the `minecraft:dimension_type` registry is now parsed,
  raw-holder-id-selected and *consumed*: per-dimension vertical shape (the
  Nether's 0..256 vs the Overworld's −64..384, which mis-decoded every Nether
  chunk before), `has_skylight`, `skybox`, ambient light, cardinal (Nether) face
  shade, sky/fog/ambient/sky-light colours + factor, `has_fixed_time`, and a
  `has_day_timeline` resolved from the `timelines` holder set — **independent of
  `has_fixed_time`**, which is a separate `DimensionType` member. Plus the End
  sky pass, the world/mesh discard-and-refence transition, and spawn info.
  Gates: **`rewo dimensioncheck --check`** (serverless) and **`rewo play
  --dimension-check`** (live, paced).
- **M15 exact packed ABI + conservative greedy cubes** — `MeshVertex` is now
  28 bytes (position f32×3, UV f32×2, packed light/shade/AO u32, packed tint
  u32), with shader-SPIR-V build guards preserving exact `/255` reconstruction.
  Full cube faces merge only when block state, packed light, tint and uniform AO
  are identical. Five directions merge; top faces deliberately remain unit
  quads because enabling them changed 11 canonical-demo pixels (10 owned by UV
  interpolation/nearest sampling, one by coverage). The replay drops
  149.13→109.39 MiB (−26.65%) and 3,723,192→3,373,772 vertices (−9.38%). Gate:
  **`rewo meshshot --check`**, an exact expanded-surface oracle against the
  frozen pre-greedy mesher, plus a byte-identical canonical demo.
- **M14 per-biome color** — exact grass/foliage/water tint and biome camera
  sky/fog, including retained section biome palettes and a permanent Vulkan
  `tintshot` oracle.
- **M10 client light engine (server-exact)** — the two-phase block+sky flood
  fill, transcribed from the decompile, so a placed torch lights and a dug
  tunnel brightens (a vanilla server sends no light for ordinary edits). Gate
  `rewo play --light-check`: **884,736 cells, 0 mismatches, both channels** vs
  the server's own engine.
- **M11 vanilla lightmap + day/night** — the real `l/(4-3l)` curve with block
  and sky kept separate and *added* (not `max`ed), no floor (an unlit cave is
  black), plus 26.x's keyframed `Timelines.OVERWORLD_DAY` sky-darken as a
  uniform (a sunrise costs one push constant, not a remesh).
- **M12 sun/moon/stars/sunrise + a smooth clock** — the clear-weather
  celestials in a Vulkan pass between sky and terrain: sun, moon through all
  eight phases, bit-exact JOML/LCG stars (780/4680), and the `Mth`-sine-table
  sunrise fan, all on the ported 26.2 `Timelines.OVERWORLD_DAY` cubic-bezier
  tracks. A `ClientClockManager` port drives a smooth world clock (fixing a
  frozen-clock bug in the M11 code). Gate: **`rewo skyshot --check`**.
- **M13 complete 26.2 lightmap** — the exact four-draw block flicker, gamma,
  night vision and darkness terms, plus the minimal local-player effect packet
  state that drives them. Terrain, water and entities consume one resolved RGB
  lightmap state. Gate: **`rewo lightmapshot --check`**, a validation-on Vulkan
  readback oracle that proves each term and rejects the old wrong block tint.

**Earlier (2026-07-22, one long session): M7 online-mode, real skins,
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
- `rewo skyshot --check` — **the sky gate** (M12): a serverless headless pass
  (validation layers on) that reconstructs each celestial transform in f64 and
  asserts read-back pixel properties — zenith tint, phase/alpha/discard/UV
  winding, the projected sun/moon envelopes, the analytic sunrise-fan
  footprint, and the 780/4680 star count. Run after any sky/celestial/lightmap
  change (`--out-dir <dir>` also dumps the rendered frames as an eyeball
  artifact).
- `rewo dimensioncheck --check` — **the serverless dimension gate** (M16). It
  grades three independent inputs against each other: a **captured** vanilla
  Configuration `registry_data` packet (read out of a recording by the
  production parser), the **bundled** built-in transcription, and the **real
  decompiled datagen JSON** at
  `…/26.2/decompiled/data/minecraft/dimension_type/*.json`, read by
  `rewo-app/src/dimension_json.rs` — a `serde_json` reader that shares no code
  with the NBT parser, extracts every client-consumed raw field itself, applies
  a default only where the codec proves one, and resolves `has_day_timeline`
  through the shipped `data/minecraft/tags/timeline/*.json`. A hand-written
  `EXPECT` table grades all three and is itself graded by the JSON. It then
  proves the world/mesh binding and the mesh pool's generation fence. Fails
  closed on a missing recording *or* a missing/malformed decompile
  (`--decompiled <dir>` overrides the version-derived path).
- `rewo eventshot --check` — **the serverless entity-event gate** (M17,
  fail-closed **28/28**): drives raw `ClientboundEntityEventPacket` bodies
  through the production dispatch → receipt-tick → `resolve_mob_anim` →
  rig-oracle path and grades Warden attack/sonic-boom + Armadillo peek against
  independent decompiled literals.
- `rewo danceshot --check` — **the serverless Allay-dance gate** (M18,
  fail-closed **24/24**): drives a raw report-resolved `set_entity_data` body
  → `route_set_entity_data`/`apply_set_entity_data` (kind-aware routing + vanilla
  missing-entity inertness) → `EntityTable` counter lifecycle →
  `live_cmd::resolve_allay_dance` → the GPU `AllayRoot`/`AllayHead` pose oracle,
  grading every value against an independent counter simulation and independent
  `AllayModel`/`AllayWing` formula transcriptions (real `packets.json`/
  `registries.json`; nothing reads the production formulas as its expectation).
- `rewo blockentityshot --check` — **the serverless block-entity gate** (M25,
  extended through M26, fail-closed **88/88**): a synthesised level-chunk
  payload through `read_level_chunk`, a `block_entity_data` body through the
  real dispatch, and `block_event` bodies through `route_block_event` into the
  chest and shulker clocks. It also re-derives the jar's model gap every run,
  and grades the block-entity classification against what the model resolver
  actually draws — in both directions, so neither half can drift.
- `rewo play --dimension-check` — **the live dimension gate** (M16): the paced
  Overworld→Nether→End→Overworld route, checking the level key, the respawn
  boundary, column discard/requeue, generation fencing and settled corrections.
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
7. **`block_event`'s `b0` is not a global opcode.** It selects a body on the
   block entity at that position — `b0 == 1` is a chest's viewer count, a
   shulker box's open/close pair (`== 0` / `== 1`, with **no else**, so a
   second viewer changes nothing) and a bell's `Direction.from3DDataValue`.
   Dispatch on the block-entity **type**, never on `b0` alone. Reading it as
   "a chest lid" made every bell ring open a phantom lid (M26).
8. **A witness that asserts a *moment* is not a guard.** `blockentityshot`'s
   old `a4` read "nothing is marked Rendered yet" and went on passing while
   four types shipped renderers, because the drift was in the table it
   restated rather than in the registry it guarded. Grade a claim against the
   code that would falsify it — `a4` now derives the rendered set from the
   model resolver — and grade it in both directions.
9. **Some source files are stored with mixed CRLF/LF terminators**
   (`rewo-data/src/lib.rs`, `rewo-gpu/src/entities.rs` at least). An editor
   that normalises them turns a 30-line change into a 3,400-line diff and
   trips `git diff --check`, since git reads the added CR as trailing
   whitespace. Check `git diff --stat` against what you meant to change.
10. **Never write these sources from a script without `encoding='utf-8'`.**
   Python's default on Windows is cp1252, which silently re-encodes a whole
   file and leaves it invalid UTF-8 — and the obvious repair then
   double-encodes every em-dash that was already correct, so the file has to be
   restored from HEAD. Pass `encoding='utf-8'` AND `newline=''` (see 9), or use
   the editor.

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
  M17 shipped the exact model-visible entity events: **Warden
  attack/sonic-boom and Armadillo re-peek fire from the `entity_event`
  packet** (§15). **M18 shipped the Allay dance** — `DATA_DANCING` metadata
  (index 16, BOOLEAN serializer 8), *not* an entity event (event 18 is heart
  particles): the exact `Allay.tick()` counters + `AllayModel` root/head
  formulas, gated by `rewo danceshot --check` (§15). Still open: the Warden
  tendril (event 61), generic `ClientboundAnimate` arm swings, creaking attack
  if its exact signal is still unclosed, and dragon flight (bespoke procedural
  code, not a rig — stays posed). Sheep wool dye-tint deferred (white),
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
  under open sky (sky 15). ~~The real gap is different and still open: there is
  no client-side light engine, so lighting never changes after chunk load.~~ —
  **RESOLVED M10 (§15)**: the two-phase block+sky flood fill relights and
  remeshes affected columns on every client edit, server-exact (gate
  `rewo play --light-check`: 884,736 cells, 0 mismatches).
- ~~**Only the overworld dimension is tested.**~~ — **RESOLVED M16 (§15)**: the
  Nether, the End and `overworld_caves` are parsed from the
  synced registry, selected by raw holder id, and exercised both serverlessly
  (`rewo dimensioncheck --check`) and live (`rewo play --dimension-check`,
  4/4 checkpoints, 3/3 transitions). Still open within it: dimensions the
  registry can express but vanilla does not ship (custom datapack dimension
  types) are parsed but unexercised, and the `{modifier, argument}` arm of an
  attribute override is a deliberate hard error rather than a modelled case.
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
- ~~No **greedy meshing**~~ — **RESOLVED M15 (§15)** for five safe full-cube
  directions. Merge keys include exact block state/light/tint and require
  uniform AO; top faces stay 1×1 because merging them changes sampled pixels.
- **AO only on cube faces**, not model quads.
- ~~**Per-biome tint** was a fixed plains color~~ — **RESOLVED M14 (§15)** for
  grass/foliage/water plus biome camera sky/fog.
- ~~No fluids + no translucent pass~~ — **RESOLVED 2026-07-21**: water
  (translucent, corner-height surfaces) + lava (opaque fullbright) with a
  CPU-sorted back-to-front translucent pass. See §15. Still open within
  it: waterlogged blocks don't render their water, no flowing-texture
  UV rotation.
- ~~No texture animation ticking~~ — **RESOLVED 2026-07-21**: `.mcmeta`
  frame order + frametime drive per-layer re-uploads on the 20 Hz tick
  (water ripples, lava churns; `demo --anim-tick N` is the deterministic
  check). Frame *interpolation* (lava's `interpolate` flag) not done.
- ~~**36-byte vertices**~~ — **RESOLVED M15 (§15)** with an exact 28-byte ABI.
  A smaller f16-UV format was measured and rejected because it changed pixels.
- **Grazing-angle far-field slivers** on flat ground at near-edge-on angles
  (candidate: MSAA / back-face cull once model-quad winding is guaranteed
  CCW). Cosmetic; the demo + normal angles are clean. (Much less visible
  now — the horizon fog covers the far field.)
- ~~**Sky is gradient + distance fog only**~~ — **RESOLVED M10–M12 (§15)**:
  time-of-day sky/fog darkening (M11) + the clear-weather celestials — sun,
  moon (all eight phases), stars, and the sunrise fan (M12). Still deferred:
  clouds and per-biome sky color (§7).
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

After M18, the most concrete remaining animation work is: (a) **the next
animation tracks** — the Warden tendril (event 61), or, as a separately scoped
milestone, generic `ClientboundAnimate` combat arm swings (handedness /
equipment-driven duration), plus any further metadata-driven rigs; (b) true
shape-union face occlusion and glow-lichen's any-face emission; (c) the
explicitly excluded redstone/stem/lily `BlockColors`; (d) physics parity outside
the on-foot flat-world subset (water, ladders, ice, cobwebs). Visual/product
work includes clouds/weather, HUD completeness and ETF random/emissive textures.
The async transfer/staging-ring idea remains measure-first; M15 closed greedy
cube meshing and vertex packing, M16 closed Nether/End + the dimension
transition, M17 closed the exact model-visible entity events (Warden
attack/sonic-boom + Armadillo re-peek), and M18 closed the Allay dance (the first
`DATA_DANCING`-metadata rig). Confirm direction with the user before diving in.

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

## 16. Forward plan — M23 to M25

*(Placed before §15 deliberately: the status log is append-only and must stay
last. This section is the forward plan; when a milestone ships, its record goes
in §15 and the entry here becomes history.)*

> **Status, 2026-07-26 — all three shipped, with two scope corrections.**
> M23 `d080ba3`, M24 `0f5d988`, M25 `88f4117`, all local on
> `codex/rewo-m19-combat-swings`, none pushed.
>
> | | planned | shipped |
> |---|---|---|
> | M23 | item-use state + the 8 poses | **in full**, plus two corrections the work turned up (see below) |
> | M24 | death **+ item entities** | death in full; **item entities carried forward** |
> | M25 | decode + registry **+ a first-class rendered set** | decode + registry + the measured gap; **rendering carried forward** |
>
> **Both cuts were estimate errors in this plan, not scope the work
> discovered was unnecessary.**
>
> - *Item entities* were called "nearly free after M22 — the same models, a new
>   placement". They are not: they need the `ITEM_STACK` metadata serializer
>   (id 7, which the skip table bails on) **and** the `GROUND` item-display
>   context, which M22 deliberately never read — it takes only
>   `thirdperson_{right,left}hand`. That is a second pipeline.
> - *Block-entity rendering* was scoped to "chest / shulker / bed / decorated
>   pot". Each needs a model transcription, an entity-atlas slot, a facing
>   transform and — for chests — a lid angle driven by `block_event`. The
>   decode does not imply any of it. M25 shipped the half that unblocks the
>   rest, plus a fail-closed boundary and a *measured* statement of what is
>   missing (86 real blocks, re-derived from the jar on every gate run).
>
> **Corrections M23 made to earlier milestones**, both from reading the two
> humanoid renderers side by side:
> 1. `AvatarRenderer.getArmPose` and `HumanoidMobRenderer.getArmPose` are
>    different functions and had been collapsed into one since M19. The mob
>    version falls through to `EMPTY`, not `ITEM` — an armed zombie's arm had
>    been sitting 18° too high.
> 2. The pose must be computed per **hand**, not per arm, because
>    `mainHandPose.isTwoHanded()` rewrites the *off-hand* pose.
>
> And one near-miss worth recording: grepping for assignments to
> `state.isUsingItem` finds exactly one, inside `HumanoidMobRenderer`, which
> reads as "players never have use state" and implies a divide-by-zero NaN in
> `CROSSBOW_CHARGE`. That was wrong — `extractHumanoidRenderState` is a
> **static helper** and `AvatarRenderer:168` calls it. Grep found the
> assignment; it could not show who called the enclosing method.
>
> **Both carried items were then closed in the same session** — `37daed9`
> (item entities) and `5e3b2e1` (chests render), so nothing from M23–M25's
> planned scope is outstanding.
>
> - *Item entities* needed exactly what the correction predicted: the
>   `ITEM_STACK` serializer (which also forced `metadata::parse` to take the
>   component ids, since it is the one serializer whose **skip** needs external
>   data) and `display.ground`. One thing the plan did not predict:
>   `ItemEntityRenderer` extends `EntityRenderer`, not `LivingEntityRenderer`,
>   so a ground item has **no** `scale(-1,-1,1)` / `-1.501` flip and needed its
>   own emitter rather than a flag on the held path. `itemshot` 18 → 28.
> - *Chest rendering* baked block-entity models into the held-item shape —
>   quads in 0..16 with their own texture is already what both are — so they
>   reuse the pool, atlas and UV lookup. The transform is separate for the same
>   flip reason. `blockentityshot` 21 → 32. Double chests
>   (`DoubleBlockCombiner` pairing), the lid angle (`block_event`) and the other
>   ten Invisible types remain, each named in the registry rather than silently
>   absent.
>
> **All four of those then shipped too** — `a13d181` (chest lids +
> double chests), `c7215e0` (shulker boxes), `0a87a4a` (world-space text /
> signs). `blockentityshot` grew 21 → **70** witnesses across them.
>
> - *`block_event`* was falling off the dispatch chain entirely. `b1` is the
>   **viewer count**, not a boolean, and `ChestLidController` is another clock
>   the server never sends — one event, ten ticks animated locally, with a
>   cubic ease that puts the lid 87.5% open half way through.
> - *Double chests* — **M25 recorded the wrong blocker.** `ChestRenderer` picks
>   the half-model from the block's own `type`; `DoubleBlockCombiner` is only
>   for the shared openness (the **max** over the pair, so both halves open when
>   one gets the event) and the shared light. Each half draws itself.
> - *Shulker boxes* (17) forced the block-entity transform to become a general
>   affine — a chest rotates about the block centre, a shulker box runs a
>   translate-scale-rotate-flip chain that ends up y-down. Its model is authored
>   upside down *because* of that trailing `scale(1, -1, -1)`.
> - *World-space text* turned out to be a small addition, not a new pass: a
>   nametag is already world-space glyph quads, and sign text is the same thing
>   with the basis taken from the surface instead of the camera. The board was
>   never missing — only the text.
>
> **Then M26 closed the shulker opening and corrected two of these claims** —
> `420baf4`, and see §15 for the blow-by-blow.
>
> - *A shulker box's own opening* **is** `block_event`; the line below calling
>   it "a different mechanism from the chest lid" was half right. Same packet,
>   same `b0 == 1` — but `ShulkerBoxBlockEntity.triggerEvent` tests `b1 == 0`
>   and `b1 == 1` with **no else**, where the chest tests `b1 > 0`. Reusing the
>   chest's rule would have opened a box on a second viewer that vanilla leaves
>   alone.
> - *`block_event` for bells* turned out to be a **bug, not a gap**. The route
>   read `b0 == 1` as "a chest lid" for every block entity, and a bell's ring
>   is also `b0 == 1` — with `b1` a `Direction.from3DDataValue`, so ringing one
>   from any side but below opened a phantom lid at the bell. Dispatch is by
>   block-entity **type** now, which is what vanilla's virtual `triggerEvent`
>   call always was.
> - And the count below was wrong: it is **seven** types, not eight. The
>   classification had gone stale — chests and shulker boxes were still marked
>   `Invisible` after shipping renderers, and the witness guarding it asserted
>   "nothing is Rendered yet", so it passed happily through all four. That
>   witness now derives the rendered set from the model resolver instead.
>
> **M27/M28 closed every one of these** — `e0b9937`, `73a6c61`, `73cd504`,
> `c428d90`, `8d3754b`, `c838d7f`; see §15. Dyed and glowing sign text and the
> line break; skulls (7 types, 14 blocks), the conduit, the decorated pot,
> banners (32 blocks), the spawner's `block_event`, the copper golem statue and
> the two end portals.
>
> **M25's Invisible list is empty: eleven types measured, eleven rendering.**
> `blockentityshot` grew 21 → **147** witnesses across the arc.
>
> **M29 then shipped the block-entity animation clock** (`de9c4e1`), which was
> the one shared gap underneath all of them. A banner sways, a pot wobbles, a
> piglin head's ears move and a dragon head's jaw opens. It also exposed two
> **rest poses that were already wrong** — `setupAnim` always runs, so those
> ears and that jaw were never at the mesh's own pose.
>
> **M29 then shipped the animation clock and M30 the conduit's world scan**,
> so the active cage, wind and eye render too. **Two items remain**, each
> naming a capability this client does not have: a spawner's caged mob needs an
> **entity model composed into a block-entity draw**, and an end portal's
> starfield needs the render type's **shader**.

M19 to M22 built the entity-visual arc — the exact swing, the mob combat rigs,
the damage flash, the item in the hand — and **each one shipped with a stated
exclusion**. Read together, those exclusions name one blocker three times:

| Exclusion | Shipped in | Blamed on |
|---|---|---|
| the eight use-driven `ArmPose`s | M19 | "item-use state is not synchronised" |
| illager `CROSSBOW_HOLD` / `CROSSBOW_CHARGE` | M20 | same |
| `animateUseItem` + `thirdPersonAttackItem` | M22 | same |

**That blocker is not real, and M23 is the correction.** `useItemRemainingTicks`
is not synchronised because it does not need to be — the client *derives* it.
The sequence below therefore starts by retiring three milestones' worth of debt,
which also shrinks the two that follow.

Ordering is by leverage, not by size.

---

### M23 — item-use state (the backlog unlock)

**The finding.** `LivingEntity.onSyncedDataUpdated` reconstructs the entire use
clock client-side from a single metadata bit:

```java
} else if (DATA_LIVING_ENTITY_FLAGS.equals(accessor) && this.level().isClientSide()) {
   if (this.isUsingItem() && this.useItem.isEmpty()) {
      this.useItem = this.getItemInHand(this.getUsedItemHand());
      if (!this.useItem.isEmpty()) {
         this.useItemRemaining = this.useItem.getUseDuration(this);   // derived
      }
   } else if (!this.isUsingItem() && !this.useItem.isEmpty()) {
      this.useItem = ItemStack.EMPTY;
      this.useItemRemaining = 0;
   }
}
```

`updateUsingItem` then decrements it once per tick. Every input already exists
in Rewo: the metadata decoder, the equipment decoder, and a per-entity tick.

**Confirmed ground truth.**

| Fact | Source |
|---|---|
| `DATA_LIVING_ENTITY_FLAGS` = index 8, BYTE | `LivingEntity:179` — `defineId(LivingEntity.class, BYTE)`, the first `LivingEntity` slot after `Entity`'s 0..7 |
| bit 1 = using, bit 2 = off-hand | `isUsingItem()` `& 1`; `getUsedItemHand()` `& 2 ? OFF_HAND : MAIN_HAND` |
| `getTicksUsingItem()` = `getUseDuration() - useItemRemaining` | `LivingEntity:3595` |
| base `getUseDuration` reads components | `Item:310` — `CONSUMABLE.consumeTicks()`, else `BLOCKS_ATTACKS`/`KINETIC_WEAPON` ? 72000 : 0 |
| `consumeTicks()` = `(int)(consumeSeconds * 20)` | `Consumable:100`, default `consumeSeconds` 1.6 |
| base `getUseAnimation` reads components | `Item:299` — `CONSUMABLE.animation()`, else `BLOCKS_ATTACKS` -> BLOCK, else `KINETIC_WEAPON` -> SPEAR, else NONE |
| only **8** item classes override either | `BowItem` 72000/BOW, `BrushItem` 200/BRUSH, `BundleItem` 200/BUNDLE, `CrossbowItem` 72000/CROSSBOW, `EnderEyeItem` 0, `InstrumentItem` `floor(useDuration*20)`/TOOT_HORN, `SpyglassItem` 1200/SPYGLASS, `TridentItem` 72000/TRIDENT |
| `CROSSBOW_HOLD` gate is `isCharged` | `CrossbowItem:101` — `CHARGED_PROJECTILES` non-empty; a **synchronised** component, so it is on the wire |
| illager charge duration | `IllagerRenderer:22` + `CrossbowItem:245` — `floor(modifyCrossbowChargingTime(1.25) * 20)` |

The component values come from the datagen per-item report, the same source
`gen_swing_animations.py` already reads. Counted on the real jar:
`minecraft:consumable` on 43 items, `blocks_attacks` on 1, `kinetic_weapon` on 7.
So the table is small, exact, and machine-extractable.

**Work items.**

1. `tools/gen_use_items.py` -> a generated `use_item_table.rs`: per item, the
   `(use_duration, use_animation)` pair, base-component rule plus the 8 class
   overrides. Fails loud on an unknown `ItemUseAnimation` name, a missing report
   tree, or an override class whose literal no longer parses.
2. Decode metadata index 8 (BYTE) in the entity table; add the use clock —
   start on the rising edge at the derived duration, decrement per tick, clear
   on the falling edge. This mirrors `onSyncedDataUpdated` exactly, including
   that a *repeated* true does not restart it.
3. Extend the `ArmPose` derivation to the full eleven, in
   `AvatarRenderer.getArmPose`'s order: empty -> charged-crossbow hold ->
   the eight use-animation cases -> STAB-while-swinging -> spear tag -> item.
4. The pose bodies, from `HumanoidModel.poseRightArm`/`poseLeftArm`.
   `BOW_AND_ARROW` and the two crossbow poses **write to both arms**, which the
   current per-arm pipeline cannot express — that restructuring is part of the
   milestone, not a reason to defer them again.
5. Illager `CROSSBOW_HOLD` / `CROSSBOW_CHARGE` via `AnimationUtils`.

**Gate.** Extend `swingshot` (arm poses live there) and `itemshot` (the
item-side animation) rather than adding a twelfth oracle — a property belongs
with its subject. New witnesses must include the clock's edge behaviour
(rising, repeated-true, falling, entity removal) and at least one
mutation partner per pose, not merely a classification assertion. The M20
lesson applies: asserting that a pose was *selected* does not prove the pose
*math*.

**Honest risk.** `InstrumentItem`'s duration reads an `Instrument` registry
value, not a literal. If that registry is not resolvable from the reports,
TOOT_HORN's duration is unknowable and must suppress rather than guess.

---

### M24 — death, and things on the ground

Two halves of "what happens when something dies", both cheap on M21/M22
groundwork.

**Death** completes an M21 exclusion that was stated rather than implied:
`hasRedOverlay = entity.hurtTime > 0 || entity.deathTime > 0`
(`LivingEntityRenderer:281`) — M21 shipped only the first term.

```java
if (state.deathTime > 0.0F) {                       // LivingEntityRenderer:164
   float fall = (state.deathTime - 1.0F) / 20.0F * 1.6F;
   fall = Mth.sqrt(fall);
   if (fall > 1.0F) fall = 1.0F;
   poseStack.mulPose(Axis.ZP.rotationDegrees(fall * this.getFlipDegrees()));
}
```

`getFlipDegrees()` is 90 by default and overridden by exactly three renderers
(`EndermiteRenderer`, `SilverfishRenderer`, `SpiderRenderer`) — a three-entry
exception table, so it can be exact rather than approximate.
`state.deathTime = entity.deathTime > 0 ? entity.deathTime + partialTicks : 0`
(`:297`); `tickDeath` increments it and the server removes the entity at 20
(`LivingEntity:572`). Health arrives as `DATA_HEALTH_ID`, **index 9, FLOAT**
(`LivingEntity:180`, the slot after the flags byte).

**Item entities** are nearly free after M22 — the same models, a new placement:

```java
float bob  = Mth.sin(state.ageInTicks / 10.0F + state.bobOffset) * 0.1F + 0.1F;
float spin = ItemEntity.getSpin(state.ageInTicks, state.bobOffset);  // age/20 + offset
poseStack.translate(0.0F, bob + minOffsetY, 0.0F);
poseStack.mulPose(Axis.YP.rotation(spin));
```

with `minOffsetY = -modelBoundingBox.minY + 0.0625` and stack-count copies
jittered by +/-0.15 from a seeded `RandomSource`.

**The one thing that cannot be exact, and why that is fine.** `bobOffs` is
`random.nextFloat() * 2 * PI` (`ItemEntity:54`) — rolled by *whichever* client
constructs the entity, never sent. There is no server truth to match. Rewo will
roll it deterministically per entity id, which is as vanilla as vanilla is; the
gate must therefore assert the bob/spin *formula* given an offset, and must not
pretend to assert the offset itself.

**Gate.** Extend `hurtshot` for the overlay's second term and the topple
rotation (it already owns the overlay); item entities get render witnesses in
`itemshot`, whose models they reuse.

---

### M25 — block entities (the largest remaining world gap)

**Verified, not assumed.** Two checks establish the gap is real and total:

```json
// assets/minecraft/models/block/chest.json — the entire file
{ "textures": { "particle": "minecraft:block/oak_planks" } }
```

No `elements`, so a chest bakes to **nothing**. And `grep` for block-entity
handling across `rewo-net`, `rewo-world` and `rewo-mesh` returns **zero** hits —
they are not decoded at all. `chunk.rs:430` walks the payload's block-entity
list purely for alignment and discards every field:

```rust
// Block entities: VarInt count, [u8 packedXZ, i16 y, VarInt type, NBT].
let be_count = r.count("block entities", 1)?;
for _ in 0..be_count { let _packed_xz = r.u8()?; let _y = r.i16()?;
                       let _type = r.varint()?; let _nbt = r.nbt()?; }
```

So every chest, sign, banner, bed, shulker box and decorated pot in every world
Rewo has ever rendered has been invisible.

**Scope is the design.** Vanilla ships **33** renderers under
`client/renderer/blockentity/`. Transcribing all of them is not one milestone,
and a partial set that fails *open* would render a silent subset while claiming
coverage. M25 therefore follows the mob-redo precedent: a **fail-closed
registry** where every vanilla block-entity type is either first-class or
explicitly listed as unsupported, and a type in neither list is an error.

First-class set (chosen because each is model + texture with no dynamic text or
bespoke shader): **chest / trapped / ender** (single and double), **shulker
boxes**, **beds**, **decorated pots**. Everything else — signs, banners,
beacons, conduits, spawners, end portals, skulls, bells, lecterns, campfires,
pistons, brushable blocks, enchanting tables, vaults, shelves — is registered
unsupported with a one-line reason.

**Work items.** Decode the block-entity list (position, type, NBT) into the
column and honour the `block_entity_data` packet; a registry keyed by the
type-registry id resolved *by name*, so a version renumber fails loud; the
model transcriptions; the render pass.

**Honest risk.** Sign text is the reason signs are excluded, and it is worth
naming: Rewo's font pass (`TextPass`) is screen-space, so a sign needs a
world-space text path that does not exist yet. That is a milestone of its own,
not a corner of this one.

---

### Deliberately not proposed

- **Particles** — a whole subsystem, and every existing gate is geometry-based;
  it would need a new verification approach before it could be shipped honestly.
- **Sound** — outside a renderer's scope.
- **First-person hand / GUI** — needs an inventory model Rewo does not have.
- **Weather and clouds** — self-contained, so it can slot in anywhere; that
  makes it filler rather than part of an arc.

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

### 2026-07-23 — M12: the sun, moon, stars, and the sunrise fan

M11 left the sky a bare gradient. M12 draws the clear-weather Overworld
celestials — sun, moon (all eight phases), stars, and the sunrise/sunset fan —
in a Vulkan pass between the gradient sky and the terrain, driven by the exact
26.2 timeline and a smooth, server-driven world clock. It also closes the M11
`SKY_COLOR` tint bug (the zenith was left blue at midnight) and roots out a
frozen-clock bug the M11 day/night code was quietly sitting on.

**The timeline, ported not approximated** (`rewo-world/src/celestial.rs`). The
sun/moon/star **angles** are not a naive lerp: `Timelines.OVERWORLD_DAY` gives
each `ANGLE_DEGREES` track two keyframes that *both sit at tick 6000* (values
360 and 0), and `KeyframeTrackSampler.bakeSegments` turns that into a
wrap-around segment pair spanning `[6000 − 24000 .. 6000]` and `[6000 .. 6000 +
24000]`. The lerp across those carries the symmetric cubic-bezier ease
`symmetricCubicBezier(0.362, 0.241)`, so the module ports `EasingType.CubicBezier`
verbatim — `curveFromControls`, the 4-iteration Newton-Raphson `solve_t` (step
clamped to ±0.25, tolerance 1e-5) with the bisection fallback, all constants
matching the decompile. Star brightness and the sunrise colour use the default
LINEAR keyframe ease; the sunrise track is an `ARGB_COLOR` override interpolated
by `ARGB.srgbLerp` — a plain componentwise `Mth.lerpInt` **including alpha**,
which is what settles the "RGB vs ARGB" question (all four channels move in
gamma space, floored not rounded). Moon phase is `floor(t / 24000) mod 8` done
with Euclidean arithmetic so negative ticks wrap like `Math.floorMod`. The
module carries 15 unit tests pinning the wrap semantics, the bezier fixed
points/monotonicity, the moon-leads-sun-by-180 relation, and the channel-by-
channel sunrise lerp.

**The render pass** (`rewo-gpu/src/celestial.rs` + four shaders). Drawn in
`LevelRenderer.addSkyPass` order — sunrise → sun → moon → stars — in
rotation-only sky space (`view_proj · T(eye)` cancels the camera translation so
the bodies sit at infinity), no depth (terrain overwrites them via reversed-Z
where it exists). The transform chains are the decompiled ones: a shared base
`Y(−90°)`, then per body `X(angle)`; the sun adds `T(0,100,0)·scale(30,1,30)`,
the moon `scale(20,1,20)`; the sunrise fan uses its own pose
`X(90°)·Z(angle+90°)·scale(z=alpha)` with no `Y(−90°)`. The moon UV winding is
reversed vs the sun (`buildMoonPhases`) and the phase selects a base vertex into
an 8-cell atlas. Blend functions match `RenderPipelines`: CELESTIAL and STARS
use `OVERLAY` (colour `src=SRC_ALPHA dst=ONE`, additive by alpha), SUNRISE_SUNSET
uses `TRANSLUCENT`; the fragment discards fully-transparent texels so a
zero-alpha sun never touches the attachment. The sun + eight moon-phase textures
come from the user's own client jar (`environment/celestial/sun.png` +
`moon/<phase>.png`, one file per phase, `MoonPhase.index()` order); a jar without
them degrades to a bare sky rather than erroring.

**Stars are generated bit-for-bit** (`buildStars`). A seed-10842
`SingleThreadedRandomSource`/`BitRandomSource` 48-bit LCG produces 1500
candidates; each is rejected if its squared length is `≤ 0.010000001` or `≥ 1.0`,
and the accepted count is *reported*, never forced to 1500 — it comes out to
**780 accepted / 4680 triangle indices**. Reaching bit-exactness against an
independent Java oracle compiled on the real JOML 1.10.8 jar required porting
JOML's exact scalar op order: `org.joml.Runtime.HAS_Math_fma` is **false** by
default, so `org.joml.Math.fma` is a plain **non-fused** `a*b + c`, making
`Vector3f.lengthSquared` the right-associative `x*x + (y*y + z*z)` — distinct by
up to 2 ULP from both a true FMA and the left-associative `Mth.lengthSquared`
the rejection test uses. `invsqrt`, the float `sin`, and `cosFromSin` (computed
entirely in float, with the float-literal constants) are likewise ported;
`libm::sin` (fdlibm-derived, added as a dep) matches Java's `Math.sin` at the
`(float)` cast where the platform libm on Windows drifts. The accepted set, its
first quad's raw f32 bits, and an FNV-1a-64 fingerprint `fef182656c6fe202` are
pinned against the oracle.

**The sunrise fan uses the `Mth` sine table, not platform trig.**
`buildSunriseFan` calls `Mth.sin`/`Mth.cos`, which index a 65,536-entry table
(`SIN[i] = (float)Math.sin(i / 10430.378350470453)`, cosine offset a quarter
turn), and `renderSunriseAndSunset` picks the fan's side from `Mth.sin(sunAngle)
< 0`. This is load-bearing at the half-turn: `Mth.sin(π)` reads the tiny
*positive* table entry `SIN[32768] ≈ 1.2e-16`, so the boundary resolves to fan
angle 0°, whereas platform `sin(π_f32)` is slightly negative (its argument rounds
just past π) and would wrongly flip to 180°. Only the 17 selected indices are
evaluated; the 18 logical fan vertices are pinned by fingerprint
`75280003503b2a33`.

**The `SKY_COLOR` zenith fix.** Vanilla's `SKY_COLOR` (`MULTIPLY_RGB`) tints the
whole sky disc; M11 tinted only the horizon, so midnight kept a blue zenith.
`draw_sky` now scales the zenith colour by the sky tint too.

**The frozen-clock bug and its fix.** M11's day/night code read `day_ticks`
from `set_time` and held the last value when a packet carried no clock state.
But `MinecraftServer.forceGameTimeSynchronization` broadcasts `SetTime(gameTime,
Map.of())` every 20 ticks with an **empty** clock map — only a real change (join,
`/time set`) carries an explicit state. So the game time advanced while the
day/night clock stayed frozen; a first repair that re-derived from the sync
alone then moved only at the 20-tick packet cadence, in visible jumps. The final
repair ports 26.2's `ClientClockManager` (`rewo-net/src/play.rs`): a `WorldClock`
with `total`/`partial`(f32)/`rate`/`last_game_time`, advanced from the same two
places vanilla advances it — `apply_set_time` (the `handleUpdates` path: advance
by the game-time delta first, so an *empty* update still moves the cycle, then
let any explicit overworld state overwrite it) and `local_tick_time` (the
per-tick `ClientLevel.tickTime` local `+1`). Because each `advance` re-bases its
delta on `last_game_time`, running both is not double-counting: an empty sync at
the already-predicted game time advances by zero, leaving exactly the one local
`+1`. The primitive semantics are ported exactly: `gameTime − lastTickGameTime`
is wrapping `long` subtraction; `Mth.floor` returns a Java **`int`** (so the
floor narrows to `i32` — saturating, `NaN → 0` — before widening to
`long fullTicks`, since a direct `as i64` would saturate to the wrong bounds);
`(float)(newPartialTicks − fullTicks)` truncates the carry back to `f32` so it
rounds bit-for-bit the way the client keeps it; `totalTicks += fullTicks` wraps.
The two clocks (overworld + the_end) are still matched by registry id captured
from Configuration, and `holderRegistry` still writes the id raw.

**Verification — `rewo skyshot --check`**, a new permanent serverless headless
gate (`rewo-app/src/skyshot_cmd.rs`, 256², validation layers on). It never
asserts "looks right"; every check reconstructs the property independently:

- *Zenith/whole-gradient tint (permanent M11 regression).* Both the up view and
  the level view must scale by a requested tint `[0.35, 0.60, 0.80]`; measured
  ratios were zenith **0.349 / 0.600 / 0.798** and horizon **0.348 / 0.599 /
  0.799** (attachment decoded sRGB→linear before the ratio). A midnight zenith
  must be near-black and a noon zenith blue-and-bright — the M11 bug fails this.
- *Phase / alpha / discard / UV orientation (synthetic textures over black).*
  The phase-0 moon (alpha 128) reads centre RGBA **[169, 27, 27, 128]**; phases
  1–7 identify to their synthetic centre colours exactly; a zero-alpha sun over
  black reads **[0, 0, 0, 255]** (the discard witness — an RGB-only write mask or
  a wrong alpha blend would leave alpha at the sun's 0). The phase-7 8×8
  orientation texture maps screen-TL→texture-BR **[255,255,30]**,
  screen-TR→TR **[30,255,30]**, screen-BL→BL **[30,30,255]**, screen-BR→TL
  **[255,30,30]**, all d²=0 — pinning the reversed moon UV winding a solid
  colour can't catch.
- *Projected transform + size oracle.* The sun/moon quad corners are pushed
  through the decompiled chain reconstructed in f64 *inside the test* (never via
  a `CelestialPass` helper) and projected through the known up-camera at a
  nontrivial +15° angle. The scale-independent shared model-centre lands at
  **(176.982, 128)**; the sun envelope expected `(122.577, 66.262)..(240.898,
  189.738)` read back **(123, 66)..(241, 190)**, the moon expected `(139.790,
  88.006)..(218.386, 167.994)` read back **(140, 88)..(218, 168)** — a swapped
  30/20 or a flipped sign moves the envelope > 17 px and fails.
- *Sunrise-fan placement oracle.* An isolated fan over black with a controlled
  positive sun angle (fan angle 0 → bright centre on the −X horizon), a distinctly
  non-1 alpha 0.5 (so a dropped `scale(z=alpha)` roughly doubles the height), and
  a warm colour. The analytic above-horizon band is y **97.533..128**; the −X
  read-back full-x bbox is y **98..129** over **7,936** warm pixels, while the
  opposite +X heading shows **0** comparable above-horizon warm pixels — rejecting
  a wrong X/Z order, a flipped sign, a dropped alpha scale, or the fan on the
  opposite horizon.
- *Accepted star count* reported as **780 / 4680**.

**Gates.** All release unit tests green: **142** across world 44, net 41, gpu
33, data 5, mesh 8, proto 11. `skyshot --check` green with validation layers on.
`mobshot --check` 243/243. The demo PNG is byte-identical to baseline
(SHA-256 `2CC56B4ACBFB92CB91398C27E5C4735885ABFF9331F66B7DC83BDBC002246635`).
`bench`: GPU avg 0.228 ms, p50 0.220, p99 0.394, p99.9 0.648, max 0.677 (GPU 1%
low 0.490, 0.1% 0.672); wall avg 0.363, p50 0.325, p99 0.748, p99.9 1.576, max
1.714 (wall 1% 0.991, 0.1% 1.651). Canonical light gate still EXACT — 884,736
cells, block 0, sky 0 — and the world clock advanced **+278 over 280 ticks** on
that run, the headless proof the frozen-clock fix works (`rewo play` now prints
`world clock: start → end (advance …)`; a frozen clock reads `advance 0`).

**Honest failure history.** The first canonical physics run after the clock
change came back **RED with CORRECTIONS 1** (clock +598/600). An immediate,
unchanged repeat was **CORRECTIONS 0** (clock +598/600), so the named parity
gate passes — but the transient red is recorded here, not concealed, because a
one-off correction that vanishes on repeat is exactly the kind of thing worth
leaving a trace of. On that clean repeat an unrelated PLACE verification reported
still-air while the DIG verify succeeded, so the action script that run was not
wholly green even though the physics meter was; the celestial/clock properties
are what M12 verifies, and those are solid. The test server was stopped
afterward.

**Left open** (unchanged from the standing list, none touched by M12): the
remaining lightmap terms (block-light flicker, gamma/brightness,
night-vision/darkness); per-biome sky/fog/water tint; the geometry-side
performance work (greedy meshing, packed vertices); Nether/End are coded but
untested (dimension-specific `ambientLight` unwired); entity-*event*-driven
animations need the `entity_event` packet; face-occlusion merging is tested
per-side rather than as a true shape union; and `glow_lichen`'s any-face
emission stays approximated at 7.

### 2026-07-23 — M13: the complete 26.2 lightmap

M11 deliberately stopped at the static lightmap curve. M13 closes the four
remaining terms from the 26.2 client itself: block-light flicker, the user's
gamma/brightness option, night vision and darkness. The ground truth was
`LightmapRenderStateExtractor`, `Lightmap`, `GameRenderer`, `LivingEntity`,
`MobEffectInstance`, `Blendable`, and the clientbound update/remove-effect
packet classes under the decompiled jar, plus
`assets/minecraft/shaders/core/lightmap.fsh`. No wiki-derived behavior is in
this pass.

**Exact state and order.** `rewo-world/src/lightmap.rs` owns the Java-compatible
48-bit `LegacyRandomSource`, the four-float flicker update and its damped
accumulator, the 65,536-entry `Mth` sine table, night-vision duration envelope,
darkness blend/cosine terms, and an independent CPU transcription of the
shader. The shader order is load-bearing: curve the two 0–15 levels with
`l/(4-3l)`; seed from max(ambient, night vision); add sky; apply the parabolic
block tint; apply boss color; subtract darkness; clamp; apply `notGamma`; then
mix by brightness. `GameRenderer` passes a fixed partial tick **1.0** to the
extractor—using render interpolation here was an initially plausible but wrong
implementation caught during review. Gamma defaults to vanilla's **0.5** and
darkness-effect scale to **1.0**, both exposed as validated `rewo live` flags.

The jar shader also corrected an older transcription error: block tint is
`-10100 = 0xFFFFD88C`, RGB **255/216/140**, not the previously used
`0xFFD86C`. Night vision is `0x999999`; ambient remains black in the currently
tested Overworld. The CPU black sample intentionally retains the shader's
`0/0` NaN semantics rather than inventing a zero guard; the production
R8G8B8A8_SRGB store is separately measured by the oracle as `(0,0,0)`.

**Effect protocol state.** `rewo-net/src/effects.rs` parses the 26.2
local-player update/remove-effect packet layouts, captures night-vision and
darkness registry raw IDs during Configuration, and tracks only the visual
state the lightmap consumes. The login player entity id gates ticking; finite
durations decrement on the same client-tick side as vanilla, removal is
immediate, night vision follows the exact 200-tick envelope, and darkness uses
the 22-tick blend state. Replacements copy the prior blend state even when the
new effect itself is non-blended—the decompiled `forceAddEffect` behavior that
an intuitive reset would get wrong. Blend interpolation is not clamped.

**One state, every consumer.** `WorldLightmapState` occupies the existing
128-byte push-constant budget. Terrain and water run the same GLSL include;
entity vertices receive the same resolved RGB rather than a scalar light. The
headless and windowed loops advance one flicker stream per successful client
tick and resolve one state for terrain, water and entities each frame, avoiding
three subtly different copies of the formula.

**Permanent property oracle: `rewo lightmapshot --check`.** This is a
serverless production-pipeline Vulkan readback with validation layers required
and reported **ON**; it fails closed if validation is unavailable unless the
caller explicitly opts out. Synthetic white textures isolate the lightmap and
independent CPU expectations grade the rendered bytes. The matrix proves the
correct warm tint (actual/expected **117,109,89**, with the old blue value 19
levels farther away), block-factor response (max-channel delta **37**), gamma
monotonicity at 0/.5/1 (luma **119.49→163.48→195.55**), night vision over
black (**203,203,203**), black NaN store (**0,0,0**), positive/zero/doubled
darkness at fixed partial 1 (double-option expected and actual
**0.11215252**), water/opaque equality (delta **0**), and production entity
geometry's normalized RGB (actual **1/.8467/.5511**, CPU
**1/.8477/.5508**, 2,416 lit pixels). A green result is not a screenshot or a
proxy: changing the old tint, term order, gamma, effect state, water include or
entity channel transport breaks a named property.

**The uncompromised M10 gate found an adjacent real bug.** The first final
`--light-check --no-relight` run was red: **884,736 cells, block 0, sky 7**,
all seven water cells stored by vanilla at 14 but recomputed at 15. A clean
server reload reproduced it. The asset baker's special fluid branch had
`continue`d before the common light assignment, silently leaving water
dampening at 0 although decompiled `LiquidBlock.propagatesSkylightDown()` is
false and `BlockBehaviour.getLightDampening()` therefore returns 1. The same
branch also left lava emission at 0 despite the generated table's 15. The
small repair reads those facts from the generated tables before the branch;
its focused regression test pins water `(emission 0, dampening 1)` and lava
`(15,1)`. Generated code was not hand-edited. The unchanged live oracle then
returned **884,736 cells, block 0, sky 0 EXACT**, twice, including a fresh join
after the final physics run.

**Final gates.** Release tests: **180/180** across world 58, net 60, gpu 37,
data 6, mesh 8 and proto 11, plus **10/10** app tests. `lightmapshot` and
`skyshot` green with Vulkan validation ON; `mobshot` **243/243**. The demo PNG
remains byte-identical at SHA-256
`2CC56B4ACBFB92CB91398C27E5C4735885ABFF9331F66B7DC83BDBC002246635`.
Physics is **CORRECTIONS: 0** over 600 ticks; final lighting is the exact result
above. Replay produced one clean sample at GPU avg **0.236 ms**, p50 0.222,
1%/0.1% tail means **0.816/0.982 ms**. Three later unchanged samples were
system-noisier: avg **0.266–0.277**, p50 **0.224–0.231**, tail means
**1.573–1.798 / 2.063–2.668 ms**. The stable median and the fact that the
fluid-bake repair does not affect the replay argue against a rendering-cost
regression, but the tail variance is recorded, not relabeled green. The test
server was stopped and port 25599 verified free.

**Honest failure history.** Besides the seven-cell water failure that caused
the fluid fix, the final physics run's named correction meter passed but its
auxiliary PLACE probe again read still-air while DIG succeeded, the same
intermittent harness artifact recorded in M12. A post-restart still run also
showed one expected spawn/teleport correction once; the unchanged canonical
run returned zero. These did not weaken any M13 assertion, but they remain in
the durable record.

**Left open after M13:** per-biome sky/fog/foliage/water tint; greedy meshing
and packed vertices; Nether/End verification and dimension ambient light;
entity-event animations; true shape-union face occlusion; and glow lichen's
any-face emission approximation. Clouds/weather and HUD completeness remain
separate future visual work.

### 2026-07-24 — M14: per-biome color (grass/foliage/water tint + camera sky/fog)

M0–M13 rendered every terrain block through one global tint baked at asset time;
the world had no biomes. M14 makes color depend on where you stand. Ground truth
was the 26.2 decompile and datagen only: `RegistryData`, `Biome` /
`BiomeSpecialEffects` / `BlockColors` / `BlockTintSources`, `BiomeManager`,
`ClientLevel.calculateBlockTint` + `BlockTintCache`, `PalettedContainer` /
`SimpleBitStorage`, `ClientboundChunksBiomesPacket`, `EnvironmentAttributeProbe`
/ `GaussianSampler` / `SpatialAttributeInterpolator`, `DimensionType.STREAM_CODEC`
/ `ByteBufCodecs.holderRegistry`, and `ARGB.srgbLerp`. No wiki-derived behavior.

**Registry + the dimension-holder correction.** The Configuration live registry
is decoded in raw wire order — **66 biomes, 4 dimension types** — and their
special-effects colors and temperature/downfall drive resolution. The play-login
dimension is a `holderRegistry`/idMapper reference: a **raw 0-based** id, NOT
`ByteBufCodecs.holder`'s inline/`id+1` convention. Production review found the
code was treating it with the inline/`id+1` scheme; correcting that adjacent
protocol bug was a prerequisite for selecting the right dimension sky/fog base,
and the fix was carried through the login path, replay and tests. The final
diagnostic attached the live context with `biomeZoomSeed`
**6105022145440815208**.

**Section biomes retained.** Biome containers are kept per section as a 4×4×4
grid indexed `((y<<2)|z)<<2|x`. The paletted-container strategy byte follows
26.2 exactly: **0** single-value, **1..3** indirect, **>3** direct at the
registry's `ceilLog2` width (66 → **7 bits**; a focused test round-trips id 65).
Both the initial level-chunk biome payload and the `ClientboundChunksBiomesPacket`
replacement are supported; a biome change or chunk load dirties the 3×3 column
neighborhood for remesh.

**Dynamic block tint.** `ClientLevel.calculateBlockTint` is transcribed as the
exact **radius-2, 5×5 same-Y integer channel mean** over the fiddled
`BiomeManager.getBiome` lookups, for the grass, foliage, dry-foliage and water
resolvers. Grass carries the `dark_forest` bit formula and the `swamp`
noise/threshold exactly; spruce and birch leaves are the **fixed** `FoliageColor`
constants (not the colormap); doubleTallGrass UPPER samples `pos.below()`. Tinted
faces select the **raw** (un-tinted) atlas layer and multiply the resolved biome
color into `MeshVertex.color` alongside face shade and AO — **no vertex-ABI
growth**. A synthetic / no-biome-context world deliberately keeps the legacy
pre-tinted layers with a white color, so the demo path is byte-identical.

**Per-job tint cache.** The decompiled `ClientLevel` wraps `calculateBlockTint`
in a `BlockTintCache` because each result averages a 5×5 window of `getBiome`
lookups, each running 8 fiddled corner-distance evaluations; the mesher asked for
a block's tint once per tinted face / model quad / fluid face (up to six times
for one leaf cube). M14 adds a `TintCache` local to a single `mesh_column` job,
keyed by the **canonical sampled position + resolver** — `GrassBelow`
canonicalizes to Grass at `y-1`, and `Constant` tints bypass it. It mirrors
vanilla's performance rationale without a global cache, lock, or invalidation
dance: the cache is dropped with the job, so a concurrent `chunks_biomes` / reload
can never observe a stale entry. The benefit is structural (fewer resolutions),
not separately benchmarked.

**Camera sky/fog is a deliberately separate path.** It is NOT the block fiddle.
`EnvironmentAttributeProbe` runs a raw-quart **6³ Gaussian** with kernel
`[0,1,4,6,4,1,0]` in z/x/y loop order, groups samples by biome identity in
insertion order (`Reference2DoubleArrayMap`), and blends with integer
`ARGB.srgbLerp`: the dimension base first, then a biome's positional override.
The GPU receives the resolved sky/fog **base colors as per-frame uniforms**, so a
camera move or a time-of-day change never forces a remesh. Rewo's existing
gradient sky and day/night timeline (M11/M12) still drive the actual sky render —
M14 only feeds it the biome-correct base; this is **not** a claim that the whole
sky renderer is a formula-exact vanilla renderer.

**Permanent oracle: `rewo tintshot --check`.** Serverless, Vulkan validation
**required and ON** (fails closed otherwise). It bakes the real 26.2 jar assets,
installs synthetic single/indirect/direct biome containers through the production
`chunks_biomes` path, runs the production `mesh_column`, and reads back the
production Vulkan framebuffer. Expectations are independent of the graded methods:
override constants, an inline colormap index, oracle vectors derived from the
decompiled algorithms and **verified under Temurin 25 then pinned** (no committed
JVM file), and a separate Gaussian reference shared by sky and fog. Pinned
values: boundary grass **[91,163,163]** (the exact 5×5 fiddle mean at an A/B
edge), dark_forest **[147,26,5]**, swamp light **[106,112,57]**, swamp dark
**[76,118,60]**, spruce **[97,153,97]**, birch **[128,167,85]**; raw-vs-legacy
atlas layers **499/500**; camera sky boundary **0xff7f807f** and camera fog
boundary **0xffac2d6d** (biome A inherits the dimension fog, B overrides); GPU
grass **[147,0,0]**, water **[0,0,139]**, biome sky **[254,128,0]** vs default
**[183,209,242]**, and a fully-fogged untinted terrain plane reading green
**[0,255,0]** then blue **[0,0,255]** under a deliberately distinct **red** sky;
**0 VUIDs**. A green run rejects constant-plains output, an axis transpose,
nearest-neighbor / wrong-fiddle / wrong-radius / wrong-mean, wrong colormap
indexing, a missing family override, spruce/birch treated as foliage, a wrong
grass modifier, a raw/legacy layer mixup, camera sampling via the block fiddle, a
missing fog inheritance/override/Gaussian, and dropped GPU mesh/sky/fog plumbing.

**Known scoped deviations / exclusions.** Biome blend radius is fixed at vanilla's
default **2** (not yet a setting). Modifier-form custom-datapack sky/fog
attributes are not applied — vanilla 26.2 ships these as bare overrides, which is
what M14 handles. The `EnvironmentAttributeProbe` last/new per-tick history is
omitted; the spatial result is sampled per frame instead. Respawn and dimension
transitions, and general Nether/End base selection, remain untested/unwired.
Redstone, stem and lily-pad `BlockColors` were explicitly out of M14. Greedy
meshing and packed vertices remain excluded.

**Final gates (independently run after code freeze).** Six release libraries
**215/215** — world 81, net 67, gpu 37, data 9, mesh 10, proto 11 — plus app
**10/10**; pre-existing warnings only. `tintshot` validation ON, exit 0, **0
VUIDs**; `lightmapshot` and `skyshot` each validation ON, exit 0, 0 VUIDs;
`mobshot` **243/243**. The demo PNG is byte-identical at SHA-256
**`2CC56B4ACBFB92CB91398C27E5C4735885ABFF9331F66B7DC83BDBC002246635`**. Canonical
physics: **600 ticks, CORRECTIONS 0**, clock +598, with walk/sprint/jump/look/dig
/place/chat/give all true — **PLACE and DIG both verified** this run. Canonical
lighting `--no-relight`: **9 columns, 884,736 cells, block 0 sky 0 EXACT**,
CORRECTIONS 0, clock +278. A short final info join decoded **66 biomes, 4
dimension types**, attached the context, **CORRECTIONS 0**, no chunk-decode
errors. Four replay samples: GPU avg **0.238–0.241 ms**, p50 0.223–0.224, 1% low
0.756–0.810, 0.1% low 0.868–0.970; serialized wall avg 0.544–0.549, 1% low
1.532–1.927, 0.1% low 3.345–3.837. The replay world has **no biome context**, so
it guards neutral rendering and does **not** measure the new tint cache's
live-meshing benefit. The test server was stopped and port 25599 verified free;
temp logs removed.

**Honest failure / correction history.** The first `tintshot` reached green only
after eight harness failures and a Vulkan teardown validation leak were fixed —
the ratio helper inspected the wrong channel, sampling windows hit the
unloaded-biome-0 fallback, blocks were set before `ensure_column`, and an early
return skipped GPU cleanup. A senior review then **rejected that first green
oracle as insufficient**: the boundary check only asserted "both colors present";
it claimed birch and both grass modifiers but exercised only spruce and no
modifier; and the GPU pass proved sky but never a fogged terrain plane. The
strengthened oracle pins the exact JVM-verified vectors, exercises real birch and
both dark_forest and swamp branches, adds an independent fog Gaussian, and reads
back a real fully-fogged terrain plane under a distinct sky. The dimension-holder
inline/`id+1` bug (above) was another review find, corrected in code, replay and
tests. Review also flagged the repeated 25×8 per-face tint resolution; the per-job
cache was added before the final gates. This history is recorded, not concealed.

Two operational notes: repository-wide `cargo fmt --check` remains **red** on
pre-existing, untouched formatting drift (about 33 files) — new files and hunks
are formatted and `git -c core.whitespace=cr-at-eol diff --check` is green, so
global fmt is **not** claimed green. And the first final server-stop verification
briefly showed port 25599 occupied by an unrelated concurrent `pvpsoft` vanilla
fixture that appeared after the Rewo diagnostic; its command line was verified, it
was not killed by this task, its owner cleaned it up, and the final port check was
free — this never overlapped the canonical Rewo physics/light runs.

**Left open after M14:** greedy meshing and packed vertices; general Nether/End
and dimension-transition base selection plus dimension ambient; entity-event
animations; true shape-union face occlusion; glow-lichen any-face emission; and
clouds/weather and HUD completeness. Per-biome tint is now shipped and off the
open list.

### 2026-07-24 — M15: exact packed vertex ABI + conservative greedy cube meshing

M15 closes the two long-deferred geometry-performance items without weakening
visual or semantic verification. The pre-M15 replay contained **3,723,192
vertices / 5,584,788 indices**, uploaded **149.13 MiB**, and used **93.080%** of
the shared arena. Its 36-byte vertex redundantly stored three float color lanes
whose values were exactly face shade × AO × an 8-bit tint.

**Packing, including the rejected version.** A 24-byte candidate using f16 UVs
was implemented and rejected: the canonical demo changed **6 pixels**, maximum
channel delta **25**, because fluid UV `1/9` crossed a nearest-sampling boundary.
The shipped ABI is therefore an exact **28 bytes**: position `f32×3` at byte 0,
UV `f32×2` at 12, light at 20, tint at 24. The light word keeps its existing
low 24 bits (`layer16 | block4 | sky4`) and uses bits 24–26 for the six face
shade codes and 27–28 for the four AO codes. Tint stores the three original
u8 channels losslessly. The vertex shader reconstructs
`FACE_SHADE[shade] * AO_LEVELS[ao] * tint_rgb / 255.0` with `precise` values.
`rewo-gpu/build.rs` parses the final optimized SPIR-V and refuses a build unless
the exact float-255 constant, `OpFDiv`, and `NoContraction` decorations survive;
this prevents a compiler rewrite to a reciprocal from silently moving a color
across an sRGB byte boundary. `lightmapshot` adds a non-identity packed-color GPU
probe whose expectation uses the legacy formula independently of the production
reconstructor. Shade codes 6–7 fail closed. Packing alone kept all counts and
the canonical demo exact while reducing upload to **120.72 MiB (−19.1%)**.

**Greedy policy.** The frozen `mesh_column_reference` remains the unit-face
control used only by tests/oracles. Production greedily merges only full
`RenderKind::Cube` faces and only when the merge key is identical: owning block
state id, the exact packed light word, and exact tint word. All four AO corners
must be equal; a non-uniform face falls back to the original unit quad. Models
and fluids continue through their byte-identical legacy emitters. Rectangles are
deterministic, never cross a column boundary, may cross section boundaries, and
scale the original face UV basis with their width/height while preserving
winding. The mesher exports measured census fields (`visible_cube_faces`,
`greedy_candidate_faces`, `greedy_quads`, `unit_fallback_faces`) with checked
invariants.

Only **down/north/south/west/east** merge. Top (`+Y`) faces are a permanent
1×1 exclusion, not an unfinished optimization: enabling all six directions
reduced the demo from 2,584 to 2,312 vertices but changed **11 pixels**, maximum
channel delta **35**. Direction isolation showed horizontal-only/up-only owned
the red result; down-only and the other five directions were byte-identical.
Forcing world UV constant reduced the difference to one pixel, proving UV
interpolation plus nearest sampling owned ten pixels while one remained a
topology/coverage difference. The shipped five-direction policy preserves the
canonical PNG exactly. Do not re-enable top merging without a new property-level
texture-sampling solution and a stronger proof.

**Permanent oracle: `rewo meshshot --check`.** This is serverless and CPU-only.
It meshes an adversarial cube fixture through both production and the frozen
reference, decodes each optimized rectangle, expands it to sorted unit faces,
and compares direction, owning block, atlas layer plus block/sky light, all four
AO codes, and tint word. The fixture measures **854 reference faces/quads → 265
optimized quads (−69.0%)**: 854 visible = 632 candidates + 222 fallbacks, 43
greedy rectangles, four section-crossing rectangles, maximum safe-face area 60,
and 211 top faces all area 1. It pins exact material, block-light, sky-light and
tint seams, a non-uniform-AO fallback inside a plane that still merges elsewhere,
four-run determinism, and byte-identical non-cube controls: model 256v/384i,
water 256v/384i translucent, lava 192v/288i opaque. Oracle failure exits nonzero
even without `--check`; `MESHSHOT: OK` prints only after every property passes.

**Measured replay result.** The production capture now contains **3,373,772
vertices / 5,060,658 indices**, uploaded **109.39 MiB**, largest column 400.3
KiB, arena use **84.344%**. Relative to packed-only this is −349,420 vertices
(**−9.38%**) and −11.33 MiB (**−9.39%**); relative to M14 it is −39.74 MiB
(**−26.65%**). The capture census is 172,456 visible cube faces = 87,992
candidates + 84,464 fallbacks, with 637 rectangles (**99.3% candidate-quad
reduction**). Four serial diagnostic mesh runs were noisy (144.6/149.9/159.9/
160.3 ms; median 154.9 ms); these are per-column wall costs, not production
rayon throughput. Final GPU replay was avg **0.232 ms**, p50 0.203, p99 0.892,
p99.9 1.521, max 1.977, 1% low 1.208, 0.1% low 1.848. Tail samples were
system-noisy/worse than some M14 samples, so M15 claims the deterministic byte
and geometry reductions, **not** a latency-tail improvement.

**Final gates.** Release libraries **237/237** (world 81, net 67, gpu 37, data
9, mesh 32, proto 11) plus app **19/19**. `meshshot`, `tintshot`, `lightmapshot`
and `skyshot` green; Vulkan gates validation ON. `mobshot` **243/243**. Two demo
renders both SHA-256
`2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635`.
Physics: 600 ticks, **CORRECTIONS 0**, PLACE and DIG verified. Independent
`--no-relight` lighting: **884,736 cells, block 0, sky 0 EXACT**. The dedicated
test server was stopped and port 25599 verified free. The work touched only
`rewo-*`; generated sources were not hand-edited.

**Left open after M15:** general Nether/End and dimension-transition/respawn
base selection plus dimension ambient; entity-event animations; true shape-union
face occlusion; glow-lichen any-face emission; redstone/stem/lily-pad
`BlockColors`; clouds/weather and HUD completeness. Packed vertices and greedy
cube meshing are now shipped and off the open list.

### 2026-07-24 — M16: dimensions (Nether/End/caves) and the whole transition — SHIPPED + VERIFIED

M16 makes the `minecraft:dimension_type` registry a *consumed* contract rather
than a parsed-and-ignored one. Before it, every Nether chunk mis-decoded: the
client used the Overworld's −64..384 shape for a 0..256 dimension, so section
indexing was off by four sections and the stale full-bright sky nibbles reached
the GPU. The work is complete, every gate below is green, and it is committed
locally on `codex/rewo-m16-dimensions` (not pushed). The vanilla test server was
stopped after the final gates and port 25599 was verified free.

**What ships.** `rewo-net/src/dimension_parse.rs` is the single parser for the
synced registry, in **raw wire order** — the vector index *is* the holder id a
login/respawn packet names, and nothing selects by name or value. It reads
`min_y`/`height`/`has_skylight`/`ambient_light` as required fields, `skybox` /
`cardinal_light` / `has_fixed_time` as optional fields with exactly the codec's
default, and the `EnvironmentAttributeMap` overrides for sky/fog/ambient/
sky-light colour and sky-light factor. Absence is preserved where it matters:
the Nether sets no `sky_color`/`fog_color` at all, and collapsing that into the
attribute's literal `0` would tint the whole biome colour stack opaque black.
A malformed entry is a connection error, never a substituted Overworld.
`has_day_timeline` is resolved from the `timelines` holder set and is
**independent of `has_fixed_time`** — they are separate `DimensionType`
members, and a codec-valid entry can have both. Downstream: per-dimension
`World` shape/sky channel/cardinal table, the Nether face-shade codes, the End
sky pass (`rewo-gpu/src/end_sky.rs`), spawn info, and a world/mesh transition
that discards the old world and refences the mesh pool by generation.

**The serverless oracle, `rewo dimensioncheck --check`.** Three inputs that can
disagree, plus a fourth that grades them:
1. a **captured** vanilla Configuration `registry_data` packet, pulled out of a
   recording *by content* and parsed by the production parser;
2. the **bundled** built-in transcription, encoded to wire bytes and pushed
   through the same entry point;
3. the **real decompiled datagen JSON** —
   `…/26.2/decompiled/data/minecraft/dimension_type/{overworld,overworld_caves,
   the_end,the_nether}.json` — read by `rewo-app/src/dimension_json.rs`, a
   `serde_json` reader that shares no code with the NBT parser, extracts every
   client-consumed raw field itself, applies a default **only** where the codec
   proves one (and reports which fields were defaulted), and resolves the day
   timeline by expanding `data/minecraft/tags/timeline/*.json`;
4. the hand-written `EXPECT` table, which grades all three and is itself graded
   by the JSON — so a stale table cannot certify a capture, and a JSON reader
   and a parser that mis-read the *same* field are still caught by a value a
   human wrote down.

The day-cycle mapping is **proved from the shipped tag files**, not asserted:
`#minecraft:in_overworld` expands to `{day, early_game, moon,
villager_schedule}` and `#minecraft:in_{nether,end}` to `{villager_schedule}`
(both via `#minecraft:universal`), so only the two Overworld entries carry
`minecraft:day`. A holder-set entry with no file under `timeline/` or
`tags/timeline/` is an error. The oracle then drives the world binding, the
mesh binding and the mesh pool's generation fence, and fails closed on a
missing recording *or* a missing/malformed decompile (`--decompiled <dir>`
overrides the version-derived path; a bogus one exits 1 naming the directory).

**A senior-review finding fixed before this entry was written.** The test then
named `the_bundled_transcription_matches_the_decompiled_json` compared the
transcription only against the handwritten `EXPECT` table — the decompiled JSON
was never read, so the claim was not executable. It is now: `dimension_json.rs`
reads the files, and the runtime gate and five renamed/added tests
(`the_bundled_transcription_matches_the_decompiled_json_files`,
`the_expectation_table_matches_the_decompiled_json_files`,
`the_json_oracle_rejects_a_drifted_transcription`,
`the_day_timeline_is_resolved_from_the_decompiled_tag_files`,
`a_missing_decompile_is_an_error`) all exercise them. The oracle's own
anti-vacuity test drifts one field at a time and asserts the diagnostic names
the field and the file.

**Measured results.**
- Unit tests **344** (proto 11, world 93, data 9, net 102, mesh 38, gpu 44,
  app 47). The app count was **37** before this fix added ten tests; the other
  crates are unchanged.
- `rewo dimensioncheck --check`: 4 captured entries exact against all three
  other inputs, world binding 16 shape/section-index probes + 16 light probes,
  mesh binding 96 vertices, Nether shade codes `[2,3,4,5,6]` and max sky nibble
  0, generation fence outputs `[0,1]`.
- Live dimension gate: **4/4 checkpoints, 3/3 transitions**, each discarding
  and requeueing **329 columns** (cumulative queue 987), new worlds 0 columns,
  chunk decode failures **0**, settled corrections **0**, teleports 4; the
  Nether's loaded and sparse sky maxima both **0**.
- `mobshot` **243/243**; `lightmapshot`, `skyshot`, `tintshot`, `meshshot`
  green, Vulkan validation **ON / 0 VUIDs**.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15**.
- Replay GPU avg **0.240 ms**, p50 0.207, p99 1.107, p99.9 2.357, max 2.622,
  1% low 1.683, 0.1% low 2.533. The tail was **system-noisy**; M16 claims no
  latency improvement, only that the dimension work did not change the
  rendered bytes.
- Physics: 600 ticks, **CORRECTIONS 0**, clock +598. (The optional place
  verification printed still air while dig verified — reported honestly; the
  named physics property is corrections.)
- Independent `--no-relight` light parity: **884,736 cells, block 0, sky 0
  mismatches**.
- Release build green.

**Left open after M16:** custom datapack dimension types are parsed but
unexercised; the
`{modifier, argument}` attribute arm is a deliberate hard error rather than a
modelled case; entity-event animations; true shape-union face occlusion;
glow-lichen any-face emission; redstone/stem/lily-pad `BlockColors`; physics
parity outside the on-foot flat-world subset; clouds/weather and HUD
completeness.

### 2026-07-24 — M16.1: the play gate's build actions now fail closed — SHIPPED + VERIFIED

M16 left one honest red result deferred (see the M16 entry's physics line): the
`rewo play` build-enabled gate printed "PLACE verify … still air ✗" on roughly
one run in four yet still exited 0, and its `place:true`/`give:true` booleans
meant only "a packet was sent". M16.1 fixes both the intermittent placement and
the fail-open gate. No protocol/network bytes changed — the packets were already
byte-correct against the decompile — so this is a **pre-M16 harness + gate
defect** (the place action and the `state != 0` proxy both date to M3), not a
protocol bug.

**Root cause (decompile evidence).** The harness placed dirt against the top of
the grass block *one* to the east and landed it at `(fx+1, fy)` — the cell
directly beside the bot's feet, at feet level. 26.2 `BlockItem.canPlace`
(`net/minecraft/world/item/BlockItem.java`) gates every placement on
`context.getLevel().isUnobstructed(state, clickedPos, CollisionContext.placementContext(player))`.
The player's 0.6-wide AABB, centred on a sub-block x anywhere in `[fx, fx+1)`,
reaches east to at most `fx+1.3`, so whenever the bot's fractional x was ≥ 0.7
its own body occupied the target cell and the server rejected the placement —
intermittently, because the resting sub-block x after the scripted
walk/sprint/jump varies run to run. `ServerPlayerGameMode.useItemOn` returns
`PASS` for the obstructed (or empty-hand) case and nothing is placed. Dig never
hits this: `handleBlockBreakAction` has no obstruction check.

The client's *observation* was never wrong. 26.2
`ServerGamePacketListenerImpl.handleUseItemOn` (lines ~1397–1398) sends the
acting player a `ClientboundBlockUpdatePacket` for **both** `pos` and
`pos.relative(direction)` on every use-item-on, accepted or not — so Rewo's
world already held the server-authoritative truth (dirt on success, air on
rejection). The bug was that the gate graded "any non-air = success" and never
touched the exit code.

**The fix.**
- **Placement geometry** (`rewo-app/src/play_cmd.rs`, `drive`): place *two* east,
  landing dirt at `(fx+2, fy)`. Column `fx+2` starts at `fx+2 > fx+1.3`, so the
  footprint can never touch it; the placement is now always geometrically valid
  and a resulting air state is a real bug. This is a correctness fix to *where we
  place*, not a moved verification target.
- **Fail closed** (`build_acceptance` + the pure `evaluate_build_actions`): after
  the session, a build-enabled, non-`--dimension-check` run reads the server's
  own world at the recorded targets and proves the **exact** property — the
  placed cell is `minecraft:dirt`'s default state (resolved from the block
  table, not "non-air"), the dug cell is air — printing `ACCEPT …` lines with
  expected-vs-actual state/identity and returning `Err` (process exit 1) if
  either is unproven or never ran. `--no-build` (the lighting gate) and
  `--dimension-check` never attempt these actions and are exempt. The actions
  line is relabelled "packets attempted — NOT proof" to separate send from
  server-observed success.
- **Acceptance tests** (`rewo-app` `play_cmd::tests`, +6): the regression guard
  is `placement_reverting_to_air_turns_the_gate_red` — flip the proven dirt
  state to air and the gate must go red; a wrong-but-non-air block, an un-broken
  dig, a never-run action and an unresolvable dirt state are all red too.

**Measured results.**
- Unit tests **350** (proto 11, world 93, data 9, net 102, mesh 38, gpu 44,
  app **53** — was 47; the six new tests are the acceptance logic; no other
  crate changed).
- Four live 30 s `--username RewoOp` sessions, all **CORRECTIONS 0**, exit 0,
  `ACCEPT place … state 10 (minecraft:dirt) ✓` and `ACCEPT dig … state 0 (air)
  ✓` on every one — the previous ~1-in-4 air result did not recur.
- Fail-closed proven live: a 16 s build run (place fires at 15 s, dig would fire
  at 18 s) prints `ACCEPT place … ✓` then `ACCEPT ✗ dig: action never ran` and
  **exits 1**.
- Canonical light gate `--seconds 14 --no-build --still --light-check
  --no-relight`: **884,736 cells, block 0, sky 0 mismatches, ✓ EXACT**, exit 0,
  no ACCEPT lines (placement correctly not required).
- Release build green. Test server stopped, port 25599 verified free.

### 2026-07-25 — M17: exact model-visible entity events — SHIPPED + VERIFIED

M17 makes `ClientboundEntityEventPacket` a *consumed* contract for the three
one-shot animations a vanilla client renders from it. Before it, every such
packet fell off the dispatch chain as an unknown id: the Warden's attack and
sonic-boom rigs never fired, and the Armadillo, once balled, never re-peeked —
it stayed in its held pose forever. The work is complete, every gate below is
green, and it is committed locally as `55388c8` on `codex/rewo-m17-entity-events`
(base `f4b54d1`, the M16.1 commit; not pushed — M0–M9 are on `origin/main`, the
M10–M17 arc is reviewed local work). The vanilla test server was stopped by
exact PID and port 25599 verified free after the final gates; the worktree was
clean after the commit.

**The packet.** 26.2 `ClientboundEntityEventPacket` is a signed fixed
big-endian `i32` entity id followed by a signed `byte` event id — *not* a
var-int either field. The packet report resolves the clientbound-play
`entity_event` id to **34**; as everywhere in Rewo it is looked up **by name**
from `packets.json`, so a version bump that renumbers it fails loud rather than
mis-firing. The two entity kinds whose events this client models resolve their
protocol type ids through the exact production `EntityTypes::id_of` path
(`registries.json`): **Warden 143, Armadillo 4**. No synthetic id is ever used
in a positive check.

**What ships — exact, model-visible events only.** `apply_entity_event`
(`rewo-net/src/lib.rs`) decodes the fixed body, looks up the entity's type, and
maps `(event, kind)` to a modelled effect; `route_entity_event` is the narrow
packet-id → decoder seam `PlaySession::handle_packet` calls (and the oracle
drives, so id selection is exercised in the gate too). Three mappings:
- **Warden event 4 (attack).** Durably stops the metadata-driven roar
  `AnimationState` for that same `ROARING` episode (so a mid-roar attack does
  not double-play), then unconditionally restarts the exact `WARDEN_ATTACK`
  keyframe rig from age 0.
- **Warden event 62 (sonic boom).** Unconditionally restarts the exact
  `WARDEN_SONIC_BOOM` rig from age 0.
- **Armadillo event 64 (peek).** Re-clocks the *existing shared* metadata
  `SCARED`/`PEEK` `AnimationState` from age 0; after it runs out, the final
  balled/held pose remains (the event is a re-peek nudge, not a separate rig).

A repeated packet restarts the clock. A missing entity, a wrong-kind pairing
(the id is right but the entity isn't a Warden/Armadillo), an unknown id, and
the deliberately-excluded events are all inert. Event state clears on entity
removal and on id reuse, so a recycled entity id cannot inherit a stale rig.
The renderer feeds a **production event-age input distinct from the metadata
gesture ages**; event ages share the session's tick/partial epoch, and the
CEM/vanilla part transforms continue through the same part pipeline — an entity
event is one more clock into the existing rig evaluator, not a parallel path.

**The generated rigs.** `WARDEN_ATTACK` and `WARDEN_SONIC_BOOM` are the exact
decompiled definitions from `WardenAnimation.java`, machine-extracted by
`tools/gen_anim_defs.ps1` into `rewo-gpu/src/anim_defs.rs` — never hand-edited.
Two mechanical consequences worth recording:
- The Warden ribcage was previously baked as static folded cubes; for
  `WARDEN_SONIC_BOOM` to swing them, they are promoted to **named body
  children** (`right_ribcage` / `left_ribcage`) — the neutral geometry is
  unchanged, so the mob's rest pose and the facelabel gate are untouched; only
  the parts the rig addresses now exist to be addressed.
- The generator's output is now written with **deterministic LF** line endings,
  so `git diff --check` is clean and a re-run reproduces the committed file
  byte-for-byte. A semantic diff that ignores EOL is exactly the two new
  definitions (**222 lines**) — nothing else in the generated table moved.

**The serverless oracle, `rewo eventshot --check`** (`rewo-app/src/eventshot_cmd.rs`).
CPU-only, no socket, no GPU device, **fail-closed** on a fixed `EXPECTED_WITNESSES
= 28`: every named property is *observed* (real value read and printed) and only
increments the counter when it passes; the run errors if any property failed
**or** the observed count differs from 28 (which catches a property silently
skipped by a missing part or a `None` from the oracle). The continuous path is
all production code:

```text
raw fixed-body packet (BE-i32 id + signed byte, built here)
  -> rewo_net::route_entity_event        (real packet-id selection seam)
  -> EntityTable::start_event            (real kind lookup + receipt-tick storage)
  -> live_cmd::resolve_mob_anim          (vanilla ownership rules, event ages)
  -> rewo_gpu::entities::oracle_part_deltas   (the exact GPU model pose math)
```

It loads the real `packets.json` and `registries.json` and proves `entity_event`
id **34** and type ids **Warden 143 / Armadillo 4** before any pose check. The
expected values are **independent decompiled literals** — nothing reads
`anim_defs` as its expectation (that is the table the renderer consumes; grading
it against itself would verify nothing), and the catmull-rom target is recomputed
by a private reimplementation of `Mth.catmullrom` over the four decompiled frame
literals. Tolerances are ~1e-4 (they absorb only the decimal-keyframe-time
reconstruction through the real `ageInTicks = (tick − start + partial)·0.05`
clock). The 28 witnesses include:
- **attack** `right_arm` at 0.1667 s → `[-π/2, -π/4, 0]`; `head` x at 0.25 s →
  `-0.52665188`; `body` position at 0.2083 s → `[0, 1, -2]`;
- **sonic boom** the two ribcages' y at 1.875 s → `±2.18166156`; `head` x at
  1.75 s → `1.3962634`;
- a **catmull-rom** sample at 1.375 s: observed/independent `1.497165` sitting
  far from the linear interpolant `1.287180` (difference `0.209985`), so a lerp
  regression fails the sensitivity discriminator;
- the **roar-ownership** case: prior-episode suppression vs a fresh transition,
  flipped by reordering ticks;
- **armadillo** held pose at age 2.5 s (`right_front_leg` → `-π/2`), the id-64
  re-clock to age 0 (`head` z `7.0` vs the held `5.2`), and the later return to
  the final hold;
- the **negative/sensitivity** partners: wrong packet id, missing/wrong entity,
  event 61 and unknown ids, a repeat restart, remove/reuse clearing, and the
  neutral (base-pose) ribcage parts.

Two consecutive release runs produced identical **PASS 28/28**.

**What M17 deliberately does *not* claim — corrections and exclusions.**
- **Allay `handleEntityEvent(18)` is heart particles only**, not a dance. The
  Allay dance is driven by `DATA_DANCING` — SynchedEntityData **index 16**,
  `BOOLEAN` serializer id 8 — with client-side dancing/spinning counters and
  the root/head formulas they feed. The generic `(16, BOOLEAN) → baby` decode
  Rewo already has is **latent and inert** for the Allay only because `is_baby`
  is not rendered for it; making the Allay dance is separate future
  *metadata-animation* work, not an entity-event claim and not M17.
- **Warden event 61 (tendril shiver) is excluded** — it needs tendril
  procedural/emissive modelling the Warden rig does not yet carry.
- **`ClientboundAnimatePacket` generic arm swings are excluded** as a future
  combat-animation milestone — they require handedness, equipment-driven swing
  duration/type/status, and the CEM closure to render correctly.
- **Hurt/damage-tilt overlays, particle/sound-only statuses, and any AI
  simulation are out of scope.**
- **No live AI-triggered event encounter was staged or claimed.** M17 is
  authoritative through exact raw-packet injection into the production
  dispatcher plus independent decompile literals: the client-receipt semantics
  these events define are not a function of vanilla's server-authoritative AI
  timing (which would be nondeterministic to script), so reproducing the AI is
  neither necessary nor a stronger proof than the packet the AI would send.

**Measured results.**
- Unit tests **360/360**: world **95**, net **110**, gpu 44, data 9, mesh 38,
  proto 11 = **307 library** + app **53**. Baseline was M16.1's 350; M17 adds
  **world +2, net +8** (the event lifecycle + decode/dispatch tests); every
  other crate is unchanged.
- Release build green — pre-existing warnings only.
- `rewo eventshot --check`: **28/28** on two consecutive release runs.
- `mobshot` **243/243** (the ribcage promotion did not disturb any texture).
- `lightmapshot`, `skyshot`, `tintshot` green with Vulkan validation **ON**;
  `meshshot` and `dimensioncheck` green; **no VUID reported**. `dimensioncheck`
  serverless: four registry entries exact against all bindings.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15** (and M16).
- Bench replay: GPU avg **0.231 ms**, p50 0.201, p99 0.787, p99.9 1.112,
  max 1.369; 1%/0.1% tail means 0.956 / 1.262 ms. **No latency improvement
  claimed** — M17 does not touch the replay's entity path.
- Live physics/build gate: 600 ticks, **CORRECTIONS 0**, clock +598, loaded 329
  columns; server-observed `ACCEPT place … state 10 (minecraft:dirt) ✓` and
  `ACCEPT dig … state 0 (air) ✓`.
- Live light `--no-relight`: 9 columns, **884,736 cells, block 0, sky 0
  mismatches, EXACT**, CORRECTIONS 0, clock +278.
- Live dimension `--dimension-check`: **4/4 checkpoints, 3/3 transitions**, each
  discarding and requeueing **329 columns** (cumulative queue 987), chunk decode
  failures 0, settled-window corrections 0, teleports 5.

**For the commit message (why / how found / evidence).** *Why:* model-visible
server events were silently dropped as unknown packets — the Warden's ribcages
could not animate and the Armadillo stayed held forever. *How found:* reading
the decompiled 26.2 class handlers plus the Warden/Armadillo animation
definitions, which also corrected the stale premise that Allay event 18 is a
dance (it is heart particles; the dance is metadata). *Evidence:* the exact
numbers above.

**Left open after M17:** the Allay dance and other metadata-animation work
(`DATA_DANCING` and friends); the Warden tendril (event 61); generic
`ClientboundAnimatePacket` arm swings; hurt/damage overlays and
particle/sound-only statuses; and the carry-forwards from M16 — custom datapack
dimension types, true shape-union face occlusion, glow-lichen any-face emission,
redstone/stem/lily-pad `BlockColors`, physics parity outside the on-foot
flat-world subset, and clouds/weather and HUD completeness.

### 2026-07-25 — M18: exact Allay dance (DATA_DANCING metadata animation) — SHIPPED + VERIFIED

M18 makes the Allay's `DATA_DANCING` a *consumed* metadata animation — the first
metadata-driven rig (distinct from M17's one-shot entity events). Before it, the
Allay's index-16 BOOLEAN was silently mis-decoded as `DATA_BABY_ID` (latent and
inert) and the dance never rendered. The work is complete, every gate below is
green, and it is committed locally as `bb8be20` on `codex/rewo-m18-allay-dance`
(base `6096bbd`, the M17 handoff commit; not pushed). The vanilla test server was
stopped by exact PID and port 25599 verified free after the final live gates.

**The metadata slot is polymorphic and only the kind resolves it.** `DATA_DANCING`
is `SynchedEntityData` **index 16, serializer BOOLEAN (id 8)** — pinned by
counting `defineId` up the hierarchy (`Entity` 0–7, `LivingEntity` 8–14, `Mob`
15, `PathfinderMob` adds none, `Allay` 16) and by the `EntityDataSerializers`
registration order (BYTE 0 … ITEM_STACK 7, **BOOLEAN 8**). `Allay` extends
`PathfinderMob`, **not** `AgeableMob`, so its slot-16 BOOLEAN is `DATA_DANCING`,
whereas `AgeableMob`/`Zombie` put `DATA_BABY_ID` at the same slot with the same
serializer — the byte parser genuinely cannot tell them apart. The resolved wire
facts (loaded from the real reports in the gate): `set_entity_data` packet id
**99**, Allay type id **2**, Zombie control type id **151**.

**The client counters are exact, and stateful (unlike M17's receipt ticks).**
`Allay.tick()` (client branch) runs every tick: `dancingAnimationTicks++` **then**
`isSpinning()` reads it (`dancingAnimationTicks % 55 < 15`); `spinningAnimationTicks0
= spinningAnimationTicks` snapshot; then `+1` if spinning else `−1`, clamped
`0..=15`; all three reset to 0 when not dancing. `getSpinningProgress(a) =
lerp(a, ticks0, ticks) / 15`. These live in `rewo-world` `EntityTable`
(`AllayDance`), advanced in `tick_lerp`; a false flag resets on the **next** tick,
a repeated true does **not** restart, and the clock clears on remove and on
re-add. `spinningAnimationTicks` ramps *bidirectionally*, so it is not derivable
from a single timestamp — the reason this is a stateful counter, not an event.

**The model formulas are exact, and the Allay got a real hierarchy.**
`AllayModel.setupAnim`'s `isDancing` branch: `danceSpeed = ageInTicks·8° +
walkAnimationSpeed`; `root.yRot = isSpinning ? 4π·spin : 0`; `root.zRot =
cos(danceSpeed)·16°·(1−spin)`; `head.yRot/zRot = cos·30°/14°·(1−spin)` (head.xRot
stays 0); dancing **suppresses** the ordinary head-look. Wings/hover apply
unconditionally, outside the branch. To make the whole-body spin propagate, the
Rewo Allay model was restructured from folded static cubes into the vanilla
`root → {head, body → {arms, wings}}` `PartDefinition` tree — provably
rest-geometry-neutral (mobshot 243/243 unchanged), `Anim::AllayRoot` /
`Anim::AllayHead` carrying the dance.

**The production chain, end to end:** raw report-resolved `set_entity_data`
→ `rewo_net::route_set_entity_data`/`apply_set_entity_data` (kind-aware routing
with **vanilla missing-entity inertness**) → `EntityTable` counter lifecycle
(`set_dancing` + `tick_lerp`) → `live_cmd::resolve_allay_dance` (the SAME app
resolver `collect_entities` uses) → the GPU model pose / CEM path. `play_cmd` and
`live_cmd` both resolve the Allay type id, so every `PlaySession` consumer
interprets the metadata, not only the live client.

**The serverless oracle, `rewo danceshot --check`** (`rewo-app/src/danceshot_cmd.rs`).
CPU-only, no socket, no GPU device, **fail-closed** on a fixed
`EXPECTED_WITNESSES = 24`. It drives the whole production chain above and grades
every observed value against an **independent** counter simulation (`sim`) and
independent transcriptions of the `AllayModel`/`AllayWing` formulas (`expect_dance`
/ `expect_wing`) — nothing reads the production `anim_delta` / `EntityTable` /
`AllayDance::tick` as its expectation. It loads the real `packets.json` /
`registries.json` and proves the ids/types before any pose check. The 24
witnesses: packet+type id resolution, **Allay dancing not baby**, Zombie
legitimate-baby control, wrong packet id inert, INT-is-size, false stops,
**missing-entity fully inert** (incl. an accompanying pose that would set on a
tracked entity), **wrong index inert**, **wrong serializer inert**, ticks-5
counters, the **exact spin boundary @14/@15/@55**, repeated-true no-restart
(0.533 vs a restart's 0.200), false→reset→true (1/15), **isSpinning distinct from
spinningProgress** (false while progress ≈0.533), partial-tick alpha,
remove/reuse clear, sway root+head, spin root.yRot, ordinary look, dance suppresses
look, dance-bit load-bearing, danceSpeed-includes-animationSpeed, and **wings
intact & dance-independent**. Two consecutive release runs produced byte-identical
**PASS 24/24**.

**Senior review corrected four things** (all folded into `bb8be20`):
- **Missing entity is now decompile-exact inert.** The first cut applied a
  *baby fallback* for an untracked id; 26.2 `ClientPacketListener.handleSetEntityData`
  does `Entity e = getEntity(id); if (e != null) assignValues(…)` — an untracked
  id mutates **no** state. `apply_set_entity_data` now returns before parsing when
  the entity is absent.
- **The shared app resolver.** `live_cmd::resolve_allay_dance` was extracted and
  the oracle made to drive it, so a regression in the live collector's kind-gate
  or counter→`AllayDance` mapping fails the gate too (it can't bypass the app path).
- **`play_cmd` resolves the Allay type id** (was live-only).
- **Explicit wrong-index and wrong-serializer witnesses** were added (22 → 24).

**Measured results.**
- Release unit tests **368 total**: world **98**, net **114**, gpu 44, data 9,
  mesh 38, proto 11 = **314 library** + app **54** (world +3 counter-lifecycle,
  net +4 routing/decode, app +1 resolver-kind-gate over the M17 baseline).
- Release build green (pre-existing warnings only). Plain `git diff --check` clean.
- `danceshot` **24/24** twice, identical; `eventshot` **28/28**; `mobshot`
  **243/243**; `lightmapshot`/`skyshot`/`tintshot` green with Vulkan validation
  **ON**; `meshshot`/`dimensioncheck` green.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15/M16/M17**.
- Bench replay GPU avg **0.220 ms**, p50 0.198, p99 0.909, p99.9 1.187, max
  1.343; 1%/0.1% tail means 1.043 / 1.297. Wall avg 0.692, p50 0.667, p99 1.530,
  p99.9 1.980, max 2.467; tail means 1.748 / 2.248. **No latency improvement
  claimed** — M18 does not touch the replay entity path.
- Live physics/build: 600 ticks, **CORRECTIONS 0**, clock +598, teleports 1, 329
  columns; server-observed `place state 10 (minecraft:dirt)` and `dig state 0
  (air)`.
- Live light `--no-relight`: 280 ticks, **CORRECTIONS 0**, clock +278, 9 columns,
  **884,736 cells, block 0 sky 0 mismatches, EXACT**, 329 columns.
- Live dimension `--dimension-check`: 435 ticks, **CORRECTIONS 0**, clock +433,
  **4/4 checkpoints, 3/3 transitions**, each discarding/requeueing **329 columns**
  (cumulative queue 987), decode failures 0, settled corrections 0, teleports 4.
- Vanilla test server (PID 16856) stopped; port 25599 free; `server.jar nogui`
  processes 0.

**Exclusions / honesty.**
- **No live jukebox/AI encounter** was staged or claimed. The deterministic proof
  is raw-packet injection into the production receipt path — the client-receipt
  semantics don't depend on the server-authoritative (nondeterministic) jukebox/AI
  that would trigger the dance.
- The Allay's pre-existing **unimplemented unconditional body flying-tilt / root
  idle-bob / arm idle-bob** remain outside this dance milestone (they are not the
  dance); the existing wing/hover behavior is preserved and witnessed.
- **No claim of exhaustive index-16 ownership** across every entity class — M18
  implements Allay dancing vs the pre-existing modeled baby path only.
- **Left open:** generic `ClientboundAnimatePacket` combat arm swings, the Warden
  tendril (event 61), and other metadata animations; plus the M16/M17
  carry-forwards.
- **Line-ending note (trap/convention).** `cargo fmt` rewrites the whole
  workspace (incl. generated `anim_defs.rs`) — never run it. Two files
  (`rewo-gpu/entities.rs`, `mobshot_cmd.rs`) are **mixed CRLF/LF in HEAD**; a
  bulk `sed -i` normalized a whole file and an `awk`-pipe patch broke context —
  both reverted. The clean method that keeps plain `git diff --check` green while
  preserving every unchanged byte: `git diff | tr -d '\r' > p; git checkout HEAD
  file; git apply --ignore-whitespace p` (LF-only insertions into the CRLF file).

### 2026-07-25 — M19: exact combat swings (ClientboundAnimate) + the ArmPose hold baseline — SHIPPED + VERIFIED

M19 makes `ClientboundAnimatePacket` a *consumed* animation. Before it the packet
fell off the dispatch chain as an unknown id, so no entity ever swung. Unlike
M17's one-shot events and M18's metadata counters, a swing is a small **state
machine** on `LivingEntity` whose length depends on the item in the swinging
hand, and whose pose is `HumanoidModel.setupAttackAnimation` layered on the
`ArmPose` hold baseline. The work is complete and every gate below is green.

**Wire facts** (resolved by name from the real reports, so a renumber fails
loud): clientbound-play `animate` id **2**, body = **VarInt entity id + unsigned
byte action** (not the fixed BE-i32 of `entity_event`); `set_equipment` **102**;
`set_entity_data` **99**. Action **0** swings the main hand, **3** the off hand;
**2** (player wake), **4**/**5** (crit / enchanted-hit particles) and every
unknown action must leave the swing untouched — M19 claims the model-visible
swing only, no particle or wake behaviour.

**The swing clock is exact.** `LivingEntity.swing` accepts iff *not swinging* OR
`swingTime >= duration/2` OR `swingTime < 0`; an accepted swing parks
`swingTime = -1`, sets `swinging`, and records `swingingArm`. So a first-half
repeat is ignored and a second-half repeat restarts. `updateSwingTime`
increments *first*, ends at `duration`, and `attackAnim = swingTime / duration`;
`getAttackAnim(partial)` wraps a negative difference by `+1` (a naive lerp falls
to 0 at the wrap and is witnessed against). Duration is the held hand's
`ItemStack.getSwingAnimation().duration()` — default `SwingAnimation(WHACK, 6)` —
adjusted exactly by DIG_SPEED and MINING_FATIGUE.

**Which entities tick a swing is machine-extracted, not a hand list.**
`tools/gen_entity_classes.py` parses the registered types out of
`EntityTypes.java` and walks the Java `extends` chains, producing
`crates/rewo-data/src/entity_classes.rs`: **93 living / 36 swing-ticking of 158
types**. Swing-ticking = `Player`/`RemotePlayer`, every concrete `Monster`
descendant, and `Mannequin`. A living non-`Monster` (a cow) *accepts* a swing and
never advances it; a non-living entity (a boat) is inert to swing, equipment and
effect input alike. This matters even where Rewo's built-in mob model ignores
`attackTime`, because OptiFine CEM's `swing_progress` reads it.

**Nothing unknowable is guessed.** An item id outside the registry, or a
component patch holding a codec this client cannot walk, marks that hand
`HandItem::Unknown`; the pose *and* CEM's `swing_progress` are then suppressed
rather than filled in from the item's prototype, and a later exact equipment
update lifts the suppression. The item-stack reader also walks **past** the swing
component to the end of the patch — returning early once the swing was found
left the reader mid-stack and silently desynced the following off-hand slot.
`tools/gen_swing_animations.py` derives the prototype table from the datagen item
components: **7 non-default swing animations over 1,537 registered items** (the
seven spears, STAB, durations 13–23); everything else inherits WHACK/6.

**The ArmPose hold baseline — caught in senior review, and the common case.**
The first cut implemented `setupAttackAnimation` exactly and omitted the stage
*before* it. `setupAnim` runs its `pose{Right,Left}Arm` dispatch at lines 248–268
and `setupAttackAnimation` at 273, so the strike is added **on top of** the hold
pose. `AvatarRenderer.getArmPose` ends:

```java
SwingAnimation attack = itemInHand.get(DataComponents.SWING_ANIMATION);
if (attack != null && attack.type() == STAB && avatar.swinging) return SPEAR;
else return itemInHand.is(ItemTags.SPEARS) ? SPEAR : ITEM;
```

so **`ITEM` is the fall-through for any ordinary held item** — a player holding a
sword. Its body is `arm.xRot = arm.xRot * 0.5F - (float)(Math.PI/10); arm.yRot =
0`, i.e. the walk swing halved and the arm dropped 18°. Omitting it posed every
armed entity from an unarmed baseline, which is the ordinary combat case, not an
exotic one. (The trap: `HumanoidModel.ArmPose` *has* `ITEM`/`BLOCK`/`BOW_AND_ARROW`
cases, but `HumanoidMobRenderer.getArmPose` returns only `SPEAR`/`EMPTY` — it is
the **Avatar** renderer that produces `ITEM`, and the player humanoid is the one
model Rewo poses. Reading the enum rather than the code path that produces it
would have dismissed this.)

Three poses are modelled, all exact: `EMPTY` (`yRot = 0`), `ITEM`, and `SPEAR`.
`SPEAR` is the unconditional half of `SpearAnimations.thirdPersonHandUse` —
`yRot = -0.1·invert + head.yRot`, `xRot = -π/2 + head.xRot + 0.8`, each clamped
through vanilla's own `180/π` degree round-trip to ±60° / −120°..30°. Its second
half is gated on `!(state.ticksUsingItem <= 0.0F)` plus a `KINETIC_WEAPON`
component, so for a non-using entity it is **exactly a no-op** — omitting it is
vanilla, not an approximation. It is also the only part that touches `zRot`,
which is why the idle bob still lands unmodified.

**Two load-bearing asymmetries in the dispatch.** `ArmPose` carries
`(twoHanded, affectsOffhandPose)`; `SPEAR` is `(false, **true**)`. In the
`isUsingItem == false` branch, a right-handed entity runs `poseLeftArm` **first**
and only then `poseRightArm` *if the left pose does not claim the offhand* — so a
spear in the off hand leaves the main arm **entirely unposed**, sword and all.
Handedness mirrors it. And because `thirdPersonAttackHand` subtracts exactly the
prologue yaw `setupAttackAnimation` added, whatever the hold pose set **survives
the strike** — a spear-swinging entity is SPEAR/SPEAR, the right arm is never
hold-posed, and its yaw returns to exactly 0 while the left keeps `+0.1`.

**`ItemTags.SPEARS` is read as a tag, from the client jar.**
`crates/rewo-data/src/item_tags.rs` reads `data/minecraft/tags/item/spears.json`
out of the production jar (the same artefact the asset bake opens), resolving 7
names to protocol ids. It deliberately does **not** reuse the swing table: "is in
`minecraft:spears`" and "its `swing_animation` is STAB" pick the same seven items
in 26.2 and are different questions — a sword the server patches to STAB is not
in the tag, and a spear patched to WHACK still is. Every unrecognised form (a
`#tag` reference, the object `{id, required}` entry, an unregistered name, an
empty list, a missing `values`) is a hard error rather than a silently smaller
set, and each is unit-tested.

**Production chain** (the gate drives all of it, with no socket and no GPU):

```text
raw animate body (VarInt id + unsigned byte)
  -> rewo_net::route_animate            (production packet-id selection)
  -> apply_animate                      (missing-entity drop + action mapping)
  -> EntityTable::swing / tick_lerp     (the exact LivingEntity swing clock)
  -> live_cmd::resolve_attack_anim      (the SAME resolver collect_entities uses)
  -> live_cmd::resolve_arm_poses        (likewise, for the hold baseline)
  -> rewo_gpu::entities::oracle_part_deltas   (the exact model pose math)
```

**Gate: `rewo swingshot --check`** — permanent, serverless, CPU-only,
fail-closed on a fixed **61/61** witnesses, byte-identical across runs (modulo
log timestamps). Expectations are **independent transcriptions**: its own `ease`
module, its own `Mth` sine table built with `std::sin` against the renderer's
`libm`, its own `poseRightArm`/`poseLeftArm` bodies, and its own transcription of
`affectsOffhandPose` / `isTwoHanded` / the pose dispatch — reading the production
accessors would have recreated the self-grading defect `dimensioncheck` once
shipped. The `Mth` witness is load-bearing rather than tolerance-absorbed: **0
bit mismatches over 60,003 sin/cos samples** while the same points differ from
the platform sine **39,917 times (max 9.577e-5)**, so a plain-trig port fails it
by construction. Every property carries a mutation partner — wrong packet id,
missing entity, malformed body, actions 2/4/5/unknown, the boat (living gate),
cow-vs-zombie (swing-ticking set), the zombie pose (the rig belongs to the
humanoid player model alone while its `swing_progress` still reaches CEM), and
for the hold pose the *unposed arm* itself as the baseline to differ from.

**Measured (all run independently, not taken from the implementer's report).**
- **404 tests**: world 108, net 132, gpu 46, data 14, mesh 38, proto 11 (**349
  lib**) + app 55. Release build green; the 3 warnings are pre-existing
  (`decode_png_rgba`, `hud.rs` mut, `cem_top`).
- `swingshot` **61/61** twice, identical; `eventshot` **28/28**; `danceshot`
  **24/24**; `mobshot` **243/243**;
  `lightmapshot`/`skyshot`/`tintshot`/`meshshot`/`dimensioncheck` green with
  Vulkan validation **ON**.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15/M16/M17/M18**. Neutral geometry is untouched:
  `ArmPoses::EMPTY` assigns `yRot = 0` where it is already 0.
- Live `--swing-check`: **1** tagged zombie tracked (tag-scoped `summon`/`kill`,
  so an unrelated mob is neither destroyed nor graded), main hand decoded
  `minecraft:iron_spear` **STAB/19**, off hand `minecraft:stone_sword`
  **WHACK/6**, `getCurrentSwingDuration=19` reading the main hand,
  **CORRECTIONS 0**, fixture cleaned up.
- Live physics 30 s: **CORRECTIONS 0**, `ACCEPT place (-44,-60,0) = minecraft:dirt`,
  `ACCEPT dig (-47,-61,0) = air`.
- Live light `--no-relight`: **884,736 cells, block 0 sky 0, EXACT**.
- `git diff --check` **clean** (see the line-ending note below).

**Exclusions / honesty.**
- **The eight use-driven arm poses are not modelled** — `BLOCK`, `BOW_AND_ARROW`,
  `THROW_TRIDENT`, `CROSSBOW_CHARGE`, `CROSSBOW_HOLD`, `SPYGLASS`, `TOOT_HORN`,
  `BRUSH`. `AvatarRenderer` only returns them while
  `getUsedItemHand() == hand && getUseItemRemainingTicks() > 0` (or for a charged
  crossbow held), and neither the use-item hand nor the remaining ticks is
  synchronised for a remote entity. `BOW_AND_ARROW` additionally writes to *both*
  arms, which the per-arm pipeline could not express without restructuring. These
  are suppressed by omission, not approximated.
- **Undead arm animation is not implemented.** `AnimationUtils.animateZombieArms`
  gives the zombie/skeleton/piglin families their own attack rig; Rewo poses only
  the humanoid **player** model from `attackTime` (witnessed by `h1`). A swinging
  zombie therefore shows no arm motion unless a CEM pack drives it. This is the
  largest remaining visible gap in combat animation and is the obvious next step.
- **Crouch, swim, fall-flying, passenger and the SPYGLASS bob guard** remain
  unmodelled, as before; each is a branch of `setupAnim` Rewo's state cannot
  enter. `state.speedValue` is 1.0 except while fall-flying, so the walk term's
  `/ speedValue` divisor is exact for every reachable state.
- **The walk term uses platform `cos`, not `Mth.cos`, in both production and the
  oracle.** They agree with each other, so the witness is honest about what it
  proves; the deviation from vanilla is bounded by one table step (≈9.6e-5 rad,
  0.0055°) and is invisible. The attack terms *are* `Mth`-exact and witnessed as
  such. Making the walk `Mth`-exact would be a one-line change plus a lockstep
  oracle update; it was left out of M19 rather than smuggled in.
- **DIG_SPEED / MINING_FATIGUE duration adjustment is reachable only for the
  local player and a ridden vehicle** — `ClientboundUpdateMobEffectPacket` is
  self/join/passenger-scoped, not broadcast to all trackers. The formula is
  implemented and witnessed; the reachability is stated rather than implied.
- **No live AI-driven combat encounter was staged or claimed.** The deterministic
  proof is raw-packet injection through the production dispatcher plus the
  tag-scoped live equipment check.
- **Line-ending note.** `crates/rewo-gpu/src/entities.rs` is **mixed CRLF/LF in
  HEAD** and `crates/rewo-data/src/lib.rs` is **uniformly CRLF**. Editing either
  normally normalizes line endings across the touched region — here that inflated
  `entities.rs` from a 341/28 diff to 529/216, all of it invisible churn. The
  documented method fixes it and keeps plain `git diff --check` green while
  preserving every unchanged byte: `git diff -- <file> | tr -d '\r' > p; git
  checkout HEAD -- <file>; git apply --ignore-whitespace p`. Verify with
  `git diff --numstat` against `git diff --ignore-all-space --numstat` — they
  should agree to within the genuinely-edited lines. Note also that
  `git diff --check` **cannot** be green for a uniformly-CRLF file whose added
  lines are CRLF, because git flags the `\r` unless `core.whitespace` includes
  `cr-at-eol` (it is unset); the LF-insertion is what makes it green.

### 2026-07-25 — M20: exact mob combat rigs (undead, skeleton, illager) — SHIPPED + VERIFIED

M19 gave the *player* an exact swing. M20 gives it to the mobs that actually
attack you. Four vanilla rigs, all of which run **after** `HumanoidModel.setupAnim`
and overwrite what it left: `AnimationUtils.animateZombieArms` (the undead
families), `SkeletonModel.setupAnim`'s own override, and `IllagerModel`'s
arm-pose switch with its two attack branches.

**The sizing discovery: the undead arms could not animate at all.** Rewo baked
the iconic arms-forward pose as a *static fold* on `STATIC_PART`:

```rust
let arms = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [-5.0, 2.0, 0.0]);   // −90°, frozen
```

Vanilla rests at `armDrop = −π/2.25` = **−80°**, deepens to `−π/1.5` = **−120°**
when aggressive, and swings from there. So the pose was ~10° wrong *and*
structurally incapable of moving. M20 promotes those arms — zombie/husk/drowned,
zombie villager, zombified piglin — to real animated parts and drives them from
the formula. The neutral pose therefore **moves** (−90° → −80°); `mobshot` stays
243/243 because the facelabel gate ray-casts the same geometry it renders, and
the canonical demo PNG is untouched (no entities in it).

**Ground truth.**

```java
animateZombieArms(leftArm, rightArm, aggressive, state) {
   if (state.swingAnimationType != STAB) {                     // STAB skips it
      boolean raiseArms = !state.isBaby || mainHand == EMPTY;  // only a BABY
      float armDrop = raiseArms ? -PI/(aggressive ? 1.5F : 2.25F) : 0.0F;
      animateAttackArms(leftArm, rightArm, state.attackTime, raiseArms, armDrop);
   }
   bobArms(rightArm, leftArm, state.ageInTicks);               // a SECOND bob
}
animateAttackArms(...) {                                       // ASSIGNS all three
   float aY = (negate ? 1 : -1) * Mth.sin(attackTime * PI);
   float aX = Mth.sin((1 - (1-attackTime)²) * PI);
   xRot = armDrop + aY*1.2F - aX*0.4F;   yRot = 0.1F - aY*0.6F;
   right.{x,y,z} = {xRot, negate ? -yRot : yRot, 0};
   left .{x,y,z} = {xRot, negate ?  yRot : -yRot, 0};
}
```

Three quirks are reproduced rather than tidied, and each has a witness. A **STAB
item skips the strike entirely**, so the humanoid pose survives — and then takes
a *second* bob, because `bobArms` sits outside the guard (witness `k9` observes
`zRot = +0.200000`, two bobs of 0.1). `animateAttackArms` **assigns rotations
only**, so `setupAttackAnimation`'s arm pivot movement survives underneath
(`k8`). And **only a baby holding an item** drops its arms — an adult always
raises them, whatever it holds (`k10`).

**The one new wire input is `Mob.DATA_MOB_FLAGS_ID` — index 15, BYTE.** Bit 1 is
no-AI, bit `2` is `isLeftHanded`, bit `4` is `isAggressive`. Index 15 is the slot
M19 already reads as the player's main arm, but the **serializer separates them**:
only `Avatar` uses HUMANOID_ARM (42) there, and M19 already had the test asserting
"BYTE at 15 is a flags byte, not the arm". The byte is additionally gated on the
type being a `Mob`, because `ArmorStand.DATA_CLIENT_FLAGS` is also a BYTE at 15
and means something else entirely (`k3`).

**It drags in a correctness fix for free.** `Mob.getMainArm()` *is*
`isLeftHanded()` — mob handedness was defaulting to Right, so M19's attack-arm
selection and `ArmPose` dispatch were wrong for every left-handed mob.
`set_mob_flags` writes handedness through to the same main-arm map, which is
also how vanilla stores it (there is no separate mob arm field).

**Three more polymorphic slots, all resolved by ancestry.**
`tools/gen_entity_classes.py` gained `ANCESTRY_SETS`, derived purely from the
`extends` chain and **failing loud if any set comes out empty**:

| set | ancestor | slot it unlocks |
|---|---|---|
| `MOB` (90) | `Mob` | 15 BYTE — the flags byte |
| `RAIDER` (6) | `Raider` | 16 BOOLEAN — `IS_CELEBRATING` |
| `SPELLCASTER_ILLAGER` (2) | `SpellcasterIllager` | 17 BYTE — `DATA_SPELL_CASTING_ID` |
| `ILLAGER` (4) | `AbstractIllager` | the `IllagerModel` pose switch |

Counting `defineId` up the hierarchy gives the indices: Entity 0–7, LivingEntity
8–14, Mob 15, `PathfinderMob`/`Monster`/`PatrollingMonster` add **none**, Raider
16, then `SpellcasterIllager` 17 and `Pillager` 17 in parallel branches.
`RAIDER` is 6, not 4 — **`ravager` and `witch` are Raiders but not Illagers**, so
their index-16 BOOLEAN is `IS_CELEBRATING`; before M20 it was silently read as
`DATA_BABY_ID` (`k4`). Resolution asserts the containment the `extends` chain
guarantees (illager ⊆ raider ⊆ mob, spellcaster ⊆ illager).

**The illager rig is a different shape.** `IllagerModel.setupAnim` calls
`super.setupAnim` and then **assigns its own walk over both arms**, wiping the
hold pose, the attack *and* the bob — so the humanoid stage is not run for
illagers at all; everything it would contribute is overwritten before it could be
seen. Then the pose switch. `getArmPose()` is derived per subclass, all from
synced state:

- **Pillager** — charging → `CROSSBOW_CHARGE`; holding a crossbow → `CROSSBOW_HOLD`;
  else aggressive ? `ATTACKING` : `NEUTRAL`
- **Vindicator** — aggressive → `ATTACKING`; else celebrating ? `CELEBRATING` : `CROSSED`
- **Evoker** (`SpellcasterIllager`) — casting → `SPELLCASTING`; else celebrating ? `CELEBRATING` : `CROSSED`
- **Illusioner** — casting → `SPELLCASTING`; else aggressive ? **`BOW_AND_ARROW`** : `CROSSED`

`ATTACKING` splits again: an **empty** main hand runs
`animateZombieArms(..., true, ...)` — the aggressive argument is a **literal
`true`**, never the mob's own flag (`k13`) — while an armed illager runs
`AnimationUtils.swingWeaponDown`, whose two arms take different terms selected by
the main arm (`k14`). `CROSSED` is a **visibility** switch, not a pose: vanilla
carries both arm sets on one model and does `arms.visible = crossedArms;
left/rightArm.visible = !crossedArms`. Rewo had two separate illager models; M20
merges them into one with `Show::IllagerCrossedOnly` / `Show::IllagerNotCrossed`.

**The skeleton rig** is gated on `isAggressive && !isHoldingBow` (the latter an
item identity test, `getMainHandItem().is(Items.BOW)`), so a bow-armed skeleton
keeps its aiming pose. Its assembly differs from the undead one despite sharing
the two sine terms: xRot is **pinned to −π/2 and the strike subtracted**, and the
yaw is not negated by an arm flag (`k11`, `k12`).

**Gate: `rewo swingshot --check` grew 61 → 77 witnesses**, still serverless,
CPU-only, fail-closed, byte-identical across runs. The M20 expectations are again
**independent transcriptions** — `want_attack_arms`, `want_zombie_arms`,
`want_skeleton_arms`, `want_swing_weapon_down`, each with the oracle's own `Mth`
table; nothing reads the production rigs as its expectation. Every metadata
witness drives a **real `set_entity_data` body through `route_set_entity_data`**
with the production `MetaKinds`, so the kind gating is proved through the shipping
router, not a parallel copy. `h1` was **inverted** and renamed: it asserted a
swinging zombie was inert (true in M19, because the arms were static folds); it
now asserts the undead rig animates *and differs from the player rig*.

**Measured.**
- **410 tests**: world 111, net 135, gpu 46, data 14, mesh 38, proto 11 (**355
  lib**) + app 55. Release build green, pre-existing warnings only.
- `swingshot` **77/77** twice, identical; `eventshot` 28/28; `danceshot` 24/24;
  `mobshot` **243/243**; `lightmapshot`/`skyshot`/`tintshot`/`meshshot`/
  `dimensioncheck` green with Vulkan validation **ON**.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15–M19**.
- Live `--swing-check` OK, **CORRECTIONS 0**; live light `--no-relight`
  **884,736 cells, block 0 sky 0, EXACT**; `git diff --check` clean.

**One live red, diagnosed and NOT an M20 regression.** One `rewo play` run in
four printed `ACCEPT ✗ place @ (-97,-60,-19): expected minecraft:dirt, observed
minecraft:grass_block`; three consecutive re-runs passed. Mechanism: the harness
clicks the top face of `(fx+2, fy-1)` so dirt lands at `(fx+2, fy)`, which
assumes the bot is standing on **undisturbed flat ground**. When an earlier run
of the same gate has dug a hole and the bot walks into it, its feet sit one block
low, `(fx+2, fy)` is then the grass **surface** rather than air, and the server
correctly rejects the placement. That is a pre-existing M16.1-era harness
assumption — M16.1 fixed the "target inside the player's own AABB" case but not
"the bot is standing in a hole the gate itself dug" — and it is world-state
dependent, not random. M20's entire diff to `play_cmd.rs` is one line resolving
the Pillager type id, nowhere near placement. **Follow-up:** have the gate assert
the target cell is air *before* clicking, or reset the world between runs.

**Exclusions / honesty.**
- **`CROSSBOW_HOLD` and `CROSSBOW_CHARGE` are derived but not posed.** Their
  bodies are `animateCrossbowHold` / `animateCrossbowCharge`, which need
  `ticksUsingItem` and `maxCrossbowChargeDuration` — not synchronised for a
  remote entity. The *pose derivation* is exact and witnessed (`k15`); the arms
  render as the plain illager walk rather than an approximation.
- **`SPELLCASTING` / `CELEBRATING` assign the arm pivot's `x` as well as `z`**
  (`rightArm.x = -5`, `leftArm.x = 5`). Those are already the rest columns, so
  only the `z` reset is modelled; if a future change moves the illager arm pivot
  off ±5 this becomes wrong and needs the `x` term.
- The eight **use-driven humanoid arm poses** remain unmodelled (M19's exclusion,
  unchanged), as do crouch/swim/fall-flying/passenger.
- **Held items are still not rendered.** Mobs now swing correctly but
  empty-handed; item-in-hand rendering is untouched by M20.
- **No live AI-driven encounter was staged or claimed.** Aggression, spellcasting
  and crossbow charging are server-AI-driven and nondeterministic; the properties
  are proved by driving real metadata bodies through the production router, the
  same standard M17/M18/M19 set.
- **Line endings**: `rewo-gpu/entities.rs` churned again (726/203 → 206/10) and
  was stripped with the documented `tr -d '\r'` + `git apply
  --ignore-whitespace` method, then four boundary lines had stray CRs removed
  individually. Verify with `git diff --numstat` against
  `git diff --ignore-all-space --numstat`.

### 2026-07-25 — M20.1 fix + M21: the combat damage response — SHIPPED + VERIFIED

Two things: the live-gate flake M20 recorded is fixed, and `ClientboundDamageEventPacket`
is now consumed — you hit a mob and it flashes red and its limbs kick, which
completes the M19 → M20 → M21 combat arc.

#### M20.1 — the build gate's premise is checked, not assumed

M20 recorded a red that reproduced ~1 run in 4: `ACCEPT ✗ place … observed
minecraft:grass_block`. The gate clicked the top face of `(fx+2, fy-1)` so dirt
would land at `(fx+2, fy)` — which assumes the bot stands on **undisturbed flat
ground**. An earlier run of the same gate digs a hole; if the bot walks into it
its feet sit a block low, `(fx+2, fy)` is then the grass *surface* rather than
air, and the server correctly rejects the placement. M16.1 fixed the "target
inside the player's own AABB" case; this is the sibling it did not cover.

The gate now **scans east from `fx+2` for the first column that is air over
solid**, using the client's own world, and reports the scan in the log line. If
no such column exists within eight blocks it leaves `placed_at` unset, which
`build_acceptance` reports as never-run — **exit 1**, not a silent skip.
Verified 5/5 green in the very world that produced the original failure.

#### M21 — the damage response

`ClientboundDamageEventPacket` was falling off the dispatch chain, so nothing an
entity took ever showed.

**The packet**, in wire order: `VarInt entityId`, the damage-type holder, `VarInt
sourceCauseId + 1`, `VarInt sourceDirectId + 1`, `Optional<Vec3>`. The holder is
`ByteBufCodecs.holderRegistry(Registries.DAMAGE_TYPE)` — a **raw 0-based
registry id**, not `ByteBufCodecs.holder`'s inline / `id+1` scheme. Nothing
model-visible depends on *which* damage type it was, but the whole body is still
walked: a short read would leave the stream misaligned for the next packet in the
same buffer, and a witness pins that (a truncated body is inert while the intact
one arms the clock).

**Two gates on receipt**, both vanilla's: `handleDamageEvent` drops the event for
an entity the client is not tracking (`if (entity != null)`), and
`handleDamageEvent` is a **`LivingEntity` override** — a non-living entity gets
`Entity.handleDamageEvent`, which has no hurt clock to arm.

**The clock**: `hurtDuration = 10; hurtTime = hurtDuration;` then
`if (this.hurtTime > 0) this.hurtTime--;` once per tick. `hasRedOverlay` is
`hurtTime > 0`, so the flash lasts exactly ten ticks; a second hit **re-arms from
10** rather than extending. `hurtDuration` is stored rather than folded to a
constant because vanilla divides by it elsewhere.

**The limb kick**: `walkAnimation.setSpeed(1.5F)` is the *first* line of
`handleDamageEvent`. Rewo's limb model was already exactly
`WalkAnimationState.update(target, 0.4, 1.0)`, so the kick is a direct
assignment — and 1.5 is **above** anything movement can produce, because the
walk target is `min(1, dist·4)`. Vanilla clamps on the render side
(`speed(partialTicks) = Math.min(Mth.lerp(...), 1.0F)`), so `limb()` now clamps
too; that is provably a no-op for walking (a unit test drives 200 movement ticks
and asserts the amplitude never reaches the clamp) and only bites after a hit.

**The flash is a shader change, and it forced a vertex-ABI split.** Vanilla's
`entity.fsh` is:

```glsl
color *= faceVertexColor * ColorModulator;
color.rgb = mix(overlayColor.rgb, color.rgb, overlayColor.a);
color *= lightMapColor;
```

The overlay lands on `texture × vertexColor` and the lightmap multiplies
**after** it. Rewo folded the world light into the vertex colour on the CPU, so
the two had to be separated: `color` now carries base × face shade, and a new
`light_hurt` attribute carries the per-channel light in `rgb` and the hurt flag
in `a`. `overlayColor` is a texel of `OverlayTexture` — the red row (v=3) is
**0xB3FF0000**, i.e. rgb (1,0,0) with **a = 179/255**, and the no-overlay texel
(u=0, v=10) is white with a = 1.0, which makes the mix the identity exactly as
vanilla intends.

**The mix is done in sRGB space, not linear.** Rewo works in linear light;
vanilla mixes gamma-encoded texture samples. `mix` is not invariant under the
transfer function, so the shader converts, mixes, and converts back. The gate
measures the difference: a linear-space mix would give `[209,142,108]` where the
GPU gives `[208,116,87]` — 26 bytes off in green.

**Gate: `rewo hurtshot --check`** — permanent, fail-closed on **18/18**
witnesses, Vulkan validation ON, **0 VUIDs**. Seven receipt witnesses (id
resolution, the arm, wrong-packet-id inert, untracked inert, non-living inert,
truncated-vs-intact body, real cause/direct ids keeping the walk aligned), six
clock witnesses (the exact 10→0 sequence, the ten-tick overlay window, re-arm,
the kick and its clamp, the decay, removal clearing), and five flash witnesses
read back from a real render.

**The flash is verified by prediction, not a hard-coded colour.** The capsule is
rendered twice, unhurt and hurt, and the hurt pixel is predicted *from the unhurt
one*: undo the light, encode to sRGB, mix the red overlay texel, decode,
re-apply the light. That needs no knowledge of the face shade, and it carries two
sensitivity partners that are the actual ways to get this wrong — mixing in
linear space, and applying the overlay after the lightmap instead of before it.
Measured: unhurt `[198,167,127]` → hurt `[208,116,87]`, predicted `[208,116,88]`.

**A latent bug the ABI change exposed.** `mobshot` dropped to 223/243 after the
stride went 36 → 52, because the upload path hard-coded `total * 36` instead of
using `VERTEX_STRIDE` — a duplicated constant that had been correct only by
coincidence. Only 36 of every 52 bytes reached the GPU, so the tail of each
vertex was stale. Fixed to use the constant; `mobshot` back to 243/243.

**Measured.**
- **415 tests**: world 116, net 135, gpu 46, data 14, mesh 38, proto 11 (**360
  lib**) + app 55. Release build green.
- `hurtshot` **18/18** twice, identical; `swingshot` 77/77; `eventshot` 28/28;
  `danceshot` 24/24; `mobshot` **243/243**;
  `lightmapshot`/`skyshot`/`tintshot`/`meshshot`/`dimensioncheck` green with
  validation **ON**, 0 VUIDs.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15–M20** (no entities in it).
- Live: physics **CORRECTIONS 0** with `ACCEPT place = dirt` / `dig = air`
  (5/5 with the M20.1 fix, in the world that used to fail); light `--no-relight`
  **884,736 cells, block 0 sky 0, EXACT**; `--swing-check` OK.
- `git diff --check` clean.

**Exclusions / honesty.**
- **`deathTime` is not modelled.** `hasRedOverlay` is
  `hurtTime > 0 || deathTime > 0`; only the first term applies. The death
  animation (the spin-and-fade, and the tint that rides it) is its own feature —
  it needs synced health plus `tickDeath`, and is deliberately not smuggled in
  here.
- **`invulnerableTime`, the hurt sound and the damage source are decoded past
  but not modelled** — none is model-visible. The damage *type* is read only to
  keep the body aligned; nothing branches on it.
- **`getHurtDir()` and the camera hurt-tilt are not implemented.** `bobHurt` is a
  first-person camera effect on the local player, distinct from the entity flash.
- **The white overlay (`whiteOverlayProgress`) is not modelled** — that is the
  creeper's charge flash, driven by a per-renderer override that Rewo has no
  equivalent for. The `u` axis of the overlay texture is therefore always 0,
  which is `NO_WHITE_U`, exactly what every non-creeper renderer passes.
- **Held items are still not rendered** — mobs swing and flash correctly but
  empty-handed. That remains the largest visible gap and is a deliberate future
  milestone: it needs item-model resolution, an item atlas and the
  `ItemDisplayContext` transform chain.
- **No live AI-driven damage encounter was staged or claimed.** The gate drives
  real packet bodies through the production router; a mob's actual aggro timing
  is server-authoritative and nondeterministic.

### 2026-07-25 — M22: held items, both geometry paths — SHIPPED + VERIFIED

M19 gave the player an exact swing, M20 gave it to the mobs, M21 made them
flash when hit — and all of them were swinging empty-handed. M22 puts the item
in the hand, through **both** of 26.x's geometry sources.

**The item pipeline splits in two, and the survey came first.** 26.x separates
the *item-model definition* (`assets/minecraft/items/<item>.json`, a small tree
that chooses a model from the stack's state) from the model itself
(`assets/minecraft/models/item/<name>.json`, parent-chained with `textures` and
`display`). Counting the real jar rather than assuming:

| definition type | count | M22 |
|---|---|---|
| `minecraft:model` | **1390** | resolved |
| `select` / `special` / `composite` / `condition` / `range_dispatch` | 147 | suppressed, witnessed |

Of the simple ones, **750 point straight at `block/…`** and the rest walk
`item/<n>` → `handheld` → `generated` → `builtin/generated`.

**`append_model_quads` was the seam that unified them.** It takes a model
*name* and emits quads carrying a texture-array **layer index**, so a
`block/…` item reuses the block model the bake already resolves — no parallel
resolver. The one thing the entity pass cannot do is sample that layer (it
reads its own atlas), so the layer's pixels are copied out. Both paths then
converge on one shape: **quads in model units 0..16 with UVs in 0..1 of their
own texture**, plus a list of textures to pack. The renderer never learns which
source an item came from, and `itemshot` proves the two land in the same hand.

**The sprite path is `ItemModelGenerator`'s extrusion**, transcribed exactly:
two full-size faces across the 7.5..8.5 slab, plus one thin side quad per texel
edge where an opaque texel meets a transparent one, UVs inset by
`UV_SHRINK 0.1`. Two details are easy to invert and are transcribed
deliberately — `SideDirection::Left` maps to `Direction.EAST` (and `Right` to
`WEST`, because the names describe the *sprite* edge, not the world axis), and
`isTransparent` returns **true** out of bounds, which is the only reason a
sprite's border extrudes at all. A diamond sword bakes to **82 quads**: 2 faces
plus 80 alpha edges.

**The transform chain**, from `ItemInHandLayer.submitArmWithItem` +
`ItemTransform.apply`:

```text
translateToHand(arm)                    // root then arm — the arm part matrix
mulPose(XP.rotationDegrees(-90))
mulPose(YP.rotationDegrees(180))
translate(±1/16, 2/16, -10/16)          // baby: ±0, 1/16, -4.5/16
<ItemTransform.apply>                   // translate, rotate, scale, centre -0.5
```

A `PoseStack` transforms the coordinate system, so a *point* runs the chain in
reverse call order: the display transform first, the arm matrix last. The item
is in 0..1 block units at that point, so it is scaled back to model px before
entering the arm matrix, which `part_transforms` expresses in px. The left-hand
fix negates the x translation and the **y and z** rotations — not the x, which
is worth not symmetrising by accident.

**The trap that would have cost hours:** `ItemTransform.Deserializer`
multiplies `translation` by **0.0625** (model units → block units, matching the
`-0.5` centring `apply` does) and then clamps translation to ±5 and scale to
±4, all before `apply` ever runs. Storing the raw JSON numbers would have put
**every item 16× too far from the hand** — which reads exactly like a
transform-order bug. `DisplayTransform` now stores post-deserializer values and
says so.

**Shading comes from the rotated normal, not the baked `dir`.** An item is
turned on its side in the hand, so its quads' baked face directions are not the
directions they end up facing; the normal is taken after the display and hand
rotations and fed to the same `shade_for` the mob quads use.

**1233 textures do not fit an atlas band**, so items got the treatment player
skins already have: a demand-filled slot pool. The atlas grew 1024 → 1280 tall
and the shelf packer now stops at `ITEM_POOL_Y`, which is exactly where it
stopped before (896) — **mob packing is byte-for-byte unchanged**, only the V
denominator moves, mapping the same texels to the same samples. `mobshot`
confirms it at 243/243.

**Gate: `rewo itemshot --check`** — permanent, fail-closed on **18/18**
witnesses, Vulkan validation ON, **0 VUIDs**, deterministic across runs. Six
resolution witnesses (every jar item accounted for, both sources populated,
each state-dependent type suppressed, and spot-checks that a sword names an
`item/` model, a block item resolves without touching a model file, and the bow
suppresses on `condition`), six geometry witnesses (the sword as an extruded
sprite inside the slab, dirt as a six-face full cube, the mirrored left-hand
transform, the block-unit translation), and six render witnesses read back from
a real render.

**Placement is verified against the hand, not a screenshot.** The same entity
is rendered empty-handed and holding an item, and the changed pixels must land
screen-left of centre and below the head — the entity's right arm at yaw 0.
Measured: sprite centroid (90, 151), block centroid (87, 156) — **the same
hand, from two different geometry sources** — and the off-hand sword moves to
x 170. A suppressed item (bow) differs from the empty hand by **0 pixels**.

**Measured.**
- **435 tests**: world 116, net 135, gpu 50, data 30, mesh 38, proto 11
  (**380 lib**) + app 55. Release build green.
- `itemshot` **18/18** twice, identical; `hurtshot` 18/18; `swingshot` 77/77;
  `eventshot` 28/28; `danceshot` 24/24; `mobshot` **243/243**;
  `lightmapshot`/`skyshot`/`tintshot`/`meshshot`/`dimensioncheck` green with
  validation **ON**.
- Canonical demo SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  **byte-identical to M15–M21**.
- Live: physics **CORRECTIONS 0** with `ACCEPT place = dirt` / `dig = air`;
  light `--no-relight` **884,736 cells, block 0 sky 0, EXACT**; `--swing-check`
  OK. `git diff --check` clean.

**A latent bug the vertex work exposed** (M21's, found here): the entity upload
path hard-coded `total * 36` beside `VERTEX_STRIDE`. That was already fixed in
M21; the lesson is recorded in the brief because the failure looked geometric
and the build was clean.

**The fail-closed count did its job on its author.** `itemshot` first ran
18 observed against a declared 14 — I had miscounted my own witnesses — and the
gate refused to pass rather than quietly reporting green.

**Exclusions / honesty.**
- **The 147 state-dependent items are suppressed, not approximated.** `select`
  (trim material, wolf armour), `special` (shield, chest, conduit — bespoke
  renderers vanilla itself special-cases), `composite`, `condition` (bow, spear
  pulling) and `range_dispatch` (draw progress) all branch on stack state this
  client does not track. A suppressed item renders as an empty hand, proved at
  0 pixels of difference.
- **First-person, GUI, ground and head display contexts are not modelled** —
  Rewo has no first-person hand, no item entities and no GUI inventory, so only
  `thirdperson_{right,left}hand` is read.
- **`SpearAnimations.thirdPersonAttackItem` and `animateUseItem` are not
  applied.** The first needs the STAB attack path to reach the item as well as
  the arm; the second needs `ticksUsingItem`, which is not synchronised for a
  remote entity — the same input that already blocks M20's illager crossbow
  poses.
- **Item textures larger than 16×16 are skipped rather than scaled**, so such
  an item is visibly absent instead of subtly wrong. The extruder itself
  handles any sprite size; it is the atlas slot that is one texel-block.
- **No enchantment glint, no item damage bar, no per-layer tint** (leather dye,
  potion fill) — `layer1..4` geometry is extruded but the tint that
  distinguishes those layers is not applied.
- **No live in-game eyeball.** The properties the gate checks are what M22
  verifies: resolution counts against the real jar, geometry shape, and a
  read-back render proving both sources land on the hand.

### 2026-07-26 — M26: `block_event` reaches the right block entity, and a shulker box opens — SHIPPED + VERIFIED

Committed locally on `codex/rewo-m19-combat-swings`; not pushed. Three things,
and two of them are corrections to what the four block-entity commits before
this one recorded.

**`block_event` was dispatching on the wrong thing.** `route_block_event` read
`b0 == 1` as "a chest lid" for any position holding a block entity. But `b0` is
not a global opcode — `Level.blockEvent` ends in
`getBlockState(pos).triggerEvent(...)`, which forwards to *that block entity's*
override, and the overrides disagree about what `1` means:

```text
ChestBlockEntity       b0==1 -> chestLidController.shouldBeOpen(b1 > 0)
ShulkerBoxBlockEntity  b0==1 -> b1==0 CLOSING, b1==1 OPENING, else nothing
BellBlockEntity        b0==1 -> clickDirection = Direction.from3DDataValue(b1)
```

So ringing a bell sent `b0 = 1` with `b1` a *direction ordinal*, and the client
made a lid entry at the bell and ticked it open. It drew nothing — no chest
model resolves for a bell's block state — but the entry was made, animated and
counted, and it would have surfaced the moment a bell renderer landed. Nothing
caught it because a bell rung from below is `b1 = 0`, the one case the wrong
rule got right. Dispatch is by block-entity **type** now, resolved by name,
which is what the virtual call always was.

**A shulker box's opening is `block_event` after all.** M25d recorded it as
"`ShulkerBoxBlockEntity`'s animation state, not `block_event`". The animation
state is what the event *sets*; it is the same packet and the same `b0`. What
genuinely differs is the rule, and not in the direction reuse would suggest:
the shulker tests `b1 == 0` and `b1 == 1` and has **no else**, so a second
player opening the same box (`b1 == 2`) sets `openCount` and leaves the
animation exactly where it stands. The chest's `b1 > 0` is the plausible wrong
answer, and `d7` is the witness that rejects it.

Its clock differs too. The chest converges on a clamped value; the shulker runs
a four-state machine where `OPENED` and `CLOSED` *assign* their endpoints every
tick, and `OPENING` does an **unclamped** `+= 0.1` with a separate `>= 1.0`
test — so which tick flips the state is decided by where the f32 sum lands
rather than counted out in advance (`o2`).

**The animated part had to become a matrix**, the same lesson M25d learned one
level up. A chest lid rotates about a fixed hinge, so a scalar openness plus a
pivot expressed it. A shulker lid does two things at once —

```text
lid.setPos(0.0F, 24.0F - progress * 0.5F * 16.0F, 0.0F);
lid.yRot = 270.0F * progress * (float)(Math.PI / 180.0);
```

— sliding half a block while turning three-quarters of a way round, which that
shape cannot hold. `BlockEntityDraw` now carries a model-space affine per
animated group, built by the caller, and the emitter has no per-type branch at
either level. The chest's cubic ease moved to `be_transform` alongside it, so
there is one definition rather than a copy in each crate. Two details worth not
smoothing over: at `progress = 0` the transform is the pose offset **exactly**,
so a shut box is the baked geometry untouched rather than approximately so
(`o6`); and `setPos` *replaces* the pose offset rather than adding to it, which
is why the shulker's part transform is not composed with its pivot the way the
chest's is.

**The classification had gone stale, and its guard asserted the drift away.**
`chest`, `trapped_chest`, `ender_chest` and `shulker_box` had been rendering
since M25b/M25d while `TYPE_TABLE` still called them `Invisible`,
`BlockEntityKind::Rendered` was never constructed, and witness `a4` read
*"nothing is marked Rendered yet — this witness fails the moment that changes"*.
It never fired, because the drift was in the table it restated rather than in
the registry it guarded. That is the interesting failure here: a witness that
asserts a **moment** goes on passing while the world moves underneath it.
`a4` now derives the rendered set from `ChestStates::draw_for` — the resolver
the client actually uses — and grades it against the table in *both*
directions, so a renderer that ships without moving its type fails, and so does
a type moved without a renderer. The knock-on correction: seven types are still
invisible, not the eight §16 claimed.

`BlockEntityRegistry` also **never ran in production** — `grep` found it only
in the gate — so the fail-closed boundary its docs describe ("a registry entry
the table does not classify means a new block-entity type shipped and nobody
decided what it looks like") protected gate runs and not sessions. The client
resolves it now, through a new `rewo_data::block_entity_types` reading the same
`registries.json` `EntityTypes` does; `d1` cross-checks the two reads.

**Gate: `blockentityshot` 70 → 88 witnesses.** Nine cover dispatch — the two
registry reads agreeing, the three types resolving to their own bodies, a bell
ring leaving both clocks empty (and all six click directions doing so, since
`b1 = 0` is the one the old rule got right by accident), the chest still
opening through the typed route, the shulker opening on its own clock and
making no lid entry, the `b1 == 2` no-op, consumption semantics, and an event
at empty space. Nine cover the animation — the open and shut clocks against a
transcription written separately from the decompile, the tick the f32 sum
crosses, `OPENED` holding exactly under twenty further ticks, the render lerp,
the rest pose being exact, the eight-px slide and 270° turn, the lid rising
half a block *in the world* after the renderer's `scale(1, -1, -1)` (the
witness a sign error would break, where the model-space one would not), and the
lid being its own group while the base is not.

Two witnesses failed on their first run and were rewritten, because they
asserted the wrong contract rather than the wrong value: `route_block_event`
returns "is this my packet id", a dispatch-chain answer, not "did the block
entity consume it". They ask `trigger_block_event` for the consumption
semantics now and read observable state through the route.

**Measured:** 479 tests (424 lib + 55 app; M25e was 469 — world +6, data +4);
`blockentityshot` 88/88, `itemshot` 28/28, `hurtshot` 38/38, `swingshot` 97/97,
`eventshot` 28/28, `danceshot` 24/24, `mobshot` 243/243; `lightmapshot`,
`skyshot`, `tintshot`, `meshshot` and `dimensioncheck` green with Vulkan
validation ON; canonical demo SHA-256 `2cc56b4a…` byte-identical to M15 onward;
`git diff --check` clean.

One hygiene note worth recording because it will recur: two of the files
touched here (`rewo-data/src/lib.rs`, `rewo-gpu/src/entities.rs`) are stored
with **mixed** CRLF and LF terminators, and an editor that normalises them
turns a 30-line change into a 3,400-line diff that trips `git diff --check`
(git reads the added CR as trailing whitespace). Both were rebuilt so every
line whose content is unchanged keeps HEAD's original bytes.

**Excluded:** a bell's ring is decoded and correctly declined, but `BellModel`'s
swing is not modelled — the block model draws the post and the renderer draws
the bell, so a bell is still a bell-shaped hole. Spawners likewise (their
`triggerEvent` drives a spinning mob inside the cage). The double-chest
brightness combiner is still unapplied. `openCount` is not stored, because only
the server's own container bookkeeping reads it. And there was no live session:
every witness drives synthesised packets through the production route, which is
the deterministic proof — a live shulker box would need a second player to open
it for the `b1 == 2` case at all.

### 2026-07-26 — M27/M28: the sign text and the invisible block entities — SHIPPED + VERIFIED

Five commits on `codex/rewo-m19-combat-swings`, not pushed: `e0b9937` (M27
sign text), `73a6c61` (M28 skulls + conduit), `73cd504` (M28b decorated pot),
`c428d90` (M28c banners), `8d3754b` (M28d spawner `block_event`). Together they
take `blockentityshot` from **70 to 125** witnesses and the still-invisible
block-entity set from eleven types to **two**.

**M27 — dyed and glowing sign text, and the break that keeps it on the board.**
M25e drew sign text, but always black and always full-length. All three gaps
are one method's business:

- **Glowing text is not "the same colour, brighter."** Unglowing text is the
  dye at 40%; glowing text is the dye at *full* strength, lit fullbright, with
  the 40% version demoted to its **outline**. The dark colour is a value both
  branches need, which is why vanilla computes it before the branch.
- The dye table is `DyeColor`'s **last** constructor argument. Each entry
  carries four colours, and red's texture diffuse (`0xB02E26`) against its text
  colour (`0xFF0000`) is the trap. Two values written out by eye were wrong,
  so the table is machine-extracted.
- **A sign does not wrap.** `getRenderMessages` splits and keeps fragment 0, so
  an over-long line is truncated at a word boundary and the tail is dropped.
  Two invertible details in `StringSplitter.LineBreakFinder`: the space that
  triggers a break is *excluded*, and `hadNonZeroWidthChar` guarantees at least
  one glyph so a too-wide character cannot silently lose a line.
- The eight-copy outline needed a `z` on `WorldTextDraw`. Vanilla keeps the
  copies coplanar and orders them under `POLYGON_OFFSET`; Rewo's world text is
  depth-tested with no such offset, so the outline sits a hair behind instead —
  same result by depth rather than by order, stated as a deviation.

**M28 — skulls (7 types, 14 blocks) and the conduit.** Skulls are **entity**
models, not block-entity ones: `SkullModelBase` is authored y-down and both
transforms end in `scale(-1, -1, 1)`. A chest has no such flip, and carrying
one family's assumption across renders the skull upside down and mirrored.

They forced four generalisations of the box machinery, all of which the later
types needed: per-box rest rotation, `CubeDeformation` as a uniform grow,
`mirror`, and a **per-model texture size** — the last being the silent one,
since `chest_quads` hard-coded 64×64 and a mob head's sheet is 64×32.

**M28b — the decorated pot**, the first block entity that is not one model: a
base draw plus four side draws, each with its own sherd sprite. The multi-draw
needed no new machinery (each side rides the animated-part matrix M26 had
already generalised) but did need a **second form of `visibleFaces`** —
`allOfEnumExcept(WEST)` omits one face, `EnumSet.of(NORTH)` builds only one,
and modelling the second as a single hide silently built the other five.

**M28c — banners (32 blocks)**, the first block entity whose texture carries no
colour. A pattern sprite is a greyscale **mask** and the dye is a per-layer
argument, so `BlockEntityDraw` grew a `tint` rather than baking sixteen dyes ×
forty-three patterns. Two asymmetries against models already here: a wall
banner's yaw is the facing's **own** `toYRot()` where a wall skull's is its
opposite, and the dye table is `getTextureDiffuseColor` where a sign's is
`getTextColor` — the same enum, sharply different values.

**M28d — the spawner's `block_event`**, the third meaning of `b0 == 1`.
Resetting `spawnDelay` is the whole client effect and it is visible only
through the spin: `spin += 1000 / (spawnDelay + 200)` makes a spawner
**accelerate** as its next spawn approaches, and the event slams it back to
slow.

**Three gate witnesses caught real bugs before they shipped**, which is the
part worth recording:

1. `k14` — the pot's side plane baked **six** quads instead of one, its north
   and south faces coincident and z-fighting.
2. `k19` — `Sheets.BANNER_BASE` reads as `entity/banner_base`, but the file is
   `entity/banner/banner_base.png`. The wrong path bakes **no** bodies while
   every pattern still loads, so a banner would have rendered as a floating
   sheet of cloth with no pole. The witness reported `None` for both bodies,
   which is also why it now prints its counts rather than only asserting them.
3. `s8` — an existing witness was measuring the wrong thing. It counted "every
   state that resolves and is not a chest" as a shulker box; once the static
   table resolved skulls too that silently became 382 states instead of 102.

And M26's `a4` earned itself four more times: every renderer that shipped
without moving its type out of `Invisible` failed the gate immediately.

**Two of my own witnesses failed on a wrong premise rather than a wrong
value**, and both are recorded rather than quietly fixed. `g11` asserted
glowing text was "brighter than" unglowing, which the fixture could not
distinguish because the synthesised chunk reads full sky light; it asserts the
exact rule now, against a finite `sample(7, 7)` rather than the NaN-producing
`sample(0, 0)`. `d11` ticked a spawner forty times before triggering it, and a
spawner with no event has no clock entry at all.

**Measured:** 490 tests (435 lib + 55 app; M26 was 479 — `rewo-data` +11 for
the sign-text module); `blockentityshot` **125/125**, `itemshot` 28/28,
`hurtshot` 38/38, `swingshot` 97/97, `eventshot` 28/28, `danceshot` 24/24,
`mobshot` 243/243; `lightmapshot`, `tintshot` and `meshshot` green with
validation ON; canonical demo SHA-256 `2cc56b4a…` byte-identical to M15 onward;
`git diff --check` clean.

**Still invisible — two types, each for a stated reason:**

- **Copper golem statues** (9 blocks). Four *separate* pose layers
  (`COPPER_GOLEM`, `_RUNNING`, `_SITTING`, `_STAR`), not one model posed, and
  each is a nested hierarchy where a child's offset rides through its parent's
  rotation. The flat box list here cannot express that; it needs an ancestor
  chain, and then ~35 boxes of careful transcription whose errors would be
  silent. Deliberately not shipped half-verified.
- **End portal and end gateway** (2 blocks). `AbstractEndPortalRenderer` is a
  bespoke shader over a flat quad, not a model at all — a different kind of
  work from everything above.

**Also excluded, and shared by almost every type here: the block-entity
animation clock.** The conduit's spin and active cage, the pot's wobble, the
banner's sway, a skull on a note block, a piglin head's ears, a dragon head's
jaw and the spawner's caged mob all key off a per-block-entity tick this
client does not keep. Each renders at rest. A player head draws the jar's
default skin, because the profile in its NBT would need a network fetch and a
gate cannot depend on one.

**One process note that recurred and is now in §0.0 as gotcha 10.** A Python
edit written with Windows' default cp1252 encoding re-encoded a source file and
left it invalid UTF-8; the repair then double-encoded every pre-existing
em-dash, and the file had to be restored from HEAD with the changes re-applied.
Any script touching these sources must pass **both** `encoding='utf-8'` and
`newline=''` — the same class of hazard as the mixed CRLF/LF endings in gotcha
9, which also bit again here.

### 2026-07-26 — M28e/M28f: the statue and the end portals — the Invisible list is empty

`c838d7f`, on `codex/rewo-m19-combat-swings`, not pushed. **Eleven types
measured by M25, eleven rendering.** `blockentityshot` 125 → **133**.

**The statue's four poses are machine-extracted, and that is the substance of
the milestone rather than a detail.** `CopperGolemStatueBlockRenderer` bakes
four *separate* layers — STANDING, RUNNING, SITTING, STAR — each its own nested
`PartDefinition` tree where a child's offset rides through its parent's
**rotation**. Thirty-eight boxes, most with rotations to four decimal places.

The previous pass declined to hand-transcribe this and said so, because the
errors would be silent: a statue with one arm a few degrees off still looks
like a statue. `tools/gen_copper_golem_poses.py` removes the risk the way
`gen_anim_defs`, `gen_block_light` and `gen_vanilla_hierarchy` already do — it
parses the decompiled source, matches each `addOrReplaceChild` to its parent by
the receiver of the call, and emits the flattened table with each box's
ancestor chain intact. It accounts for every `addBox` in all four methods
(9/9/11/9) and emits deterministic LF.

**`k25` is the witness that earns it.** It compares each box's real position
against a naive offset-sum that ignores every ancestor rotation: the two must
**agree** across STANDING (no ancestor rotations) and must **differ** somewhere
in RUNNING (which has them). That difference *is* the nested hierarchy — the
one property a flat box list could not express, asserted directly rather than
by eye.

Two statue details worth keeping. The flip is not in vanilla's matrix: it is
`setupAnim` setting `root.zRot = PI`, folded in here because this client does
not run that step. And the weathering suffix **follows** the stem
(`copper_golem_exposed.png`) while the *block* names run the other way
(`exposed_copper_golem_statue`) — the same irregular copper naming M10's
generator recorded, and prefixing by analogy silently loaded nothing until
`k23` said so.

**The end portals were half-misdescribed by every earlier record here,
including mine.** "A bespoke shader rather than a model at all" is true of the
render *type* and false of the geometry: `submitCube` builds ordinary unit-cube
faces, and only `rendertype_end_portal.fsh` — fifteen scrolling samples of
`end_sky.png` faking depth — is a shader. The geometry therefore ships exactly
and the shader is approximated by one static layer of `end_portal.png`: a
stated approximation, and far better than the alternative, since an end portal
that renders nothing is an invisible hole you fall through.

Two geometry facts pinned: a portal builds **only its horizontal faces**
(`shouldRenderFace` is `getAxis() == Y`, which is why an edge-on portal shows
nothing in vanilla), and it is a **slab from y 0.375 to 0.75** — a pool set
into the middle of its block, not a full block and not floor-flush. A gateway
pushes no transform and builds all six.

`a3` was rewritten. It asserted a specific still-invisible membership; the set
is empty now, so it asserts *that*, and it is henceforth the witness that fires
if a future version adds an invisible type and nobody writes a renderer.

**Measured:** 490 tests (435 lib + 55 app); `blockentityshot` **133/133**,
`itemshot` 28/28, `hurtshot` 38/38, `swingshot` 97/97, `eventshot` 28/28,
`danceshot` 24/24, `mobshot` 243/243; `lightmapshot`, `skyshot`, `tintshot`,
`meshshot` and `dimensioncheck` green; canonical demo SHA-256 `2cc56b4a…`
byte-identical to M15 onward; `git diff --check` clean.

**What remains here is one shared gap, not a list of types: the
per-block-entity animation clock.** The conduit's spin and active cage, the
pot's wobble, the banner's sway, a skull's bob, a piglin head's ears, a dragon
head's jaw, the spawner's caged mob and the portal's scrolling starfield all
key off a tick this client does not keep, and all render at rest. Also out: a
gateway draws all six faces because Rewo has no neighbour context at bake time
(over-draw, invisible for the free-standing gateways they always are), the
gateway's beacon beam is not drawn, and a player head uses the jar's default
skin because the profile in its NBT would need a network fetch.

### 2026-07-26 — M29: the block-entity animation clock — SHIPPED + VERIFIED

`de9c4e1`, on `codex/rewo-m19-combat-swings`, not pushed. `blockentityshot`
133 → **147**. The gap M28f named as "one shared clock" turns out not to be one
clock at all: **what a block entity animates *from* varies**, and grouping them
by that is what made it tractable.

| driven by | example |
|---|---|
| position and game time only | a banner's sway — no state whatsoever |
| an event and a start tick | a pot's wobble |
| an accumulating counter | a skull's animation |

**A pot's wobble is a FOURTH meaning of `b0 == 1`.** After a chest's viewer
count, a shulker's open/close pair and a spawner's reset,
`DecoratedPotBlockEntity.triggerEvent` reads `b1` as a **`WobbleStyle`
ordinal** — and the tick the event arrived on *is* the animation's start, so
`route_block_event` now carries the game time as part of the payload. The
guard is real (`data >= 0 && data < values().length`), so an out-of-range
ordinal is not consumed. The two styles also last **different lengths**, 7 and
10 ticks.

Both wobbles turn about `(0.5, 0, 0.5)` — the block's **floor** centre — where
the pot's facing rotation immediately above them turns about `(0.5, 0.5, 0.5)`.
A pot rocks on its base rather than pivoting in mid-air.

**The clock exposed two rest poses that were already wrong**, which is the part
worth carrying forward. `SkullModelBase.setupAnim` **always runs** — it is not
gated on the animation being active — so a piglin head's ears and a dragon
head's jaw sit at their formula values even at `animationPos = 0`. Rewo drew
the mesh's own `PartPose`:

- the piglin's ears belong at ∓0.7 rad, not the ∓30° in the mesh — **about 10°
  off on every piglin head in the world** since M28;
- the dragon's jaw rests **0.2 rad open**, and Rewo drew it shut.

Neither read as a bug, because both look like plausible heads. That is the
recurring lesson: a wrong *rest* pose is invisible precisely because nothing
moves to contradict it, and it took building the motion to see it.

The ears also forced the emitter to grow. One animated group per model covered
a chest lid, a shulker lid and a banner flag; a piglin's two ears animate to
**different formulas at once** — the `1.2` asymmetry is on the left ear only,
so the pair drifts in and out of phase rather than flapping together. So
`part_transform` became `part_transforms: [_; MAX_PARTS]`, indexed by a quad's
group, 0 still meaning "as baked".

The skull counter lives on `World`, not on the block-entity map, because its
driver is a **block-state** property (`SkullBlock.POWERED`) rather than
anything in the NBT — a skull animates because the note block beneath it is
powered, and only the world knows that.

Two banner details worth not smoothing over: the phase is
`floorMod(x*7 + y*9 + z*13 + gameTime, 100)` — **`floorMod`, not `%`**, because
a negative coordinate must wrap rather than go negative — and the cloth never
hangs straight, since the constant term (−0.0125) exceeds the amplitude (0.01).

**Measured:** 490 tests (435 lib + 55 app); `blockentityshot` **147/147**,
`itemshot` 28/28, `hurtshot` 38/38, `swingshot` 97/97, `eventshot` 28/28,
`danceshot` 24/24, `mobshot` 243/243; `lightmapshot`, `skyshot`, `tintshot`,
`meshshot` and `dimensioncheck` green; canonical demo SHA-256 `2cc56b4a…`
byte-identical to M15 onward; `git diff --check` clean.

**What remains is no longer a clock.** Each leftover names a different missing
capability, which is more useful than one shared excuse:

- a **conduit's** active cage, wind and eye need `updateShape`'s
  prismarine-frame **world scan** to decide `isActive`. Its dormant shell is
  already exact — `activeRotation` advances only while active, so a conduit
  that has never activated genuinely sits at zero.
- a **spawner's** caged mob needs an **entity model composed into a
  block-entity draw**, a seam this client does not have.
- an **end portal's** starfield needs the render type's **shader**.
- a skull whose counter has stopped reads 0 rather than holding its last count.
  Vanilla keeps the count and stops adding the partial; the ear formula is
  periodic, so a stationary head looks the same either way — a stated
  simplification, not an equivalence.

### 2026-07-26 — M30: the active conduit — SHIPPED + VERIFIED

`42e02b0`, on `codex/rewo-m19-combat-swings`, not pushed. `blockentityshot`
147 → **157**.

M29 recorded that the conduit's active cage was "blocked on a world scan, not a
clock", and that was exactly right. **A conduit decides whether it is active
itself.** The server sends no flag, no angle and no activation packet, so
`updateShape` — a scan of the water and prismarine around the block — is the
entire prerequisite. That is why this looked like an animation problem and was
not one.

**The shell is 42 positions, not 48, and that is the hunting threshold.** Each
of the three axis rings borders a 5×5 plane (sixteen apiece), but the rings
**share their axis ends**, so the union is 42 — and `updateHunting` is
`effectBlocks.size() >= 42`. A conduit therefore opens its eye exactly when its
frame is **complete**, not when it is nearly so. I wrote 48 down first from the
shape of the condition; a unit test corrected it, and `q1` now pins the
coincidence. Both thresholds read one count, which is why the eye costs nothing
once the scan exists.

`isWaterAt` is `getFluidState(pos).is(FluidTags.WATER)`, so a **waterlogged**
block counts — a frame built with waterlogged stairs is legal, and reading only
`RenderKind::Fluid` would refuse to activate a perfectly good conduit. The bake
grows a per-state `water` table for it.

Like M29's skull tick, the conduit tick lives on `World` because it needs
**block states**. Unlike every other entry in `BlockEntities`, a conduit's
clock is created on its first *tick* rather than by an event: nothing the
server sends says a conduit exists.

Three details worth not normalising:

- The renderer converts the rotation to degrees and immediately back to radians
  — an **exact round trip**, therefore a no-op. "Correcting" one half of it
  would break the cage.
- `activeRotation` advances, and takes its partial tick, **only while active**,
  so a conduit that switches off stops dead. That is also why Rewo's dormant
  shell was already correct at zero.
- The shape is rescanned only on `gameTime % 40 == 0`, so a conduit snaps on at
  the next multiple rather than flickering while a player lays its frame.

The four active draws: a cage tumbling about the **tilted** axis `(0.5, 1, 0.5)`
normalised — not plain Y, so it tumbles rather than spinning flat — the wind
shroud **twice** (once upright, once at 0.875 scale half-turned about X and Z,
so the shells counter-rotate and read as a churn), and a **camera-facing eye**.
The shroud's phase (`tickCount / 66 % 3`) changes both its axis and its
texture. The eye is the **one input in this whole block-entity path that is a
property of the view rather than of the block**, so the collector now takes the
camera axes.

`q8` needed rewriting: it asserted the cage sat at 0.4 at `animTime = 0`, which
was my number and not vanilla's — the drive is at its *midpoint* there and the
height is 0.45. It now asserts the range endpoints and that the midpoint falls
below where a linear map would put it, which is the actual claim (`hh*hh + hh`
is convex, so the cage dwells low and snaps up).

**Measured:** 495 tests (440 lib + 55 app; M29 was 490 — `rewo-world` +5);
`blockentityshot` **157/157**, `itemshot` 28/28, `hurtshot` 38/38, `swingshot`
97/97, `eventshot` 28/28, `danceshot` 24/24, `mobshot` 243/243; `lightmapshot`,
`skyshot`, `tintshot`, `meshshot` and `dimensioncheck` green; canonical demo
SHA-256 `2cc56b4a…` byte-identical to M15 onward; `git diff --check` clean.

**Two block-entity items remain**, and neither is a conduit problem: a
**spawner's** caged mob needs an entity model composed into a block-entity
draw, and an **end portal's** starfield needs the render type's shader. Also
out here: the conduit's damage beam, its ambient sounds and `applyEffects` (the
status it grants nearby players) are all server-side and carry no geometry.
