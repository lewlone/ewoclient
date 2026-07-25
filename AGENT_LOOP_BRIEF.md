# Agent-loop brief — Rewo

The process/working-agreement layer for the headless two-agent loop.
**`REWO_PLAN.md` §0.0 HANDOFF remains the technical entry point**; this file
covers how we work, not what the code does.

---

## Roles

- **Sol (GPT-5.6) — senior dev.** Directs, reviews, decides scope and
  priority, challenges claims. Has full repo access.
- **Claude — implementer.** Writes code, runs the gates, reports measured
  results, keeps the docs and memory current.

Both operate headlessly. The user is away; nothing may depend on them
manually launching, clicking, or looking at anything.

---

## What Rewo is

A from-scratch native Minecraft client in Rust: vanilla protocol (pin **26.2 /
protocol 776**), raw Vulkan via `ash`, single-threaded tick + rayon mesh pool.
It lives in the `crates/rewo-*` half of the EwoClientV3 workspace and produces
one binary, `rewo.exe`. The `ewo-*` crates are a **different, unrelated
project** (a Skia launcher + a JNI in-game HUD) — Rewo work must not touch
them.

Performance is defined as **1%/0.1% frame-time lows and input latency**, not
average fps.

---

## The two non-negotiables

**1. Ground truth is the decompiled jar, never a wiki.**
`%APPDATA%/EwoClient/rewo/26.2/decompiled/` (Vineflower output) plus the
datagen reports under `.../26.2/datagen/generated/reports/`. Community
documentation is frequently wrong or version-stale. 26.x moved a great deal
(`LightTexture` → `Lightmap`, `getSkyDarken` → a keyframed timeline,
`ClientboundSetTimePacket` → a clock map), and each time the wiki would have
lied. If a fact is not in the decompile or a report, say so explicitly rather
than filling the gap.

**2. Verification is headless-first, and must check the property, not a
proxy.** The user has stated they will not manually test what a machine can
check. Every milestone ships a self-check path. "It looks right" is not a
result — a previous mob pass shipped textures that were silhouette-correct and
UV-scrambled, and was reported as verified. That is the failure mode to design
against.

---

## The gates

Run the relevant ones before claiming anything; run all of them before
declaring a milestone done.

```bash
cargo test --release -p rewo-world --lib   # 98
cargo test --release -p rewo-net   --lib   # 114
cargo test --release -p rewo-gpu   --lib   # 44
cargo test --release -p rewo-data  --lib   # 9
cargo test --release -p rewo-mesh  --lib   # 38
cargo test --release -p rewo-proto --lib   # 11   → 314 lib
cargo test --release -p rewo-app           # 54   (app-level) → 368 total
```
(`cargo test --workspace` also pulls in the unrelated `ewo-*` crates; run the
`rewo-*` ones individually.)

```bash
./target/release/rewo.exe mobshot --check
```
Facelabel gate: face-coloured debug textures vs a perspective ray-cast of the
same geometry, occlusion-exact, serverless. **243/243.** Run after any mob,
model or UV change.

```bash
./target/release/rewo.exe lightmapshot --check
./target/release/rewo.exe skyshot --check
./target/release/rewo.exe tintshot --check
./target/release/rewo.exe meshshot --check
```
Permanent serverless Vulkan property oracles, validation layers on (each fails
closed if validation is unavailable). The first checks the complete 26.2
lightmap matrix across terrain, water and entities; the second the sky/celestial
transforms, textures and alpha; the third (M14) the per-biome tint end to end —
production jar bake + synthetic single/indirect/direct biome containers +
production `mesh_column` + Vulkan readback, with independent Temurin-25-verified
vectors (boundary **[91,163,163]**, dark_forest **[147,26,5]**, swamp
light/dark **[106,112,57]**/**[76,118,60]**, spruce/birch
**[97,153,97]**/**[128,167,85]**), a raw-quart camera sky/fog Gaussian (fog
boundary **0xffac2d6d**, A inherits / B overrides), and a fully-fogged terrain
readback. **0 VUIDs.** Run after any biome, tint, colormap or chunk-biome change.

`meshshot` is M15's permanent CPU geometry oracle. It expands production
greedy rectangles back to unit faces and compares them with the frozen
pre-greedy mesher by direction, owning block, atlas layer + block/sky light,
four AO codes and tint. It also pins exact model/water/lava legacy controls,
material/light/tint seams, section crossing, deterministic output, and the
top-face 1×1 exclusion. Run after any mesh format or merge-policy change.

```bash
./target/release/rewo.exe dimensioncheck --check
```
M16's permanent **serverless** dimension oracle, CPU-only. It grades four
mutually-independent inputs: a **captured** vanilla Configuration
`registry_data` packet (production parser), the **bundled** built-in
transcription, the **real decompiled datagen JSON** under
`…/26.2/decompiled/data/minecraft/dimension_type/` (read by
`rewo-app/src/dimension_json.rs`, a `serde_json` reader sharing no code with the
NBT parser, with `has_day_timeline` resolved by expanding
`data/minecraft/tags/timeline/*.json`), and a hand-written `EXPECT` table that
grades all three and is itself graded by the JSON. Then the world binding
(16 shape + 16 light probes), the mesh binding (96 vertices, Nether shade codes
`[2,3,4,5,6]`, max sky 0) and the mesh pool generation fence (`[0,1]`). Fails
closed on a missing recording or a missing/malformed decompile
(`--decompiled <dir>` overrides). Run after any dimension, registry, world-shape
or cardinal-shade change.

```bash
./target/release/rewo.exe eventshot --check
```
M17's permanent **serverless** entity-event oracle, CPU-only (no socket, no GPU
device), **fail-closed on a fixed 28/28 witnesses** — every named property is
observed and the run errors if any failed *or* the count is short (a silently
skipped property). It drives the whole production receipt path for the three
model-visible `ClientboundEntityEventPacket` animations (Warden attack, Warden
sonic boom, Armadillo peek):

```text
raw fixed-body packet (BE-i32 id + signed byte)
  -> rewo_net::route_entity_event   (production packet-id selection)
  -> EntityTable::start_event       (real kind lookup + receipt-tick storage)
  -> live_cmd::resolve_mob_anim     (vanilla ownership rules, event ages)
  -> rewo_gpu oracle_part_deltas    (the exact GPU model pose math)
```

It loads the real `packets.json` and `registries.json` and proves `entity_event`
id **34** and type ids **Warden 143 / Armadillo 4** before any pose check. The
targets are **independent decompiled literals** — it does **not** read
`anim_defs` as its expectation (that is the renderer's own table), and the
catmull-rom target is recomputed from the four frame literals. Tolerances are
~1e-4 (they absorb only the decimal-keyframe-time reconstruction through the
real `ageInTicks` clock). Every property carries a mutation/sensitivity partner
(wrong packet id, missing/wrong-kind entity, event 61/unknown, repeat restart,
remove/reuse clear, neutral base-pose parts). Run after any entity-event,
`gen_anim_defs`, Warden/Armadillo rig, or dispatch-seam change.

```bash
./target/release/rewo.exe danceshot --check
```
M18's permanent **serverless** Allay-dance oracle, CPU-only (no socket, no GPU
device), **fail-closed on a fixed 24/24 witnesses**. The Allay dance is
*metadata*, not an entity event (event 18 is heart particles) — `DATA_DANCING`
at SynchedEntityData **index 16, BOOLEAN serializer 8**, driving the exact
`Allay.tick()` client counters and the `AllayModel` root/head formulas. It drives
the whole production metadata path:

```text
raw set_entity_data body (VarInt id + metadata delta stream)
  -> rewo_net::route_set_entity_data / apply_set_entity_data
       (production id selection + kind-aware DANCING vs BABY + vanilla
        missing-entity inertness — getEntity==null drops the whole packet)
  -> EntityTable::set_dancing + tick_lerp   (the exact client counter lifecycle)
  -> live_cmd::resolve_allay_dance          (the SAME app resolver the collector uses)
  -> rewo_gpu oracle_part_deltas            (the exact AllayRoot/AllayHead math)
```

It loads the real `packets.json`/`registries.json` and proves `set_entity_data`
id **99**, Allay type **2**, Zombie control **151** before any pose check.
Expectations are an **independent** counter simulation + independent
`AllayModel`/`AllayWing` transcriptions — nothing reads the production
`anim_delta`/`EntityTable`/`AllayDance::tick` as its expectation. Witnesses pin
Allay-not-baby, the Zombie legit-baby control, wrong packet id, INT-is-size, the
missing-entity full inertness (incl. an accompanying pose), a **wrong index** and
a **wrong serializer** at slot 16, the exact spin boundary **@14/@15/@55**,
repeated-true no-restart, false→reset→true, `isSpinning` distinct from
`spinningProgress`, partial alpha, remove/reuse, and the sway/spin/suppress-look/
dance-bit/animationSpeed/wings-intact pose properties. Run after any Allay-dance,
metadata-routing, `EntityTable` counter, or Allay-model change.

```bash
./target/release/rewo.exe play --username RewoOp --seconds 90 --dimension-check
```
The live dimension gate: the paced Overworld→Nether→End→Overworld route.
**4/4 checkpoints, 3/3 transitions**, each discarding and requeueing 329
columns, chunk decode failures 0, settled corrections 0.

```bash
./target/release/rewo.exe demo --out C:/tmp/demo.png
```
Synthetic block-model showcase, no server. Its PNG is expected to stay
**byte-identical** across refactors that should be visually neutral — that
property has caught more than one silent regression, and it is worth
preserving deliberately.

```bash
./target/release/rewo.exe bench --replay "$APPDATA/EwoClient/rewo/26.2/m1-soak.rewo"
```
Deterministic render benchmark, the merge-gate metric. Currently ~0.23 ms avg
GPU. Watch the 1%/0.1% lows, not the average.

```bash
./target/release/rewo.exe play --username RewoOp --seconds 30
```
Physics parity: **`CORRECTIONS: 0`**. A `tp` in `--setup` costs exactly one
correction — that is the teleport, not a regression. This build-enabled gate
also **fails closed** on the scripted build actions (M16.1): it proves the
server-observed final state — the placed cell is `minecraft:dirt`, the dug cell
is air — from the `block_update` the server echoes for both `pos` and
`pos.relative(direction)` on every use-item-on, and **exits 1** if either
property is unproven. The `ACCEPT …` lines are the proof; the `actions sent …`
line is only packets attempted. `--no-build` and `--dimension-check` never
attempt these actions and are exempt.

```bash
./target/release/rewo.exe play --username RewoOp --seconds 14 \
    --no-build --still --light-check --no-relight
```
Lighting parity: recompute the loaded columns from scratch and diff against
the server's own light engine. **884,736 cells, 0 mismatches, both channels.**

---

## Test server

`%APPDATA%/EwoClient/rewo/26.2/testserver/` — vanilla 26.2, offline mode,
superflat, port 25599, `enforce-secure-profile=false`.

```bash
cd "$APPDATA/EwoClient/rewo/26.2/testserver"
nohup java -Xmx2G -jar ../server.jar nogui > server.log 2>&1 &
sleep 28   # then grep server.log for "Done"
```

Stop it when finished — leaving a JVM running is untidy and can hold a world
lock. It idle-pauses when empty, which is normal.

**The op account is `RewoOp`.** Not `RewoBot`, not `RewoLive` — only `RewoOp`
is in `ops.json`, and a non-op's `--setup` commands are silently rejected,
which looks exactly like a code bug.

---

## Traps that have already faked results

Each of these produced a confident, wrong conclusion at least once. They are
listed because they are invisible in the output.

- **Setup commands must be paced.** `--setup` accepts `;`-separated commands
  and sends one per 250 ms. Firing them in one tick trips the server's chat
  rate limit and the tail is dropped — the structure never appears, and the
  symptom reads as a lighting or meshing bug.
- **A structure built right after a `tp` is already present when the chunks
  stream in.** That masked `section_blocks_update` being entirely unhandled
  for a long time. To prove chunks are loaded first, pad with paced no-ops
  (`say w1;say w2;…`) before the real commands.
- **`--light-check` can grade the engine against itself.** It diffs a
  recomputation against the *stored* light, and incremental relighting writes
  that store. Pass `--no-relight`, or build in one run and grade from a fresh
  join in a second. A vanilla server sends **no light packets for ordinary
  block edits at all** — that is why the client engine exists.
- **Stale binaries.** A run that contradicts the previous one for no reason is
  usually a build that had not finished replacing the exe. Rebuild and repeat
  before theorising.
- **Farmland reverts to dirt** when unhydrated, and the server leaves stale
  block-light inside the now-opaque cell. Use `dirt_path` as the stable
  stand-in in lighting tests.
- **Absolute coordinates beat `~` after a `tp`.** The `~` resolves against
  wherever the entity is when the command executes, which may not be where you
  think.
- **A fixture graded against its own transcription proves nothing.** M16's
  `dimensioncheck` test was named
  `the_bundled_transcription_matches_the_decompiled_json` while only comparing
  the bundled built-ins to a handwritten table — the decompiled JSON was never
  opened. Senior review caught it. If a check's *name* claims a file is the
  oracle, the check must read that file, and the reading must not go through
  the code under test.
- **A block placed beside the player can land inside the player.** 26.2
  `BlockItem.canPlace` rejects a placement whose cell overlaps any entity
  (`isUnobstructed(state, clickedPos, placementContext(player))`). The bot's
  0.6-wide body reaches to `fx+1.3`, so placing at the adjacent cell `(fx+1, fy)`
  failed intermittently — about one run in four, whenever the resting sub-block x
  was ≥ 0.7 — and looked like a protocol bug when the packets were byte-perfect.
  Place two cells out (`fx+2`), which the footprint can never reach. The server
  still sends a `block_update` for the target on *rejection* too (air), so the
  client's observation is authoritative either way — grade the exact block, not
  "non-air", and let the gate exit nonzero.
- **Fixed time and the day timeline are different fields.** `has_fixed_time`
  and the `timelines` holder set are independent members of `DimensionType`;
  deriving one from the other happens to give the right answer for all four
  vanilla dimensions and is still wrong. The tag files decide the timeline.
- **Metadata for an untracked entity is dropped whole.** 26.2
  `ClientPacketListener.handleSetEntityData` does `Entity e = getEntity(id); if
  (e != null) e.getEntityData().assignValues(…)` — an untracked id mutates **no**
  state (not a baby fallback, not anything). M18's first cut applied a baby
  fallback for a missing id; senior review caught it against the decompile.
  `apply_set_entity_data` must return before applying when `entities.get(id)` is
  `None`.
- **A metadata slot can be polymorphic by entity kind.** `DATA_DANCING` (Allay)
  and `DATA_BABY_ID` (`AgeableMob`/`Zombie`) both sit at SynchedEntityData index
  16 with the BOOLEAN serializer (id 8) — the byte parser cannot separate them.
  Only the entity *type* (resolved from `registries.json`) can, at the routing
  layer. The parser must surface the raw bit and let the kind-aware caller route
  it, not prejudge it.

When a render looks wrong, the fastest diagnostic is to **force one term to a
constant** (e.g. `lm = vec3(1.0)`) and see which term owns the pixel. That
found the ground-plane lighting bug in minutes after speculation had failed.

---

## Current state

`M0–M18` shipped and verified. **M0–M9 are pushed** (`origin/main` @
`973ea5e`); the **M10–M18 arc is reviewed local work, not yet pushed** (M12 is
`06dd3eb`, M14 `88b5112`, M15 `5b0f437`, M16 `f6e0201`, M16.1 `f4b54d1`, M17
`55388c8`; M18 is committed locally as `bb8be20` on branch
`codex/rewo-m18-allay-dance`, base `6096bbd` — the M17 handoff commit — not
pushed). M18 is green on every gate; its vanilla test server was stopped by exact
PID (16856) and port 25599 verified free. See `REWO_PLAN.md` §15 for each
milestone's exact ground truth and measured gates.

A playable online client: joins online-mode servers with signed chat, real
player skins, 88 vanilla mob models with formula-exact procedural and keyframe
animation, native OptiFine CEM (Fresh Animations runs with no mod loader),
GPU-driven rendering, a server-exact client light engine, vanilla's lightmap
curve and a real day/night cycle with clear-weather celestials (sun, moon
through all eight phases, stars, and the sunrise/sunset fan) driven by a smooth
server-driven world clock — and now **per-biome color**: grass/foliage/water
tint (exact radius-2 mean, dark_forest + swamp modifiers, spruce/birch
constants), retained 4×4×4 section biomes, and biome-driven camera sky/fog via a
raw-quart Gaussian feeding per-frame GPU base uniforms. M15 replaces the
36-byte vertex with an exact 28-byte ABI and greedily merges five safe cube-face
directions, reducing the canonical replay upload from 149.13 to 109.39 MiB
(−26.65%) without changing the canonical demo image. **M16 makes the
`dimension_type` registry a consumed contract**: per-dimension vertical shape
(the Nether's 0..256 mis-decoded every chunk before), sky channel, skybox,
ambient light, cardinal face shade, sky/fog/ambient/sky-light colours, fixed
time and a tag-resolved day timeline — selected by raw holder id — plus the End
sky pass and a world/mesh transition that discards and refences by generation.
The canonical demo PNG is byte-identical to M15. **M17 consumes
`ClientboundEntityEventPacket`** (id 34): the Warden's attack and sonic-boom
rigs and the Armadillo's re-peek now fire from the exact decompiled
`WardenAnimation`/metadata definitions, threaded through the production
dispatch → tick-store → ownership → pose path and proved by the serverless
`eventshot` 28/28 oracle; the demo PNG stays byte-identical. **M18 consumes the
Allay's `DATA_DANCING` metadata** (index 16, BOOLEAN serializer 8): the exact
`Allay.tick()` client counters + `AllayModel` root/head dance formulas fire
through the production metadata path (kind-aware routing with vanilla
missing-entity inertness → `EntityTable` counters → `live_cmd::resolve_allay_dance`
→ GPU pose), with the Allay model restructured into the real
root→{head,body→{arms,wings}} hierarchy (rest geometry neutral). Proved by the
serverless `danceshot` 24/24 oracle; the demo PNG stays byte-identical.

Subcommands: `net` (protocol), `view` (snapshot), `play` (headless bot),
`live` (windowed client; `--out` renders the eye view headless), `demo`,
`bench`, `mobshot`, `skyshot`, `lightmapshot`, `tintshot`, `meshshot`,
`dimensioncheck`, `eventshot`, `danceshot`.

**Open work**, roughly in descending obviousness:

- Custom (datapack) dimension types are parsed but unexercised; the
  `{modifier, argument}` arm of an attribute override is a deliberate hard
  error rather than a modelled case. (Biome blend radius is fixed at vanilla's
  default 2 — not yet a setting.)
- Entity-*event* coverage is the three model-visible ones (Warden attack +
  sonic boom, Armadillo peek — M17). **The Allay dance shipped in M18** as the
  first `DATA_DANCING`-metadata rig. The Warden tendril (event 61) and generic
  `ClientboundAnimatePacket` arm swings are the next animation steps; dragon
  flight is bespoke procedural code, still posed. The Allay's own unconditional
  body flying-tilt / root idle-bob / arm idle-bob remain unimplemented (they are
  not the dance; the wing/hover behavior is implemented + witnessed).
- Face-occlusion merging is tested per-side rather than as a true shape union
  — differs only for complementary partial faces, which no vanilla pair
  produces.
- `glow_lichen`'s "any face" emission predicate is approximated at a constant 7.
- Redstone/stem/lily-pad `BlockColors` were explicitly out of M14.

---

## Conventions

- **Commit messages explain the why**, not just the what: what was wrong, how
  it was found, what was measured. Several of this project's commits are the
  only record of a subtle vanilla behaviour. End with
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **After a milestone**, update `REWO_PLAN.md` §15 (the status log — it is the
  durable record), the Rewo section of `CLAUDE.md`, and the `rewo_client`
  memory file. Load-bearing conventions belong there so they are never
  re-derived.
- **Generated code is generated**: `tools/gen_block_light.py`,
  `tools/gen_vanilla_hierarchy.py`, `tools/gen_anim_defs.ps1`. Re-run after a
  version bump; never hand-edit their output. They are written to fail loud
  on an unrecognised form rather than silently defaulting.
- **Never run `cargo fmt`.** The Rewo code is hand-formatted; `cargo fmt`
  rewrites the whole workspace (~21k lines, incl. the generated `anim_defs.rs`).
  Keep the semantic diff narrow with targeted edits. A couple of Rewo source
  files (`rewo-gpu/src/entities.rs`, `rewo-app/src/mobshot_cmd.rs`) are **mixed
  CRLF/LF in HEAD**; the editor may normalise a whole file to CRLF, which trips
  plain `git diff --check` on your added lines. The clean fix that keeps
  `git diff --check` green *and* preserves every unchanged byte is to insert
  LF-only lines: `git diff | tr -d '\r' > p; git checkout HEAD <file>; git apply
  --ignore-whitespace p`. Verify with plain `git diff --check` (not
  `-c core.whitespace=cr-at-eol`).
- **Leave the tree clean and the server stopped** at the end of a work block.
- **Report failures as failures.** If a gate regresses, say so with the
  numbers. An honest red result is worth more than a green one that was
  obtained by weakening the test.
