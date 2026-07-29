# REWO_PLAN.md — Rewo: the from-scratch native Minecraft client

**Rewo** (from "rewolution", as Ewo came from "ewolution") is a from-scratch
Rust Minecraft: Java Edition client speaking the vanilla protocol, rendered
with raw Vulkan. This file is the plan of record. It supersedes both the
hand-off design doc (`~/Downloads/rust-mc-client-design.md`, drafted under
codename "Ferric") and the interim `FERRIC_PLAN.md` (deleted). The design
doc's reasoning was pressure-tested against the live repo and the on-disk
26.2 jar on 2026-07-21; its four product decisions are kept, a set of factual
errors is corrected (§2), and several missing workstreams are added (§3).

**Status: shipped and headlessly verified through `f7901f2` (2026-07-28).**
`origin/main` carries all of it, and the long-standing branch risk (everything
from M10 on living on one unmerged branch) is closed. See §0.0 for the
fresh-session handoff and §15 for the per-milestone log.

---

## 0.0 HANDOFF — read this first (fresh session, 2026-07-28)

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

### Where it is: M0–M57 shipped and merged to `main`

`origin/main` is at **`aadd8e9`** and everything from M0 to M57 is on it. The
working tree is clean and there are no stashes.

**A numbering note, because the ladder is not contiguous by accident.** M52 is
the **EwoClient module port**, which landed on `main` from a second concurrent
session (`ec531cd`) while five agents were building M51/M54–M57 in parallel
worktrees. Three of those agents independently self-numbered their work M52
before that commit existed; they were renumbered on merge. **M53 is a
specification, not code** (`REWO_HEALTH_BAR_SPEC.md`) — its implementation is
still open. If §15's entries read out of order, that is why: they were authored
concurrently and merged, not written in sequence.

**The one side branch that carried real unlanded work is landed** —
`claude/rewo-entity-fidelity-aa134c` (`e5bbb8b`), the ETF/emissive/dye work, is
**ported and shipped as M52** (renumbered, because its own `M36a`–`M36d` collide
with the M36 already on `main`, the inventory's player preview). It closes the
"ETF random/emissive textures (M9b)" item this plan listed as open. It did *not*
go in as a rebase: the merge-base was two commits after M11 and `main` had moved
109 commits, which gave 33 conflict hunks plus a compile wall the markers never
flag, so it was ported file-by-file instead. See the M52 entry in §15 for what
changed against the branch — and note it also **closed two of the branch's own
caveats** by decoding the wire inputs that did not exist at M11 (entity_event 61
for the warden's tendrils, and the metadata slots for the creaking's eyes and
the sheep's wool). Two of the branch's handoff notes about those slots were
wrong; §15 records the corrected indices. Nothing else is unlanded.

This closed on 2026-07-27, and it had been the project's largest non-code risk
for a long time: 78 commits, everything from M10 on, on a branch whose name
stopped describing its contents around M20. It went in as a clean
fast-forward — `main` had nothing the branch lacked — so nothing was rebased,
squashed or lost, and the per-milestone history in §15 still lines up
one-to-one with the commits. **New work branches from `main` now.** The
`codex/rewo-*` branches and `claude/fabric-client-ui-*` were redundant and have
since been deleted; `claude/rewo-entity-fidelity-*` is the one that was not (see
above).

**Latest (2026-07-28): the armour arc, M46–M49.** Mobs wear armour, it is
dyed, it is trimmed, and the trim shows on the icon.

- **M46 worn armour.** The layer follows the **renderer**, not the mesh: all
  eight `HumanoidArmorLayer` sites are the player/zombie/skeleton/piglin
  families, so an allay, an illager and a creaking each have enough humanoid
  mesh to pass a geometric test and wear nothing in vanilla. Only the *player*
  has a named `body` part (M19 gave it one), so every mob's chestplate torso
  box resolved to nothing — **and the witness passed anyway, because it asked
  the player model**.
- **M47 the leather dye.** Zero is not a black tint, it is "do not draw this
  layer" — that is the whole of `Layer.onlyIfDyed`. An undyed leather piece is
  **brown** (`LEATHER_COLOR`), not the greyscale its sheet is authored in.
- **M48 armour trims.** The sprites do not exist as files; a
  `paletted_permutations` source generates them. `assetId(equipmentAsset)` is
  what stops a same-material trim vanishing. Drawn depth-**EQUAL** over the
  armour, as a fourth vertex range.
- **M50 the worn-armour glint** — and the finding under it. Two of the facts
  gathered in advance were wrong: `VIEW_OFFSET_Z_LAYERING` is carried by *all
  three* armour render types, so it cancels within the stack and the foil is an
  ordinary depth-EQUAL pass; and the foil is **untinted**, because
  `RenderPipelines.GLINT` binds `POSITION_TEX`, which has no Color element.
  Then the real work: the foil rendered a byte-delta of **exactly 0**, because
  the glint's blend squares its input and **squaring is not invariant under the
  sRGB transfer function** — vanilla evaluates it in gamma space and Rewo was
  blending in linear. No fixed-function blend can bridge that (every candidate
  needs to read the destination), so the glint now renders through a **UNORM
  view of the same image**. The item glint had carried the same error since M43
  and hid it, because a dropped stack sits against a dark background where the
  sRGB curve is steep. **When an effect is provably drawn and provably
  invisible, ask what space its blend is in** — the arc's fourth "the whole
  story was one line of pipeline state".
- **M49 trims on GUI icons**, including the bake refactor: variants live under a
  composed `"<item>#<material id>"` key rather than the map's key becoming a
  pair. **The bug that hid the whole feature** was a depth comparison: a
  multi-layer sprite is coplanar by construction, and the GUI pipeline tested
  strict `GREATER`, so layer1 was rejected at exactly layer0's depth. Vanilla
  tests `LEQUAL` → reversed-Z `GREATER_OR_EQUAL`. **That was the third time in
  this arc a depth comparison was the whole story** — reach for it first when
  geometry is provably present and provably invisible.

**Verified at M51c:** 641 tests, zero failures; **eighteen** serverless gates
green with Vulkan validation ON and 0 VUIDs (`itemshot` 62, `inventoryshot` 91,
`blockentityshot` 172, `swingshot` 97, `hurtshot` 38, `weathershot` 35,
`handshot` 34, `particleshot` 34, `eventshot` 28, `danceshot` 24, `captureshot`
17, `portalshot` 12, `mobshot` 243/243, plus `skyshot`, `lightmapshot`,
`tintshot`, `meshshot` and `dimensioncheck`); demo PNG SHA-256 `2cc56b4a…`,
byte-identical since M15.

### What to do next

**§16 is history, not a forward plan** (M23–M25, all shipped). The live queue is
`REWO_FEATURE_SURVEY.md` §5 "Sequence", and items 2–5 of it are now **done**:
ETF (M57), tooltips (M54 data + M56 image pass), screenshots (M51), and the data
half of health bars (M55).

Two pieces are finished-but-for-one-step, and are the cheapest things to pick up:

- **The health bar's render half.** The spec is written
  (`REWO_HEALTH_BAR_SPEC.md`) and the data is decoded (M55 `update_attributes`,
  43 witnesses). What is missing is a `push_health_bar` sibling to
  `entities.rs::push_tag` — the nametag's backing plate is already a
  camera-billboarded untextured coloured rect, so this needs no new geometry
  type, texture, pipeline or blend state. **This is the first Rewo feature with
  no vanilla oracle**; read the spec's preamble before writing a witness.
- **The bundle grid's cell chrome.** M56 computes and grades every cell
  position but does not blit `container/bundle/slot_background`, the two
  highlights or the three `bundle_progressbar_*` sprites — they need fields on
  `assets::ContainerSprites`, and that agent was fenced out of `assets.rs` to
  keep it from racing the ETF port. Geometry is done; only the blits are absent.

**Survey item 1 — porting the EwoClient module + HUD set — is USER-GATED.** The
repo owner's instruction: it "must not go ahead without my explicit go ahead, it
has a couple caveats and it itself isn't fully finished and working, so we'd be
porting broken stuff." `ec531cd` landed a first slice of it from a different
session; that is not the same as the gate being lifted. **Ask before extending
it.**

After those, the survey's remaining ranked items are capes (larger than the
survey implies — see the M-series notes: the cape URL is *not* parsed today, and
`rotateBy` composes `Rx·Rz·Ry`, not Rewo's `rotate_zyx`) and then the Tier-2 set.


### How to run the live checks

The test server is `%APPDATA%/EwoClient/rewo/26.2/testserver-inv`, port
**25610**, opped player **RewoOp**. It **stops on stdin EOF**, so a backgrounded
`java` from a shell dies immediately; start it detached instead
(`Start-Process -WindowStyle Hidden`), and stop it when done. `REWO_PRECMD`
runs `/`-commands as that player on join and `REWO_SETTLE=<n>` holds the
session before the shot — together they make a scene reproducible in one frame,
which matters because **two live runs are not the same scene** (mobs move,
weather changes, spawn drifts).

**Before it (2026-07-27): M37 — particles.** The one milestone §16 refused to
propose, because every gate here is geometry-based and particles are stochastic
and time-driven. They are not: `Particle.tick()` contains no randomness at all,
every generator is `java.util.Random`'s LCG, and a fixed seed turns the whole
subsystem into assertable numbers. Two anchors keep that from being circular —
the JDK's own `Random` for the generator, and a Java harness of **verbatim
decompile source** for the physics, which caught `+ 0.1` where vanilla writes
`+ 0.1F` on its first run. `nextGaussian` is the one primitive graded to a ULP
bound instead of to the bit, because `Math.log` is a JIT intrinsic and vanilla's
own scatter is not bit-reproducible between two JVMs. Gate: **`rewo particleshot
--check`, 34/34**, mutation-tested. Merged to `main` as a fast-forward.

**Before it (2026-07-27): M36 — the player preview.** The black rectangle M35 left
in the middle of the inventory is `inventory.png`'s own window, and this fills
it. The transform composes `PictureInPictureRenderer.prepare` and
`GuiEntityRenderer.renderToTexture`; the step that is easy to miss is on the
*camera* — `orientation.rotateY(PI)` — and it is not decorative, because
`bodyRot = 180 + xAngle` already points the model away from an unturned camera.
The first build rendered Steve's back. The preview owns a **second**
`EntityPass` (two `set_draws` into one ring would cross the draws) built on
first open, and clears depth over its window (reversed-Z, so to 0.0) or the
model comes out sliced by the terrain behind the panel. Measuring beat
squinting again: the render looked too large and mispositioned, and the
measured feet/head matched the decompile exactly — the size was right and the
eye was wrong; what *was* wrong was the facing. `inventoryshot` 39 -> **44**.
Detail in §15.

**Before it (2026-07-27): M35 — the inventory screen.** The panel, all 46 slots,
the hover highlight, the stack on the cursor, and clicking. The click is a
*prediction*: the packet carries the client's own belief about every changed
slot, and the only thing that triggers a resynchronisation is
`packet.stateId() != menu.getStateId()` — the first live click failed on exactly
that, because the harness clicked while `/give` was still advancing the id.
`tools/gen_item_props.py` extracts the two per-item facts the arithmetic needs
(295 non-default stack sizes, 83 equippable slots), neither of which is on the
wire. `isHovering` is an **18x18** box, not 16x16, so slots tile without a dead
column; the hotbar row is a named `top + 58`, not three rows of 18. The one
honest approximation is `isSameItemSameComponents`: Rewo knows *whether* a stack
carried components, never what they were, so a patched stack swaps rather than
merging — one-directional by construction. The panel looked washed out with a
black hole in it; sampling showed six of seven probes byte-identical to
`inventory.png` (the seventh was the F3 overlay) and the black is the texture's
own, the window vanilla covers with the 3D player — which Rewo does not draw
yet, and which is the most visible remaining gap. `rewo inventoryshot --check`
16 → **39**. Detail in §15.

**Before it (2026-07-27): M34 — the inventory, and icons in the hotbar.** The
client now knows what it is carrying and draws it. Two coordinate systems meet
here and never line up: the wire's 46 **menu slots** (hotbar from 36, offhand
45) against the game's **inventory indices** (hotbar 0..8) — and the three
packets are split across them, `container_set_*` speaking the first and
`set_held_slot` the second. Three decode rules are not obvious: an
out-of-range held slot is **ignored, not clamped**; `container_set_slot` carries
its index as a **signed short** among var-ints; and any container id but 0 is
an open screen this client does not have, so it is dropped whole. The icons
needed `display.gui` — absent for a sprite, which is *correct* (identity maps
0..16 model units onto exactly the 16 px slot), and `scale 0.625` +
`rotation [30, 225, 0]` for a block, which reaches 8.37 px against the slot's
8. Lighting is a third model, neither the world's `Direction` shade nor the
hand's. Building the gate found two bugs first — `init_gui_items` leaked an
image, sampler and pipeline per hotbar change, and the atlas was repacked every
frame — then the gate's own first measurement counted "non-black" pixels
against a painted sky and measured exactly zero while the PNG showed both icons
rendering perfectly. One witness also had its reasoning backwards ("a sprite
covers more of its slot than a block" — a sword is mostly transparent); it was
replaced by a mutation that renders the same block with an identity transform.
`rewo inventoryshot --check`, **16/16**. Detail in §15.

**Before it (2026-07-27): M33 — weather and clouds.** Rain, snow and a cloud deck.
Three facts read backwards until checked: **`START_RAINING` sets the rain level
to 0 and `STOP_RAINING` to 1** (the names describe the server's transition; the
client sets the value its ramp starts *from*); the client **does not
interpolate** the level at all (`setRainLevel` writes both slots, so
`getRainLevel` lerps between identical numbers — the smoothing is server-side);
and clouds are absent **by attribute, not by dimension check** (`CLOUD_COLOR`
defaults to a transparent 0 and the pass is skipped on zero alpha, which is how
the Nether and End have none). A cloud is not a texture — `clouds.png` is a map,
one texel per 12×12×4 cell, and the mesh is three bytes per quad the vertex
shader expands. Weather needed `MOTION_BLOCKING`, which Rewo had been decoding
and discarding. A `weathershot` witness caught a wrong front-face convention
that **looked right from below alone**, which is why it grades a cloud deck from
both sides. Wired into `rewo live`, where the first frame exposed that vanilla's
weather geometry is **camera-relative** while Rewo's `view_proj` already carries
the camera — the gate had missed it by rendering at the origin, and now renders
2,500 blocks away.

**M33b then found the sky was greying through the wrong mechanism entirely.**
`applyWeatherDarken` is real but secondary; 26.2 puts weather's visuals in the
**environment attribute system** (`WeatherAttributes`), whose RAIN/THUNDER
layers rewrite SKY_COLOR (`BLEND_TO_GRAY`), FOG_COLOR, CLOUD_COLOR, the three
SKY_LIGHT_* attributes and STAR_BRIGHTNESS before any renderer reads them. That
also corrected two earlier claims: the lightmap **does** darken in rain, and
stars are **removed** rather than dimmed. The rain fog ramp shipped with it,
which needed a second, *environmental* fog band in the world pass. Gate:
`rewo weathershot --check`, **35/35**, plus four fog witnesses in
`lightmapshot`.

**Earlier (2026-07-26): M27/M28 — sign text, and the invisible block entities.**
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
hunting threshold**, so its eye opens exactly when the frame is complete. **M31 then mounted the spawner's caged mob** — which
belonged in the *entity* path all along, positioned by a mount affine rather
than by the world. **M32 then wrote that shader** — it samples in
**screen space**, which is why it needed a pipeline of its own. **M32b then
graded its pixels**, closing the read-back gap M32 recorded: `rewo portalshot
--check`, **12/12**, validation ON, 0 VUIDs. Uniform textures collapse the
fifteen layer matrices so the frame becomes a number the CPU can compute
outright; one layer then isolates a single sample and makes the column-major
matrix directly observable — mutating the shipped shader to the transposed
multiply drops that witness 21/21 → 9/21 while every other witness still
passes. Its first `v7` asserted that moving the camera changes the pixels; it
does not, and the corrected witness is that a screen-covering portal is
**byte-identical** under camera and world motion. **Every block-entity item
from M25's list is now closed.** See §15.

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

### The current numbers (2026-07-27; test/gate counts re-measured at M57, 2026-07-29)

Per-milestone figures inside §15 are the measurement taken at that milestone
and will not match.

**Re-measured at M57 (2026-07-29):** **705 tests** — `rewo-world` 208,
`rewo-net` 141, `rewo-gpu` 103, `rewo-data` 93, `rewo-mesh` 38, `rewo-proto` 11,
app 65. **Twenty gate invocations green, 0 VUIDs**, adding `mobshot
--emissive-check` 5/5, `--etf-check` 8/8 and `--tint-check` 4/4 to the seventeen
below; `itemshot` is 62/62 and `inventoryshot` 91/91 as of the M40–M50 arc. The
list that follows is the 2026-07-27 snapshot and is left as written.

- **623 tests** — 562 lib + 61 app. By crate: `rewo-world` 208, `rewo-net` 136,
  `rewo-gpu` 97, `rewo-data` 74, `rewo-mesh` 38, `rewo-proto` 11, app 61.
- **Seventeen serverless gates**, all green with Vulkan validation ON and
  **0 VUIDs**: `mobshot` 243/243, `blockentityshot` 172/172, `swingshot` 97/97,
  `inventoryshot` 91/91, `hurtshot` 38/38, `weathershot` 35/35, `particleshot`
  34/34, `eventshot` 28/28, `itemshot` 37/37, `danceshot` 24/24, `handshot`
  34/34, `portalshot` 12/12, plus `skyshot`, `lightmapshot`, `tintshot`,
  `meshshot` and `dimensioncheck` (which report pass/fail rather than a witness
  count).
- **Live gates**: `play --light-check --no-relight` 884,736 cells / 0
  mismatches both channels; `play --dimension-check` 4/4 checkpoints + 3/3
  transitions; physics **CORRECTIONS 0** over 600 ticks *on the paths the
  harness exercises* (M67's audit: it is never knocked back, exploded at or
  mounted, so `explode`/`set_entity_motion`/`move_vehicle`/`set_passengers`
  are outside what that number can test); build actions prove
  place == dirt and dig == air.
- **Canonical demo PNG** SHA-256
  `2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
  byte-identical from M15 through M38. Any change to it is a regression until
  argued otherwise.

### What to do next

Nothing is mid-flight — every milestone through M37 is shipped, gated and
merged. Three candidates, in the order I would take them:

1. **Worn armour** — not the glint, the armour. Rewo renders none at all, on
   any entity or on the inventory's player preview, which is why M45 could
   not ship `ARMOR_ENTITY_GLINT_TEXTURING`: there is no geometry to shimmer.
   The equipment slots are decoded already; what is missing is the armour
   models and their layer. Beyond that: the seven syncable components still
   without codecs (`equippable`, `can_place_on`, `can_break`,
   `blocks_attacks`, `jukebox_playable`, `kinetic_weapon`, `bees`), and
   armour trim *models*, which need the trim's material and pattern resolved
   to asset ids rather than merely walked past.
2. **The hand's remaining unknowns** — `SPEAR`'s use rig and the crossbow
   charge both need inputs the wire does not carry, and the arm still wears
   the default skin rather than the player's.
3. **Something from the survey** — [`REWO_FEATURE_SURVEY.md`](REWO_FEATURE_SURVEY.md)
   is the roadmap for picking the next *feature* rather than the next milestone.

~~Deliberately not next: **particles**~~ — **shipped as M37.** The approach that
unblocked it: vanilla's particle system is not actually stochastic, it is a
deterministic function of a seed, so a fixed seed turns the whole subsystem into
assertable numbers. Two anchors keep that honest — a real JVM for the LCG, and a
Java harness of *verbatim decompile source* for the physics, which is the only
thing that can catch a misreading rather than a mistranslation. See §15.

Still deliberately not next: **sound** (outside a renderer's scope).

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
  extended through M32, fail-closed **172/172**): a synthesised level-chunk
  payload through `read_level_chunk`, a `block_entity_data` body through the
  real dispatch, and `block_event` bodies through `route_block_event` into the
  chest, shulker, spawner and pot clocks. It also re-derives the jar's model gap
  every run, and grades the block-entity classification against what the model
  resolver actually draws — in both directions, so neither half can drift.
- `rewo inventoryshot --check` — **the inventory + screen + preview gate**
  (M34/M35/M36, fail-closed, **44/44**, validation required). Six witnesses drive synthetic
  `container_set_content` / `container_set_slot` / `set_held_slot` bodies
  through the production `route_inventory` (the menu-slot ↔ inventory-index
  conversion, the ignore-don't-clamp guard, the i16 index, the foreign
  container, the truncated list); four grade `display.gui` placement and the
  GUI diffuse on the CPU; six render real baked items into real hotbar slots
  offscreen and measure them as a **difference against the same scene with an
  empty draw list** — counting "non-black" pixels measures nothing, because the
  world pass paints a sky behind everything.
- `rewo particleshot --check` — **the particle gate** (M37, fail-closed
  **34/34**, serverless, CPU-only). Three layers. `w1`–`w10` drive raw
  `level_particles` / `level_event` bodies — hand-assembled from the decompiled
  *write* methods, not from a Rewo encoder, so a transposed field cannot
  round-trip — through the production decoders; they pin that `count` is a plain
  big-endian i32 rather than a VarInt, that `minecraft:block` alone carries an
  options payload, that unsupported types are dropped rather than guessed at,
  and that all 47 truncated prefixes decode to `None` without panicking.
  `f1`–`f13` grade the spawn fan-out, including the `count == 0` inversion (the
  `*_dist` fields become a **direction** and `max_speed` its magnitude) and the
  sprite sets against the jar's own `particles/*.json`. `s1`–`s11` assert seeded
  trajectories **bit-for-bit** against vectors emitted by a Java harness whose
  class bodies are copied *verbatim* from the decompile — the only anchor that
  can catch a misreading of vanilla rather than a mistranslation of it, and the
  one that caught `+ 0.1` where vanilla writes `+ 0.1F`. Mutation-tested against
  five deliberate breakages. It does **not** grade pixels: a particle quad is a
  camera-facing billboard the existing passes already cover.
- `rewo weathershot --check` — **the weather + cloud gate** (M33, fail-closed
  **27/27**, validation required, 0 VUIDs). Three layers: the `game_event` wire
  driven through `route_game_event` (including that **`START_RAINING` sets the
  level to 0 and `STOP_RAINING` to 1**), the precipitation rule against an
  independent transcription (threshold, height cutoff, the FROZEN patch noise),
  the cloud cell packing and ring-walk mesh, and then read-back pixels for both
  production passes. Its `g2` grades a solid cloud deck from **both** sides,
  which is what caught a wrong front-face convention that looked right from
  below alone. `--out-dir` dumps the frames.
- `rewo portalshot --check` — **the end-portal pixel gate** (M32b, fail-closed
  **12/12**, validation required, 0 VUIDs): renders the production
  end-portal/gateway pass offscreen and grades the read-back pixels. It never
  reproduces a layer matrix to predict the sum — **uniform synthetic textures
  collapse all fifteen**, so the frame is exactly
  `sky*COLORS[0] + portal*sum(COLORS[0..layers])` from an independent
  transcription of the constants. Then **one layer isolates one sample**, at
  which point the sampled `u` is an affine function of the screen UV alone and
  the column-major matrix is directly observable against its transpose. Also
  pins the screen-welded sampling (a screen-covering portal is byte-identical
  under camera and world motion) and the clock scroll. `--out-dir <dir>` dumps
  the frames.
- `rewo captureshot --check` — **the screenshot-capture gate** (M51c,
  fail-closed **17/17**, validation required, 0 VUIDs, and it needs neither a
  jar nor a server): the only thing in the suite that renders through a **BGRA**
  `Offscreen`, which is the format a live capture uses and which all sixteen
  other call sites take the RGBA default instead of. It grades both halves of
  the swizzle — the raw readback really is byte-permuted (`a2`, the fault) and
  the saved file is nevertheless red-first and byte-identical to the RGBA path
  (`a1`/`a3`, the correction) — so it can tell a missing swizzle from a spurious
  one. Also pins opacity, the row order (the "do not copy vanilla's vertical
  flip" rule, previously a comment), production `capture::grab` end to end, and
  vanilla's filename pattern and dedup ladder. `--out-dir <dir>` keeps the
  frames.
- `rewo swingshot --check` — **the combat-animation gate** (M19/M20,
  fail-closed **97/97**, serverless, CPU-only): the exact `LivingEntity` swing
  state machine, item-driven swing duration, the `ArmPose` hold baseline, and
  the undead / skeleton / illager attack rigs, each against an independent
  transcription.
- `rewo hurtshot --check` — **the damage-response gate** (M21, **38/38**,
  validation required): the hurt clock, the limb kick, and the red flash —
  verified by *predicting the hurt pixel from the unhurt one*, with sensitivity
  partners for linear-space mixing and post-lightmap application.
- `rewo itemshot --check` — **the held-item gate** (M22, **28/28**, validation
  required): both geometry paths (the sprite extrusion and the block bake)
  verified *against the hand* — a sprite centroid and a block centroid that land
  together prove one transform chain serves both sources.
- `rewo lightmapshot --check` — **the lightmap gate** (M13, validation
  required): a production Vulkan readback matrix over tint, block factor, gamma
  ramp, night vision, darkness, water parity and entity RGB. Extended by M33b
  with four **fog** witnesses that pin the `max` of the render-distance and
  environmental bands (mutation-verified against `min` and a sum).
- `rewo tintshot --check` — **the per-biome colour gate** (M14, validation
  required): grass/foliage/water tint and camera sky/fog against
  Temurin-verified vectors, over the production jar bake and synthetic biome
  containers.
- `rewo meshshot --check` — **the geometry gate** (M15): expands greedy
  rectangles back to reference unit faces and pins every direction / block /
  layer / light / AO / tint seam, plus exact model, water and lava controls.
- `rewo play --light-check [--no-relight]` — **the live light gate** (M10):
  recomputes the loaded columns and diffs against the server's own light engine.
  **884,736 cells, 0 mismatches**, both channels. Pass `--no-relight` or the
  engine grades its own incremental writes.
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
   (`rewo-data/src/lib.rs`, `rewo-gpu/src/entities.rs`, `rewo-world/src/chunk.rs`
   at least). An editor that normalises them turns a 30-line change into a
   3,400-line diff and trips `git diff --check`, since git reads the added CR
   as trailing whitespace. Check `git diff --stat` against what you meant to
   change.
   **New lines in such a file must go in as LF**, whatever the file's dominant
   ending — an *added* CRLF line trips `diff --check` every time, which is why
   these files are mixed at all: the CRLF is original, and every LF line is a
   previous session's addition. To repair a normalised file, realign it against
   `git show HEAD:<path>` and re-emit HEAD's exact bytes for equal lines,
   `content + b"\n"` for new ones. Python multi-line `str.replace` anchors
   silently no-op against CRLF text — a replacement that reports success and
   changes nothing is this, not a missing string.
10. **Never write these sources from a script without `encoding='utf-8'`.**
   Python's default on Windows is cp1252, which silently re-encodes a whole
   file and leaves it invalid UTF-8 — and the obvious repair then
   double-encodes every em-dash that was already correct, so the file has to be
   restored from HEAD. Pass `encoding='utf-8'` AND `newline=''` (see 9), or use
   the editor.
11. **GLSL `mat4(...)` is COLUMN-major, and vanilla's shader source is GLSL.**
   In `end_portal_layer` the translate's `17/layer` sits at `m[0][3]`, not
   `m[3][0]`, and it reaches the coordinate because the sampling is
   `texProj0 * matrix` — a **row-vector** multiply, `v * M == transpose(M) * v`.
   Copy such literals verbatim; "tidying" them into the slots they look like
   they belong in transposes every layer and still produces a plausible swirl.
   M32 nearly shipped exactly that, and `portalshot`'s `v10` is what would
   catch it (21/21 → 9/21 under the mutation).
12. **A projective screen-space sampler does not move with its geometry.**
   `texProj0 = projection_from_position(gl_Position)`, so the end portal's
   sampled texel is a function of the pixel's screen position alone. A
   screen-covering portal renders **byte-identically** whether you slide it
   through the world or roll the camera — the starfield is welded to the
   screen and swims against the world. The first `v7` asserted the opposite
   and failed; the intuition to distrust is "moving something must change
   what it looks like."

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
- **A mob can render with another mob's texture when more than one is in
  the scene** — OPEN, found 2026-07-28 during M46, **pre-existing** (a
  stashed pre-M46 build reproduces it exactly). Two zombies summoned side
  by side both rendered with a villager's brown head and magenta legs;
  a *single* zombie in the same spot rendered correctly. Ruled out: the
  atlas (the packed slot table is byte-identical with and without M46's
  fifteen armour sheets — `zombie -> (768, 512, 64, 64)` either way), and
  `skin_uv` (only set for players). Not yet ruled out: the per-entity draw
  ranges, or kind resolution for entities that stream in together.
  **`mobshot` is structurally blind to it** — its check substitutes
  per-face debug colours, so it verifies UV/face correspondence and cannot
  see the wrong *sheet* being sampled. A gate for this wants a
  real-texture witness (a known mob's known pixel colour), not a facelabel
  one. Repro: `REWO_PRECMD` two `summon zombie` at one spot, `REWO_SETTLE=13`.

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
  formulas, gated by `rewo danceshot --check` (§15). **M19/M20 then shipped the
  combat rigs** — `ClientboundAnimate` arm swings with the `ArmPose` hold
  baseline, and the undead / skeleton / illager attack rigs (`swingshot`), so
  that item is closed. Still open: the Warden tendril (event 61), creaking
  attack if its exact signal is still unclosed, and dragon flight (bespoke
  procedural code, not a rig — stays posed). Sheep wool dye-tint deferred (white),
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
- ~~**Sky is gradient + distance fog only**~~ — **RESOLVED M10–M14, M33 (§15)**:
  time-of-day sky/fog darkening (M11), the clear-weather celestials — sun,
  moon (all eight phases), stars, the sunrise fan (M12) — per-biome sky/fog
  (M14), and **clouds plus rain/snow and the full weather attribute stack**
  (M33/M33b). Nothing on this line is deferred any more.
- **HUD is crosshair + hotbar + hearts + hunger only** — no item icons in the
  hotbar slots, no XP bar, no armor/air, no effect icons, no
  gamemode-awareness (creative shows hearts/hunger). M22 built the item
  *models*, so the icons are now blocked on an **inventory model** rather than
  on geometry: `container_set_content` / `container_set_slot` / `set_held_slot`
  are all clientbound in 26.2 and have **zero references** in `rewo-net`, so
  nothing knows what is in the nine slots.
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

### 9.4 Velvet UI track — RESOLVED, and not the way this section planned

**Superseded 2026-07-28.** This section proposed Skia-on-Vulkan (`skia-safe`
with the `vulkan` feature wrapping Rewo's device/queue), and flagged a check
item about whether a prebuilt existed for that feature combo. **That question
is moot: the Velvet UI shipped as raw Vulkan.** No Skia, no interop, no
prebuilt risk, no LLVM source build. See `REWO_VELVET_UI_PLAN.md`.

What landed instead, in `crates/rewo-gpu/`:

- `velvet_glyph.rs` — variable-font rasterization via **`swash`** into a
  shelf-packed R8 coverage atlas, keyed by a quantized (family, size, axes)
  tuple. Blurred shadow glyphs live in the same cache under the same key.
- `velvet_text.rs` — the glyph quads, ring-buffered like `TextPass`.
- `velvet_chrome.rs` + `shaders/velvet_chrome.frag` — the glass plate as **one
  instanced quad and one fragment shader over a rounded-box SDF**. Skia's six
  `draw_rrect` calls collapse because a mask blur over a rounded rect is a
  smoothstep over the distance field; no blur pass, no ping-pong target.
- `velvet_widgets.rs` — one widget (Coords), as a proof the three compose.
- `rewo hudshot --check` — 41 witnesses, mutation-verified.

**Why raw Vulkan turned out to be the cheaper answer.** The thing that looked
hardest — `ewo-render`'s `liquid_glass.rs` — is not Skia logic at all. It is an
SDF fragment shader that already avoids `fwidth`/screen-space derivatives
(SkSL runtime effects cannot rely on them), which is exactly the constraint
that makes it drop into GLSL unchanged. The genuinely hard part was the *text*
stack, and that would have been hard under either approach.

**The one renderer-level constraint that outlives any redesign:** the Velvet
passes must be constructed with `world::unorm_of(target_format)` and drawn
inside `WorldRenderer::with_gamma_space`. EwoClient's `rgba()` is a plain
`/255` with no transfer function, so Skia composites in **gamma** space while
an sRGB attachment blends in **linear**. The half that actually bites is the
pipeline format — Vulkan requires it to match the attachment, so a pass built
against sRGB and drawn in that scope is a validation error rather than a
subtle colour shift.

**Scope is deliberately frozen past one widget.** EwoClient's HUD is getting a
visual overhaul, so the widget transcription stopped at Coords and the in-game
editor was not started; the chrome palette is de-baked into a `ShellStyle`
table so a redesign is a data edit rather than a shader edit.
`REWO_VELVET_UI_PLAN.md` §8/§9 records what is safe to resume during the
freeze (anything with no visual coupling) and what waits for the new design.

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
- **D14** Vanilla-asset HUD first — held. The Velvet layer that followed is
  **raw Vulkan, not Skia-Vulkan**; see §9.4, which this decision predates.
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

## 16. Forward plan — M23 to M25 (SHIPPED; this section is now history)

*(Placed before §15 deliberately: the status log is append-only and must stay
last. This section was the forward plan; when a milestone ships, its record goes
in §15 and the entry here becomes history. **Everything in §16 has shipped** —
M23 through M25 and every block-entity item that followed. There is no live
forward plan in this file; §0.0's "what to do next" and
[`REWO_FEATURE_SURVEY.md`](REWO_FEATURE_SURVEY.md) are where the next thing
comes from.)*

> **Status, 2026-07-27 — all three shipped, with two scope corrections.**
> M23 `d080ba3`, M24 `0f5d988`, M25 `88f4117`, on
> `codex/rewo-m19-combat-swings`, pushed with the rest of the branch.
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

### Deliberately not proposed (as of §16's writing)

- **Particles** — a whole subsystem, and every existing gate is geometry-based;
  it would need a new verification approach before it could be shipped honestly.
  **Still true.**
- **Sound** — outside a renderer's scope. **Still true.**
- **First-person hand / GUI** — needs an inventory model Rewo does not have.
  **Still true, and now the single largest gap** — see §0.0.
- ~~**Weather and clouds**~~ — called filler here, **shipped as M33/M33b**
  (§15). It stopped being filler once M12's celestials made "clear weather
  only" the last thing missing from the sky.

---

## 15. Status log

*Append-only. Two things inside these entries are **as of the moment they were
written** and are not maintained: the "not pushed / reviewed local" notes (all
of M0–M33b is pushed now — see §0.0), and the per-milestone test and gate
counts, which are the measurement taken at that milestone rather than the
current total. Both are left as written on purpose: rewriting them would
falsify the record of what was actually measured when. §0.0 carries the current
numbers.*

### M77 — the minecart's own interpolation, the leash holder, the projectile's power (2026-07-29)

Three class-A rows out of `REWO_PACKET_COVERAGE.md`: `move_minecart_along_track`
(55), `set_entity_link` (100), `projectile_power` (135). Two are small. The
first is a whole second interpolation model, and answering "how does it compose
with the one Rewo already has" is what this milestone is actually about.

#### The composition answer: it overrides the generic lerp at the render seam and leaves it running

The question was framed as *replace or feed*. The decompile says **neither**,
and it says so at four separate places that all have to agree:

1. **`ServerEntity.sendChanges` has an `else if` for a minecart.** An
   `AbstractMinecart` whose behaviour is `NewMinecartBehavior` goes to
   `handleMinecartPosRot`, which **replaces the entire generic position
   branch** — so such a cart is never sent `move_entity_pos`,
   `teleport_entity` or `entity_position_sync` at all. `move_minecart_along_track`
   is its only movement channel, and even a cart at rest gets one (a single
   `weight 1.0` step at the current position).
2. **`AbstractMinecart.getInterpolation()` returns null for it.** It forwards to
   `behavior.getInterpolation()`, and `NewMinecartBehavior` does not override
   `MinecartBehavior`'s `return null` — `OldMinecartBehavior` is the half that
   owns a real `InterpolationHandler`. So a stray positional packet would take
   `Entity.moveOrInterpolateTo`'s null branch and **snap**.
3. **The schedule's per-tick write is an ordinary `setPos`.** So `xOld →
   getX()` keeps tracking it and `EntityRenderer.extractRenderState` still
   produces `Mth.lerp(partialTicks, xOld, getX())` — the tick-quantised chord
   between two consecutive schedule samples.
4. **`AbstractMinecartRenderer.newExtractState` then overrides that**, with
   `getCartLerpPosition(partialTicks)` — the same schedule at the true partial
   tick — but only `if (behavior.cartHasPosRotLerp())`.

And vanilla **measures one against the other**: a passenger of a lerping cart
gets `state.passengerOffset = getCartLerpPosition(partialTicks) -
lerp(partialTicks, xOld, getX())`, which is literally the difference between
the schedule and the generic lerp. Both are live by construction.

This is the mirror image of M72's passenger finding. There a rider's own
three-step lerp is computed and **thrown away** by `positionRider`. Here the
generic lerp is computed, kept, and is the baseline the finer sample is
measured against.

In Rewo that lands as: `EntityTable::tick_minecarts` writes the schedule's
`sample(1.0)` through `set_derived_pos` — the same writer `positionRider` uses,
which moves *this* tick's position without touching `prev` or the synced target
— and `EntityTable::minecart_render(id, alpha)` is `newExtractState`, `Some`
only while `cartHasPosRotLerp()`. Two consequences the gate pins: the two agree
**exactly at alpha 1.0** (that is the sample the tick wrote), and the synced
target `x/y/z` never moves at all, so the generic three-step lerp is never armed.

**They only disagree across a step boundary.** Within one step index the
schedule's `indexedPartialTick` is affine in the partial tick, so a
single-step segment makes the schedule and the chord *identical at every
alpha* — a witness built on one would have witnessed nothing. `rideshot`'s
fixture is therefore an L-shaped two-step segment with weights `3` and `1`,
which puts the boundary two thirds of the way through the third tick and makes
the disagreement 1.76 blocks.

#### The rest of the schedule, transcribed

`crates/rewo-world/src/minecart.rs` is the client half of
`NewMinecartBehavior` and nothing else (the server half — `moveAlongTrack`,
`adjustToRails`, the rail-shape speed maths — never runs on a client). Details
that invert if assumed:

- **The countdown is `3` and the alpha's divisor is a separate literal `3`.**
  `alpha = (3 - lerpDelay + partialTick) / 3.0F` with `lerpDelay` set to
  `POS_ROT_LERP_TICKS` at ingest, pre-decremented every tick — so a segment is
  traversed in exactly three ticks and lands *on* its last step.
- **A weight of 0 is not "no movement", it is a snap.**
  `adjustToRails(…, instant = true)` emits one; the index search skips every
  `weight <= 0` step and the `!foundIndex` fallback selects the **last** step at
  `indexedPartialTick = 1.0`. A whole segment of zero weight also sets
  `lerpDelay = 0`, so the next tick re-ingests immediately.
- **The arithmetic mixes widths on purpose.** `alpha` and `countUp` are
  `float`, `currentLerpStepsTotalWeight` is a `double`; the threshold
  comparison and the in-step fraction both evaluate in `f64` with the floats
  widened, and only the fraction narrows back. Doing it all in one width drifts.
- **`currentLerpStepsTotalWeight` is reset inside the non-empty branch only.**
  With an empty inbox the stale total survives — unread, because
  `cartHasPosRotLerp()` is false the moment `currentLerpSteps` is empty.
- The packet is an **append** (`lerpSteps.addAll`), so two packets between two
  client ticks are one segment.

**The wire has both of this project's paid-for traps in one packet.** The
vectors are `Vec3.STREAM_CODEC` — three plain `f64`s, 24 bytes — and **not** the
`LP_STREAM_CODEC` bit-packing `set_entity_motion` uses (M68); the two codecs sit
on the same `Vec3` class. And a rotation is one **signed byte** through
`Mth.unpackDegrees` (`rot * 360 / 256f`). One deliberate divergence: vanilla's
`readCount` accepts a **negative** count and its loop then yields an empty list,
where Rewo's hardened `PacketReader::count` rejects it — both mutate nothing.

#### `set_entity_link` and `projectile_power`

`set_entity_link` is **two fixed big-endian i32s** (`readInt()` twice) — the
same shape `entity_event`'s id has (M17) and the opposite of nearly every other
entity-addressed packet. `destId == 0` is the wire's null. The handler is
`setDelayedLeashHolderId`, whose body is `setLeashData(new LeashData(entityId))`
then `dropLeash(this, false, false)` — and **that drop is a no-op by
construction**, because it is guarded on `leashData.leashHolder != null` and the
data it reads is the one just installed, whose resolved holder is null. So the
whole handler is one assignment, *including for 0*, which installs a leash
record holding nothing rather than clearing the record. `getLeashHolder` is then
a lazy two-step: `delayedLeashHolderId != 0 && level.getEntity(id) != null`.
**The rope is not drawn** — that is class B and untouched.

`projectile_power` is a VarInt id (unlike `set_entity_link`'s fixed i32, one
packet away) then an `f64`, written onto `AbstractHurtingProjectile` and nothing
else. The cast is worth naming: **an arrow is not one**. It is an
`AbstractArrow` on a sibling branch, so a `projectile_power` naming one mutates
nothing. Six types pass — the fireball family, the wither skull, the two wind
charges.

#### `Leashable` is an interface, so the class table grew a second kind of set

All three packets gate on a Java class the wire cannot carry, which is
`tools/gen_entity_classes.py`'s job. `ABSTRACT_MINECART` already existed (M72);
`ABSTRACT_HURTING_PROJECTILE` is one more `ANCESTRY_SETS` row. `Leashable` is
not a class, and the `extends` walker cannot see it — so the generator gained
**`INTERFACE_SETS`**: it scans every `implements` clause, **asserts the set of
declaring classes** (exactly `Mob` and `AbstractBoat` in 26.2), asserts that no
interface *widens* `Leashable` (which would leave the union short), and then
unions those classes' `extends` subtrees. Pinning the implementor set is the
point: if 26.3 makes `AbstractMinecart` leashable, the generator stops instead
of shipping a set that is one subtree short. `EntityClasses::resolve` adds two
invariants on top — every mob must be leashable and the leashable set must be
strictly larger (the boats are the difference), and a hurting projectile must
never be living. All four fail-closed paths were mutation-run.

#### One guard that is genuinely un-mutable alone, and said so

`push_minecart_steps` refuses an untracked id, and so does the net-side
`entities.get(eid)` lookup. Dropping either **alone** leaves the gate green.
The net-side one is load-bearing (it is also where the type id for the class
gate comes from, so it cannot be removed in isolation); the world-side
`contains_key` is a belt, and `m6`'s detail string says exactly that — the same
status `rideshot`'s `r4.a_rider_that_moved_on` gives `position_riders`' own
`vehicle_of` re-check.

#### Two witnesses were wrong first, and the mutations are what said so

- **`m4` measured the rotation's sign through `rotLerp`, which erases it.**
  `Mth.rotLerp` is `from + a * wrapDegrees(to - from)`, and `wrapDegrees`
  normalises into (-180, 180] — so an off-by-360 from reading the byte
  *unsigned* is destroyed by the very next operation, and the mutation left the
  gate green. The witness now asserts the **decoded step**, where the mutation
  bites, and separately asserts the entity write (which has its own mutation:
  drop `e.yaw = sample.y_rot`).
- **`m15` tested `remove` then `add`, and the two clears covered for each
  other.** Dropping either one alone left the gate green, because the other
  still ran. It now has three clauses — `remove` alone, `remove` then `add`, and
  a bare `add` over a live schedule (the dropped-`remove_entities` case the
  `add` clear exists for) — and each of the two mutations reddens exactly one.

#### Gate and measurements

`rewo rideshot --check` grew from **24 to 45 witnesses** — extended rather than
given its own command, because its three existing concerns (a derived position,
an entity→entity relation, and what wins over `render_pos`) are exactly these
three packets'. Serverless, CPU-only, fail-closed on the count.

**25 mutations run in total**: 20 against the Rust (19 predicted red and
observed red, 1 predicted green and observed green — the belt above), 3 against
the generator's fail-loud checks, 2 against `EntityClasses::resolve`'s
invariants. Two of the original 20 came back green and were the two witness
defects above; after the rework both, plus their two new partners, are red.

**Measured.** **1333 tests** (was 1323: `rewo-world` +6 in the new `minecart`
module, `rewo-net` +4 in `m77_wire_tests` — the two things `rideshot` cannot
reach, because no encoder produces a **negative** element count and the gate
samples two rotation bytes rather than all 256). Every gate green with
validation ON and **0 VUIDs**: rideshot **45**, abilityshot 47, labelshot 47,
capeshot 69, itemshot 62, inventoryshot 152, healthbarshot 33, attributeshot 43,
captureshot 17, blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35,
handshot 34, particleshot 34, eventshot 28, danceshot 24, portalshot 12,
hudshot, mobshot 246/246 + its four sub-checks, skyshot, lightmapshot, tintshot,
meshshot, dimensioncheck. Release `demo` PNG SHA-256 byte-identical to M15
onward (`2cc56b4a…`). `git diff --check` clean; every file touched was already
pure LF and stayed that way.

**Not verified:** no live server run, and nothing is drawn. `rewo live` never
calls `minecart_render`, so a cart still renders at its generic position — the
schedule is state, and wiring the render seam is one call site in
`live_cmd.rs`'s entity loop (recorded here rather than half-started, because
Rewo draws minecarts as capsules and the change is unverifiable headlessly).
Three scoped exclusions, all in the coverage doc: the handler's second guard
(`getBehavior() instanceof NewMinecartBehavior`) needs the
`minecart_improvements` feature flag, which needs `update_enabled_features` — a
**configuration** packet, outside that survey's scope; the leash rope is class
B; and `getLeashHolder`'s *cache* is not modelled, so a holder that leaves the
tracking range reads as no holder where vanilla keeps the stale reference.
### M78 — session, server metadata and chat: the eight class-A packets (2026-07-29)

Eight packets picked as one layer rather than eight rows: everything the
connection itself knows about the session it is in. Seven are a reader plus a
field (`crates/rewo-net/src/session.rs`); the eighth changes how packets are
*applied* and gets its own module (`crates/rewo-net/src/bundle.rs`), the split
`client_state.rs` states.

| id | packet | body |
|---:|---|---|
| 0 | `bundle_delimiter` | empty |
| 24 | `custom_payload` | `Identifier` + a payload chosen by it |
| 33 | `disguised_chat` | `Component` + `ChatType.Bound` |
| 39 | `game_rule_values` | counted map, `Identifier` → string |
| 66 | `player_combat_end` | one VarInt |
| 67 | `player_combat_enter` | **empty** |
| 86 | `server_data` | `Component` + `Optional<byte[]>` |
| 120 | `store_cookie` | `Identifier` + `byteArray(5120)` |

#### The bundle: an empty body that is not an inert packet

`ClientboundBundlePacket` never serialises. `PacketBundleUnpacker` expands it on
the sending side into `delimiter, sub-packets…, delimiter` and
`PacketBundlePacker` reassembles it on the receiving one, so the delimiter is a
**pipeline instruction** — `BundleDelimiterPacket.handle` throws
`AssertionError("This packet should be handled by pipeline")` if it ever reaches
a listener. Decoding it as a no-op is the one way of being wrong that leaves no
trace at all.

Four rules, from `PacketBundlePacker.decode` and `BundlerInfo`:

1. **Applied all at once, and only on close.** `handleBundlePacket` is a plain
   `for` loop calling `subPacket.handle` directly, on one scheduled task whose
   `ensureRunningOnSameThread` the sub-handlers then satisfy for free.
   **`REWO_PACKET_COVERAGE.md` §3 said "in one tick" and that was wrong** — the
   guarantee is that no *frame* renders part-way through; nothing defers a
   bundle to a tick boundary. The distinction matters because the tick reading
   suggests an implementation vanilla does not have.
2. **An unterminated bundle is withheld, not dropped and not applied.**
   `currentBundler` stays non-null across `decode` calls. This is the property
   that makes bundling worth having in Rewo: `drain_inbound` is `try_recv` until
   `Empty`, so a socket that hands over a bundle in two reads would otherwise
   apply the first half a frame early — the `add_entity` without its
   `set_entity_data`, i.e. a mob drawn for a frame as a nameless, unequipped,
   default-metadata version of itself.
3. **There is no nesting.** `Bundler.addPacket` opens with
   `if (packet == delimiterPacket) return constructor.apply(bundlePackets)`, so
   a second delimiter *always* closes and a third opens a fresh run. A depth
   counter — the natural implementation — never closes the outer bundle and
   withholds every packet after it for the rest of the session.
4. **The size limit is an error, not a cap.** `BUNDLE_SIZE_LIMIT` is 4096 and
   the check runs *before* the add (`if (size() >= 4096) throw`), so 4096
   sub-packets fit and the 4097th kills the connection; neither delimiter
   counts. Moving that check above the delimiter test — which reads like a
   tidy-up — makes a legitimately full bundle fatal **at the moment it correctly
   closed**, i.e. only on the servers that send large bundles.

Plus `verifyNonTerminalPacket`: a terminal packet inside a bundle is a
`DecoderException`, and in clientbound-play there is exactly one
(`start_configuration`). Rewo's `PlaySession` does not dispatch that packet, so
the other half of vanilla's terminal handling — removing the bundling stage from
the pipeline once one passes through *outside* a bundle — has nothing to model.

Wired around the existing dispatch rather than into it: `feed` sits between the
frame decoder and `handle_packet`, exactly where `PacketBundlePacker` sits in
vanilla's pipeline, and the `else if` ladder never learns bundles exist.

#### What `store_cookie` changes about the `cookie_request` reply

`handleRequestCookie` is
`send(new ServerboundCookieResponsePacket(key, serverCookies.get(key)))` — a
straight jar lookup where `null` is the miss. Rewo already answered
`cookie_request`; it answered **`false` unconditionally**, because
`store_cookie` is the only thing that ever calls `put` and it was not resolved.
After M78 the reply carries the stored payload for a key the server has set and
is byte-identical for one it has not. On a network that uses `transfer` between
backends that is the difference between a session that survives a hop and one
that forgets itself on every one.

The 5120-byte limit is an **error**, not a truncation:
`FriendlyByteBuf.readByteArray(input, maxSize)` throws before copying a byte. A
client that clamped would store a cookie it would later hand back to a server
that never issued it.

**"Rewo already answered `cookie_request`" was half true, and closing that is
part of M78.** Only `Connection::run_play` — the M1-era harness behind `rewo
net` / `rewo view` — had an arm for the *play-state* request; `PlaySession`,
the loop `rewo play` and `rewo live` actually run, had none, so the client that
matters never replied at all. The jar is observable **only** through that reply,
so shipping `store_cookie` without it would have been a jar nothing reads.
`PlaySession` gained the arm and both loops now write through the same
`session::write_cookie_response`.

That is a new *shape* of §4 partial, and the coverage document now says so: the
machine check asks whether *anything* in `rewo-net` dispatches a field, and a
packet handled in **one of the two play loops** reads as fully handled. Nothing
mechanical distinguishes them.

#### Two ids, not one — M69's finding, one packet over

`custom_payload` and `store_cookie` are `common` packets and exist in
**configuration** as well (ids 1 and 10). For the payload that is not a nicety:
`ServerConfigurationPacketListenerImpl` sends
`new BrandPayload(getServerModName())` from its opening burst and a vanilla
server **never sends a second one in play**, while `serverBrand` lives on the
common listener both states extend. A play-only implementation would read no
brand from any server that exists. Both configuration ids are resolved, and both
states route through the same `session::apply`; `Connection::into_play` moves
the jar and the brand across exactly as it moves M69's tags.

This is the second time a play-scoped survey has named the id a vanilla server
does *not* send. `REWO_PACKET_COVERAGE.md` §6 already records the scope limit;
M78 is the second instance of it biting.

#### Three more things that read backwards

- **`custom_payload`'s unknown identifier is discarded, not rejected**, and the
  fallback **consumes the remainder** (`buf.readableBytes()`, throwing only
  above 1 048 576). The instinct is M41's — an untranscribed member of a
  discriminated union is fatal because the reader cannot skip it — and it is
  wrong here precisely because this union *has* a fallback codec. Rejecting
  would kill the connection to any modded server. (Vanilla goes further:
  `handleCustomPayload` returns for a `DiscardedPayload` before
  `ensureRunningOnSameThread` and does not even log. The
  `handleUnknownCustomPayload` warn one layer down is dead code on this path,
  because the only registered type is the one that is handled.)
- **`disguised_chat`'s chat type is `ByteBufCodecs.holder`, not
  `holderRegistry`.** `0` means an inline `ChatType` follows — two whole
  `ChatTypeDecoration`s, each a string, a counted list of VarInt parameters and
  an NBT `Style` — rather than chat type 0. A raw reading takes the first
  decoration's translation key as the sender's name and every field after it
  with it. M16 records the *opposite* convention for the dimension holder and
  M65 found two enum conventions one field apart; this is the same hazard inside
  one three-field record.
- **The two vestigial packets are not the same shape.** Both handlers are `{}`,
  but `player_combat_enter` is `StreamCodec.unit` (zero bytes) while
  `player_combat_end` carries a VarInt `duration`. **Nothing is stored for
  either**, because vanilla stores nothing and inventing a field would be a
  divergence dressed as decode-and-state — which leaves **reader position as the
  only gradeable property**, and is why both readers return the bytes they
  consumed. With no observable state, a reader one byte off is indistinguishable
  from a correct one right up until it desynchronises whatever follows.

#### What is decoded and deliberately not rendered

`disguised_chat`'s visible line in vanilla is `boundChatType.decorate(message)`,
a `Component.translatable` over the chat type's `ChatTypeDecoration` — so `/msg`
should read "X whispers to you: …". Decorating needs the
`minecraft:chat_type` registry (which `parse_registry_data` does not capture)
and a language table `rewo-net` cannot see, so Rewo appends the raw message.
That is **exactly the fidelity `player_chat` has had since M7**, and the two now
match rather than one being silently better. `server_data`'s icon is bytes:
vanilla's `validateIcon` is a PNG parse capped at 1024², and there is no PNG
decoder in `rewo-net`. `game_rule_values` is kept wholesale — vanilla has **no
store at all** (the map goes to a screen if one is open and nowhere otherwise,
and the screen ignores every packet after its first), so replacement is the
reading rather than merge.

#### The gate

`rewo abilityshot --check` grew two sections (47 → **67** witnesses) rather than
M78 minting a 26th `*shot` command. It is the right host by construction: it is
already the serverless CPU-only oracle that resolves **real packet ids through
`Ids::resolve` on the pinned version's datagen report**, which is what these
eight need on top of pure decode, and M75's subject is the adjacent one. The
seven session decodes are driven through the production `rewo_net::route_session`
seam with a real `Ids` — the M45/M47 rule that a gate reimplementing a slice of
the app's setup misses whatever the app adds to it — and the bundle machine is
driven with the **resolved** delimiter and `start_configuration` ids.

**27 mutations run, 27 caught.** Four things the battery turned up:

- **A gate witness that did not sit where its mutation bites.**
  `w9.a_malformed_session_body_changes_nothing` started from a
  `SessionState::default()`, so an `apply` mutated to clear `game_rules`
  *before* decoding left an already-empty map empty and the gate stayed green —
  the unit test, which seeds a rule first, was the only thing that caught it.
  Fixed by pre-populating the state through the real router before the malformed
  bodies. The strengthened witness then caught a **further** mutation the unit
  tests cannot (clearing `server_brand` before its decode), because the unit
  test's fixture has no brand set either. Same lesson twice in one witness:
  *"changed nothing" is only assertable about state that was something.*
- **Two properties are unit-test-only, structurally.** `unwrap_or` on the
  terminal id (which makes every packet terminal when `start_configuration` does
  not resolve) and a `take` that panics outside a flush are invisible to the
  gate, because the gate always has a resolved terminal id and never
  mis-sequences `take`. So is `apply` returning `true` on a decode error —
  `route_session` discards that return by design, so no caller can observe it.
  Recorded rather than papered over: those three live in `rewo-net`'s tests by
  necessity, not preference.
- **Three properties are gate-only.** Resolving the configuration
  `custom_payload` to the play id, the pre-M78 unconditional-`false` cookie
  reply, and a `route_session` that claims every id are all invisible to the
  unit tests and caught only by the gate, which is what earns it its place next
  to them.
- **The mutation partner originally written for
  `w8.only_the_resolved_delimiter_opens_a_bundle` was unreachable**, because
  `bundle_delimiter` *is* 0 in 26.2 — so "hard-code `0` instead of taking the id
  from `Ids`" is a no-op. That is the self-calibrating-witness failure mode
  §0.0 warns about, one step removed: the property was real and the named
  partner could not fail. Replaced with two that can (shifting the comparison by
  one, and opening on every packet), and the witness feeds a non-delimiter id so
  the second is visible.

One more thing the gate caught on its own: `EXPECTED_WITNESSES` was set to 66 by
hand and the run observed 67, so the fail-closed count rejected it. The
mechanism works on the person adding witnesses, which is the case it exists for.

#### The live run, and the two things only it could say

`bundle_delimiter` changes packet *application*, so this milestone owes a live
session. `rewo play --host 127.0.0.1 --port 25610 --username RewoOp --seconds 40
--setup "summon minecraft:cow …;summon minecraft:pig …"` against the bundled
26.2 server: **`CORRECTIONS: 0`** over 800 ticks, 329 columns, place and dig both
server-observed (`ACCEPT place … minecraft:dirt`, `ACCEPT dig … air`).

**`CORRECTIONS 0` on its own would have proved nothing about bundling** — it is
equally true of a bundle machine that never fired, and the first run had no way
to tell the two apart. So `BundleAssembler` counts closed runs and `rewo play`
prints them:

```
[rewo-m3] bundles applied: 177  (largest run: 3 sub-packets)
```

177 bundles in 40 seconds against a stock vanilla server, largest run 3 — which
is the entity-spawn shape §3 describes (`add_entity` + `set_entity_data` +
`update_attributes`). The path is exercised, not merely unbroken.

The second live finding is the one the whole two-ids argument rests on. With
`RUST_LOG=rewo_net=debug`:

```
DEBUG rewo_net::session] net: server brand "vanilla"
```

That line comes from the **configuration** arm. A vanilla server sends no
`custom_payload` in play at all, so a play-only implementation would have logged
nothing here and looked exactly like a server that declines to identify itself.
The argument was read out of `ServerConfigurationPacketListenerImpl` before the
code was written; this is it observed.

#### Measured

**1344 tests** (was 1323: +21 in `rewo-net` — 11 in `bundle`, 10 in `session`;
proto 11, world 340, data 175, net **495**, mesh 38, gpu 205, app 80). All
**29** serverless gates green with Vulkan validation ON and **0 VUIDs**:
abilityshot **67** (was 47), labelshot 47, rideshot 24, capeshot 69, itemshot
62, inventoryshot 152, healthbarshot 33, attributeshot 43, captureshot 17,
blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35, handshot 34,
particleshot 34, eventshot 28, danceshot 24, portalshot 12, hudshot 41, mobshot
**246/246** (+ emissive 5, etf 8, tint 11, variant 13), skyshot, lightmapshot,
tintshot, meshshot, dimensioncheck. Release `demo` PNG SHA-256 byte-identical to
M15 onward (`2cc56b4a…`). Live `rewo play`: **CORRECTIONS 0** over 800 ticks,
329 columns, place + dig server-observed, **177 bundles applied**. `git diff
--check` clean; every file touched was already pure LF and stayed that way.

**Not verified:** nothing here is eyeballed, and two things are worth naming as
unobserved rather than green. The **rendering** consequence bundling exists for
— a mob no longer drawn for a frame with default metadata — is an argument from
the decompile plus a counter that says the path fires; nobody watched a spawn.
And no server was made to send a `store_cookie`, so the *filled* reply is graded
by the gate's byte comparison and by the unit tests, not by a live round trip;
what the live run proves about cookies is only that the empty-jar path is
unchanged.

### M75 — `player_abilities`, flight, and the gamemode binding (2026-07-29)

M71 modelled the local player's **gamemode** from `game_event`'s
`CHANGE_GAME_MODE` and explicitly did not act on it, recording why: `physics`
had no concept of flight, no-clip or invulnerability, and neither
`player_abilities` packet was in `ids.rs`. It wrote the four-step job into
`REWO_PACKET_COVERAGE.md` §4.1 rather than half-starting it. All four steps
shipped here.

**The flags byte, first, because it is the thing most likely to be guessed:**
`INVULNERABLE = 1, FLYING = 2, CAN_FLY = 4, INSTABUILD = 8`, then `readFloat`
**flyingSpeed**, then `readFloat` walkingSpeed. Nine fixed bytes, no var-ints,
no length prefix. The **serverbound twin is one byte** and declares only
`FLAG_FLYING` — writing the clientbound body there desyncs the stream by eight.
The server does not take our word for even that one bit:
`handlePlayerAbilities` server-side is `flying = packet.isFlying() &&
player.getAbilities().mayfly`, so an unauthorised claim is **ignored, not
kicked**.

**Flight does not go through `travelFlying`.** That was the milestone's central
misdirection — the method exists, it is named for this, and it is for mobs and
swimming. `Player.travel`'s flying arm captures `originalMovementY`, delegates
to the *ordinary* `LivingEntity.travelInAir`, and then **overwrites** the Y it
just computed with `originalMovementY * 0.6`. `travelInAir`'s gravity
subtraction and its 0.98 vertical drag both still run and are then discarded
whole; the move has already happened, using the pre-gravity velocity. So
**flight has no gravity term at all**, its vertical drag is **0.6**, and — a
consequence worth knowing — flying into a ceiling does not zero your upward
velocity, because the capture precedes the `move` that clipped it.

**Three more that read backwards, each mutation-tested:**

- **`walkingSpeed` is not the client's walking speed.** Its only client
  consumer is `AbstractClientPlayer.getFieldOfViewModifier`, where it is the
  *divisor* for `Attributes.MOVEMENT_SPEED`. Its one movement role
  (`Player.readAdditionalSaveData` seeding that attribute) is server-side NBT
  load. At the defaults the two agree (`0.1 == 0.1`), so wiring it into the
  walk path looks correct until a server changes one without the other.
- **Sneaking does not slow a flying player.** The 0.3 `SNEAKING_SPEED` factor
  is gated on `isMovingSlowly()` → `isCrouching()` → the `crouching` field,
  and `aiStep` assigns `crouching = !abilities.flying && …`.
- **The vertical impulse is an f32 product.** `inputYa * getFlyingSpeed() *
  3.0F` is `int * float * float`, widened only on assignment. Widening first
  gives 0.15000000223…; the faithful path gives 0.15000000596…. M12's
  `Mth.floor`-returns-an-`int` rule again.

**`GameType.updatePlayerAbilities` is asymmetric in exactly one direction, and
it is step 4.** CREATIVE grants `mayfly`/`instabuild`/`invulnerable` and says
**nothing** about `flying`; SPECTATOR additionally sets `flying = true`; the
`else` arm clears all four. So **entering creative does not start you flying** —
deriving `flying` from `mayfly` is right for three modes and wrong for the one
a tester is most likely to be in, and it would look like it worked. And
**leaving creative actively drops flight** rather than merely ceasing to permit
it. `ModeAbilities.flying` is therefore an `Option<bool>`, so the CREATIVE case
is unrepresentable as a boolean.

**Three gamemode sources, not one.** `spawn_info.rs` has decoded `gameType` and
`previousGameType` since M16 and nothing read either. Vanilla routes all three
— login, respawn, `game_event` — into `MultiPlayerGameMode.setLocalMode`, whose
two overloads differ in exactly one way: the one-argument form guards the
previous-mode write on the mode actually changing, and the **two-argument form
(login/respawn) assigns both directly with no guard at all**, including a
`None`. Without the login path a client that joins in creative and never
switches has no idea it is in creative.

**Two pre-existing physics facts surfaced while placing the min-movement
clamp**, both fixed:

- Its position is load-bearing *only* for flight. Vanilla clamps as the **first
  statement of `LivingEntity.aiStep`** — after `LocalPlayer`'s vertical impulse
  and before `travel`. Between two walking ticks nothing happens, so end-of-tick
  and start-of-next-tick are the same; a flying tick has the impulse in that gap.
- **A player's horizontal clamp is a joint test on the pair**
  (`horizontalDistanceSqr() < 9.0E-6`), not the per-axis `0.003` every other
  entity gets. Rewo took the non-player arm. They disagree where each axis is
  under 0.003 but the magnitude is not — `vx = vz = 0.0025` has magnitude
  0.00354, which vanilla keeps. Below the server's 0.25-block correction
  threshold by two orders of magnitude, which is why `CORRECTIONS 0` never saw
  it.

**Verification, and an honest limit on the headline number.** `rewo play`'s
correction meter is **structurally unable to grade flight**: the server's
"moved wrongly!" check is `… && !this.player.isCreative() &&
!this.player.isSpectator()`, and vanilla grants `mayfly` in no other mode, so a
creative client's claimed position is simply `absSnapTo`'d. Same shape as M68's
finding that the meter cannot see a dropped knockback. So `rewo play
--fly-check` grades the flight phase by **measured kinematics against closed
forms**, and leans on the **survival walk after the revoke** for its one
server-graded property — a binding that failed to clear `flying` would leave
the client applying flight physics while the server checked it as a walker.
Live: **8/8, CORRECTIONS 0**, ascent **0.3750 blocks/tick** against a predicted
0.3750, cruise 0.5413 against 0.5444.

Three of that gate's own errors are worth recording, because two were mine and
all three were caught by measuring rather than reading:

1. It revoked creative at altitude; the bot fell 60 blocks, **died**, and the
   respawn teleport landed inside a measurement window. It now descends and
   lands first — which bought a witness, since flight must end there by the
   landing clause before any command revokes it.
2. The sampler divided displacement by the **sample count** rather than the
   interval count, reading 39/40 = 97.5% of the true rate. That looked exactly
   like a 2.5% physics error, and the 4% band would have absorbed it: a wrong
   divisor hiding inside a tolerance. Fixed, the ascent matches to four
   decimals.
3. The cruise closed form was the one for **carried velocity** applied to a
   **displacement** measurement. Per tick the distance covered is
   `v_carried + a = a/(1 − 0.91)`, while the carried fixed point is
   `0.91a/(1 − 0.91)` — a ratio of exactly 1/0.91, which is what the failing
   run reported. The ascent's `2.5·I = 1.5·I + I` already encoded the same
   distinction; the cruise did not.

**Gate: `rewo abilityshot --check` — serverless, CPU-only, fail-closed,
47/47.** It drives the real path end to end: nine raw bytes →
`PlayerAbilities::parse` → `apply_to` → `FlightControl::before_travel` →
`physics::tick_with` → `after_travel`; and for the gamemode half, a
`CommonPlayerSpawnInfo` body → the M16 decoder → `play::apply_spawn_game_mode`,
which was made a **free function** precisely so the gate runs what the session
runs rather than a copy of it (M45's `install_shapes` lesson).

**A 30-mutation battery was run against it; 29 were caught and the one survivor
was real.** The per-axis clamp reversion left the gate green because that
property lived only in a `rewo-world` unit test — so the gate gained a witness
for it and the mutation is now caught (47 witnesses, not 46). The battery also
caught a witness that **passed by coincidence**: `holding_jump_never_toggles`
sampled only the final tick, and dropping the rising-edge test makes a held key
toggle on a repeating cycle, so the end state was a coin flip. It now asserts
every tick. Both re-run and confirmed: **30/30**.

**Measured:** 1279 unit tests (world 340, net 430, data 205, mesh 38, gpu 175,
proto 11, app 80 — from 1247). All 25 serverless gates green with Vulkan
validation ON and **0 VUIDs**, including `mobshot` 246/246 and its four
sub-checks. Demo PNG SHA-256 **byte-identical** to M15 onward
(`2cc56b4a…46635`). Live: `--fly-check` 8/8 CORRECTIONS 0; ordinary `rewo play`
**CORRECTIONS 0** with place and dig both server-observed ACCEPT.

**Scoped exclusions, recorded rather than guessed:** a mounted player cannot
toggle flight (vanilla's guard is `getVehicle() == null || jumpableVehicle() !=
null`, and Rewo models no rideable-jumping, so it takes the boat arm for every
vehicle); fluids are outside this milestone, so `travelInFluid` and
`Player.travel`'s swimming pre-step are not modelled; and `instabuild`,
`invulnerable` and `may_build` are stored and **not acted on**, because nothing
in Rewo does client-side block-break timing or damage application and acting on
them would be inventing behaviour. `crates/rewo-world/src/physics.rs` was
uniformly CRLF and is **normalised to LF**, as M68 did for `motion.rs` — `git
diff --check` cannot pass on added CRLF lines.

**Coverage rows this milestone would have written** (the doc is owned by a
concurrent agent, so they are recorded here for reconciliation rather than
edited in): clientbound-play `player_abilities` (id 64) moves from *never
resolved* to **consumed**; serverbound `player_abilities` (id 40) is now sent;
and `game_event`'s `CHANGE_GAME_MODE`, which M71 listed as applied-but-inert,
now drives `Abilities` — as do the login and respawn `gameType` /
`previousGameType` fields, which the audit did not cover because it surveyed
clientbound-play only.

### M73 — the entity raycast (2026-07-29)

M70 shipped the label ladder with one clause it could not evaluate:
`EntityRenderer.shouldShowName` is `entity.shouldShowName() || (hasCustomName()
&& entity == crosshairPickEntity)`, and Rewo's raycast was voxel-only, so the
second disjunct was transcribed, graded both ways by the gate and fed a hard
`false` live. This is the pick that answers it, in
`rewo-world/src/entity_pick.rs`.

**It is not a second, label-only raycast.** `Minecraft.pick` assigns
`crosshairPickEntity` from `player.raycastHitResult(partial, cameraEntity)` —
*the* `hitResult`, the same one that decides which block you are mining. The
brief looked for it in `GameRenderer`; in 26.2 it is a **private static
`LocalPlayer.pick`**, called from `LocalPlayer.raycastHitResult`, called from
`Minecraft.pick`. `GameRenderer` has no pick at all.

**The inflation is `entity.getPickRadius()`, and it is `0.0F` for every entity
but a `Projectile`** (which returns `isPickable() ? 1.0F : 0.0F`). It is
emphatically *not* the `DEFAULT_ENTITY_HIT_RESULT_MARGIN = 0.3F` declared at
the top of the same file — that feeds `computeMargin`, which belongs to the
**projectile** overload of `getEntityHitResult` (a flying arrow's forgiveness
ramp, `max(0, min(0.3, (tickCount - 2) / 20))`). A mob is swept at its exact
hitbox with no forgiveness whatsoever. `labelshot` g7's mutation partner is
literally "inflate by 0.3", because that is the wrong answer a reader is most
likely to reach for.

**The tie-break is nearest-first on `from.distanceToSqr(clipPoint)`, strict,
seeded at `maxValue`** — so the range bound and the tie-break are the same
comparison. Two arms of that loop read strangely and are transcribed as
written: an entity *containing* the eye short-circuits to `nearest = 0.0` if it
`canBePickedFromInside()` (and `AABB.clip` tests only near faces, so a segment
starting inside clips nothing and `clipPoint.orElse(from)` is the live branch);
and a candidate sharing the source's root vehicle is skipped only *because*
`nearest != 0.0` in the ordinary case — the guard is `dd < nearest || nearest
== 0.0`, and after an inside-pick that arm assigns `hovered` without updating
`nearest`.

**The range that bounds the sweep is `max(block, entity)`, not the entity
range.** The entity range is applied afterwards by `filterHitResult`, which
rewrites an over-range entity hit into a `BlockHitResult.miss` — and a miss
carries no entity, so `crosshairPickEntity` becomes null. With
`blockInteractionRange` 4.5 above `entityInteractionRange` 3.0, a mob at 4
blocks is found, measured, and *then* discarded.

**A mutation that survived, and what it found.** g5 asserted that a dead heat
between a block and a mob goes to the block, naming `>=` → `>` as its partner.
Running it left the gate green. The reason is that vanilla enforces this
precedence **twice**: the entity ray is truncated at the block hit *and* the
surviving hit is compared against it — and because the truncation feeds
`getEntityHitResult`'s `maxValue`, whose `dd < nearest` is itself strict, the
tie is already excluded by the sweep bound. `entityHit.distSq < blockDistSq`
is therefore unreachable for an ordinary candidate; the only path that can
reach it is the same-root-vehicle arm above, which runs after an inside-pick
without consulting `maxValue`. Neither half alone is observable, so g5's
mutation now removes both. This is the kind of claim that only a mutation can
falsify — the witness was passing, and passing for the wrong reason.

**Do not hard-code 3.0 / 4.5.** Both are `RangedAttribute`s
(`entity_interaction_range` 3.0 in [0, 64], `block_interaction_range` 4.5 in
[0, 64]) and resolve through M55's machinery, so a server's modifiers and the
clamp both apply — **creative mode is itself a `+2.0 ADD_VALUE` modifier on the
entity range**, not a special case in the pick. That exposed a real gap:
`apply_update_attributes` opens with `handleUpdateAttributes`'s own
`getEntity(id) == null` gate, and the local player **is not in Rewo's
`EntityTable`** (the server sends no `add_entity` for your own player), so every
snapshot addressed to it was being dropped. `PlaySession` now keeps
`local_attributes` beside the table, filled by
`rewo_net::attributes::apply_local_attributes` — which stores only for the
camera entity, because another entity's ranges landing there would silently
give the player a mob's reach (g4).

**Two machine-extracted tables** (`tools/gen_entity_pick.py` →
`rewo-data/src/entity_pick_table.rs`, 158 types), because neither is in any
datagen report:

- **`isPickable()` defaults to `false`.** That inverts the intuition: the pick
  does not look for reasons to exclude an entity, it is that only the thirteen
  classes overriding the method are pickable at all. A dropped **item**, an
  experience orb, a `text_display`, a `marker` and a `lightning_bolt` are
  invisible to the crosshair — and so is the **ender dragon**, which is a
  `LivingEntity` that would inherit `true` and overrides it back to `false`,
  delegating to `EnderDragonPart` hitboxes that are not registered entity
  types. Census: 119 `Alive`, 15 `RedirectableProjectile`, 12 `Never`, 7
  `Always`, 3 `RedirectableProjectileNotInGround`, 1 each for the player's
  spectator test and the armour stand's marker test. The generator asserts each
  override's **body text** verbatim, so a rule changing meaning under the same
  class name is a hard error.
- **Base dimensions** — `EntityType.Builder.sized(w, h)`, all 158 parsed with
  zero falling back to the builder default. This replaced a hand-written
  14-entry table whose default was `(0.6, 1.8)` for everything it did not name;
  g13 is the consequence made observable, a ray 1.9 above the feet hitting a
  zombie (1.95) and missing a player (1.8).

`Projectile.isPickable()` is a **tag**, `EntityTypeTags.REDIRECTABLE_PROJECTILE`
(3 entries: fireball, wind_charge, breeze_wind_charge), read from the client
jar's data pack rather than baked into the generator — the same rule M19
records for `ItemTags.SPEARS`.

**The half-width is `0.6F / 2.0F` widened, not `0.6 / 2.0`.** Vanilla halves
the width as a `float` and lets the `AABB` constructor widen it, so a mob's
near face sits at `x - 0.30000001192…`. Two witnesses were written with a
hand-computed `0.3` and both landed a hundred-millionth off the bound they
claimed to sample — exactly M70's `b4` failure again, and the reason g2 now
asserts the placement (`near face == 3.0`) before asserting the answer.

**Scoped exclusions, recorded rather than hidden.** The `AttackRange` branch of
`raycastHitResult` (a wholly different algorithm — `getHitEntitiesAlong` +
`getManyEntityHitResult`, with a minimum reach, a motion-dependent maximum and
a two-stage re-clip) is not implemented; in vanilla 26.2 only the spear builder
carries that component, so it is inert unless a spear is held or in use, and
while one is this falls back to the ordinary pick. The 47 per-class
`getDefaultDimensions` overrides are not modelled either — the base chain
(`sized`, the `SLEEPING` substitution, `Avatar`'s pose map, `getAgeScale()`, the
`SCALE` attribute) is exact, but 30 of those overrides substitute an explicit
`BABY_DIMENSIONS` that is only sometimes the adult box halved (a baby cow's
0.45×0.7 is; a baby chicken's 0.3×0.4 is not half of 0.4×0.7). Two inputs Rewo
cannot decode — a *remote* player's spectator flag and an armour stand's marker
flag — are answered **permissively**, against the usual house rule, because
suppressing on them would make every player and every armour stand unpickable.

**Gate: `rewo labelshot --check`, 32 → 47 witnesses**, serverless, validation
ON, 0 VUIDs. The new g-section drives `live_cmd::crosshair_pick_from_table` —
the same function `resolve_crosshair_pick` calls every frame — and `f7` is the
property the milestone exists for, measured end to end as a **vertex count**: a
name-tagged mob whose `CustomNameVisible` is unset emits 24 label vertices
standing on the ray and 0 standing four blocks to the side, the two runs
differing only in the mob's `z`. No `max_health` is synced for it, deliberately,
so the text range holds the nametag alone. **All 13 named mutations were run**;
twelve were caught first time, g5 was not and is described above.

**Measured.** **1206 tests** (was 1180 at this branch point: +18 `rewo-world`,
+6 `rewo-data`, +2 `rewo-net`). All 27 gate invocations exit 0 with **0 VUIDs**:
labelshot 47, capeshot 69, itemshot 62, inventoryshot 152, healthbarshot 33,
attributeshot 43, captureshot 17, blockentityshot 172, swingshot 97, hurtshot 38,
weathershot 35, handshot 34, particleshot 34, eventshot 28, danceshot 24,
portalshot 12, hudshot 41, mobshot 246/246 + emissive 5 + etf 8 + tint 11 +
variant 13, skyshot, lightmapshot, tintshot, meshshot, dimensioncheck. Demo PNG
SHA-256 `2cc56b4a…46635`, byte-identical to M15 onward. `gen_entity_pick.py`
reproduces its output byte-for-byte on a re-run.

**Open.** No live server session was run, so the live glue
(`frame_crosshair_pick` → `resolve_crosshair_pick`, ten lines over the gated
seam) is compiled and unexercised — nobody has watched a nametag appear as the
crosshair crosses a mob. The `AttackRange` branch and the 47
`getDefaultDimensions` overrides are the scoped exclusions above. And the
numbering: this shipped as **M73** because the parallel session took M71 and M72
out from under it mid-flight; the code was written against a base where M71 was
free.

### M70 — the entity-label visibility rules (2026-07-29)

Three features float a label over an entity — the nametag (M2-era), the health
bar (M59), and whatever comes next — and each had implemented its own slice of
the same predicate. The nametag's slice was **nothing**: it drew whenever a
string existed. The bar's was three gates (living, name-tag distance,
invisible), which M59 itself recorded as a strict subset, listing what was
ungated. This is the whole predicate, once, in `rewo_world::label`, consumed by
both.

**`shouldShowName` is overridden four times and they do not compose the way
the class names suggest.** With `nameSource` for `EntityRenderer`'s own body
(`entity.shouldShowName() || (hasCustomName() && entity ==
crosshairPickEntity)`) and `living` for `LivingEntityRenderer`'s:

| renderer | rule |
|---|---|
| `EntityRenderer` (boat, minecart, item entity, …) | `nameSource` |
| `LivingEntityRenderer` | `living` |
| `MobRenderer` (every `Mob`) | `living && nameSource` |
| `AvatarRenderer` (players) | `living && nameSource` |
| `ArmorStandRenderer` | `isCustomNameVisible()`, and nothing else |

**`LivingEntityRenderer` does not consult `nameSource` itself**, and reading it
alone puts a type name over every mob in the world — because `extractNameTags`
then assigns `getNameTag(entity)` = `entity.getDisplayName()`, which is never
null. `MobRenderer` and `AvatarRenderer` re-add the clause, spelled out rather
than shared, and they are what make an un-named mob silent. `ArmorStandRenderer`
is a *full* override: an armour stand's name ignores the sneak cut-off, every
team rule, invisibility, the camera entity and `isVehicle` alike.

**The load-bearing shape is an early return inside the team branch.** The
`switch` on `Team.Visibility` *returns*, so `hud.isHidden()`,
`getCameraEntity()` and `isVehicle()` are only ever reached by an entity with
**no** team. A player on a team keeps their nametag with F1 pressed, and a
teamed horse keeps its while ridden. That is not an artifact of this
transcription; it is what the source does, and `e7` is the witness.

**Three things the brief got wrong, all found by reading rather than assuming.**

- **The sneak cut-off is not 32.** It is `distanceToCameraSq >= 1024.0`, a
  **hard-coded, folded** literal — the `float maxDist = 32.0F` beside it is a
  dead local the decompiler kept. The distinction is observable: the bound is
  **not** `Mth.square(nameTagDistance)`, so a server that raises
  `NAME_TAG_DISTANCE` to 128 reaches further with a standing entity and leaves
  a sneaking one capped at 32. `b3` is that witness. The comparison is `>=`
  (returning false), so the bound itself is excluded.
- **`isDiscrete` in `EntityRenderer` is not a distance rule at all** — there it
  is `!state.isDiscrete`, passed to `submitNameTag` as the see-through flag.
  The cut-off lives only in `LivingEntityRenderer`.
- **Rewo already decodes teams.** `ClientboundSetPlayerTeamPacket` shipped with
  the tab list (`rewo-net/src/teams.rs`, "nothing here is wired to a
  renderer"), so M70 wired it rather than decoding it. Its discriminator is the
  **`method` byte** — `input.readByte()`, signed and widened to int, compared
  against five literals — with `shouldHaveParameters` = {0, 2} and
  `shouldHavePlayerList` = {0, 3, 4}; note **Add carries both**, so reading the
  parameters one field short eats the founding roster. Verified against the
  decompile; the existing decode was already exact.

**`NAME_TAG_DISTANCE` is an attribute, and `EntityRenderer`'s default is not.**
`extractNameTags`'s no-argument form passes a literal `64.0`
(`Entity.DEFAULT_NAME_TAG_DISTANCE`); `LivingEntityRenderer` overrides it to
read `Attributes.NAME_TAG_DISTANCE`, a `RangedAttribute(64.0, [0, 512])`. So
the attribute governs living entities only — which is every entity Rewo can
resolve one for, since `DefaultAttributes.SUPPLIERS` is keyed by
`EntityType<? extends LivingEntity>`. That equivalence is how the renderer
ladder is selected without a renderer registry.

**Two genuinely new wire inputs, one genuinely new key.**

- **`set_passengers` (id 107)** — `readVarInt()` then `readVarIntArray()`,
  which `REWO_PACKET_COVERAGE.md` had listed as absent at priority A. It exists
  here for `Entity.isVehicle()`, which is `!passengers.isEmpty()` — **something
  is riding this entity**, not the reverse. The table keeps the inverse index
  too, because `handleSetEntityPassengers` calls `startRiding`, which detaches
  a passenger from its previous vehicle first; without it a rider moving
  between mounts leaves the old one reading as ridden forever, and that
  vehicle silently loses its label for the rest of the session. The same
  applies on despawn, from both directions. **An empty roster is meaningful**
  and is the only thing that brings a label back, so decoding it as a
  truncation would be silent.
- **`DATA_CUSTOM_NAME_VISIBLE` — metadata index 3, BOOLEAN.** `Entity` owns
  0..7 (shared flags, air supply, custom name, this, silent, no-gravity, pose,
  ticks frozen), the same counting argument that pins `DATA_POSE` to 6. It is
  `entity.shouldShowName()` for everything except a player, which overrides it
  to a literal `true`. Applied on `false` as eagerly as on `true`: a latch
  would leave a nametag up after the flag was cleared.
- **`Entity.getScoreboardName()` is two different strings.** A player's is the
  profile name; everything else's is `this.stringUUID`, the dashed lowercase
  form. Using one for the other is silent — the team simply never matches.
- **`isDiscrete()` is shared flag 1**, and three `Entity` methods share that
  bit verbatim. It is *not* `isCrouching()`, which reads the pose.

**Team identity is by name, and that is exact rather than an approximation.**
Vanilla compares `PlayerTeam` object references — `Team.isAlliedTo` is
`other == null ? false : this == other`, and `isInvisibleTo` uses
`player.getTeam() == team` — but `Scoreboard` holds exactly one object per
name, so reference and name equality coincide. It is *identity*: two different
teams are never allied. And `PlayerTeam`'s constructor defaults
`seeFriendlyInvisibles` to **`true`**, not false, which is the fallback for a
team whose parameters never arrived.

**`canSeeFriendlyInvisibles` is read by two different paths and the arms are
not symmetric.** `isInvisibleTo` consults it (so an `ALWAYS` team shows an
invisible team-mate), and `HIDE_FOR_OTHER_TEAMS` consults it again in
`team.canSeeFriendlyInvisibles() || isVisibleToPlayer`. `HIDE_FOR_OWN_TEAM` has
no such escape.

**What Rewo still cannot answer, and answers by suppressing.**
`crosshairPickEntity` needs an entity raycast — `LocalPlayer.raycastHitResult`
→ `ProjectileUtil.getEntityHitResult`, plus two interaction-range attributes
and the `AttackRange` item component — and Rewo's raycast is voxel-only. The
clause is transcribed, driven both ways by the gate, and fed `false` live.
**This narrows what the client draws**: a name-tagged mob whose
`CustomNameVisible` is unset now shows nothing where it used to show a name
unconditionally. That is closer to vanilla, not further — the old behaviour was
wrong for such a mob at all times, this is wrong only while it is under the
crosshair — but it is a visible change and the entity pick is the named
follow-up.

> **Closed by M73** (entry above). The pick ships in
> `rewo-world/src/entity_pick.rs`, both interaction ranges resolve through the
> attribute machinery, and `labelshot`'s `f7` measures the clause end to end as
> a vertex count. The `AttackRange` branch named here is the one part that
> remains unimplemented, and is recorded as a scoped exclusion in M73's entry.
> The witness counts below are M70's measurement; `labelshot` is now 47.

**Two mutations found real gaps in my own witnesses**, and both were the same
shape: a property that looked tested but had no sample where the mutation could
bite.

- **`b4` straddled the name-tag distance without ever sitting on it.** With
  samples at 63.99 and 64.01, flipping the source's `<` to `<=` left the entire
  gate green. Fixed by sampling the bound exactly. (`b1`, the sneak cut-off,
  had had an exact-bound sample from the start, which is why the equivalent
  mutation there failed immediately.)
- **`e6` tested `HIDE_FOR_OWN_TEAM` only from a same-team viewer.** Bolting a
  `canSeeFriendlyInvisibles ||` escape onto that arm is then invisible, because
  `team.name != mine` is already false and `&&` short-circuits past it. The
  discriminating viewer is on the **other** team. Fixed by adding that sample.

**Gate: `rewo labelshot --check`, 32 witnesses**, serverless, fail-closed,
validation ON, 0 VUIDs. It drives raw `set_entity_data` / `update_attributes` /
`set_passengers` / `set_player_team` bodies through the production routers into
`EntityTable` + `Teams`, then through the same `label_inputs_from_table` +
`teams::label_team` + `resolve_labels` the collector uses, and finally counts
vertices in `EntityPass`'s text range — so a suppression is measured as *zero
label vertices*, not as a boolean the gate computed itself. Every witness names
its mutation partner and **all fourteen were run**; the two above failed, were
fixed, and the mutations were re-run.

`f6` is the property the milestone exists for: over six scenarios the nametag
and the health bar never disagree, with a non-vacuity check on the count —
M59's `e3`/`e4` passed on two empty vectors for exactly that reason, and before
M70 the "invisible" scenario genuinely disagreed (a name and no bar).

**Measured.** **1136 tests** (was 1098: +24 in `rewo-world`'s `label`, +14 in
`rewo-net` for the two new decodes and the UUID form). Twenty-three gates green
with validation ON and **0 VUIDs**: labelshot 32, capeshot 69, itemshot 62,
inventoryshot 143, healthbarshot 33, attributeshot 43, captureshot 17,
blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35, handshot 34,
particleshot 34, eventshot 28, danceshot 24, portalshot 12, hudshot 41, mobshot
243/243 + emissive 5 + etf 8 + tint 6 + variant 8, plus skyshot, lightmapshot,
tintshot, meshshot and dimensioncheck. Demo PNG SHA-256 byte-identical to M15
onward. `healthbarshot`'s 33 witnesses were re-pointed at the unified predicate
and still pass unchanged.

**Open.** No live in-world sighting — the gate is authoritative for the
properties it names, and nobody has watched a nametag disappear behind a team
rule on a real server. `crosshairPickEntity` (above) is the one input still fed
a constant — **closed by M73**, which resolves it from a real entity raycast.
`ArmorStandRenderer`'s arm of the ladder is transcribed and
unit-tested but unreachable live, because Rewo models no armour stand.
`ItemFrameRenderer.shouldShowName` — its own third rule, `!hud.isHidden() &&
crosshairPick == entity && item.getCustomName() != null` — is deliberately not
implemented, since Rewo renders neither item frames nor their held item's name.
`hud.isHidden()` suppresses labels but not Rewo's own hotbar, hearts or F3
block, where vanilla hides the whole GUI layer through
`guiRenderState.isHudHidden`. And `set_passengers` is consumed for `isVehicle`
only: a passenger still renders at its own last-reported position rather than
on its vehicle, which is the gap `REWO_PACKET_COVERAGE.md` records against that
packet and which this milestone does not close.

### M64 — closing M57, M60 and M61's open entity-rendering items (2026-07-29)

Four jobs, no new subsystems: the texture variants M57b said were "one decode
away", the sheep shearing M57d left, the cape the M36 preview never wore, and
the collision M61's spec rule 10 recorded as un-re-projected.

#### The metadata-driven texture variants

Six mobs picked their sheet from synched metadata and Rewo baked one of each —
every cat a tabby, every horse brown, every axolotl leucistic. M57b was right
that the rendering half already existed: a pack's ETF alternate is a same-sized
texture packed elsewhere in the entity atlas and addressed by a per-draw
variant id, which is exactly the shape vanilla's own variants need. What was
missing was the sheets, the id → texture mapping, and the decode.

**The indices, counted `defineId` by `defineId` up the 26.2 decompile.** The
base chain is `Entity` 0..7, `LivingEntity` 8..14, `Mob` 15, `PathfinderMob`
none, `AgeableMob` **16 and 17**, `Animal` none, `TamableAnimal` 18 and 19.

| mob | index | serializer | why |
|---|---:|---|---|
| Cat | 20 | `CAT_VARIANT` (21) | `Cat extends TamableAnimal`, so 18/19 are taken |
| Wolf | 23 | `WOLF_VARIANT` (25) | after `DATA_INTERESTED_ID` 20, `DATA_COLLAR_COLOR` 21, `DATA_ANGER_END_TIME` 22 |
| Frog | 18 | `FROG_VARIANT` (27) | `Frog extends Animal` |
| Axolotl | 18 | `INT` (1) | `Axolotl extends Animal` |
| Horse | 19 | `INT` (1) | `AbstractHorse` declares exactly one, `DATA_ID_FLAGS` at 18 |
| Llama | 21 | `INT` (1) | `AbstractChestedHorse` 19, `DATA_STRENGTH_ID` 20 |

**Two kinds of variant, and only one is a constant.** Horse, llama and axolotl
carry an enum ordinal, so those are transcribed tables — with **three different
out-of-bounds strategies**, which is not a detail that can be averaged:
`Axolotl.Variant`'s `ByIdMap` is `ZERO` (an id past the end is LUCY),
`Llama.Variant`'s is `CLAMP`, and `equine::Variant`'s is `WRAP`, so a horse at 7
is WHITE again rather than DARK_BROWN. The horse's coat is also the **low byte**
of its int (`typeVariant & 0xFF`); the high byte is the markings layer.

Cat, wolf and frog moved to **datapack registries** in 26.x
(`Holder<CatVariant>` over `ByteBufCodecs.holderRegistry`, so the wire value is
a raw 0-based id). Their contents *and their id order* are the server's — §0.0's
rule, the same one M16 records for dimension types and M42 for enchantments —
and `RegistryDataLoader.SYNCHRONIZED_REGISTRIES` gives all three a
`NETWORK_CODEC` that carries the asset ids. **So the join is on the texture
path, never on the id.** Keying the atlas on the wire id passes on a vanilla
server and paints the wrong coat on every cat the moment a datapack reorders
the registry; the mutation was run and `n3` is the only witness that sees it.
Joining by path has a second payoff: a variant that resolves to a sheet Rewo
already bakes gets id 0 and costs no atlas space.

**Index 18 BYTE now has two claimants with the same serializer.**
`Sheep.DATA_WOOL_ID` and `TamableAnimal.DATA_FLAGS_ID` are both a BYTE at 18,
because `Sheep` and `TamableAnimal` both extend `Animal` and `Animal` declares
nothing — so only the entity **kind** separates them, M18's rule again. Reading
a wolf's flags as a wool byte gives every tame wolf dye 4 (yellow) and a fleece
it does not have. `isTame()` is bit 0x04 there, and it is what `Wolf.getTexture`
branches on between `assets.wild` and `assets.tame`.

**The atlas grew the other way round, and that is the finding.** M22, M48 and
M60 each added a band at the *bottom*, leaving the mob shelf region above
untouched — the recipe §0.0 recommends. What ran out here is the shelf region
itself: seven of the 42 sheets did not pack and **silently fell back to their
base texture**, which no render would have shown. The shelf ceiling is defined
by subtraction from `ATLAS_H`, so raising it 128 rows (1472 → 1600)
*necessarily* slid the item, skin, trim and cape pools down with it. Nothing on
disk depends on those origins and mob packing is byte-for-byte unchanged (the
packer is sequential and the region only grew at its far end), so `mobshot`
stays 243/243 — but `capeshot`'s `f2`, which asserted the cape pool sits at
exactly the pre-M60 `ATLAS_H`, had to be rewritten to say what is now true. It
had also hard-coded `(0, 1408)` as a fixture origin, which is the same mistake
one layer down; it reads `cape_slot_origin(0)` now.

Vanilla's variant wins over a pack's ETF rule where both apply, and the reason
is not precedence for its own sake: an ETF rule randomises the *base* texture,
and a black cat is not drawing that texture at all. The two occupy disjoint id
bands (`VANILLA_VARIANT_BASE = 0x4000`).

**Excluded, each recorded where a reader would look for it.** Baby sheets — the
baby model is a uniform 0.5 scale approximation, so there is no baby model for a
baby texture to sit on. Wolf `angry` — `isAngry()` is
`remainingPersistentAngerTime > 0`, i.e. `DATA_ANGER_END_TIME` (index 22, LONG)
against the world clock, which makes it a texture that changes with *time*
rather than with a synched value; the field **is** decoded so the gap is visible
in the parsed data rather than only in a comment. Tropical fish — its packed int
selects a **model** (`TropicalFishSmallModel` vs `TropicalFishLargeModel`, which
is what `tropical_a.png` and `tropical_b.png` belong to) plus a pattern layer
and two dye tints, none of which is a texture swap on a model Rewo has, so it
belongs to a mob-model milestone. Collars, horse markings and llama carpets are
all second render layers.

Gate: **`rewo mobshot --variant-check`, 8 witnesses** — the bake and its size
constraint, the three ordinal tables and their three strategies, the
path-not-id join, the wolf's tame branch, the atlas slots, a render of every
variant of every mob against its base *and* its neighbours, and the wire
through the real `route_set_entity_data` at each index **and its two
neighbours**.

#### Sheep shearing

`SheepWoolLayer.submit` opens `if (!state.isSheared)`, so shearing does not
recolour the fleece — the fur model is never submitted. Rewo bakes
`SheepFurModel`'s inflated boxes as the sheep model's second texture slot, so
"do not submit that layer" is "drop the quads that sample it". **Removing the
geometry is the point**: the fleece sits 0.6/1.75/0.5 proud of the body, so a
shorn sheep is thinner, not differently coloured — measured, the silhouette
drops 27,502 → 21,290 px. `shearable_texture` is deliberately a second table
beside `tinted_texture` even though both answer `sheep_wool` for the one mob
that has either: they are two independent facts about two different lines of
the same layer, and a wolf's dyed collar will be tinted-but-not-shearable the
moment it exists.

**Found and not shipped:** 26.x added `SheepWoolUndercoatLayer`, a second
fleece drawn from `sheep_wool_undercoat.png` over the **body** mesh
(`SHEEP_WOOL_UNDERCOAT` maps to `sheepBodyLayer`, not the fur one) for any
non-white non-baby sheep — and it is **not** gated on `isSheared`, so vanilla
leaves a shorn coloured sheep with a dyed undercoat where a shorn white one is
bare. Rewo bakes no such texture; recorded as a missing layer rather than a
wrong one.

`mobshot --tint-check` 4 → 6. `t5` asserts the change is contained in the
independently-derived wool set **and** that the silhouette shrinks; `t6` that a
shorn sheep is inert to a dye that moves a woolly one. Both mutations were run,
and the second is why both rows exist: implementing shearing as "skip the tint"
instead of "skip the layer" leaves the silhouette byte-for-byte where it was —
failing `t5` while *passing* `t6`.

#### The inventory preview's cape

M36's preview owns a **second** `EntityPass` with its own atlas, so it needed
its own cape pool and its own upload. This is the stronger of the two cases,
not the weaker: a cape address is an absolute texel origin rather than a UV
delta, so a borrowed one samples a fixed wrong rectangle. And because both
pools fill from empty, **the first cape in each lands on the same texel** — a
borrowed address would have looked right until a second player joined, which is
what `p3` exists to catch (it claims a second world slot to move the two apart,
then shows the preview rendering nothing from the world's address).

The three cape angles are zero, and that is not a simplification of vanilla so
much as the same consequence as the preview's still legs:
`capeFlap`/`capeLean`/`capeLean2` are driven entirely by the gap between the
player and their lagging cloak anchor, and a player standing in an open
inventory has let that gap close. What is genuinely missing is the *moving*
preview, for the reason the limbs are missing. `chest_humanoid` is false
because the preview draws no armour at all, so neither of `CapeLayer`'s other
two gates has anything to act on.

`preview_cape` was extracted as the production resolver so the gate grades the
decision the client makes rather than a restatement of it — M45's and M41's
gates both quietly stopped testing their subject by reimplementing a slice of
the app. `capeshot` 65 → 69; the preview draws only with the container screen
open, so the gate builds that too, and `p1`'s threshold is derived from the
sheet's own projected area (478 marker px of a possible 562 with the model
turned away; 46 in the pose a player actually sees, where the body hides all
but the edges).

#### The wavy cape's re-projected collision

`REWO_WAVY_CAPE_SPEC.md` rule 10 recorded that M61 relaxed and *then* collided
once, so a joint the push-out shoved off the torso left its links stretched
until the next tick — 0.230 model units, a fifth of a slab, on a 30°/tick turn.
`solve` now runs `RELAX_PASSES` iterations of (relax, push-out), so every
push-out but the last is answered by the sweep after it.

**Measured over that turn, worst post-tick link error against pass count:**
`2.30e-1 / 9.60e-3 / 4.27e-4 / 5.14e-5` at 1/2/3/4. The alternation converges
geometrically, and `RELAX_PASSES = 4` turns out to be the *first* count that
clears the spec's own 1e-4 tolerance — both numbers were fixed before the
interleave existed, so that is a coincidence recorded rather than a derivation.

**The push-out stays last inside the loop**, which is the load-bearing half:
ending the solve on a relax would satisfy the links exactly and let the final
sweep pull a joint back inside the cylinder, trading rule 5's assertable
guarantee for a tolerance in the one place a naive chain visibly fails. The
closest approach through the turn is still `TORSO_RADIUS` to the bit under
either order. Passes 2–4 are no longer the exact no-ops M61 recorded.

`capeshot` gains `w23`, the only witness that can tell the two orders apart —
`w9` asserts the residual is small and `w12` that the cylinder holds, and the
M61 build passed both because it read its residual at a point the finished
state no longer occupied. `w9` moved its measurement from mid-solve to the end
of the tick, which is where it can now live. The mutation (revert `solve` to
the M61 order) fails `w9` and `w23` and nothing else — and it **withdrew an M61
claim in spec rule 10**: that a cloak gap "never fires the push-out at all,
only a turn does". A 1.5-block gap swinging through a full circle leaves 2.4e-2
of un-re-projected stretch under the old order, which it could only have got
from the push-out. A gap that *rotates* sweeps the cape across the body; a held
one does not. Spec rules 1, 8 and 10 and the gate table are updated — the spec
is the source of truth for that feature and must not be left describing the old
order.

#### Measured

**950 tests** (was 944; +2 `mob_variants`, +3 `variant_parse`, +1 `wavy_cape`),
zero failures. Every gate green with Vulkan validation ON and **0 VUIDs**:
`capeshot` 69, `itemshot` 62, `inventoryshot` 127, `healthbarshot` 33,
`attributeshot` 43, `captureshot` 17, `blockentityshot` 172, `swingshot` 97,
`hurtshot` 38, `weathershot` 35, `handshot` 34, `particleshot` 34, `eventshot`
28, `danceshot` 24, `portalshot` 12, `hudshot` 41, `mobshot` 243/243 + emissive
5 + etf 8 + tint 6 + **variant 8**, plus `skyshot`, `lightmapshot`, `tintshot`,
`meshshot`, `dimensioncheck`. Demo PNG SHA-256
`2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
byte-identical since M15. `git diff --check` exits 0.

**Not verified:** no live sighting of any of it. Nobody has watched a black cat,
a shorn sheep or a caped inventory preview on a real server, and M60/M61's "no
live sighting" note still stands for the wavy cape.

**Process note.** The Edit tool normalises a file's line endings, and
`mobshot_cmd.rs` and `rewo-data/src/lib.rs` are both mixed CRLF/LF — each
ballooned to a whole-file diff (1,860 and 117 lines) and had to be rebuilt
per-line against `HEAD`, keeping each unchanged line's own terminator and
giving new lines LF. §0.0 warns about this; the warning is worth a second
mention because the *first* symptom is `git diff --check` reporting trailing
whitespace on lines nobody touched.


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
M10–M17 arc was reviewed local work at the time; it is pushed now). The vanilla
test server was stopped by
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

### 2026-07-27 — M31: the spawner's caged mob — SHIPPED + VERIFIED

`6a48208`, on `codex/rewo-m19-combat-swings`, not pushed. `blockentityshot`
157 → **166**.

M29 named this as needing "an entity model composed into a block-entity draw",
and that framing was one word off. **The mob does not belong in the
block-entity path at all** — it belongs in the *entity* path, positioned
differently.

Every other entity **stands** in the world: `EntityDraw.pos` is where its feet
are. A spawner's display mob does not — `submitEntityInSpawner` pushes a
translate-rotate-tilt-scale chain and renders the entity at the resulting
origin. So `EntityDraw` grew an optional **`mount`**, an affine applied to the
feet-relative position before `pos` places it, and the caged mob rides the
exact path every other mob uses: same models, same rigs, same animations. A
second emitter would have had to duplicate all of that.

The display entity is `SpawnData` → `entity` → `id`, two levels down. An empty
or absent id means **no mob rather than a default** (`getOrCreateDisplayEntity`
returns null), and so does an id naming a type this version does not register —
a substitute would be a very visible invention.

Three numbers are easy to get wrong, and each is pinned:

- the scale is `0.53125 / max(bbWidth, bbHeight)` **only when that exceeds
  1.0**, so a zombie is squeezed into the cage while a silverfish is left small
  rather than enlarged to fill it;
- the render spin is `lerp(partial, oSpin, spin) * **10**` — the block entity's
  counter advances a couple of degrees a tick and the renderer multiplies by
  ten, so the mob whirls rather than drifting;
- `scale_mul` stays 1, because the fit-to-cage scale is inside the mount and
  applying it in both places would shrink the mob *squared*.

**A witness disproved my own comment.** I wrote that the inner
`translate(0, -0.2, 0)` sits inside the spin and therefore makes the mob orbit
rather than turn on the spot — "swapping the two translates would give a mob
that pirouettes in place". That is wrong: `(0, -0.2, 0)` lies **along the spin
axis**, so it commutes with the Y rotation and the two translates could be
swapped with no effect whatsoever. `r6` asserts the feet stay fixed through a
quarter turn, which is what caught it. Both the comment and the witness now say
so, and name the **−30° X tilt** as the part that *is* load-bearing: it leans a
caged mob towards the viewer instead of standing it bolt upright.

That is the third time in this arc a witness has corrected a claim I had
written down as fact — after the 42-position frame shell and the two skull rest
poses. The pattern is worth naming: **the claims that survive unchallenged are
the ones nothing in the render moves against.**

**Measured:** 495 tests (440 lib + 55 app); `blockentityshot` **166/166**,
`itemshot` 28/28, `hurtshot` 38/38, `swingshot` 97/97, `eventshot` 28/28,
`danceshot` 24/24, `mobshot` 243/243; `lightmapshot`, `skyshot`, `tintshot`,
`meshshot` and `dimensioncheck` green; canonical demo SHA-256 `2cc56b4a…`
byte-identical to M15 onward; `git diff --check` clean.

**Excluded:** the display entity is a *model*, not a simulated mob — vanilla
loads it once and never ticks it, so it does not walk, look around, swing or
take damage, and every such input is left neutral. A spawner's smoke and flame
particles need a particle system this client does not have. The **trial
spawner** shares `extractSpawnerData` but has its own block, states and
ominous/normal display logic, and is not covered.

**One block-entity item remains:** an end portal's starfield needs the render
type's shader (`rendertype_end_portal.fsh`, fifteen scrolling samples of
`end_sky.png` faking depth). Its geometry already ships exact.

### 2026-07-27 — M32: the end-portal shader — SHIPPED + VERIFIED

`1519cce`, on `codex/rewo-m19-combat-swings`, not pushed. `blockentityshot`
166 → **172**. **This closes every block-entity item from M25's list.**

M28f shipped the geometry and approximated the shader with one static layer of
`end_portal.png`, saying so. This is the shader.

**It samples in SCREEN space, not model space.** The vertex format is
position-only and `texProj0` is `projection_from_position(gl_Position)`, so the
starfield slides as the camera moves rather than being painted onto the quad.
That is why the portal's mesh UVs were never used, and why this needed a
pipeline of its own rather than a texture swap. The two portals therefore leave
the block-entity resolver entirely — `p6` asserts `draw_for` returns nothing for
them, because leaving them in would draw each **twice**, once textured and once
shaded.

`PORTAL_LAYERS` is **15 for a portal and 16 for a gateway** — a shader *define*
in vanilla, which is why they are two pipelines there and the only difference
between them. Here it is a push constant, so one pipeline serves both.

Sampler0 is `end_sky.png` and Sampler1 is `end_portal.png` — the opposite of
what the render type's name suggests. Both REPEAT, and here that is load-bearing
in a way it usually is not: `end_portal_layer` scales the coordinate by up to
9×, so every layer past the first samples far outside `0..1`.

**I nearly shipped a real bug, and my own comment caused it.** I wrote that
vanilla's `mat4(...)` literals read row-wise in the source and therefore needed
transposing, and rewrote them by assigning elements to the slots they *look*
like they belong in. They do not: GLSL's `mat4` constructor is **column-major**,
vanilla's source *is* GLSL, so column 0 is `(1, 0, 0, 17/layer)` and the
translate genuinely lives at `m[0][3]`. It reaches the coordinate because the
sampling is `texProj0 * matrix` — a **row-vector** multiply, where `v * M` is
`transpose(M) * v`. The rewrite would have put every translate in the wrong slot
while still producing a plausible swirl. Both matrices are literal copies now,
and the comment says why that is the only safe transcription.

`GameTime` is vanilla's **daily fraction**, not a tick count: the layer
translate multiplies it by up to ~17, so a raw tick count would scroll the
starfield thousands of times too fast and read as static noise.

`a4` earned itself again — it failed the moment the portals stopped resolving a
model. Rather than exempt them with a wildcard, the dedicated-pass set is named
explicitly: a catch-all would give back exactly the drift the witness exists to
catch.

**Measured:** 497 tests (442 lib + 55 app; M31 was 495 — `rewo-gpu` +2);
`blockentityshot` **172/172**, `itemshot` 28/28, `hurtshot` 38/38, `swingshot`
97/97, `eventshot` 28/28, `danceshot` 24/24, `mobshot` 243/243; `lightmapshot`,
`skyshot`, `tintshot`, `meshshot` and `dimensioncheck` green; canonical demo
SHA-256 `2cc56b4a…` byte-identical to M15 onward; `git diff --check` clean.

**Excluded.** Vanilla's fog term is omitted because Rewo applies fog in its own
world pass and a second application would double it. Depth uses `GREATER`
rather than vanilla's `LESS`, because Rewo's world pass is reversed-Z — the
comparison matches its own depth buffer, not vanilla's.

The third exclusion — "there is no read-back oracle for the rendered pixels" —
was closed the same day by **M32b**, below.

### M33b — `WeatherAttributes`, where a rainy sky actually greys (2026-07-27)

M33 shipped `AtmosphericFogEnvironment.applyWeatherDarken` — `1 - rain*0.5` on
red and green, `1 - rain*0.4` on blue — transcribed exactly and gated. The live
sky then stayed obviously blue, which is what prompted looking again.

**It was the wrong mechanism.** 26.2 moved weather's visual effect into the
**environment attribute system**: `net/minecraft/world/attribute/
WeatherAttributes.java` defines RAIN and THUNDER as attribute *layers* that
rewrite the resolved values before any renderer reads them.

| attribute | RAIN | THUNDER |
|---|---|---|
| `SKY_COLOR` | `BLEND_TO_GRAY(0.6, 0.75)` | `BLEND_TO_GRAY(0.24, 0.94)` |
| `FOG_COLOR` | `MULTIPLY_RGB(0.5, 0.5, 0.6)` | `MULTIPLY_RGB(0.25, 0.25, 0.3)` |
| `CLOUD_COLOR` | `BLEND_TO_GRAY(0.24, 0.5)` | `BLEND_TO_GRAY(0.095, 0.94)` |
| `SKY_LIGHT_LEVEL` | `ALPHA_BLEND(4.0, 0.3125)` | `ALPHA_BLEND(4.0, 0.527)` |
| `SKY_LIGHT_COLOR` | toward the night colour | stronger |
| `SKY_LIGHT_FACTOR` | `ALPHA_BLEND(0.24, 0.3125)` | same |
| `STAR_BRIGHTNESS` | `set 0` | `set 0` |
| `SUNRISE_SUNSET_COLOR` | `MULTIPLY_ARGB` | stronger |

`BLEND_TO_GRAY(brightness, factor)` greyscales the subject with luma weights
(0.30 / 0.59 / 0.11, truncating), scales that to `brightness`, then `srgbLerp`s
`factor` of the way toward it. At RAIN's `(0.6, 0.75)` the Overworld's
`#78a7ff` goes to `(102, 114, 136)` — the channel spread collapses from 135 to
34. That is **desaturation**, which `applyWeatherDarken` never does; it only
scales, and its blue-favouring ratio actually makes a rainy sky *bluer*.

`applyWeatherDarken` is still real and still applies — but only to the SKY
colour, inside `getBaseColor`, on top of the already-greyed value. M33 had
applied it to the **fog** as well, which double-darkened the fog with a curve
meant for the sky; the fog's own weather change is the `MULTIPLY_RGB` row.

**Three things this corrected beyond the sky.** The lightmap *does* darken in
rain, through `SKY_LIGHT_LEVEL` / `SKY_LIGHT_COLOR` / `SKY_LIGHT_FACTOR` — M33
had concluded it did not, from a `client/`-only grep for `getRainLevel` that
this path reaches through the attribute system rather than directly. Stars are
**switched off** (`.set(0)`), not dimmed. And clouds grey too.

**Two layering details worth not re-deriving.** The levels *partition* rather
than stack — `rainLevel = getRainLevel() - thunderLevel`, so a full
thunderstorm runs the THUNDER row alone. And THUNDER is applied to the RAIN
row's *output*, not to the base.

Measured live at `[9.5, -60, 1.5]` on the local 26.2 server, sampling the same
sky and ground pixels across three runs:

| | sky | ground |
|---|---|---|
| clear | `(119, 166, 255)` | `(113, 128, 90)` |
| rain | `(50, 56, 81)` | `(99, 113, 85)` |
| thunder | `(10, 11, 15)` | `(89, 101, 81)` |

The rain sky is the attribute layer's `(102, 114, 136)` with
`applyWeatherDarken`'s `×0.5 / ×0.5 / ×0.6` on top — the two mechanisms
composing exactly as vanilla composes them.

**The rain fog ramp** — the fourth and last `getRainLevel` consumer — shipped
with it. It is the only one that moves a *distance* rather than a colour, and
it is **stateful**: the multiplier eases toward its target at
`deltaTicks * 0.2`, so fog closes in over roughly five ticks. Two inputs beyond
the rain level are easy to miss — sky light gates it entirely
(`clamp((skyLight - 8) / 7, 0, 1)`, so a cave is clear in a storm), and a biome
that never rains still thickens at half strength.

Getting it right needed a correction of its own. Applying the −160 / −256
offsets to **Rewo's** fog band made rain half-fog the air ten blocks from the
camera. Rewo's band is a *render-distance* fade — its job is dissolving the
chunk edge into the sky — whereas vanilla's `total_fog_value` is the `max` of
that and a separate **environmental** term, and only the environmental one is
what rain thickens. So the world pass gained a second band, carried in the
`LightmapExtra` UBO because the push block is exactly at its 128-byte budget,
and defaulted to *disabled* so clear weather renders exactly as before. At full
rain the environmental band is vanilla's `(0, 1024)` → `(-160, 768)`, which
adds roughly 18–28% fog in the near and mid field where Rewo previously had
none, while the render-distance band still dominates at the edge.

**The `max` is pinned by a pixel oracle**, in `lightmapshot` rather than
`weathershot` — the fog case needs terrain, and `lightmapshot` already renders a
quad at a known distance. Its camera sits at `(8, 80, 8)` looking straight down
at a plane at `y = 64`, so every sampled pixel is exactly 16 blocks away and the
fog fraction is exact rather than approximated. The case renders the quad
unfogged, then predicts each fogged result on the CPU by redoing the shader's
`mix` in linear space and re-encoding — read `[243, 196, 187]` against a
predicted `[243, 196, 188]`.

Four witnesses: the environmental band alone matches its prediction and is
distinguishable from unfogged; the render-distance band at the same numbers
lands on the same colour, so the two really are interchangeable inputs; the two
together land on the **max** and not on a sum, a min or a product, each of which
the case computes and prints; and a nearly-disabled environmental band cannot
pull the render-distance band's result down. Mutation-verified — rewriting the
shader's `max` to `min` reads `[249, 228, 224]` against the case's predicted min
of `[249, 228, 225]`, and to a sum reads `[237, 154, 137]` exactly; both exit 1.

Ground pixels live at `[9.5, -60, 1.5]` for the end-to-end shape:
`(101,115,80)` clear, `(97,108,97)` rain, `(83,92,75)` thunder.

Gates: **561 tests**, `weathershot` 27 → **35/35**, `lightmapshot` extended with
the four fog witnesses, every other gate green with validation ON and 0 VUIDs,
demo SHA-256 `2cc56b4a…` byte-identical.

### M33 — weather and clouds (2026-07-27)

Rain, snow and a cloud deck. Both were listed under §16 "deliberately not
proposed" as self-contained enough to be filler; they are shipped now because
they are also the last thing missing from a *clear-weather-only* sky.

**Three facts that read backwards until you check them.**

`START_RAINING` sets the rain level to **0** and `STOP_RAINING` sets it to
**1**. That is `ClientPacketListener.handleGameEvent` verbatim: the names
describe the server's weather transition, and the client is setting the value
the server's `RAIN_LEVEL_CHANGE` ramp will start *from*. Making it intuitive
would snap rain to full the instant it began.

The client does not interpolate the level at all. `Level.setRainLevel` writes
the clamped value to *both* `oRainLevel` and `rainLevel`, so
`getRainLevel(partialTick)` is a `Mth.lerp` between two identical numbers. The
smoothing is server-side, broadcast every tick the value moves.
`getThunderLevel` does interpolate — and then multiplies by the rain level, so
thunder only darkens weather that is already falling.

Clouds are absent **by attribute, not by dimension check**. `CLOUD_COLOR` is an
ARGB attribute defaulting to 0, and `LevelRenderer` skips the pass when its
alpha is zero; the Overworld sets `#ccffffff` and the Nether and End set
nothing. Same discipline M16 recorded for `sky_color`. It needed an ARGB colour
parser distinct from the existing RGB one, which forces opacity — and that
difference is exactly what decides whether clouds exist.

**What each piece actually is.** A cloud is not a texture: `clouds.png` is a
*map*, one texel per 12×12×4 cell, packed into a `u64` with its four
neighbours' emptiness, and the mesh is three bytes per quad that the vertex
shader expands from a fixed 24-entry table with six fixed face colours.
Transcribed as written, including that `prepare`'s east/west neighbour lookups
wrap `x` against the image **height** — invisible on a square cloud map, and
not ours to quietly fix. A weather column is one camera-facing quad per block
column, whose facing comes from a precomputed 32×32 table of unit
perpendiculars rather than a per-frame billboard.

**Two vanilla details worth not re-deriving.** The per-column seed is
`x*x*3121 + x*45238971 ^ z*z*418711 + z*13761`, and `^` binds looser than `+`
in Java — a xor of two sums, not a left-to-right chain, and reading it wrong
reseeds every column in the world. And a column's alpha ramps to **half** at the
radius rather than to nothing, which is why heavy rain reads as a wall at the
horizon instead of fading out.

**The heightmap had to stop being discarded.** Weather columns run from the
terrain height upward, so `MOTION_BLOCKING` is now decoded — a
`SimpleBitStorage` of 256 entries at `ceillog2(height + 1)` bits storing
`y - minY`, with the wire id taken from the enum's explicit `id` field rather
than its ordinal. Same shape of gap M10 found with `empty_sky`.

**A witness corrected the implementation, again.** The cloud pipeline's
front-face convention was written as `CLOCKWISE` from reasoning about Rewo's
y-flipped viewport. Grading a solid deck from below alone passed at 15,224
covered pixels — it looked entirely right. Grading it from **above as well**
gave 880, and `COUNTER_CLOCKWISE` gives 15,550 / 10,503. The measurement that
settles it also shows `BACK`+CCW is numerically identical to no culling here,
because the mesh only builds faces on the camera's side of each cell — culling's
real job is the inward-wound interior faces, not the deck.

**Deviations, stated rather than implied.** Vanilla renders weather into its own
framebuffer and picks a depth-writing pipeline when shader transparency is on;
Rewo has no such target and uses the no-depth-write branch in the main pass, so
terrain still occludes rain but overlapping columns are not ordered. Vanilla's
fog term is omitted in both new passes, as in every other Rewo pass. And
`LevelChunk.setBlockState` keeps heightmaps current as blocks change while Rewo
does not, so rain over a freshly-dug hole falls from the old surface until the
chunk is resent — confined to weather, since nothing else reads it.

**The live wiring, and what it caught.** Both passes are wired into `rewo live`
(headless and windowed), along with the rain-darkened sky and the
`rainBrightness` celestial fade. `REWO_FORCE_WEATHER=<rain>[,<thunder>]` is the
headless knob — the same shape as `REWO_FORCE_GESTURE` and `REWO_SUMMON` —
because shooting rain otherwise needs an op'd bot running `/weather`.

The first live frame showed **no rain at all**, and the reason is worth keeping.
Vanilla's weather and cloud geometry is **camera-relative**, because its
model-view carries the camera translation; Rewo's `view_proj` already includes
it, so emitting the relative form draws every storm and every cloud deck around
the **world origin**. The gate had not caught it because its scenes sat at
`[0, 0, 0]` and `[0, 4, 0]`, where camera-relative and world-space nearly
coincide. Both passes now emit world space, `weathershot` renders at
`[1536.5, 70, -2048.5]`, and a transform that forgot to add the camera back
cannot pass there.

That fix exposed a second one immediately: the cloud shader's fog fade is
`length(pos)`, correct on a camera-relative position and nonsense on a
world-space one — at 2,500 blocks from the origin every cloud faded to nothing.
The eye is now a uniform and the distance is measured from it.

Verified live against the local 26.2 server: rain renders as camera-facing
streaks that fade with distance, and the cloud pass uploads 209 faces at the
Overworld's `#ccffffff` with the deck at y=192.33. The deck itself is not *in*
that frame — the headless camera looks level and 192.33 over a y=-60 flat world
is 250 blocks straight up, outside a 70° fov at any horizontal distance the
192-block mesh reaches — so the cloud pass's pixels are verified by
`weathershot`, not by that shot.

Gates: **547 tests**, `weathershot` 27/27, and every prior gate green —
`portalshot` 12/12, `blockentityshot` 172/172, `mobshot` 243/243, `eventshot`,
`danceshot`, `swingshot`, `hurtshot`, `itemshot`, `meshshot`, `dimensioncheck`,
`skyshot`, `lightmapshot`, `tintshot`, all with validation ON and 0 VUIDs;
canonical demo SHA-256 `2cc56b4a…` byte-identical to M15 onward;
`git diff --check` clean.

### M36 — the player preview (2026-07-27)

M35 left a black rectangle in the middle of the inventory. That rectangle is
`inventory.png`'s own — vanilla paints it so the model has something to stand
against — and this fills it.

**The transform, composed from two classes.**
`PictureInPictureRenderer.prepare` and `GuiEntityRenderer.renderToTexture`
together are

```text
T(w/2, h/2, 0) . S(s, s, -s) . T(0, bbHeight/2 + offsetY, 0) . Rz(pi) . Rx(yAngle)
```

with `s = guiScale * size` and `size = 30`. The negative z scale turns the model
to face the viewer; `Rz(pi)` is what puts a y-up model the right way up in a
y-down GUI. Vanilla renders that into an offscreen texture and blits it; Rewo
draws it where it belongs and folds the window's placement into the projection
instead, which is one target and one blit less.

**The step that is easy to miss, because it is on the camera.**
`renderToTexture` ends with
`cameraRenderState.orientation = overrideCameraAngle.conjugate().rotateY(PI)`.
Rewo's entity pass takes no camera state, so that half turn has to be applied to
the model instead — and it is not decorative: `bodyRot = 180 + xAngle` already
points the model away from an unturned camera, so the first build rendered
Steve's back. The two are mirror images, which is exactly what the gate asserts.

**A second entity pass, not a mode on the first.** The world's pass carries every
visible entity and is drawn once through one matrix; the preview carries one
model drawn through another. Two `set_draws` calls into a single vertex ring
would leave the first draw reading the second's vertices, so the preview owns an
`EntityPass` of its own, built the first time the screen opens. Its atlas is
separate, which is also why the skin needs its own upload — a UV from the world's
atlas would land on some mob's texture.

**Depth.** The preview shares the frame's depth buffer with the world, so
without a `vkCmdClearAttachments` over its window the model would be depth-tested
against whatever terrain sits behind the panel and come out sliced by a hillside.
Cleared to **0.0**, the far plane, because Rewo is reversed-Z throughout — and so
is vanilla here: `Projection.getMatrix` reads `near = this.zFar; far = this.zNear`,
swapping the two on purpose.

**Verified by measuring, again.** The first render looked both too large and
mispositioned. Measuring the model's bounding box against the placement computed
from the decompile put the feet at 191.6 px down a 210 px window and the head at
29.6, matching to within the saturation filter's slack — the size was right and
the eye was wrong. What *was* wrong was the facing, which no amount of squinting
at proportions would have found.

Gate: **`rewo inventoryshot --check` 39 -> 44 witnesses** — the window against an
independent transcription of `InventoryScreen`'s call, its scaling with the
panel, the `atan`-bounded mouse follow (at most 31.4 degrees, so it can never
turn away), the feet and head projected through the production matrix against an
independently computed placement, and the camera half turn as a mirror pair. One
formulation was written and rejected: it asserted the mirrored point flips sign
about clip zero, which is not the property — the window sits left of screen
centre, so both land negative and a matrix with no half turn would have passed.

Measured: **586 tests**; all fifteen serverless gates green with validation ON;
demo SHA-256 `2cc56b4a...` byte-identical to M15 onward; live
`play --light-check --no-relight --no-build` 884,736 cells / 0 mismatches and
`CORRECTIONS 0`. Verified in-frame with a real fetched skin
(`REWO_PREVIEW_SKIN=<username|url>`), slim model and all, and with
`REWO_MOUSE=x,y` to photograph the follow.

**Open.** The model stands still: vanilla poses it from the live player's render
state, so a walking player's legs move in the preview too, and Rewo has no
local-player animation state to read. Lighting is Rewo's entity shading rather
than vanilla's `Lighting.Entry.ENTITY_IN_UI` rig. Armour is not shown on the
model — armour items have no baked geometry at all yet (their `select` trim
definitions are among M22's 147 suppressed). And in a live session the skin is
uploaded only when one is available; an offline-mode server carries no textures
property, so the preview wears the default there, as vanilla does.

### M66 — the remaining tooltip stages: advanced, container, held-item (2026-07-29)

The three stages M54 (the language layer), M56 (the image pass) and M58 (the
bundle chrome) left. `inventoryshot` grew from 131 witnesses to **143**.

**It was written as M62 and rebased.** Two agents ran in parallel and both
decoded `minecraft:container` — this one and M63's. Main carries M63's, this
one dropped its copy rather than being hand-merged, which is the right way
round: an independent second protocol decoder is exactly where a silent desync
gets introduced. What survived the drop is recorded under stage 4 below. The
number moved because M60–M65 were all taken by the time it landed.

**Stage 3 — the advanced block (F3+H).** `ItemStack.addDetailsToTooltip` ends
with three lines whose *order* is the transcription and whose arguments are the
trap. `item.durability` takes **remaining then max**
(`getMaxDamage() - getDamageValue()`, `getMaxDamage()`), and swapping them
still renders a well-formed `Durability: 1561 / 1500`. The registry key is a
`Component.literal` in DARK_GRAY, not a translation key — running it through
the language file finds nothing and `TranslatableContents`' fallback then
prints the key anyway, so the mistake is invisible until a datapack defines
that key. The old `item.nbt_tags` line is gone from 26.x entirely.

**The count is the merged map's, not the patch's.** `int count =
this.components.size()` reads a `PatchedDataComponentMap`, whose `size()` is

```java
int size = this.prototype.size();
for (entry : this.patch) {
   if (entry.getValue().isPresent() != this.prototype.has(entry.getKey()))
      size += entry.getValue().isPresent() ? 1 : -1;
}
```

The prototype dominates it. An unpatched dirt reads **12 component(s)** and a
diamond sword **18** — reading the patch's own entry count would print `0` for
nearly every stack in the game, and `count > 0`'s suppression branch would then
fire constantly instead of never. This needed data the wire does not carry, so
**`tools/gen_item_components.py`** extracts it: twelve components are on all
1,537 items and only 47 more exist at all, so the table is those twelve once
plus a 64-bit mask per item — 912 non-universal entries instead of 19,356
strings. Every item is listed, including the 1,091 with an empty mask, because
*membership is an answer*: an item the table does not know drops the line
rather than guessing the base count.

**Stage 4 — the container's lines.** `ItemContainerContents.addToTooltip`'s
guard is `lineCount <= 4` **with the increment inside it**, so five stacks fit
and the sixth becomes `and 1 more...`; `lineCount < 4` loses a line *and*
invents a remainder under the four it kept. Both keys resolve **only** through
M54's `deprecated.json` rename — `en_us.json` still spells them
`container.shulkerBox.itemCount`/`.more`, so a raw read produces no container
block at all (witness `l1` pins the rename, `cn3` pins that the lines come out).

The decode is M63's, and its positional shape is the right one: the gaps stay
as `None` because `ItemContainerContents.items` is indexed by slot number, and
the tooltip filters them at the point of use (`nonEmptyItemsStream`).

**One capability was kept from the dropped copy.** M63's `ContainerSlot`
captured `custom_name` alone, on the stated reasoning that `ITEM_NAME` is
"answered by the item table on the rendering side". Verified rather than
assumed, and it is half right: the item table answers `item.getName()`, which
*is* the prototype's `item_name`, but a **patched** `item_name` is a different
value that nothing else carries. `getHoverName` is two levels of override, not
a name and a fallback:

```java
getHoverName() = getOrDefault(CUSTOM_NAME, getItemName())
getItemName()  = getOrDefault(ITEM_NAME, item.getName())
```

so `ContainerSlot` gained an `item_name` and a `hover_name` helper, and
`read_container_slot` takes a `NameIds` pair rather than one id — a pair
because two loose `i32`s in a call is exactly where they get swapped, and a
swap would render every renamed stack under its default name and look like a
missing feature rather than a bug. `walk_patch_with` needed no change: both
components are `fromCodecWithRegistries` chat components, i.e. one
`Shape::NbtTag` each, so the capture reads exactly what the walk would.

**The carrier.** M61 recorded that its blocker had *moved* from the decode to
the carrier, and that is still true: `ItemSlot` is `Copy` and `SlotText` would
need its `is_empty` taught a new field. This milestone does not touch
`rewo-world` at all, so the container slots and the raw patch ids ride a
**third** carrier — `rewo_net::item_stack::StackDetails`, keyed by the same
component fingerprint, owned by whoever routes the packets. Same shape, same
lifetime, one more table; `route_inventory` takes it as an `Option` so a caller
that draws no tooltips passes `None` and the decode is unchanged either way.
M63 was decode-only, so nothing else was competing for the wiring.

**Stage 5 — the held-item label.** Two clocks and a placement rule.
`Hud.tick`'s re-trigger is three-part —
`last.isEmpty() || !selected.is(last.getItem()) || !selected.getHoverName().equals(last.getHoverName())`
— and it is the **third** clause that matters: comparing item identity alone is
the obvious reading, and an anvil rename hands back the same item, so the new
name would never appear. The fade is `min(255, timer * 256 / 10)`, i.e. opaque
for thirty of the default forty ticks and linear over the last ten; spreading
it over the whole timer makes the label translucent the moment it appears *and*
overflows 255 with no clamp to catch it. The row is `guiHeight() - 59`, **plus
14 when `!canHurtPlayer()`** — and `canHurtPlayer()` is
`localPlayerMode.isSurvival()`, which is SURVIVAL **or ADVENTURE**.

F3 became a **modifier**: `keyDebugModifier` and `keyDebugOverlay` are the same
key, so vanilla toggles the overlay on *release* and skips it when a chord
already fired. Toggling on press, which is what Rewo did, would flip the
overlay every time you pressed F3+H.

**A bug the transcription found.** `isDamageableItem()` is
`has(MAX_DAMAGE) && !has(UNBREAKABLE) && has(DAMAGE)`, and `isBarVisible()` is
`isDamaged()` — so an **Unbreakable** tool draws no durability bar however much
damage it carries. M41's `item_bars` read the damage alone and drew one. Fixed,
along with `getDamageValue()`'s clamp to the maximum.

**Gates.** `inventoryshot --check` 131 → **143**, every new witness naming a
mutation partner that is *actually executed*: the durability args swapped
(renders `Durability: 61 / 1561`), the `count > 0` guard dropped (renders
`0 component(s)`), the id run through the language file (finds `None`), the
count read as the patch size (`(0, 1, 1)` against `(12, 13, 18)`), the hover
name resolved from `custom_name` alone (loses the middle stack's patched
`item_name`), `lineCount < 4` (`(4, 1)` for five stacks), the remainder as the
total, the raw `en_us.json` (0 lines), the identity-only timer comparison (38
where vanilla resets to 40), and a fade over all 40 ticks (`256` at the top).
The container *wire* decode is not re-graded here — M63's `ct1`–`ct4` already
pin the optional list, the gaps and the alignment to the byte, and a second
copy of those witnesses would be the same duplication the decoder itself was.

**1037 tests** (from 1019); all 21 other gates green with validation ON and 0
VUIDs — capeshot 64, itemshot 62, healthbarshot 33, attributeshot 43,
captureshot 17, blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35,
handshot 34, particleshot 34, eventshot 28, danceshot 24, portalshot 12,
hudshot 41, mobshot 243/243 + emissive 5 + etf 8 + tint 4, plus skyshot,
lightmapshot, tintshot, meshshot, dimensioncheck. Demo PNG SHA-256
`2cc56b4a…` byte-identical to M15 onward.

**Two hand-counted numbers were wrong and a machine caught both.** The
generator's emitted test first claimed 13 components for dirt and the gate's
`ad5` first claimed 19 for a sword; both are off by one from the report. Fixed
by deriving them — the generator reads the two counts out of the report, and
`ad5` asserts **deltas from the table** rather than absolutes. Hand-counting a
JSON object is exactly the operation a gate exists to remove.

**Open.**

- **No live sighting.** Nobody has pressed F3+H in a running client, hovered a
  shulker box, or watched the label fade. The gate is authoritative for the
  properties it names and for nothing else.
- **`minecraft:tooltip_display` is decoded-and-discarded**, so
  `display.shows(DataComponents.DAMAGE)` is hard-coded true. A server hiding
  the durability line would still see it. This predates the milestone — every
  component line M40 onward ships ignores the same gate.
- **ITALIC does not reach the HUD.** The tooltip's span model carries it (the
  container's `and N more...` really does slant through the Velvet pass), but
  `text.rs` has one colour per line and no italic face, so a `CUSTOM_NAME`
  stack's held-item label stands upright. Colour and fade do carry. Same gap
  M42 records for the bitmap tooltip fallback.
- **`textWithBackdrop`'s fill is never drawn.** That is not a divergence at
  defaults — `getBackgroundColor(0.0F)` is zero while `backgroundForChatOnly`
  is set, which it is — but Rewo has no options file, so a player who set "Text
  Background: Everywhere" would get nothing. The rule is transcribed and both
  arms are graded; only the input is pinned.
- **`advancedItemTooltips` does not persist.** Vanilla writes it to
  `options.txt`; Rewo resets it each session.
- A container slot's *other* components are still reduced to one bit, so a line
  cannot show an enchanted sword's colour — M63's exclusion, unchanged by the
  `item_name` addition.

### M63 — the container's contents, and a flake that was hiding real failures (2026-07-29)

Three jobs in `rewo-net` + `rewo-data`. The first turned out to be already
done, and saying so is the finding.

**The codec table is complete, and that is now measured rather than counted.**
M41 shipped 97 of 111 components and recorded 14 missing, 7 of them
"never network-synchronised"; M52e then closed the other 7. Both halves of
that claim were **inherited**, so M63 re-derived them from scratch: every
`= register(...)` in `DataComponents.java` parsed with its balanced-paren body,
each classified by whether that body contains `.networkSynchronized(`. The
answer is **111 registrations, 104 synchronised, 7 not** — and the 104 names
match `CODECS` exactly, name for name, with **0 missing and 0 extra**. A third,
independent reading agrees: the datagen `registries.json` report lists 111
entries, and `synchronised ∪ never == registry` with the two sets disjoint.
The seven that a server can never send are `custom_data`,
`intangible_projectile`, `map_decorations`, `debug_stick_state`, `recipes`,
`lock` and `container_loot` — each verified individually as `b.persistent(…)`
with no `.networkSynchronized(…)` anywhere in its registration. **The real
missing-codec count is zero**; nothing was transcribed because nothing was
missing.

**`minecraft:container` now keeps what it walked past.** The shape was already
there, so a shulker box parsed without desynchronising — but
`walk_item_template` discarded the id, the count and the nested patch, and
`ItemContainerContents.addToTooltip` needs all three: it counts the occupied
slots, renders `item.container.item_count` from `itemStack.getHoverName()` for
the first **five**, and adds one italic `item.container.more_items` for the
rest. A hover name can only come from the *nested* patch, so this is the first
capture that reads a component **inside** another component's value.

**Two ways it differs from M61's bundle, both invertible.** A container slot is
an `Optional<ItemStackTemplate>` where a bundle's is a bare one — one presence
byte per slot — and the empty ones are **kept as `None` rather than dropped**,
because `ItemContainerContents.items` is indexed by slot number and `copyInto`
reads it positionally. The tooltip is the one consumer that does not care, and
it filters (`nonEmptyItemsStream`), which is why the filter lives in the
accessor and not in the walk.

**The capture shares the patch loop rather than repeating it.**
`walk_patch_counted` became a thin call on a new `walk_patch_with`, which
offers each added entry's type id to a closure before `shape_for_id` sees it.
One body, so a capturing walk and a plain one consume identical bytes **by
construction** — the same reason M61 gives for `read_item_template`, and it
matters more here than anywhere else: the patch has no length prefix, so a
capture that read one byte differently would park the reader mid-value and turn
every stack after it in the packet into garbage. The one span the capture reads
itself is the `custom_name` tag, and it reads exactly what `Shape::NbtTag`
would.

Only `custom_name` is captured. `getHoverName` is
`getOrDefault(CUSTOM_NAME, getItemName())` over
`getOrDefault(ITEM_NAME, item.getName())` — a two-level override, and the lower
two levels are answered by the item table on the rendering side. A patch that
*removes* `custom_name` leaves it `None`, which is the same answer as an absent
one and correct for the same reason: both send `getHoverName` to the next
fallback.

Gate: `inventoryshot --check` 127 → **131** (`ct1` alignment-and-content,
`ct2` gaps kept while the tooltip view skips them, `ct3` absent ≠ empty ≠
removed, `ct4` fail-closed on an unwalkable nested component), plus 10 unit
tests. **Three mutations were run, not just named**: reading the name as a
`Str` instead of a tag kills 5 witnesses, dropping the empty slots kills 5, and
omitting the `Optional` presence byte kills 7; the gate itself was
mutation-checked separately and drops to 129/131 with a non-zero exit.

**The flaky test: a machine-wide path, not a timing bug.** `item_tags`'s
fixture helper wrote `std::env::temp_dir().join("rewo-item-tags-test")` — one
**fixed, absolute** path — and every caller writes the fixture then reads it
back. `fs::write` opens with `O_TRUNC`, so a reader arriving between the
truncate and the bytes sees a **zero-byte file**, and `Items::load` fails with
`EOF while parsing a value at line 1 column 0`. That is the exact panic the
wild failures carried. It has two independent reaches: the two tests in the
binary run on separate threads, and `temp_dir()` is machine-wide, so every
concurrent sweep on the box — another worktree's `cargo test`, a re-run
overlapping the last — landed on the identical file. That is why it presented
as "roughly one run in four" rather than as anything reproducible, and why it
masked real failures instead of reporting itself.

It would not reproduce at 1 or 4 processes on an idle machine (40/40 and 60/60
green); it took **10 concurrent processes** to catch it once in 100 — which is
itself the evidence for the diagnosis, since the window is a few microseconds
wide. The fix is a per-call directory keyed by **process id *and* an atomic
counter**; both are needed and neither alone closes it. Measured on a stress
witness that makes the latent race deterministic: **before 0/20, after 30/30**
sequentially and **100/100** at the 10-process contention that originally
reproduced it.

**956 tests** (944 + 12); every gate green with validation ON and 0 VUIDs;
demo PNG byte-identical to M15 onward.

**Open:** the tooltip *lines* themselves — `item.container.item_count` and
`item.container.more_items` are decoded but not rendered, which is the tooltip
milestone this was groundwork for. A nested slot's own components beyond
`custom_name` are still walked and discarded, so a container holding an
enchanted or renamed-and-damaged stack shows the name and nothing else.

### M58 — the bundle tooltip's cell chrome (2026-07-29)

Shipped. M56 computed every rectangle of `ClientBundleTooltip`'s grid and
blitted none of them, because the six sprites it needs live behind
`rewo-data`'s jar bake and that file was fenced off. This is the six blits, and
what they turned out to be.

#### The draw order, and the names

`extractImage` splits on `contents.isEmpty()`, and neither arm is the other with
a loop removed:

```java
// extractBundleWithItemsTooltip — per visited cell:
if (hasHighlight) blitSprite(SLOT_HIGHLIGHT_BACK_SPRITE,  drawX, drawY, 24, 24);
else              blitSprite(SLOT_BACKGROUND_SPRITE,      drawX, drawY, 24, 24);
graphics.item(item, drawX + 4, drawY + 4, slotIndex);
graphics.itemDecorations(font, item, drawX + 4, drawY + 4);
if (hasHighlight) blitSprite(SLOT_HIGHLIGHT_FRONT_SPRITE, drawX, drawY, 24, 24);

// extractProgressbar — the fill first, the border over it:
blitSprite(getProgressBarTexture(weight), x + 1, y, getProgressBarFill(weight), 13);
blitSprite(PROGRESSBAR_BORDER_SPRITE,     x,     y, 96, 13);
centeredText(font, getProgressBarFillText(weight), x + 48, y + 3, -1);
```

All six identifiers carry a `bundle/` path segment, and for two of them **that
segment is the whole difference between two different files**:
`container/bundle/slot_highlight_back` and `..._front` are not the
`container/slot_highlight_back` / `_front` pair M35 already loads for the
inventory's hover box. Both pairs exist in the jar, both are 24x24, and reusing
the inventory's would have rendered something that looks approximately right.
The six are `container/bundle/{slot_background, slot_highlight_back,
slot_highlight_front, bundle_progressbar_border, bundle_progressbar_fill,
bundle_progressbar_full}`.

**Four things in that order are not the obvious reading.**

- **The badge cell gets no chrome at all.** The brief flagged this as an
  assumption to check; the check comes out on the side of the brief's
  suspicion, for a reason that reads clearly once found: `extractCount`'s entire
  body is one `centeredText`. Only `extractSlot` blits, so the `+N` sits
  directly on the tooltip's own background with no cell art under it, and the
  grid's art stops at the last occupied slot rather than filling the frame.
- **The highlight replaces the background; it does not cover it.** The `if
  (hasHighlight) … else …` picks *one* sprite. That distinction is invisible in
  a sprite sheet and loud in the render, because `slot_highlight_back` is white
  at **alpha 96** and `slot_highlight_front` white at **alpha 32**, where
  `slot_background` is opaque `(33,26,35)`. Drawing both would put the ordinary
  dark slot art under the highlight — a different picture, and one that no
  longer shows what is behind it.
- **The bar's fill goes down first and the border over it.** The border art is a
  hollow 1 px frame, so the two overlap along every edge and the second blit
  wins. Reversing them paints the fill's own top row across the frame.
- **`getProgressBarFill` scales by 94, not by the bar's 96**, and truncates —
  `Mth.mulAndTruncate`. A quarter-full bundle is 23 pixels, not 24. The `x + 1`
  is `PROGRESSBAR_BORDER`, which is what leaves the frame a pixel to sit in.

And the two fills are **not two shades of one bar**: `bundle_progressbar_fill`
is the chat palette's blue `5555FF` and `bundle_progressbar_full` its red
`FF5555`. A single sprite tinted by the weight lands on neither.

The empty arm is a separate method with no cell loop, and it hands
`extractProgressbar` a literal **`Fraction.ZERO`** rather than the bundle's own
weight — which for an empty bundle happens to be the same number, but is not
written as one and is what the transcription copies.

#### Nine-slice, and the branch three of the six always take

All six declare `nine_slice` in their `.mcmeta` and **none sets
`stretch_inner`**, so vanilla tiles their five inner pieces. Two consequences:

- `blitNineSlicedSprite`'s first branch is `width == nineSlice.width() &&
  height == nineSlice.height()` → blit the whole sprite once, no slicing. The
  three 24x24 cell sprites are blitted at exactly 24x24, so they take it every
  time and their declared `border: 4` never affects anything. Only the bar's
  three are really sliced: a 12x12 border into 96x13 and a 6x6 fill into
  `fill`x13.
- Rewo emits one **stretched** quad per piece where vanilla tiles. The two are
  the same picture exactly when each inner slice is uniform along the axis it
  repeats on — which every one of these sprites is, but as a fact about 26.2's
  art rather than a theorem. So the gate measures it against the real jar
  (`bc8`) instead of a comment asserting it, and proves the measurement
  discriminates by flipping one texel of the fill's centre and watching the same
  check reject it.

`container::nine_slice` was generalised from its hard-coded 100x100 to take the
sprite's authored size, and gained the identity branch. `push_quad` became
all-six-corners-or-none: it silently dropped vertices past `MAX_VERTS`, which
would have left a torn triangle rather than one fewer quad, and M58 adds up to
31 quads to the frame's worst case (the cap moved 1024 → 2048).

#### What is wired, and the one thing that is not

`ContainerPass::set_state` now takes a `TooltipDraw { pos, size, bundle }` — one
struct rather than two setters, so a box and the grid inside it cannot be set
out of step. The chrome is emitted into the tooltip's own vertex range, so it
draws over the panel, the icons and the carried stack, as vanilla's tooltip
stratum does.

**The grid has no contents to draw from, and that is a `rewo-net` gap, not a
render one.** `BundleItem.getTooltipImage` reads `minecraft:bundle_contents`;
`component_wire` has a `Shape` for it (`List(ItemStackTemplate)`) so the patch
parses without desyncing, but `walk_item_template` *discards* the id, the count
and the nested patch it reads. So a live bundle would produce a grid of nothing
— worse than no grid — and `screen_tooltip` passes `bundle: None` with the
reason written at the call site. The gap between the two highlight blits, where
`graphics.item` and `itemDecorations` go, is empty for the same reason. The
whole of `bundle_image` + `bundle_chrome` is driven and graded by
`inventoryshot`; wiring it to a live stack waits on a `BUNDLE_CONTENTS` decoder.

Also not shipped: `extractSelectedItemTooltip`, which nests a *whole second
tooltip* at `y - 15` carrying the selected item's name — it needs the same
contents. The `+N` badge and the bar's "Empty"/"Full" labels are text, so they
belong to the text pass and have the same blocker.

#### Verification

Nine new witnesses in `inventoryshot`, **118 → 127**, all pixels against the
production walk. Every mutation each one names was **run**, not asserted:

| mutation | witnesses it fails |
|---|---|
| blit nothing (the pre-M58 state) | bc1, bc2, bc4 |
| fill the columns left to right | ti5, ti6, ti8, ti9, ti12, **bc1** |
| give the badge cell a background | bc2 |
| key the highlight off the visit order | **bc3** (its two answers swap), bc4 |
| draw the background *and* the highlight | bc4 |
| blit the fill at `x` rather than `x + 1` | bc5 |
| scale the fill by 96 rather than 94 | ti10, bc5 |
| one fill sprite for every weight | bc5, bc6 |
| border first, fill over it | bc7 |
| pass the weight to an empty bundle's bar | bc9 |

`bc8`'s mutation runs inside the witness. `bc4` is the one that needed a shape
rather than a colour prediction: taking the tooltip's box out from behind an
otherwise unchanged grid changes the selected cell's interior and cannot change
an opaque one, so "replaced, not covered" is observable without reproducing an
alpha blend in a gate.

**708 tests** (705 + 3 new in `rewo-gpu`, pinning the identity branch, the
exact tiling of the sliced pieces however the borders shrink against a small
destination, and a zero-width fill emitting no geometry). All gates green with
Vulkan validation ON and **0 VUIDs** — `itemshot` 62, `inventoryshot` **127**,
`blockentityshot` 172, `swingshot` 97, `hurtshot` 38, `weathershot` 35,
`handshot` 34, `particleshot` 34, `eventshot` 28, `danceshot` 24, `portalshot`
12, `captureshot` 17, `attributeshot` 43, `mobshot` 243/243 + emissive 5 + ETF 8
+ tint 4, plus `skyshot`, `lightmapshot`, `tintshot`, `meshshot`,
`dimensioncheck`. Demo PNG SHA-256 byte-identical to M15 onward
(`2cc56b4a…`).
### M59 — the health bar's render half, and a gate with no oracle (2026-07-29)

M55 shipped the data (`update_attributes`, `AttributeModifier.Operation`, the
`RangedAttribute` clamp) and called itself "the data half of a health bar".
This is the other half: the drawing, the resolver that decides whether to draw
at all, and `rewo healthbarshot --check`.

**It is the first Rewo feature with no vanilla behaviour to transcribe.**
Every gate before it predicts an answer from an independent reading of the 26.2
decompile and asserts the render matches; that method has nothing to bite on
here, because **vanilla renders no health bar over any entity** — hearts, a
server-driven boss bar, a horse's inventory screen, and nothing else. So the
numbers were written down first, as a decision, in
[`REWO_HEALTH_BAR_SPEC.md`](REWO_HEALTH_BAR_SPEC.md), and the gate grades the
render against *that*. The gate re-declares the spec's constants rather than
importing `entities.rs`'s: a gate that imports the implementation's constants
asserts only that the implementation equals itself, which is M41's `t4` failure
mode exactly.

**Every spec number was used verbatim**: `BAR_W` 40, `BAR_H` 3, `BAR_PAD` 1,
`BAR_GAP` 2, `CRITICAL_FRAC` 0.25, plate `[0,0,0,0.25]`, healthy
`[0.85,0.20,0.20,1]`, critical `[0.95,0.55,0.15,1]`, anchor
`pos.y + height + TAG_LIFT`. Nothing was adjusted.

**The render is `push_tag` twice over** — the spec's own framing, and it held:
no new geometry type, texture, pipeline or blend state. `push_health_bar` is a
literal sibling of `push_tag`, emitting into the same alpha-blended
**text range** (not the solid range world-space sign text takes), on the same
camera basis, in the same font-pixel units, sampling the same
guaranteed-opaque white texel. The whole emitter is layout and arithmetic.

**Two spec witnesses are unobservable, and the reason is the spec's own rule
3.** Both are recorded in the gate's detail strings rather than quietly
dropped:

- *"exactly `BAR_W` at max"* (the monotonicity row). Rule 3 hides the bar at
  `fraction >= 1`, so `BAR_W` is the fill's **supremum** and never an emitted
  value. `b3` asserts the strongest observable statement instead: the last
  visible sample (19/20) is exactly `0.95 * BAR_W = 38.0` px, and 20/20 emits
  nothing.
- *the **upper** clamp* (the clamping row). `clamp(_, 0, 1)`'s ceiling and rule
  3's `>= 1.0` hide the same set, so an unclamped division is indistinguishable
  from a clamped one from outside. `b5` grades rules 1 and 3 composed and says
  so; the clamp stays as the spec writes it, defensively. The **lower** clamp
  *is* observable and is a real witness (`b4`): unclamped, health −5/20 gives a
  −10 px fill escaping the plate to the left.

**One spec ambiguity, resolved toward the operative sentence.** The anchor row
says the bar sits `BAR_GAP` "below **it**" with a tag, "at the anchor itself"
when not — so *it* is the anchor, and the implementation drops the plate's top
edge to `-BAR_GAP`. The name column's gloss calls `BAR_GAP` "the vertical gap
between the bar and the nametag above it", and by that reading the gap would be
measured from the tag plate's bottom edge at `-1`, putting the bar one pixel
lower. The two readings differ by exactly `BAR_PAD`. The contrast with "at the
anchor itself" only parses under the first, so that is what shipped.

**The gating fell out of two things already built.** Spec rule 5 suppresses a
bar wherever a nametag is suppressed, and in 26.x that needs almost no new
machinery:

- **Living-only is free.** `rewo_world::attributes::resolve` answers `None`
  when `DefaultAttributes.SUPPLIERS` has no entry for the type, so a boat has
  no max health and therefore no bar — with no `matches!` list to keep in sync.
- **The name-tag distance is itself an attribute.**
  `EntityRenderer.extractNameTags` gates on
  `distanceToCameraSq < Mth.square(nameTagDistance)` where `nameTagDistance =
  entity.getAttribute(Attributes.NAME_TAG_DISTANCE).getValue()` — a
  `RangedAttribute` defaulting to **64.0** over `[0, 512]`, already in the
  generated supplier table. So it resolves through M55's machinery, modifiers
  and clamp included. `f9` is the witness that matters: syncing
  `name_tag_distance = 128` must make a mob at 100 blocks show, which a
  hard-coded 64 fails while still passing the 63.99/64.01 pair.
- **One genuinely new wire input**, and it was already on the floor:
  `Entity.DATA_SHARED_FLAGS_ID`, metadata **index 0, BYTE**, `FLAG_INVISIBLE =
  5`. `metadata::parse` has decoded it since M1 into `EntityMeta::flags` and
  `apply_set_entity_data` **discarded it**. No kind gate on the way in — index
  0 is `Entity`'s own first `defineId`, so every entity claims it and nothing
  else can.

**Spec rule 4 is the load-bearing one and it is a refusal, not a fallback.**
Only a max health an `update_attributes` actually established draws a bar:
`Source::Default` is rejected even though the supplier's 20.0 is a real number
for every living entity. Rewo cannot distinguish "the server never sent health"
from "this mob has 1 HP" either — `DATA_HEALTH_ID` is seeded at `1.0F` — so a
bar with an unverified denominator would be a confident lie in both directions.
`f1`/`f2`/`f3` pin it: the same entity, before and after one packet, plus the
explicit health-1.0 case the spec names.

**The mutation runs found a bug in two of my own witnesses.** All thirteen
named partners were actually run — source broken, rebuilt, gate re-run,
reverted — and each fails its partner. The `swapped_division` run
(`max/current`) hid the bar entirely, and `e3`/`e4` **passed anyway**: they were
`base.iter().zip(&moved).all(...)`, and `zip` over two empty vectors makes
`all` vacuously true. A witness that is vacuous when its subject disappears is
the M45/M41 failure in miniature — the length check is now load-bearing, not
defensive.

**Process trap worth recording: a mutation script that restores source but not
the build.** After the last mutation run the tree was clean and
`target/debug/rewo.exe` still carried `swapped_division`. A full 23-gate sweep
then ran on that binary; every other gate passed (none touch health bars) and
`healthbarshot` reported 12/33 while a direct run minutes earlier had reported
33/33. The tell was a gate that passes standalone and fails in a batch with an
unchanged tree — **rebuild before believing a sweep**. The sweep was redone
clean.

**Measured.** `healthbarshot --check` **33/33** (Vulkan validation ON, 0 VUIDs),
two consecutive clean runs. **705 tests** unchanged (the gate covers the new
world state through the production path, so no duplicate unit tests were
added). All 23 gate runs green with 0 VUIDs: itemshot 62, inventoryshot 118,
blockentityshot 172, swingshot 97, attributeshot 43, hurtshot 38, weathershot
35, handshot 34, particleshot 34, eventshot 28, danceshot 24, captureshot 17,
portalshot 12, mobshot 243/243 + emissive 5 + etf 8 + tint 4, plus skyshot,
lightmapshot, tintshot, meshshot, dimensioncheck. Demo PNG SHA-256
byte-identical to M15 onward.

**Open.** No live in-world sighting — the gate is authoritative for the
properties it names, and nobody has yet watched a bar over a real mob on a real
server. Not gated: scoreboard team name-tag visibility and
`canSeeFriendlyInvisibles` (Rewo decodes no teams), `isDiscrete()`'s 32-block
sneak cut-off, `isVehicle()`, `hud.isHidden()`. The live call site passes the
eye→feet distance in one expression that no witness covers — the comparison is
graded, the sourcing is not. And the spec's own exclusions stand: no numeric
text, no local player, no boss bars, no armour/absorption.

### M61 — the wavy cape (2026-07-29)

Shipped and gated, **opt-in** (`rewo live --wavy-cape`, or `REWO_WAVY_CAPE=1`).
Vanilla's rigid slab is still the default and is still what M60's 38 witnesses
grade; the flag cannot reach them, because with it off `EntityTable` allocates,
ticks and returns nothing and `resolve_cape` hands the renderer `wavy: None`.

Second Rewo feature with **no vanilla oracle**, after the health bar.
[`REWO_WAVY_CAPE_SPEC.md`](REWO_WAVY_CAPE_SPEC.md) is the source of truth and
was written first; the mod that popularised the behaviour is
reference-unsafe under `REWO_FEATURE_SURVEY.md` §2 and none of it was read.
The design is textbook constrained-particle cloth — Verlet 1967, Provot 1995,
Jakobsen 2001 — over vanilla's own already-gated cape state.

**The reduction rule is the whole safety net, and it is a *bypass*.** At one
segment `emit_cape` returns to the M60 code unchanged, so the vertices are the
vanilla cape's bit-for-bit rather than within a tolerance of it. The spec's
first draft said the reduction would hold "with infinite stiffness"; it would
not, and `w18` measures why — a settled one-segment chain hangs **5.843°** off
the vanilla spine (6° of rest tilt, less the 0.157° the push-out lifts the free
end by), putting the hem 1.63 model units away. Stiffness fixes a link's
*length*, never its orientation.

**Cape space is the load-bearing choice.** The chain lives in a frame that is
world-axis-aligned, measured in model units, and **translates with the player
without rotating**. In a body-attached frame a pure yaw rotates the whole chain
rigidly and there is no wave at all; here the anchor orbits the body axis
(`w4`: radius 2.4973 moves axis for axis on a quarter turn) and the chain has to
catch up. The frame's absolute Y is deliberately unobservable — gravity is
uniform, the torso cylinder infinite, the clamp anchor-relative — so the
renderer re-pins joint 0 onto the true attachment point (which carries the
animated body transform, the clearance shift and the death roll, none of which
a world tick can see) and translates the rest rigidly.

**`TORSO_RADIUS = 2.5` is not an arbitrary number.** The vanilla spine leaves
its pivot at radius **2.49726**, so the push-out cylinder grazes the rest pose
by three thousandths of a pixel and bites only when the chain swings inward
(`w5`). A 180° turn takes a joint to radius **0.458** with the push-out
disabled — clean through the player's chest.

**`GRAVITY` acts along world down.** Reading "downward" as the *cape's* local
down would make the rest state exactly the vanilla drape — and would cost the
reduction rule its mutation partner, since a single simulated segment would
then settle onto the vanilla angle instead of hanging straight down. The
strong witness wins over the weak one; the price is `y2`, where the rest
silhouettes are IoU **0.9750** rather than 1.000, the residue being 6° of tilt
lying nearly along the camera's view axis.

**`ANCHOR_ACCEL` is the number the spec was missing, and it is derived.** The
first two drafts said the acceleration was "gravity plus the delta", with no
conversion — which flies the cape to **80.9°** from vertical on a drift the
vanilla cape renders as 5°, because a gap in *blocks* is a hundred times
gravity. Vanilla maps the gap linearly (`capeLean = 100 · delta` degrees); a
chain under gravity `g` with horizontal acceleration `a_h` settles at
`atan(a_h/g)`, which is `a_h/g` radians at small angles. Equating the two:

```text
a_h  =  GRAVITY · 100 · pi/180 · delta   ≈  0.0139626 · delta
```

Written in the source as that expression, not as the number, so a reader can
check it. Measured before and after:

| gap (blocks) | context | raw delta | with `ANCHOR_ACCEL` | vanilla's `capeLean` |
|---:|---|---:|---:|---:|
| 0.05 | drift | 80.9° | **4.99°** | 5.0° |
| 0.20 | slow walk | 87.7° | **19.24°** | 20.0° |
| 0.40 | walk | 88.9° | **34.92°** | 40.0° |
| 0.864 | sprint | 89.5° | **56.45°** | 86.4° |

**The divergence at larger gaps is intended and is asserted, not tolerated.**
`atan` is the angle a hanging cloth actually takes; vanilla's `100 · delta` is
its linear approximation, so they must part company away from zero. `w22`
fails if a second coefficient is ever added to chase vanilla's number out
there.

**Gauss–Seidel with the upper joint held is what makes 1e-4 reachable.** The
spec asks for four passes *and* every link within 1e-4 of `REST_LEN` after
them. Symmetric mass weighting cannot do that: gravity's uniform per-tick shift
breaks link 0 against the pin every tick and four sweeps only halve that error
four times — **measured 2.9e-2**. Walking the links from the pin with
`w_upper = 0` is exact in one pass (measured **1.1e-15**), which is the only
reading that satisfies both rows. Passes 2..4 are then exact no-ops and are
still run.

**The backstop had to be reachable to be tested.** `MAX_JOINT_RADIUS` is
unreachable while the constraints hold — the chain is 16 units of link — so "no
joint beyond 24" passes whether or not the clamp exists, which is the vacuity
the health-bar spec's upper clamp turned out to have. The one thing the
constraints *cannot* absorb is a link whose squared length overflows: `relax`
declines it by design rather than inventing a direction, and `clamp`
deliberately normalizes differently (largest component first) so it can still
recover. `w15` injects 1e200 and measures the worst joint at exactly
**24.000000** — the clamp fired — and `w16` shows the chain back within 6.7e-16
of `REST_LEN` one tick later.

**Geometry.** `mobs::cape_slab_quads(n)` subdivides the same 64×32 box UV; the
frames are **per joint**, not per slab, so consecutive slabs share their
boundary quad exactly and the surface is watertight with no internal caps (which
would be coincident z-fighting quads at every joint). At `n == 1` it is
`cape_faces()` face for face — a structural check, and *not* the reduction rule.
`part_transforms` / `neutral_quads` / `oracle_part_deltas` stay untouched, as
M60 left them.

**Gate: `rewo capeshot --check` — 38 → 64 witnesses**, serverless, validation
ON, 0 VUIDs, fail-closed. It drives the production `EntityTable::tick_lerp`,
`WavyCape::tick` and `live_cmd::resolve_cape` — no parallel copy. `w3` is the
guard on the one duplication the crate graph forces: rewo-gpu depends on no
other rewo crate, so the simulation derives the cape rotation itself in f64 and
the gate grades it against the shipped `cape_transform` (worst disagreement
1.0e-7). `y2`/`y3` compare two offscreen renders of the *same* scene differing
only in cape mode, with the marker colour isolating the cape — not the frame
diff §0.0 forbids, whose failures came from live runs and world-mutating
triggers.

**Five mutations were run — source broken, rebuilt, gate re-run, reverted:**

| mutation | caught by |
|---|---|
| `ANCHOR_ACCEL` dropped (the raw delta, i.e. the first M61 build) | `w20`, `w21`, `w22` — **and nothing else**, which is precisely why they exist. Every other witness in the file passed while the cape flew to 80.9° at a 0.05 gap, because none of them measured what the simulation settles *to* |
| the reduction bypass removed (`segments() >= 1`, so one segment simulates) | `y1` **alone**, and everything else still passes — which is the point: without that witness the safety net is gone silently |
| symmetric Gauss–Seidel (both endpoints share the correction) | `w9` (2.85e-2 settled, 9.6e-1 moving), `w8`, `w16`, and `w14` — the chain stops staying within its own 16-unit reach and hits the clamp |
| `clamp` removed from `tick` | `w15` and `w16` only |
| `push_out` removed from `tick` | `w12` (closest approach **0.458**, matching the prototype exactly), `w13`, and `w18` — which reads 6.000° instead of 5.843° once the 0.157° nudge is gone |

**Measured.** capeshot 64/64. **891 tests** (was 883; +8 in `wavy_cape`). All
21 gates green, 0 VUIDs: capeshot 64, itemshot 62, inventoryshot 127,
healthbarshot 33, attributeshot 43, captureshot 17, blockentityshot 172,
swingshot 97, hurtshot 38, weathershot 35, handshot 34, particleshot 34,
eventshot 28, danceshot 24, portalshot 12, hudshot 41, mobshot 243/243 +
emissive 5 + etf 8 + tint 4, plus skyshot, lightmapshot, tintshot, meshshot,
dimensioncheck. Demo PNG SHA-256 `2cc56b4a…` byte-identical to M15 onward.
`git diff --check` exits 0.

**Open.**

- **The collision response is not re-projected.** The spec's order is relax,
  *then* push-out, so a joint shoved off the torso leaves its links stretched
  until the next tick. Small in practice — **0.230** model units, a fifth of a
  slab, on a 30°/tick turn, and only on a turn, since a cloak gap blows the
  cape away from the body rather than across it. One relax pass after the
  push-out, or collision inside the relax loop, would close it.
- **The push-out builds a cylinder, and the spec's witness says "torso AABB".**
  The torso box is 8 wide; a joint at x 3.5, z 0 is outside a 2.5 cylinder and
  inside the box. `w12` asserts what the rule actually creates.
- **No live sighting**, as M60. Nobody has watched a cape wave on a real server.
- A dying player's chain does not topple with the body's death roll — the roll
  is applied to the finished chain about its anchor, which is a rotation of the
  cape and not of the cloth's own history. The vanilla path is unaffected.
- Wind, per-player configuration and self-collision are the spec's own
  exclusions and remain out.

### M60 — the vanilla player cape (2026-07-29)

Shipped and gated. One 10×16×1 slab hanging off the player's `body`, chasing a
lagging anchor. The wavy/cloth extension is a separate milestone with its own
spec ([`REWO_WAVY_CAPE_SPEC.md`](REWO_WAVY_CAPE_SPEC.md)) and none of it is
here; that spec's own §"the split" puts everything below this line — geometry,
`moveCloak`, the three angles, the `Rx·Rz·Ry` composition, the four gates,
metadata index 16 — under `capeshot`, which is where it now is.

**The rotation is the milestone.** Every other part in the entity renderer is
a [`Part`](../crates/rewo-gpu/src/mobs.rs): a Euler triple, composed
`Rz·Ry·Rx`, with animation deltas *summed* onto the base pose. The cape needs
`Rx·Rz·Ry`, which that composition cannot produce — and the scoping pass called
this "the milestone's one structural change".

**It needed no structural change at all, and the reason is worth keeping.**
`PlayerCapeModel.setupAnim` builds a quaternion and hands it to
`ModelPart.rotateBy`, which post-multiplies onto the `PartPose`'s
`rotationZYX`:

```java
// PartPose is offsetAndRotation(0, 0, 2,  0, PI, 0)
cape.rotateBy(new Quaternionf().rotateY(-PI).rotateX(a).rotateZ(b).rotateY(c));
// rotateBy: oldRotation.rotate(rotation), then getEulerAnglesZYX + setRotation
```

JOML's `rotate*` post-multiply, so the quaternion is
`Ry(-π)·Rx(a)·Rz(b)·Ry(c)`, and the product with the pose's `Ry(π)` is

```text
Ry(π) · Ry(-π) · Rx(a) · Rz(b) · Ry(c)  =  Rx(a) · Rz(b) · Ry(c)
```

**The leading `rotateY(-PI)` exists to cancel the pose.** So the net rotation
replaces the `PartPose` rotation rather than composing with it — but the pose's
*translation* `(0, 0, 2)` still applies, which is the asymmetry that makes this
easy to get wrong in either direction. The ZYX decompose/recompose `rotateBy`
ends with is an exact round-trip on the matrix, so Euler can be skipped
entirely.

And then the cape does not need to be a `Part`, because **vanilla does not make
it one either**: `createCapeLayer` calls `clearRecursively()` precisely so the
humanoid mesh does not come along, and `CapeLayer` is a render layer. Rewo
emits it through `emit_cape`, the seam `emit_armor` already established —
taking the `xf` the body just used (the body is *animated*: `body.xRot = 0.5`
crouching, `body.yRot` during an attack swing) and applying the child transform
`m = m_body·R`, `o = m_body·pivot + o_body` that `part_transforms` would have.
`part_transforms`, `neutral_quads` and `oracle_part_deltas` are untouched, so
`mobshot`'s geometric prediction still grades exactly the code it did before.
Teaching `part_transforms` a matrix override would have put a branch in the
path every mob's geometry runs through, for one quad.

**Two more things the scoping pass expected turned out not to be needed.** A
per-texture-index UV array (because `upload_skin` returns one delta for the
whole draw) is unnecessary — a render-layer emitter resolves its own atlas
origin per frame from raw pixel UVs, as `emit_armor` does with a trim's, so
`upload_cape` returns an **origin**, not a delta. And the cape's slot is
64×32 rather than a skin's 64×64: `createCapeLayer` builds against a 64×64
`LayerDefinition`, but the box carries `xTexScale 1.0, yTexScale 0.5` and
`CubeDefinition.bake` multiplies them in, so the UVs normalize against 64×32.
A 64-tall slot halves every V — `a3`, and the mutation run measures exactly the
0.26562 it predicts.

**Wire and state.**

- `Avatar.DATA_PLAYER_MODE_CUSTOMISATION` is **index 16, BYTE**, and bit 0 is
  `PlayerModelPart.CAPE`. **This is a 26.2 layout and the 1.21 answer was 17**:
  `Avatar` was inserted between `LivingEntity` and `Player` and defines
  `DATA_PLAYER_MAIN_HAND` then `DATA_PLAYER_MODE_CUSTOMISATION`, where 1.21's
  `Player extends LivingEntity` put absorption and score first. Index 16 is the
  same slot Rewo already reads as slime size (INT), baby/dancing/celebrating
  (BOOLEAN) — the serializer separates the BYTE, but only the **kind** says it
  is a player's, so routing is kind-gated on `minecraft:player` exactly as M18
  gates the Allay's.
- `ClientAvatarState.moveCloak` is per-axis and its threshold is **exclusive on
  both sides** (`if (!(d > 10.0) && !(d < -10.0))`), so a gap of exactly ±10
  eases and only a larger one snaps — and the snap rewrites `O` as well, so no
  partial tick draws the cape streaking across the gap.
- The anchor starts at **0,0,0**, as vanilla's fields do. That is not neutral —
  a player spawning within 10 blocks of the world origin on an axis has their
  cloak converge onto it over several ticks instead of snapping — but it is
  what vanilla does. An earlier draft seeded the anchor on the first tick to
  "fix" this and was reverted: the milestone is a transcription.
- `LivingEntity.fallFlyTicks` is client-simulated (`if (isFallFlying()) ++
  else = 0`, off shared flag 7), and `fallFlyingScale` squares it, so ten ticks
  of gliding suppress `capeLean` completely.
- `walkDist` and `bob` are both zero for every entity Rewo draws, **for
  different reasons**. `walkDist`'s only writer chain ends at
  `LocalPlayer.move`, and the local player is not in the entity table. `bob` is
  *not* zero for remotes — `RemotePlayer.tick` calls `updateBob()` — so gating
  the walk term on `bob != 0` looks equivalent and is wrong. `c4` is the
  witness, and it passes `bob = 1.0` to make the point.
- `capeFlap`'s clamp lands **before** the walk-bob add, so a local player
  walking legitimately exceeds 32°; `capeLean`'s lower clamp is **0**, not
  −150, so the cape never leans forward however far a player walks backwards.

**`ArmorLayer::Wings` had to exist, and its textures deliberately do not.**
`CapeLayer` has four gates, and two of them ask different questions about the
same chest item: `hasLayer(WINGS)` suppresses the cape outright, and
`hasLayer(HUMANOID)` shifts it clear by `(0, -0.053125, 0.06875)` blocks.
Before M60 the layer table held only the two humanoid entries, so **an elytra
and a carved pumpkin were indistinguishable** — neither has a humanoid layer.
A carved pumpkin is `Equippable.builder(HEAD)` with no `setAsset`, so it names
no equipment asset at all and both questions answer no. `d5` is the only
witness where "has a humanoid layer" and "chest slot is occupied" disagree, and
the mutation run confirms it: keying the shift on `chest.is_some()` **passes
`d3` and `d4`** and fails only there. The wings *sheet* is not decoded, because
`equipment.textures` is shelf-packed into the entity atlas in order and adding
an entry would move every texture after it — `mobshot` staying at 243/243 is
the empirical half of that claim.

**The atlas grew by exactly the new band.** `ATLAS_H` 1408 → 1472, with the
cape pool at `y = 1408` and `TRIM_POOL_Y` re-anchored to it — M48's recipe, and
the reason every mob, item, skin and trim texel address is numerically
unchanged. Only the V *denominator* moves, which maps the same texels to the
same samples.

**Wire decode: a cape-only profile now resolves.** `skins.rs` reached
`["textures"]["SKIN"]` and returned `None` otherwise, **with a test asserting
it** — so a profile carrying only a cape was invisible to the whole client.
`CapeLayer`'s gate is `skin.cape() != null` alone, so every field is now
independently optional and the decoder succeeds on any of them. `skin_fetch`'s
decode is size-preserving; a cape is 64×32 in vanilla and third-party ones are
power-of-two multiples, which box-downsample into the slot (a non-multiple is
rejected rather than stretched).

**Gate: `rewo capeshot --check` — 38 witnesses**, serverless, validation ON, 0
VUIDs, fail-closed. It drives the shipped `cape_transform` / `cape_face_uv` /
`cape_clearance_shift`, the production `EntityTable` tick, the real
`route_set_entity_data`, and `live_cmd::resolve_cape` — no second copy of any
rule. Every expectation is independently transcribed: the rotation witnesses
build their own `Rz·Ry·Rx` and their own 3×3 apply, because reading the
comparison out of the code under test would assert nothing. The pixel half uses
a **marker-magenta** cape with an empty frame *and a bare player* both asserted
to contain none of it — M38's rule, and `g2` exists because a default-Steve
skin would defeat any "non-background" detector.

**Four mutations were run — source broken, rebuilt, gate re-run, reverted:**

| mutation | caught by |
|---|---|
| `cape_rotation` composed `Rz·Ry·Rx` | `b1` (cape spans the torso, 0.328..2.995), `b2`, `b3`, and `b4` measuring **0.000 px** apart — the shipped code *is* the wrong ordering |
| the flap clamp moved after the walk-bob add | `c3` (48.00 → 32.00), plus the `rewo-world` unit test |
| `chest_humanoid = chest.is_some()` | `d5` only — `d3` and `d4` still pass |
| the cape slot baked 64 tall | `a3`, measuring exactly the 0.26562 it predicts; `f1`/`f2` still pass, correctly |

`b1`'s own mutation partner is computed inline, and `b2` asserts that partner
is genuinely wrong rather than a near-miss: applying the `PartPose`'s `Ry(π)`
as well does not mirror the cape to the front (which is what the scoping pass
predicted) — it conjugates the 6° rest tilt into `Rx(-6°)` and drops the
nearest corner to z = −0.667, **inside** the torso. `b5` is the control: with
only one non-zero angle the two orderings agree exactly, so `b4`'s 3.406 px gap
is the ordering and not the angles.

**Process traps hit.** (1) A `Copy-Item` revert restored the source with the
backup's *older* mtime, so cargo skipped the rebuild and the next mutation run
showed the previous mutation's failures — M59's "rebuild before believing a
sweep", in a new disguise. Touch the file after restoring. (2)
`mobshot_cmd.rs` is **mixed CRLF/LF**, and one `Edit` normalised it into an
899-line spurious diff; restoring from HEAD and re-applying byte-wise brought
it back to 3/1. Its line 241 ends LF and its line 518 ends CRLF, in the same
file. Note that `git diff --check` treats a CR on an **added** line as trailing
whitespace, so new lines in such a file must be LF even where their neighbours
are not.

**Measured.** `capeshot --check` 38/38. **755 tests** (was 744; +11: cape
angles 5, cloak anchor + fall-fly 3, cape PNG decode 4, minus 1 replaced skins
test). All 21 gates green, 0 VUIDs, on a freshly rebuilt binary: capeshot 38,
itemshot 62, inventoryshot 127, healthbarshot 33, attributeshot 43,
captureshot 17, blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35,
handshot 34, particleshot 34, eventshot 28, danceshot 24, portalshot 12,
hudshot 36, mobshot 243/243 + emissive 5 + etf 8 + tint 4, plus skyshot,
lightmapshot, tintshot, meshshot, dimensioncheck. Demo PNG SHA-256
`2cc56b4a…` byte-identical to M15 onward. `git diff --check` exits 0.

**Open.**

- **No live sighting.** The gate is authoritative for the properties it names;
  nobody has watched a cape on a real server. It needs an online-mode session
  with a profile that actually owns one.
- **The inventory preview wears no cape.** M36's preview owns a *second*
  `EntityPass` with its own atlas, so it needs its own cape pool and upload;
  vanilla's does show one. The most visible remaining gap.
- **`use_player_texture`** (elytra.json sets it) is read as data and still
  never honoured — an elytra would wear the player's cape texture. Nothing
  renders an elytra yet, so it is unreachable rather than wrong.
- The cape does not take the hurt flash, which is correct
  (`OverlayTexture.NO_OVERLAY`) and is hard-zeroed rather than read off
  `d.hurt` — but no witness distinguishes "zeroed" from "the flash never
  reaches this vertex range".
- A >32-resident-cape network recycles slots round-robin with no eviction
  bookkeeping, as every pool before it.

### M56 — the tooltip's image pass, and vanilla's bundle grid (2026-07-28)

Shipped. M40 built the tooltip's first pass — measure, position, nine-slice
box, draw the lines — and stopped there. `GuiGraphicsExtractor.tooltip` walks
its component list **twice**:

```java
int localY = y;
for (int i = 0; i < lines.size(); i++) { lines.get(i).extractText(this, font, x, localY);
                                         localY += lines.get(i).getHeight(font) + (i == 0 ? 2 : 0); }
localY = y;                                                     // <- the restart
for (int i = 0; i < lines.size(); i++) { lines.get(i).extractImage(font, x, localY, w, h, this);
                                         localY += lines.get(i).getHeight(font) + (i == 0 ? 2 : 0); }
```

**The `localY = y;` is the load-bearing line, and the two passes are a
layering device rather than a layout one.** Both loops advance identically, so
a component's image lands at exactly the `y` its text would have; what the
split buys is that *every* image draws after *every* text line, whatever order
the components are in. Run them as one continuous cursor and the grid drops
below its own box by the height of all the text — 57 px in the gate's fixture —
which is the overlap the two passes exist to avoid.

Sizing needs no second mechanism. `getWidth`/`getHeight` are polymorphic and
the measure loop has no special case, so an image contributes its own width and
height to the box the same way a line does. The one asymmetry worth knowing:
`lines.size() == 1 ? -2 : 0` counts **components**, not text lines, so giving a
one-line tooltip an image does not merely add the grid's height — it also hands
back the two pixels the single-component case subtracts.

`crates/rewo-gpu/src/tooltip.rs` is the whole transcription;
`container::tooltip_size` now delegates to its `measure` so there is one loop
rather than two that can drift.

#### Four things the brief had backwards, and the decompile settled

The milestone arrived with an audit's summary of `ClientBundleTooltip`. Half of
it held and half did not, which is the usual yield and the reason the rule is
*read the file*.

- **The `+N` badge is the BOTTOM-RIGHT cell, not the top-left.**
  `shouldRenderSurplusText` is `isOverflowing && column * row == 1`, so the
  badge takes the first cell *visited* — and both start positions are the
  grid's far edge with both loops subtracting (`xStartPos = x + offset + 96`,
  `drawX = xStartPos - columnNumber * 24`), so column 1 is the **rightmost**.
  The fill order the brief gave was right; the cell it named was the other
  corner of it.
- **Thirteen stacks show EIGHT items, not twelve.**
  `BundleContents.getNumberOfItemsToShow` subtracts the ragged row using the
  **total** stack count, not the eleven cells a badge leaves free:
  `13 % 4 = 1`, so three come off eleven and eight show. The grid is still
  three rows, so its top row comes out blank beside the badge. Reading it as
  "eleven and a badge" fills three cells vanilla leaves empty.
- **The badge counts hidden ITEMS, not hidden stacks.**
  `getAmountOfHiddenItems` is `items().stream().skip(shown).mapToInt(count).sum()`,
  so thirteen single-item stacks badge `+5` and thirteen full ones badge
  `+320` — never `+1`.
- **`SLOT_MARGIN` is the icon's inset inside its cell**, not a gap between
  cells. Cells tile at exactly `SLOT_SIZE`; the 4 is `graphics.item(item,
  drawX + 4, drawY + 4, …)` centring a 16 px icon in 24.

What did hold: `getWidth` is a literal `return 96`; `slotCount` is
`min(12, size)`; the fill order is bottom-right to top-left; the image inserts
at index 1; `BundleItem.getTooltipImage` is the only override of it in the
tree; and `ClientTooltipComponent.create`'s third arm throws, so exactly three
implementations exist and only one of them can come from a stack.

Three more details that are only visible in the source. `getContentXOffset(w)`
is handed the **tooltip's** measured width, not the component's own 96, so a
long enchantment line slides the grid right rather than leaving it hard against
the box's left edge. `extractCount`'s `centeredText` is `x - font.width(str) / 2`
— integer division, and its vertical anchor is a flat **10**, not the cell's
half-height of 12. And the whole of `extractImage` sits inside
`if (!weight.isError())`, so an over-weight bundle contributes its size to the
box and then draws none of it.

#### The gate

`inventoryshot --check` 91 → **103**. Ten witnesses are arithmetic driven
through the production functions, two are pixels. Every one names its mutation
partner, and the four the brief asked for were run as real mutations rather
than asserted:

| mutation | caught by |
|---|---|
| walk top-left to bottom-right | `ti5`, `ti6`, `ti8`, `ti9`, **`ti12`** |
| one continuous cursor across both passes | `ti4` |
| badge at twelve (`size >= 12`) | `ti6` |
| a text-only width | `ti2`, `ti3`, **`ti11`** |

`ti12` is the one worth pointing at: it renders three stacks into the grid as
real icons through the production `gui_item` pass, positioned by the production
cell walk, and measures **which column stays empty**. Three stacks in a
four-column grid leave exactly one unused, and the walk's direction decides
which — so reversing it swaps the two numbers the witness prints, and it does,
exactly (678 changed pixels ↔ 0). `ti11` renders the box at the size `measure`
returns and asserts it reaches at least the grid's 96; a text-only measure
leaves it 56 px narrower than its own contents.

#### Scope, and one honest deviation

The **cell chrome is not drawn**: `container/bundle/slot_background`,
`slot_highlight_back`/`front` and the three `bundle_progressbar_*` sprites are
jar sprites, and reaching them means adding fields to
`assets::ContainerSprites` and `ContainerSpriteData` — files this milestone was
fenced out of, and a struct-literal construction that cannot gain a field
without its caller changing. The geometry for all of it is computed and graded
(`ProgressBar` carries its rect, fill width, sprite choice and label; `Cell`
carries its icon and badge anchors); only the blits are missing. Wiring a real
bundle stack into `live_cmd`'s `screen_tooltip` is the other half of the same
fence — the contents come from a `BUNDLE_CONTENTS` component patch, which is
`rewo-net`. **Follow-up:** add the six sprites to the container atlas, feed
`bundle_image`'s cells to the existing item pass, and the grid renders whole.

Also out: `ClientActivePlayersTooltip` (a server-list component, never an
item's), the empty bundle's blurb text (`font.split(…, 96)` is passed in as a
line count rather than wrapped here, since `rewo-gpu` holds no font), and the
selected-item sub-tooltip `extractSelectedItemTooltip` recurses into.

**And the note the brief itself made: vanilla's shulker-box preview is TEXT** —
five lines plus an italic `+N more` from `ItemContainerContents` — not an icon
grid. The grid is the bundle's alone. Pointing this code at a shulker box would
be inventing a feature, so it is not pointed there.

#### Verification

**648 tests** (641 + 7 new in `rewo-gpu`), all seventeen gates green with
Vulkan validation ON and **0 VUIDs** — `itemshot` 62, `inventoryshot` **103**,
`blockentityshot` 172, `swingshot` 97, `hurtshot` 38, `weathershot` 35,
`handshot` 34, `particleshot` 34, `eventshot` 28, `danceshot` 24, `portalshot`
12, `captureshot` 17, `mobshot` 243/243, plus `skyshot`, `lightmapshot`,
`tintshot`, `meshshot`, `dimensioncheck`. Demo PNG SHA-256 byte-identical to
M15 onward (`2cc56b4a…`).

### M51c — `captureshot`, and the one line the whole suite was blind to (2026-07-28)

Shipped. M51a parameterised `Offscreen` by colour format so a screenshot can
target the live swapchain's `B8G8R8A8_SRGB`, and added a swizzle to `save_png`
because a BGRA attachment reads back B,G,R,A while PNG is R,G,B,A. M51b wired F2
to it and recorded the hole in as many words: *no gate exercises a BGRA
`Offscreen`*. All sixteen existing call sites take the RGBA default, so nothing
in the suite could see the branch that decides whether every screenshot this
client will ever take has its red and blue exchanged.

**The swizzle is correct, and that was not the foregone conclusion.** Two
failure modes were equally available and both produce a plausible picture: the
swizzle *missing* (M51a's own worry), or the swizzle *spurious* — if the copy
did not actually hand back permuted bytes, applying it would introduce the swap
it claims to fix. A gate that only checked "the file is red-first" could not
tell those apart, because it would pass in the second world too, having been
made right by a correction that was itself the bug.

`a2` is the discriminator, and it is the reason the gate is worth more than a
green tick: it reads the **raw** `read_rgba` from both targets and compares them
against the explicit channel-0/2 permutation. Measured: 32,768 of 65,536 bytes
differ before the exchange and **0 after** — exactly two bytes per texel, in
every texel. `cmd_copy_image_to_buffer` copies the image's memory verbatim and a
`B8G8R8A8` image stores B,G,R,A, so the fault is real and the correction is the
right one. `a1` and `a3` then show the saved file is nevertheless red-first and
byte-identical to the RGBA path. Either half alone would have been worth little.

#### Facts the gate pinned that were previously only asserted

- **`VkClearColorValue`'s four floats map to the format's R,G,B,A *components*,
  not to its memory layout** — index 0 is red in a BGRA image exactly as in an
  RGBA one. That is what makes an absolute channel-order assertion possible at
  all, with no reference render to compare against.
- **The clear value *is* sRGB-encoded on an sRGB attachment; alpha is not.**
  Measured from one clear: linear `0.25` stores as **137**, while alpha `0.5`
  stores as **127**. Only the ordering is load-bearing for `a1` (any transfer
  function is monotonic), but the asymmetry is worth having written down.
- **Rewo must not copy vanilla's vertical flip** — `capture.rs` said so in a
  comment and nothing tested it. `a7` now does, using the overlay chart as a
  spec-anchored ruler: Vulkan puts a framebuffer's *and* `gl_FragCoord`'s origin
  at the upper left, `read_rgba` copies rows tightly packed from image row 0,
  and the encoder writes row 0 first. A chart at framebuffer (8,8) size 40×40
  occupies exactly file rows 8..47 and columns 8..47 — 1,600 texels, 0 misplaced.
  `Screenshot.takeScreenshot` writes `setPixelABGR(x, height - y - 1, …)` only
  because `glReadPixels` hands back a bottom-up image.
- **Vanilla's screenshots are opaque by construction** (`argb | 0xFF000000`) and
  Rewo's are opaque by consequence: the overlay pipeline's `color_write_mask` is
  `R|G|B`, so the clear's alpha survives untouched. `a6` renders the same frame
  at clear alpha 0.5 and reads 127 back, so `a5` is a measurement rather than a
  channel the encoder was hard-writing.

#### Every witness names a mutation, and every mutation was run

| mutation | fails |
|---|---|
| delete the `if self.is_bgra()` swap | `a1` (reads `[0, 137, 255, 255]` — blue-first), `a3`, `b1` |
| add `R8G8B8A8_SRGB` to `is_bgra`'s match (double-apply) | `a3`, `a4` |
| copy vanilla's row inversion into `save_png` | `a7` (3,200 texels misplaced) |
| start the dedup ladder at `_1` | `c2` |
| `/` and `%` for `div_euclid`/`rem_euclid` | `c3` — and the stem really does come out `…4294967295`, verbatim as its detail string predicted |

The first of those is the whole point: without the gate, a live screenshot would
have shipped with red and blue exchanged and would have looked entirely
plausible.

#### `grab` is driven, not reimplemented

The M45/M47 lesson — a gate that reimplements a slice of the app's setup misses
whatever the app later adds to it. `b1`–`b3` call production `capture::grab`
with a `WorldRenderer` built for `B8G8R8A8_SRGB`, which only renders at all
because `grab` passes the caller's format through to `Offscreen::with_format`
(a `WorldRenderer` bakes its colour format into every pipeline it builds). `b2`
then shows `grab`'s file is byte-identical to a hand-driven
`with_format` + `render(Some(world))` + `save_png` — with `grab`'s clear
transcribed in the gate rather than exported from `capture.rs`, so a change to
it shows up as a difference instead of as two implementations agreeing with each
other. The frame is a red-dominant gradient sky spanning 74 units top to bottom,
so `b2` is not two black images agreeing.

`grab` writes into the user's real screenshots directory, because that is what
F2 does; the gate removes what it wrote, and does so even on the mutated runs.

**Needs no client jar and no server** — `upload_texture_array` substitutes one
white layer for an empty slice, so a frame with no terrain in it needs no bake,
and every colour graded is a clear value or a sky uniform the gate chose itself.

#### A small process note

The gate's own output is grepped for validation-id tokens, so a detail string
that *quotes* one registers as a validation failure in exactly the check that is
supposed to catch leaks. `b1` names the mismatch in prose instead. Worth
remembering for any future witness that wants to describe what validation would
say.

**Measured.** `captureshot --check` **17/17**, validation ON, 0 validation
messages, two consecutive release runs identical apart from the two lines that
legitimately carry the wall clock. **641 tests** (637 + 4). All eighteen gates
exit 0 with zero validation messages: `mobshot` 243/243, `blockentityshot`
172/172, `swingshot` 97/97, `inventoryshot` 91/91, `itemshot` 62/62, `hurtshot`
38/38, `weathershot` 35/35, `handshot` 34/34, `particleshot` 34/34, `eventshot`
28/28, `danceshot` 24/24, `captureshot` 17/17, `portalshot` 12/12, plus
`skyshot`, `lightmapshot`, `tintshot`, `meshshot`, `dimensioncheck`. Demo PNG
SHA-256 `2cc56b4a…` byte-identical to M15 onward.

**Open, and deliberately so.** Proving a capture matches what the *window*
presented is out of reach — Rewo's swapchain images carry no `TRANSFER_SRC`, so
there is nothing to read back from a presented frame, and a gate cannot open a
window; `b2`'s equivalence against a hand-driven `Offscreen` is what stands in
for it. `local_offset_seconds()` still returns 0, so filenames are UTC where
vanilla's are local — `c4` records that in place rather than pretending
otherwise. Vanilla's `downscaleFactor` (the supersampled capture) is not
implemented, and neither is the chat feedback `Screenshot.grab` sends through
its callback.
### M55 — entity attributes, the data half of a health bar (2026-07-28)

Shipped. Rewo has decoded an entity's *current* health since M24 (`DATA_HEALTH_ID`,
metadata index 9) and has had no way at all to know its *maximum*, because
`MAX_HEALTH` is not metadata — it is an **attribute**, and
`ClientboundUpdateAttributesPacket` (id **131**) was falling off the dispatch
chain as an unknown id. This is the data half only: no rendering, nothing in
`rewo-gpu`.

**The wire shape**, from the decompiled `STREAM_CODEC`:

```text
VarInt entityId
VarInt snapshotCount                 // ByteBufCodecs.list(128)
  VarInt attributeHolder             // holderRegistry — RAW 0-based id
  f64    base                        // big-endian
  VarInt modifierCount
    String id                        // Identifier.STREAM_CODEC = STRING_UTF8
    f64    amount
    VarInt operation                 // idMapper — a VarInt, not a byte
```

**The holder is `holderRegistry`, so the id is raw and 0-based** — the question
the brief flagged, and the answer is the one that has bitten this project twice
already (M16 dimension types, M21 damage types).
`Attribute.STREAM_CODEC = ByteBufCodecs.holderRegistry(Registries.ATTRIBUTE)`
resolves through `registry(...)`, whose decode is a bare `VarInt.read` into
`byIdOrThrow`; there is no `id + 1` and no `0 = inline`. Here the failure mode
would have been quiet rather than loud: `max_health` is **23** and
`max_absorption` is **22**, both real syncable attributes on the same entity, so
an off-by-one would have clamped health against the wrong range rather than
throwing. The gate pins it from both sides (`a4`/`a5`).

**Two more fields are not what their shape suggests.** The operation is a
**VarInt** (`Operation.STREAM_CODEC` is `ByteBufCodecs.idMapper`, whose decode
is `VarInt.read`), not the single byte a three-valued enum invites — `a6` sends
a redundant two-byte encoding, which a byte reader would desynchronise on. And
an **out-of-range operation id is not an error**: `BY_ID` is
`ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`, so operation 9 is
`ADD_VALUE`, not a rejected packet.

**The operation order** (`AttributeInstance.calculateValue`) is the thing that
cannot be guessed from the enum:

```text
base = baseValue
for m in ADD_VALUE:            base   += m.amount
result = base
for m in ADD_MULTIPLIED_BASE:  result += base * m.amount
for m in ADD_MULTIPLIED_TOTAL: result *= 1.0 + m.amount
return sanitizeValue(result)
```

`ADD_MULTIPLIED_BASE` reads the **post-`ADD_VALUE`** base, and every such
modifier reads that same base — they do not compound with each other, where
`ADD_MULTIPLIED_TOTAL` does. **A single modifier cannot tell the two apart**:
either one at `+0.5` turns base 20 into 30, and so does either one at `+0.5`
after an `ADD_VALUE` of 10 (both give 45). It takes *two* to separate them —
`b3` (two `ADD_MULTIPLIED_BASE` → 40) against `b4` (two `ADD_MULTIPLIED_TOTAL`
→ 45). `b6` sends all three groups **reversed** in the packet and still requires
90, which is what pins grouping-by-operation rather than packet order; applied
in packet order the same modifiers give 50.

**The clamp is per attribute and it is not optional.** Every one of the 40
registered attributes is a `RangedAttribute`, so `sanitizeValue` always runs:
`NaN ? min : Mth.clamp(v, min, max)`. `max_health` is `(20.0, 1.0, 1024.0)`, so
a server sending base 0 resolves to **1.0**, and **NaN resolves to the minimum**
— an entity whose modifiers multiply out to NaN has half a heart, not an empty
bar and not a full one.

**`DefaultAttributes` is a filter, not just a fallback** — the part of this
milestone that was larger than it looks. `handleUpdateAttributes` looks each
attribute up with `AttributeMap.getInstance`, which returns null (logged
`"Entity {} does not have attribute {}"` and **skipped**) when the entity type's
`AttributeSupplier` does not declare it. So the supplier decides what an entity
may hold at all: a zombie has `spawn_reinforcements`, a pig does not, and a
snapshot naming it must not stick to the pig.

That table is `DefaultAttributes.SUPPLIERS`, ~93 entries of
`EntityTypes.X -> SomeClass.createAttributes().build()`, whose builders chain up
the class hierarchy (`Zombie` → `Monster` → `Mob` → `LivingEntity`) across ~85
files. **`tools/gen_entity_attributes.py`** extracts it, for the same reason
`gen_copper_golem_poses.py` exists — hand-copying fails silently. Four things it
has to get right, each of which would otherwise be invisible:

* **`add()` twice keeps the LAST** — `build()` is `buildKeepingLast()`, so
  `Zombie`'s `FOLLOW_RANGE 35.0` overrides the `16.0` it inherited from `Mob`
  rather than throwing (`d11`).
* **A float literal is widened, not rounded.** `.add(MOVEMENT_SPEED, 0.23F)` is
  a `float` promoted to `double` = `0.23000000417232513`, not `0.23`. Same class
  of error as M37's `+ 0.1F`; `d9` requires the widened value *and* rejects the
  decimal.
* **The constant name is not the registry name** —
  `SPAWN_REINFORCEMENTS_CHANCE` registers as `"spawn_reinforcements"`. The
  extractor parses the mapping instead of lowercasing the constant, which is the
  same irregularity `gen_block_light.py` records for copper.
* **A static can be inherited both ways** — `DefaultAttributes` calls
  `Cow.createAttributes()` though `AbstractCow` declares it, and `ArmorStand`
  calls `createLivingAttributes()` unqualified. Both resolve by walking
  `extends`.

**Fail-closed resolution.** `rewo_world::attributes::resolve` returns
`Option<(f64, Source)>`, and the `Source` is the point: `Synced` means a packet
established the value, `Default` means the type's supplier did, and **`None`
means nothing does** — an unknown type, a type with no supplier (a boat), or an
attribute its supplier does not declare. A health bar that got `Some(20.0)` for
a boat would draw a full bar over it, so `d3` requires `None` there and `d1`
requires `Some((20.0, Default))` for a zombie.

**Gate: `rewo attributeshot --check`** — serverless, CPU-only, fail-closed,
**43 witnesses**, driving raw packet bodies through the production path
(`route_update_attributes` → `apply_update_attributes` → `attributes::parse` →
`EntityTable::set_attribute` → `resolve`). Nothing reimplements the decoder and
grades the reimplementation; the expected numbers are hand-derived literals from
`calculateValue`/`sanitizeValue`. **The fail-closed count immediately earned its
keep**: the first run reported 42 observed against a declared 34 and exited
nonzero on my own miscount.

**Ten mutations were applied to the production sources and nine are caught** by
the witnesses named for them — `holder_id_plus_one` (23 witnesses, `a4`/`a5`
among them), `multiplied_base_uses_pre_add_base` and `packet_order` (`b5`,
`b6`), `no_clamp` (`b7`, `b8`), `nan_to_max` (`b10`),
`default_instead_of_none` (`d3`, `d4`, `d5`), `no_supplier_filter` (`c3`),
`oob_operation_rejected` (`a7`), `remove_keeps_attributes` (`c6`).

**The tenth is uncatchable, and finding out why was the useful part.** Deleting
the living-entity gate from `apply_update_attributes` changes **no** outcome.
The witness I wrote for it (`c2`, a boat) passed for the wrong reason: a boat is
not a `LivingEntity` *and* has no supplier, so the lookup two lines later
rejects it anyway. Measured over the whole registry, `is_living` and "has an
`AttributeSupplier`" are the **same 93 types of 158, exactly** — which follows
from vanilla's own typing (`SUPPLIERS` is keyed by
`EntityType<? extends LivingEntity>`) but was worth measuring rather than
assuming. The gate is kept, because it is the gate `handleUpdateAttributes`
actually has and it is the documented reason a boat is inert; `d14` now asserts
the set equality over all 158 types, so the redundancy is a checked fact and the
gate stops being redundant the moment a version ships a non-living type with a
supplier.

**Deviations, all deliberate and all recorded in the code:**

* A **non-living** entity is dropped where vanilla **throws**
  `IllegalStateException` and kills the packet thread — the same choice
  `apply_damage_event` makes.
* A **duplicate modifier id** within one snapshot keeps the first, where
  vanilla's `addModifier` throws on the second (`putIfAbsent` returning
  non-null). The first-wins state is what its map would have been left in.
* Modifiers are applied in **packet order within each operation group**.
  Vanilla iterates `Object2ObjectOpenHashMap.values()`, whose order is
  unspecified, so vanilla's own `ADD_MULTIPLIED_TOTAL` product is not
  bit-reproducible between two JVMs when several are present; packet order is
  deterministic and agrees to within float-multiplication reassociation.
* An attribute id outside the registry drops that snapshot, where
  `byIdOrThrow` throws.

**Measured:** **651 tests** (637 + 14 new: 8 in `rewo-world/attributes.rs`,
6 in `rewo-net/attributes.rs`), all passing. Eighteen serverless gates green
with **0 VUIDs** — `blockentityshot` 172, `swingshot` 97, `inventoryshot` 91,
`itemshot` 62, **`attributeshot` 43**, `hurtshot` 38, `weathershot` 35,
`handshot` 34, `eventshot` 28, `danceshot` 24, `portalshot` 12, `mobshot`
243/243, plus `particleshot`, `skyshot`, `lightmapshot`, `tintshot`,
`meshshot`, `dimensioncheck`. Demo PNG SHA-256
`2cc56b4a…46635`, byte-identical to M15 onward. `git diff --check` clean.

**One house-rule trap re-confirmed**, worth the line because it cost a revert:
`crates/rewo-data/src/lib.rs` is mixed CRLF/LF, and editing it as *text*
normalised the whole file (50 insertions / 48 deletions for a 2-line addition).
Re-done at byte level it is 8 insertions / 0 deletions with **zero** unchanged
lines' endings altered. The follow-on detail: an **added** line must be LF even
inside a CRLF neighbourhood, because `git diff --check` reads a trailing CR on
an added line as trailing whitespace — which is exactly why the repo's mixed
files are mixed.

**Open.** Nothing renders this yet: the health bar itself, and the nametag
health display, are a rendering milestone. Attributes are stored for every
living entity the server syncs, which in practice is the syncable subset
(`isClientSyncable`), and no consumer reads anything but `max_health` so far.
`AttributeInstance`'s permanent-vs-transient modifier distinction is not
modelled, because the packet only ever carries the merged set.
### M54 — the language map, the substitution, and the rarity a stack never sent (2026-07-28)

The foundation a tooltip line generator needs, and one live bug found on the way
in. Three pieces, none of which touches the entity pass.

#### `en_us.json` is not the language map

It is step 1 of three, and `ClientLanguage.loadFrom` — the same sequence
`Language.loadDefault` runs for the built-in instance — is:

1. `Language.loadFromJson` parses the file **and rewrites every unsupported
   format specifier**: `UNSUPPORTED_FORMAT_PATTERN` is `%(\d+\$)?[\d.]*[df]`,
   replaced with `%$1s`. This is why `decomposeTemplate` only understands `s`.
   Neither the brief nor I expected this step; it is **inert on 26.2** (0 of
   8,123 values match) and is transcribed anyway, because its absence would be
   invisible until the day a pack carried a `%d` — and then the whole line
   would collapse to its raw pattern.
2. `DeprecatedTranslationsInfo.applyToMap`, from
   `assets/minecraft/lang/deprecated.json`: **383 removed keys and 146
   renames**.
3. `Map.copyOf`.

`applyToMap` is two passes, and **the order between them is load-bearing**:
remove first, then for each rename move the value off the old key, deleting the
new key when the old one is absent. The rename map is kept in **file order** —
`Codec.unboundedMap` decodes an insertion-ordered gson `JsonObject` into an
`ImmutableMap` and `forEach` walks it in that order. 26.2's data happens to be
order-free (no two renames share a target; the one rename whose target is also a
source is `subtitles.entity.sulfur_cube.squish` onto **itself**), but a version
that chained two would depend on it.

The cost of skipping the pass, measured rather than asserted: of the 146 rename
targets, **105 do not appear in `en_us.json` at all** — including
`item.container.item_count` (`%s x%s`) and `item.container.more_items` (`and %s
more...`), which resolve to nothing today.

Rewo read the raw file in two places with narrow filters
(`assets.rs::bake_item_names` and `enchantments.rs`). Both now go through one
`rewo_data::lang::Language`, which `assets::bake` builds and `BakedAssets`
carries — so no two callers can disagree about what a key resolves to.

#### Two things the brief had were incomplete, and the gate says so

**The safety check was on the *absent* targets only.** It is true that 0 of the
105 absent targets start with `item.minecraft.` or `enchantment.`. But 41
targets **are** present and are *overwritten* (35 of them with a different
string; the other six are handed a value identical to the one they had). Those
overwrites are mostly `item.minecraft.<x>.new -> item.minecraft.<x>`, and the
consequence is that **27 item display names change** — the eighteen smithing
templates stop reading "Smithing Template" and start reading "Bolt Armor Trim",
"Coast Armor Trim", …, and the nine banner patterns stop reading "Banner
Pattern". Every one of those changes is *toward* vanilla. `inventoryshot`'s `t8`
stays green because no item **loses** a name — 1537 items, 0 without one, which
is the property that actually had to hold. The enchantment strings are
genuinely untouched: 54 keys, 0 that read differently.

**"The 383 removed keys are gone after load" is false for three of them.**
`debug.crash.message`, `debug.profiling.start` and
`selectWorld.backupRequiredTooltip` are each *also* a rename target, so removal
deletes them and a later rename writes them back. (Two of the 383 were never in
the file: 381 are present before the pass.) That is not a wrinkle to work
around — it is the **observable consequence of the pass order**, and it makes a
better witness than the one the brief asked for: running the renames first
loses all three.

#### `Component.translatable`'s substitution

`FORMAT_PATTERN` is `%(?:(\d+)\$)?([A-Za-z%]|$)`, hand-scanned (no regex dep),
with Java's greedy-then-backtrack attempt order for the optional positional
group. Three details are easy to get wrong:

- **A positional specifier does not advance the implicit counter.** `%s %1$s %s`
  over `["a","b"]` is `"a a b"`, not `"a a c"` — and a version that advanced it
  would run off the end of a two-argument list.
- **`%%` is guarded on the whole match, not just the format type.** `%1$%` also
  has `%` for a type, and is an *error*.
- **Every error renders the unsubstituted pattern.** `decompose` wraps
  `decomposeTemplate` in `catch (TranslatableFormatException) ->
  FormattedText.of(format)`, so an unsupported type, too few arguments, a stray
  `%` in the prefix or tail, and `%0$s` all render the raw string rather than
  dropping the line. That is why `lang::format` returns a `String` and not a
  `Result`.

Java's non-multiline `$` also matches before a *final* line terminator; that is
implemented as end-of-input, and the difference is unobservable — both readings
end in the same exception for the same template.

#### The rarity a stack never sent

`live_cmd::rarity_color` did `rarity.unwrap_or(0)`. But `ItemStack.getRarity()`
is `getOrDefault(DataComponents.RARITY, Rarity.COMMON)` over the **prototype**
patched by the delta, and the wire carries only the delta — so **115 items**
(78 uncommon, 18 rare, 19 epic, measured from the datagen component report)
rendered their hover name white. A music disc's name is yellow in vanilla.
`tools/gen_item_props.py` grew a sixth column; the ids come from `Rarity.java`
(read, not assumed, like the equipment slots beside them) and the default from
`ItemStack.getRarity`'s own `getOrDefault` call, because the table is a delta
from it.

Then the promotion: enchanted, `COMMON, UNCOMMON -> RARE`, `RARE -> EPIC`,
`default -> baseRarity`. Two arms are worth naming — COMMON and UNCOMMON
**collapse onto the same value** (a one-step promotion would send COMMON to
UNCOMMON), and the `default` arm is what stops EPIC becoming a 4.

**The input it reads was not there, and taking the obvious one would have been a
new bug.** `isEnchanted()` is `!getOrDefault(ENCHANTMENTS, EMPTY).isEmpty()` —
`minecraft:enchantments` **alone**. Rewo's decoder merges `enchantments` and
`stored_enchantments` into one list, deliberately, because a tooltip lists an
enchanted book's stored ones the same way. Deriving `isEnchanted` from that
merged list promotes an enchanted book RARE → EPIC, which vanilla does not do.
`StackComponents`/`SlotText` gained an `is_enchanted` flag set from the one
component. (`has_foil` still reads the merged list — a pre-existing
approximation that happens to be right for books by the wrong route, since
`enchanted_book`'s prototype carries `ENCHANTMENT_GLINT_OVERRIDE: true` and Rewo
cannot see a prototype. Left alone; it is M43's question, not this one.)

#### Gates and measurement

`inventoryshot --check` **91 → 106**: `l1`–`l9` the language map (the two
container keys resolving only after the pass; the 146 renames measured; the
383/381/3 removals; the pass order; the absent-source deletion, pinned
synthetically because 0 of 26.2's renames take that branch; the item names; the
enchantment strings; the substitution; the literal percent and the three error
shapes) and `r1`–`r6` the rarity. The subject throughout the language witnesses
is `baked.lang` — the map the production bake built — with
`Language::raw` (step 1 alone, literally what Rewo did before this) as the
mutation partner, so a gate that assembled its own map could not hide a renderer
still reading the raw file.

**`r1` was verified to fail before the fix**, not just argued to: reverting
`stack_rarity` to `patch.unwrap_or(0)` drops the gate to 104/106 (`r1` and `r6`),
and mutating `is_enchanted` to the merged list drops it to 105/106 (`r6` alone).

**652 tests** (rewo-data 79 → 94: 14 in `lang`, one pinning the rarity buckets;
the rest unchanged), release build green, `git diff --check` clean, demo PNG
SHA-256 `2cc56b4a…` byte-identical to M15 onward, and all seventeen serverless
gates exit 0 with 0 VUIDs: `itemshot` 62, `inventoryshot` 106,
`blockentityshot` 172, `swingshot` 97, `hurtshot` 38, `weathershot` 35,
`handshot` 34, `particleshot` 34, `eventshot` 28, `danceshot` 24, `portalshot`
12, `mobshot` 243/243, plus `skyshot`, `lightmapshot`, `tintshot`, `meshshot`,
`dimensioncheck`.

#### Open

- **The tooltip line generator itself.** `item.container.item_count` now
  resolves and `format` can fill it, but nothing calls either yet — the lines
  are still assembled ad hoc in `live_cmd::screen_tooltip`. That refactor is the
  next milestone, and it is what the two container keys exist for.
- **Only `en_us` and only the client jar.** `ClientLanguage.loadFrom` walks a
  *stack* of language codes across every resource-pack namespace; Rewo reads one
  file from one place. A resource pack's overrides are not applied.
- **The substitution takes `&str` arguments.** Vanilla's are `Object`, and a
  `Component` argument brings its own style — so a nested coloured argument
  would flatten. Nothing Rewo renders needs one yet.
- **`Rarity` is an `i32` throughout**, matching the wire (`STREAM_CODEC` is
  `idMapper(BY_ID, r -> r.id)`) rather than an enum. An id outside 0..3 passes
  through the promotion unchanged and colours as common.
### M57 — entity fidelity: emissive layers, ETF textures, the dye tint (2026-07-28)

Shipped. This closes the entity-appearance gaps §0.0 had left open after the mob
redo and M9/M9d: mobs that vanilla lights from within rendered dark, a resource
pack's random entity textures were the never-built half of M9b, and every sheep
was white.

Everything here is transcribed from the decompiled 26.2 jar **except ETF**,
which has no decompile because it is an OptiFine feature; that section says
exactly what it is derived from and where it is knowingly not bit-compatible.

*Provenance: built on a branch off M11 (`claude/rewo-entity-fidelity-aa134c`,
numbered M36 there) and ported file-by-file onto main 109 commits later. A
rebase gave 33 conflict hunks and a compile wall the markers never flag, so
nothing here came through `git merge`. The renumber to M57 is because M36 on
main is the inventory player preview. What the port changed relative to that
branch is recorded at the end.*

#### M57a — the emissive path

Eight of the registry's mobs have emissive layers in vanilla and none of them
glowed. A spider in a dark cave was a black spider; the warden, whose whole
visual identity is bioluminescence, had none.

**Two `RenderLayer` shapes, both of which re-render the mob's *own model* with a
second texture at full brightness.** `EyesLayer` (spider, cave spider, enderman,
phantom) draws `getParentModel()` through `RenderTypes.eyes`.
`LivingEntityEmissiveLayer` (warden x5, creaking, copper golem, breeze) takes a
model baked from a *filtered* mesh (`retainExactParts` /
`retainPartsAndChildren`), an alpha function of `(state, ageInTicks)`, and a
render type; it skips the submit entirely when the alpha is <= 1e-5.
`mobs::emissive_layers(kind)` is that table, one entry per `addLayer` call in
the matching `EntityRenderer`. It is a function rather than a `MobDef` field so
the 89-entry `MOBS` table did not have to be touched.

| Mob | Texture | Parts | Alpha |
|---|---|---|---|
| Spider, cave spider | `spider_eyes` | all | 1 |
| Enderman | `enderman_eyes` | all | 1 |
| Phantom | `phantom_eyes` | all | 1 |
| Breeze | `breeze_eyes` | `head` | 1 |
| Creaking | `creaking_eyes` | `head` | `isActive() ? 1 : 0` |
| Copper golem | `copper_golem_eyes` | all | 1 |
| Warden | `warden_bioluminescent_layer` | head + limbs | 1 |
| Warden | `warden_pulsating_spots_1` | body + head + limbs | `max(0, cos(age*0.045)*0.25)` |
| Warden | `warden_pulsating_spots_2` | " | `max(0, cos(age*0.045 + pi)*0.25)` |
| Warden | `warden.png` (the base) | the two tendrils | `tendrilAnimation` |
| Warden | `warden_heart` | `body` | `heartAnimation` |

Two entries are worth noticing. The breeze's eyes layer bakes
`retainPartsAndChildren({"eyes"})`, and vanilla's `eyes` part is an exact
duplicate of `head`'s two boxes at a zero offset with no children — so filtering
our model to `head` is the same geometry, and no `eyes` part had to be invented.
And the warden's tendril layer samples the **base** warden texture: the tendrils
light up their own texels rather than a separate overlay.

**The geometry is the same quads, another texture.** Because a layer re-renders
the same model, `build_emissive` takes the mob's own quads, filters them by the
layer's part set, and re-points their model-px UVs at the overlay texture's
atlas slot. That is why an overlay must share its base texture's pixel
dimensions — and they all do (spider_eyes is 64x32 like spider.png; the four
warden layers are 128^2). One that does not, or is missing from the jar, is
dropped with a warning rather than rendered scrambled. Part names come from the
built-in models, and from the `.jem` bone names on a CEM pack model — which
OptiFine *requires* to be vanilla's part names, since that is how a `.jem` says
what it is replacing. So the same filter serves both, and a Fresh Animations
warden keeps its glow.

**The pipeline, also transcribed.** `RenderPipelines.EYES` and
`ENTITY_TRANSLUCENT_EMISSIVE` both specify translucent blend +
`CompareOp.GREATER_THAN_OR_EQUAL` + **depth-write off**. Two things fall out of
reading that:

1. The `GREATER` half confirms 26.x is reversed-Z, the same convention Rewo
   adopted in M4 — pleasant, if incidental.
2. The `OR_EQUAL` half is load-bearing. An emissive layer redraws geometry whose
   depth the solid pass *just wrote*, so under Rewo's existing strict `GREATER`
   every fragment would be rejected. The emissive draw needed its own pipeline;
   it could not borrow the nametag one.

And per `entity.vsh`, the `EMISSIVE` shader define means the vertex shader
**samples no lightmap at all**:

```glsl
#ifndef EMISSIVE
    lightMapColor = sample_lightmap(Sampler2, UV2);
#endif
```

So the fullbright is not a brightness the layer adds — it is a multiply the
layer omits. Rewo's emissive draw writes the identity `[1,1,1]` into
`light_hurt.rgb`, which is exactly the same statement in its ABI, since
`entity.frag` ends on `c *= v_light_hurt.rgb`.

The two pipelines differ in two further defines, both reproduced:
`ALPHA_CUTOUT 0.1` on emissive and none on eyes, and `NO_CARDINAL_LIGHTING` on
eyes (flat) against `PER_FACE_LIGHTING` on emissive (directionally shaded). The
cutout is applied CPU-side as a layer-wide alpha floor rather than in the
shader, which is exact for these textures: all nine were checked and every one
is either fully transparent or at least 0.19 opaque, so `texel.a * alpha < 0.1`
can only ever fire layer-wide.

**The warden's tendrils** were flat plates folded onto the head, which meant
they could not carry `animateTendrils` and could not be retained by the tendril
layer. They are now real head children whose pivots are vanilla's `PartPose`
offsets — which are exactly the static folds they replaced, so rest geometry is
unchanged, and the facelabel gate agrees at 243/243. (The same promotion M17 did
for the ribcages, and for the same reason.) `animateTendrils` is
`xRot = +/-tendrilAnimation * cos(age*2.25) * pi * 0.1`, the left tendril
positive.

#### M57b — ETF, a pack's random entity textures

The texture half of M9b. A pack ships a `.properties` file per vanilla entity
texture listing alternates with weights and conditions; a client picks one per
entity, stably, so a herd of cows is not all the same cow.

**Provenance — the one subsystem with no ground truth.** Every other part of
Rewo is transcribed from the decompiled jar. This one cannot be: random entity
textures are an OptiFine feature and OptiFine is closed source. What
`rewo-data/src/etf.rs` implements is its *documented*
`random_entities.properties` format, and two consequences are stated in the
module docs rather than quietly assumed:

1. **The choice function is ours.** OptiFine's hash is unpublished, so `pick`
   uses a documented splitmix over the entity UUID. It has the properties that
   matter — stable per entity, uniform, weight-respecting — but will not give
   the same cow the same variant OptiFine would. Nothing syncs this between
   clients in vanilla either, so it is cosmetic.
2. **Conditions we cannot evaluate never match.** `biomes`, `health`,
   `professions`, `collarColors`, `weather`, `blocks` and `nbt` are not decoded
   by Rewo, so a rule carrying one is inert. That direction is deliberate:
   skipping falls back to the vanilla texture, whereas assuming a match would
   paint swamp textures on every mob everywhere.

Supported, because Rewo genuinely knows them: `weights`, `names` (plain,
`pattern:` and `ipattern:` wildcards), `baby`, `sizes`,
`heights`/`minHeight`/`maxHeight`, `moonPhase`, `dayTime`. One further
constraint is Rewo's own, not OptiFine's: an alternate must match the vanilla
texture's dimensions, because the atlas gives it the same UV rectangle.

**How a variant reaches a pixel.** Alternates are shelf-packed into the entity
atlas alongside the base textures, and each mob gets a table of
`variant id -> per-texture-slot UV offset`. `EntityDraw` carries one `variant`
field; a quad adds the offset for **its own** texture slot, which is why
`GpuQuad` now remembers which texture it came from — the base bake folds `tex`
into the UVs and would otherwise lose it. Slots a variant does not cover keep
the vanilla texture, which is what a pack varying only one of a mob's textures
wants: a rule on `sheep_wool` recolours the wool and leaves the face and legs
alone. This is the same mechanism real player skins already used (a whole-model
UV shift onto an uploaded slot), generalized per slot. It is also the machinery
vanilla's *own* metadata-driven variants will need — cat, horse, llama,
axolotl, frog, tropical fish — when their metadata indices get decoded; that
recorded gap is now one decode away rather than a rendering problem.

**Three bugs the tests found**, all invisible in a screenshot and wrong in
aggregate:

- A rule naming the *vanilla* texture (`textures.1=cow.png`, or the
  MCPatcher-era `skins.1=1`) is how packs give the original a share of the
  weighting, and the loader was dropping it as "textureless" — which would have
  handed that share to the alternates and made nearly every cow a variant cow.
  Such a rule is now a first-class variant with no image, and `pick` resolves it
  to 0; a unit test holds the 1:1 split at half the herd.
- A range with a negative low bound (`-64--30`, a perfectly ordinary `heights`
  value) split at the sign and failed to parse.
- `nbt.<index>.<path>` carries its index in the middle, unlike every other key,
  so those lines were dropped entirely instead of marking the rule unsupported.

#### M57c — ETF emissive overlays

The other half of what ETF does with textures, and cheap once M57a existed. A
pack shipping `<texture>_e.png` beside a mob texture means "these texels are
lit"; the loader walks the baked mob textures looking for that sibling (or
whatever `optifine/emissive.properties` sets `suffix.emissive` to), and the
entity pass turns each into a whole-model layer with alpha 1 and the alpha
cutout OptiFine renders them through. The overlay covers exactly the quads that
sample the texture it belongs to, so on a two-texture mob an overlay on one
leaves the other alone. It rides the variant atlas machinery under a reserved id
far above any properties rule index; the constant is mirrored in `rewo-gpu`
(which cannot depend on `rewo-data`) and a test pins the two together.

#### M57d — the sheep's dye tint

`SheepWoolLayer` renders the fur model tinted by
`ColorLerper.Type.SHEEP.getColor(woolColor)`, which is
`DyeColor.getTextureDiffuseColor()` at 0.75 brightness — except WHITE, which
`ColorLerper.getModifiedColor` overrides outright to `-1644826` (0xE6E6E6)
rather than dimming. `SHEEP_WOOL_COLORS` is that table with both rules folded
in, and the entity pass multiplies it into the vertex colour of exactly the
quads that sample the mob's tinted texture.

**An undyed sheep is not an untinted sheep.** `SheepRenderState.woolColor`
starts at `DyeColor.WHITE` and the layer tints unconditionally, so vanilla's
plain white sheep has its wool multiplied by 0xE6E6E6. Rewo was rendering it at
full brightness. `EntityDraw::dye` is therefore `None = the mob's vanilla
default`, not `None = no tint`, and every sheep in live play now gets white's
0.90 multiply — a visible change, and the correct one.

#### The wire inputs (new in the port — these were the branch's open caveats)

The branch shipped all three of the above with the renderer half only, reading
"vanilla's synched defaults" because the packets were not decoded at M11. All
three are decoded now, and each is one mapping. **Two of the branch's handoff
notes were wrong about the details, and the decompile settled both.**

- **entity_event 61 -> the warden's tendrils.** `Warden.handleEntityEvent(61)`
  is `tendrilAnimation = 10`, decremented once per client tick, read back as
  `getTendrilAnimation = Mth.lerp(a, prev, cur) / 10`. It is not an
  `AnimationState` like ids 4 and 62, but it is stamped the same way, so it
  takes a fourth `EntityEvent` slot through M17's `route_entity_event` and the
  renderer reads `max(0, 10 - elapsed) / 10`.
- **`Creaking.IS_ACTIVE` is index 17 BOOLEAN**, not BYTE as the handoff said.
  `Creaking` declares `CAN_MOVE` first, so `IS_ACTIVE` is its second.
- **`Sheep.DATA_WOOL_ID` is index 18, not 17.** `AgeableMob` declares **two**
  accessors — `DATA_BABY_ID` (16) *and* `AGE_LOCKED` (17) — so a `Sheep`'s own
  first accessor lands one slot further down than a naive count suggests.
  Counted `defineId`-by-`defineId` up both hierarchies in the 26.2 decompile:
  Entity 8, LivingEntity 7, Mob 1, PathfinderMob 0, AgeableMob 2, Animal 0. The
  low nibble is the dye and 0x10 is sheared, per `Sheep.getColor`/`isSheared`.

Index 17 BOOLEAN now has three claimants — `Pillager.IS_CHARGING_CROSSBOW`,
`Creaking.IS_ACTIVE` and `AgeableMob.AGE_LOCKED` — so the kind gate is what
separates them, the M18 rule again. `AGE_LOCKED` drives nothing the client
renders and falls through both gates.

`eventshot`'s `c1.excluded_tendril_inert` witness was renamed and
**strengthened rather than retired**: it now asserts that 61 stamps the tendril
slot *and* still leaves both keyframe rigs alone. `Warden.handleEntityEvent` is
an if/else-if chain, so 61 reaching the attack or sonic-boom `AnimationState`
would mean the dispatch had fallen through. Witness count unchanged at 28.

#### Verification

Three permanent serverless gates, all following the M17+ named-witness
convention (each property recorded by name, fail-closed on the count).

**`rewo mobshot --emissive-check` (5 witnesses): an emissive layer ignores world
light, and nothing else does.** Every mob is rendered lit (the silhouette) and
at world light 0, where the base model multiplies to exactly black — so any
pixel still bright can only have come from an emissive layer. That gives a
two-sided assertion over the whole registry: 7 mobs must glow inside their own
silhouette, and the other 82 must be perfectly black. The control half is the
point: it fails if the emissive pass leaks onto the wrong mob, or if the base
pass ever stops respecting world light. The glow count is also compared against
what the decompiled layer table predicts, so a mob quietly gaining or losing a
layer is an error rather than a silent pass.

**A mutation found a real hole in that gate before it was trusted.** The state
assertion originally compared glow counts at a generic age — and a build with
the tendril alpha hard-wired to `0.0` still passed, because
`EmissiveState::tendril` feeds *both* the sway and the alpha, and rotating
tendrils uncover a few pixels of the glowing head behind them. The gate now
renders at the exact zero of `cos(age*2.25)`, freezing the sway so only the
alpha can move the number; the same mutation then fails. Four mutations in
total: emissive respecting light (8 mobs fail), the tendril alpha pinned (1), a
layer leaked onto another mob (1), the base pass ignoring light (84).

**`rewo mobshot --etf-check` (8 witnesses).** There is no published pack to
grade against and no decompile to transcribe, so the gate builds its own
resource pack in a temp directory — real `.properties` files, real PNGs, loaded
through the same `load_pack` the live client uses — and paints the alternates
flat primary colours so the assertions are exact. It pins that a variant paints
its own texture (catching an offset computed against the wrong slot, an
alternate that never reached the atlas, and an id that silently falls back),
that an id with no atlas slot falls back to vanilla rather than sampling
whatever is packed next door, that a vanilla-texture rule is kept rather than
dropped, that a rule on one texture moves *only* that texture (a per-slot table
indexed by the wrong axis passes everything else and fails here), and the pack
`_e.png` overlay's three properties.

`f7`'s fixture patch was a *quarter* of the sheet at first and lit 87% of the
rendered pig: the top-left of a 64x64 mob sheet carries the head and most of the
body, so **the fraction of the sheet an overlay covers is not the fraction of
the model it lights**. A sixteenth (45%) leaves the "alpha ignored" mutation an
unambiguous margin.

**`rewo mobshot --tint-check` (4 witnesses), all sixteen dyes.** Comparing
pixels to the colour table directly would be defeated by the per-face shade, so
the gate compares **ratios**: because vanilla multiplies the dye into the vertex
colour, every wool pixel must satisfy
`linear(dyed)/linear(white) == linear(color[k])/linear(color[0])` per channel,
whatever the shading or geometry. That prediction comes from the layer's
semantics, not from the renderer, and it holds for all sixteen at once.

The first version had the same flaw the emissive gate did, and again it took a
mutation to find: it defined "wool pixels" as those two dyes disagree on, which
is derived from the behaviour under test — a tint leaking onto every texture
simply redefined the whole sheep as wool and passed the containment check
vacuously. The wool set is now bounded against the silhouette, which is
independent: vanilla's sheep shows a bare face, four legs and hooves, so a
correct tint can never cover the whole mob. With that fixed, tinting every slot
fails, and so does dropping the sRGB linearize before the multiply — render
discipline #1, caught numerically as a 0.805x ratio where vanilla's is 0.621x.

**Measured.** 659 tests (was 637: +14 in `etf.rs`, +8 in `entities.rs`). All
seventeen gates green with Vulkan validation ON and **0 VUIDs**: mobshot 243/243
+ emissive 5/5 + etf 8/8 + tint 4/4, itemshot 62, inventoryshot 91,
blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35, handshot 34,
particleshot 34, eventshot 28, danceshot 24, portalshot 12, plus skyshot,
lightmapshot, tintshot, meshshot and dimensioncheck. The demo PNG is SHA-256
byte-identical to M15 onward (`2cc56b4a...46635`).

`rewo bench` is unaffected by construction: it never initializes the entity pass
(`grep -c init_entities crates/rewo-app/src/bench_cmd.rs` -> 0). Its numbers on
this machine swing between 0.22 ms and 2.4 ms average across back-to-back runs
depending on what else is using the GPU, so quoting a delta would be noise
rather than a measurement.

#### What the port changed relative to the branch

- The branch introduced a `PipeKind` enum to select the emissive pipeline's
  depth compare. Main's `build_pipeline` already takes
  `(solid: bool, compare: vk::CompareOp)` — M48 gave it that for the trim — so
  the enum was **dropped** and the emissive pipeline is one more call site.
- The vertex ring went from 2 ranges at M11 to **5** on main
  (`solid | text | glint | trim | armor_glint`). The emissive range is appended
  at the **end** rather than after `solid`, so every existing first-vertex
  offset is untouched; storage order is not draw order, which main's own comment
  there already said. `draw_emissive` still runs immediately after
  `draw_solid`, which is where vanilla's `order(1)` layer submits land.
- `EntityDraw::light` became `[f32; 3]` (M13) and `Vertex` gained
  `light_hurt: [f32; 4]` (M21's hurt flash). The branch's "leave `d.light` out"
  becomes "write the identity into `light_hurt.rgb`" — the same claim in the
  current ABI.
- `emit_model` is a different function now (armour, held items, glint and trim
  all emit from it). The branch's `place`/`visible` closure extraction was
  redone against the current body; `visible` had to grow the two `Show` variants
  M20 added (`IllagerCrossedOnly` / `IllagerNotCrossed`), and the boolean form
  was checked arm-by-arm against main's guard chain.
- `collect_entities` went 8 -> 12 params and the branch's `sky: SkyLighting`
  argument is gone — M13 replaced the entity-light input with
  `lightmap: &LightmapState`. `SkyLighting` still exists on main but only feeds
  the sky and dimension path, so it is *not* the entity-light input any more.
- The three gates were rewritten from the branch's ad-hoc pass/fail loops into
  the named-witness convention, keeping every assertion and every mutation
  argument. Per-mob detail survives in the failure strings.
- The branch's `REWO_M36_ENTITY_FIDELITY.md` is folded into this entry rather
  than kept as a separate file.

#### Open

- **Vanilla's metadata-driven texture variants** (cat, horse, llama, axolotl,
  frog, tropical fish, wolf). Still fixed picks. The rendering half now exists —
  this is the same per-slot variant table ETF uses — so what remains is decoding
  their metadata indices in `rewo-net`.
- **Sheep shearing.** The byte is decoded (0x10) but the model has no sheared
  variant, so our sheep always wear wool. So is the jeb_ rainbow, which is
  `getLerpedColor` over this same table on a 25-tick cycle.
- **ETF variant emissive** (`cow2_e.png`): an alternate's own emissive overlay.
  The base texture's `_e` applies to every variant.
- **`regex:`/`iregex:` name matching.** Plain and wildcard forms work; a rule
  using regex is treated as unsupported and never matches — the safe direction
  but not the complete one.
- **A real ETF pack.** None was on disk (Fresh Animations is CEM models and
  animations, no random textures), so the gate builds a fixture. A real pack
  should be run through `--pack` when one is available; the loader logs every
  rule and texture it accepts or drops.
- **The ender dragon's flight animation**, examined during the branch and
  deliberately deferred. `EnderDragonModel.setupAnim` is driven by
  `DragonFlightHistory`, a **64-entry ring of `(y, yRot)` samples** the dragon
  records once per client tick; the five neck segments sample delays 5->1, the
  head 0, the twelve tail segments 12->23, and the body's roll is the difference
  between delays 5 and 10 — which is what makes neck and tail trail behind its
  turns. Rewo already has both inputs per tick. Two mechanisms are missing: a
  per-entity *tick-rate* ring (`set_draws` runs per frame, and the ring must
  survive the entity being briefly culled), and chain accumulation in
  `part_transforms` (vanilla walks the neck imperatively, each segment's
  position computed from the previous one's resolved angles; Rewo's per-part
  animation is declarative). Add the model's own transcription and it is a
  milestone in its own right — against which: the dragon is the rarest mob in
  the game, one per world, and it already renders with the correct 30-cube mesh.
  It is posed, not missing.

### M50 — the worn-armour glint, and the glint's colour space (2026-07-28)

Shipped. The milestone the facts entry below predicted turned out to be a small
thing wrapped around a much larger one: the armour foil itself is a second sheet
and a fifth vertex range, but making it *visible* meant putting every glint's
blend back into the space vanilla evaluates it in.

#### Two of the recorded facts were wrong, and the decompile says so plainly

**`VIEW_OFFSET_Z_LAYERING` is not the glint's mechanism.** All three armour
render types carry it — `ARMOR_CUTOUT_NO_CULL` (`RenderTypes.java:38`),
`ARMOR_DECAL_CUTOUT_NO_CULL` (`:410`) and `ARMOR_ENTITY_GLINT` (`:243`) — each
applying the same bias to a fresh `getModelViewMatrixCopy()`, so it **cancels
exactly within the armour stack**. What it separates is the armour from the
*body*, not the foil from the armour. And `RenderPipelines.GLINT` is
`DepthStencilState(CompareOp.EQUAL, false)`: the foil *is* a depth-EQUAL pass,
the same one Rewo had shipped three times. That cancellation is the only reason
an EQUAL test can work there at all.

For `PERSPECTIVE` the nudge is `matrix.scale(1 - 1/4096)` on the **modelview**,
which is a uniform scale about the camera — a slide along the view ray. The
perspective divide cancels it in x and y (`a·k·x / −k·z`), so it moves depth and
nothing else, which is what "view offset Z" means.

**The foil is untinted.** The facts entry had it carrying the layer's colour.
Three independent confirmations: `RenderPipelines.GLINT` binds
`DefaultVertexFormat.POSITION_TEX` — Position and UV0, **no Color element** —
and `BufferBuilder.beginElement` returns `UNKNOWN_ELEMENT` for an absent one, so
`setColor` is a no-op; `glint.vsh` declares only `in vec3 Position; in vec2
UV0;`; and `RenderType.writeDynamicTransforms` calls the `(Matrix4f, Matrix4f)`
overload, which writes `ColorModulator` as **WHITE**. The `color` argument
reaches the glint's `submitModel` and is dropped. A dyed leather chestplate's
foil is the plain sheen.

The one fact that held is the headline: **the trim must not glint**.
`renderLayers` clears `renderFoil` inside the layer loop and submits the trim
after it, so the foil rides the first layer that draws, once, and the trim never
gets one.

#### The glint's blend does not survive a colour-space change

The armour foil went in structurally correct — geometry, depth, UVs and sheet
all verified — and rendered a byte-delta of **exactly 0**. Not a threshold
artefact; nothing at all.

`BlendFunction.GLINT` is `(SRC_COLOR, ONE, ZERO, ONE)`, so the contribution is
`src²`, and vanilla evaluates that in **gamma** space: Minecraft binds no sRGB
framebuffer and no sRGB texture view, so both the sampled texel and the
destination are gamma-encoded numbers. Rewo works in linear light. **Squaring is
not invariant under the transfer function.** A mid armour-glint texel (byte 85)
adds `(0.333 × 0.75)² = 0.0625` in gamma — **+16/255** on a bright chestplate —
against `(0.0925 × 0.75)² = 0.0048` in linear, which is +0.9/255 and quantises
away. The item glint had the same error since M43 and hid it, because a dropped
stack sits against a *dark* background where the sRGB curve is steep enough to
make a tiny linear increment visible: measured side by side on one frame, item
glint max byte delta **137**, armour foil **0**.

**No fixed-function blend can fix it.** `(SRC_COLOR, ONE)` gives `dst + out²`,
needing `out² = to_linear(to_srgb(dst) + g²) − dst`; `(DST_COLOR, ONE)` gives
`dst·(1 + out)`, needing `out = 2.4·g²/to_srgb(dst)`. Both require reading the
destination, which a fragment shader cannot do here. An intermediate attempt
that emitted `sqrt(to_linear(g²))` is exact against black and falls away as the
destination brightens — it left iron still byte-identical — and was dropped.

So the glint renders **through a UNORM view of the same image**: the offscreen
image and the swapchain are created `MUTABLE_FORMAT` with a format list naming
both, `world::draw` closes the surrounding rendering scope around each glint
draw and reopens it against the UNORM view (`LOAD`/`STORE` on both attachments,
so the depth the solid pass wrote survives for the EQUAL test), and the sheets
upload UNORM so `texture()` hands the shader vanilla's raw byte/255. Both glint
shaders are then vanilla's line verbatim, needing no conversion in either
direction. `VK_KHR_swapchain_mutable_format` is an extension rather than core;
without it there is no gamma target and **no glint is drawn**, the same
degradation as a missing sheet.

#### The milestone proper

One `EntityGlint` per sheet over one shared pipeline (vanilla has a single
`RenderPipelines.GLINT` behind all four glint render types); a fifth vertex
range, `solid | text | glint | trim | armor_glint`; and `draw_armor_glint`
before `draw_trim`, because `renderLayers` submits layer, foil, trim at
increasing `order` and `SubmitNodeStorage` keeps its phases in an
`Int2ObjectAVLTreeMap` — a sorted map, drained ascending — so a trim's opaque
texels paint over the foil. The foil quads are pushed from inside the armour
emitter, beside the vertex they shadow, for the reason M45 records: the pass
depth-tests EQUAL, so a position derived a second time would be rejected
fragment by fragment. Their UV is the quad's own `0..1` in its own **64×32**
sheet, which is what `ModelPart.Cube` already divided by.

#### Three detector errors, all mine

The pattern M38 named recurred, and each was caught by measuring rather than
looking. `a3` used `changed`, whose `> 8` threshold was built for the item glint
against a dark background, and read a real 5/255 sheen as nothing. `a5` compared
a ~0.005 linear contribution *per channel* through an 8-bit sRGB pipe where one
LSB at that brightness is ~0.006 — the quantisation step was larger than the
signal. And its first fixture used brown against red leather, both bright enough
that red and green pinned at 255 and were skipped, so it measured **exactly
zero** in two of three channels. The fix moved the measurement into the space
the blend now works in: vanilla's add is base-independent **in bytes**, so the
byte delta between two dyes is the property — and it comes out **0**.

A hue ratio would not have worked either: `enchanted_glint_armor.png` is
blue-dominant (mean R18 G7 B46), and at scale 0.16 the patch each face samples
is blue-only, so red/green is 0/0.

#### Verified

`itemshot --check` **54 → 62**. Eight new witnesses: the armour foil has its own
sheet; its own texture scale (0.16 against the entity glint's 0.5, a factor of
3.125); it lands on the armour's own fragments and nowhere else (3,475 pixels, 0
strays); the same piece unenchanted twice is byte-identical; it is not tinted by
the layer's colour (byte contribution differs by **at most 0** between two
opposite dyes); one foil per piece however many layers it draws (108 vertices
for one layer and for two); the trim never glints (a trimmed enchanted piece
emits the same 108, a trimmed unenchanted one emits 0); and the trim paints over
the foil, measured on 870 pixels identified as fully-opaque trim by rendering
the same trim over two different armour colours and taking where they agree.

**633 tests**; all seventeen gates green with Vulkan validation ON and **0
VUIDs**; demo PNG SHA-256 `2cc56b4a…`, byte-identical since M15.

#### The live check, and what it could not do

The wire path is the one thing the gate cannot cover — it constructs
`ArmorPiece` directly — so it was verified live: an enchanted chestplate on a
summoned zombie decodes `ench=[(28, 4)]` off the equipment packet's component
patch and reaches the renderer with `foil=true`, where a plain one does not.

**The frame diff was rejected as an oracle.** Enchanted-against-plain changed
16,329 pixels; a same-item control run changed **41,284** — the noise exceeded
the signal, so the number means nothing. That is M37's rule (a frame-diff
witness must hold everything but the subject constant) and its diagnostic tell.
The likely source is the pre-existing entity-atlas collision M46 records, which
makes a mob's texture vary run to run.

**And the first live run failed on the harness, not the client.** The summon
used the pre-1.21.5 `enchantments:{levels:{…}}` wrapper; 26.2's
`ItemEnchantments` codec takes the map directly, so the server silently produced
an *unenchanted* chestplate, and the component id that showed up was a stale
entity's trim. Cross-checking Rewo's id table against the datagen report —
`swing_animation` 40, `damage` 3, `charged_projectiles` 49, `dyed_color` 44, all
exact — is what pointed at the command rather than the decoder. Same shape as
M35's click against a stale state id and M20.1's build gate.

#### Open

- The glint is dropped entirely on a device without
  `VK_KHR_swapchain_mutable_format`. Every desktop driver we target advertises
  it; a fallback would need the gamma-space add done somewhere it can read the
  destination.
- The gamma-space transcription is exact for the *blend*; Rewo's remaining
  colour handling is unchanged, so this closes the glint's discrepancy and says
  nothing about any other additive effect.
- No `humanoid_baby` layer, and `usePlayerTexture` (the elytra cape) is still
  read as data and never honoured — both inherited from M46/M47.

### M50 (superseded) — the armour glint: the facts, not the code (2026-07-28)

**Not implemented.** Established from the 26.2 decompile and jar; the build is a
fresh session's work. Recorded because one of these facts changes what the
milestone *is*.

#### The trim does not glint, and that is a rule to reproduce

`EquipmentLayerRenderer.renderLayers` clears the flag inside the layer loop:

```java
for (Layer layer : layers) {
   int color = getColorForLayer(layer, dyeColor);
   if (color != 0) {
      submitModel(..., armorCutoutNoCull(layerTexture), ..., color, ...);
      if (renderFoil) submitModel(..., armorEntityGlint(), ..., color, ...);
      renderFoil = false;
   }
}
ArmorTrim trim = itemStack.get(DataComponents.TRIM);   // submitted after; never foiled
```

So the foil rides the **first layer that draws** — once, with *that layer's
colour* — and the trim, submitted after the loop, never gets one. A glinting
trim would be a Rewo invention. What is actually missing is the **worn-armour
glint**, which M45 called its fourth surface and could not reach because
nothing rendered armour; M46 made it reachable.

#### What the armour glint is

- **A different sheet.** `ARMOR_ENTITY_GLINT` binds
  `misc/enchanted_glint_armor.png`, not the `enchanted_glint_item.png` M43
  loads. Both exist in the jar; only the item one is baked today.
- **Scale 0.16** — `ARMOR_ENTITY_GLINT_TEXTURING` is
  `setupGlintTexturing(0.16F)`, against the entity contexts' 0.5 and the item
  contexts' 8.0. It matches the `GLINT_SCALE_ARMOR` constant M43 already
  transcribed and nothing has used.
- **`VIEW_OFFSET_Z_LAYERING`**, *not* a depth-EQUAL pass — the projection is
  nudged toward the viewer (`applyLayeringTransform(matrix, 1.0F)`). That is a
  different mechanism from every glint Rewo has shipped, all of which depth-test
  EQUAL, and it is the part to get right rather than to assume.
- **Tinted by the layer's colour**, so a dyed leather piece's foil carries the
  dye — the same `color` argument the armour layer itself took.

#### The one structural piece

Rewo's `EntityGlint` owns its pipeline, image, sampler, pool and set, and the
glint is one vertex range. A second sheet needs a second `EntityGlint` (the
pipeline can be shared) plus a fifth vertex range and its draw — the same shape
as M48's fourth range, and about that much work.

### M49 — trims on GUI icons (2026-07-28)

The blocker M48 named, and the bake refactor under it.

#### The icon is a different model

`assets/minecraft/items/iron_chestplate.json` is a **`select` on
`minecraft:trim_material`** whose `when` values are material **registry ids**
(`minecraft:quartz`, not the `asset_name` suffix M48 uses for the worn sheet),
each case naming a whole different model, with the plain model as `fallback`.
Those models exist — 337 of them — and each is an ordinary two-layer
`item/generated`: layer0 the base item texture, layer1 the trim sprite.

That sprite comes from a **second** paletted-permutations atlas.
`armor_trims.json` covers only the 36 `trims/entity/*` sources; `items.json` is
a separate source over four 16x16 sheets, through the *same* key palette and
the *same* sixteen permutations — so M48's `apply_palette` is already the whole
generator, and `TrimAssets` needed one more prefix.

#### Composed keys, not a restructured map

`ItemModels` was `HashMap<String, ItemModel>` keyed by item name and baked
once. Rather than making the key a pair — which would touch the bake, every
icon caller, and the pool 337 extra models feed — a variant is baked under
**`"<item>#<material id>"`**. Every existing lookup is by plain name and is
untouched, and `HeldItems::any` falls back from a composed name to the base.

That fallback is not a nicety: an item can be trimmed with a material its own
definition names no case for, and vanilla's answer there is the `fallback` —
the untrimmed icon. So a missing variant has to degrade to it rather than to no
icon at all.

The variants are driven by the definition's own `cases`, not by the material
registry, because this is a load-time bake of the **jar** and the registry is
the **server's**.

#### The bug that hid the whole feature

Everything above resolved — the variants baked, the sprite permuted, the
composed name matched the baked key — and the icons still rendered plain.

A multi-layer sprite item is **coplanar by construction**:
`ItemModelGenerator` puts every layer in the same `z 7.5..8.5` slab, and the
extruder's `layer` argument selects the texture, not a depth. The GUI item
pipeline depth-tested strict **`GREATER`** with write enabled, so layer1 — at
*exactly* layer0's depth — was rejected outright.

Vanilla's item pipeline tests `LEQUAL`, whose reversed-Z counterpart is
**`GREATER_OR_EQUAL`**. One word, and the layered icon is a layered icon: equal
depth passes, so a later layer paints over an earlier one in submit order.

This is the same shape as M48's trim-on-armour (coplanar geometry against a
strict test) and the third time this arc that a depth *comparison* was the
whole story — worth reaching for first when geometry is provably present and
provably invisible.

#### Verified

`itemshot --check` **51 -> 54**, **633 tests**, all seventeen gates green with
validation ON and 0 VUIDs, demo PNG byte-identical to M15 onward. Live: an iron
chestplate with a `gold`/`coast` trim wears gold banding in the hotbar beside a
plain one that does not, and diamond leggings with `redstone`/`eye` wear red.

Two new witnesses measure the thing that was broken rather than the plumbing
around it: `u1` that a variant bakes **one more sprite layer** than its base,
`u2` that an unnamed material resolves to the base's single layer. `a1` learned
to count items as the keys without a `#`, and `a1b` checks every variant sits
over an item that resolved on its own.

#### Open

- The **worn armour does not glint** — M45's fourth surface, reachable since
  M46. The *trim* correctly must not: `renderFoil` is cleared before the trim
  is submitted, so a glinting trim would be an invention. Facts gathered in the
  M50 entry.
- No `humanoid_baby` layer.
- `usePlayerTexture` (the elytra cape) is read as data and never honoured.

### M49 (superseded) — the original investigation note (2026-07-28)

**Not implemented.** The facts below are established from the 26.2 jar and
decompile and cost nothing to re-verify; the implementation is a fresh
session's work. Recorded here so that session starts at the code.

#### What the icon actually is

`assets/minecraft/items/iron_chestplate.json` is a **`select` on
`minecraft:trim_material`**, whose `when` values are material **registry ids**
(`minecraft:quartz`, not the `asset_name` suffix), each case naming a whole
different model, with the plain model as `fallback`:

```json
{"model": {"type": "minecraft:select", "property": "minecraft:trim_material",
  "cases": [{"when": "minecraft:quartz",
             "model": {"type": "minecraft:model",
                       "model": "minecraft:item/iron_chestplate_quartz_trim"}}, ...],
  "fallback": {"type": "minecraft:model", "model": "minecraft:item/iron_chestplate"}}}
```

Those models exist — **337** `_trim` model files — and each is an ordinary
two-layer `item/generated`:

```json
{"parent": "minecraft:item/generated",
 "textures": {"layer0": "minecraft:item/iron_chestplate",
              "layer1": "minecraft:trims/items/chestplate_trim_quartz"}}
```

#### The layer1 sprite is generated, by a *second* atlas

`armor_trims.json` holds only the 36 `trims/entity/*` sources M48 reads.
The item sprites come from **`assets/minecraft/atlases/items.json`**, a second
`paletted_permutations` source over just four textures —
`trims/items/{helmet,chestplate,leggings,boots}_trim` — through the **same**
`trims/color_palettes/trim_palette` key and the same 16 permutations.

So M48's `equipment::apply_palette` is already the whole generator. Only
4 x 16 = **64** sprites, 16x16 each, which is small enough to pre-generate at
bake time and register under their permuted names rather than demand-fill.

#### The one hard part

`ItemModels` is `HashMap<String, ItemModel>` — **keyed by item name alone**,
baked once. A trimmed icon is a *different model*, so the key has to become
`(item, trim material)`, which touches the bake, every caller that resolves an
icon (GUI slots, the hotbar, the hand, ground items), and the item texture pool
that 337 extra models feed. That is the milestone; the rest is small.

`SelectionContext` needs the material's registry id, which means either a
lifetime on a currently `Copy` lifetime-free struct or an owned field. The value
comes from `StackComponents.trim` (M48 captures `(material, pattern)` ids)
through `session.trim_materials[id].id`.

#### Two things that are already done

- The wire half: `minecraft:trim` is captured (M48), so a slot knows its
  material id — `ItemSlot` needs to carry it, nothing needs decoding.
- The palette half: `apply_palette` + the key palette are loaded and gated
  (`itemshot` t1/t2). `TrimAssets::load` reads `trims/entity/` and
  `trims/color_palettes/`; it needs `trims/items/` added, one line.

### M48 — armour trims (2026-07-28)

The third armour layer, and the one that is not a texture in the jar.

#### The sprite does not exist until you make it

`assets/minecraft/atlases/armor_trims.json` declares a **`paletted_permutations`**
source: 18 greyscale pattern sheets, one key palette, 16 material palettes, and
the client generates every `pattern x material` sprite at load by swapping
colours. The algorithm is small and has two invertible details:

```java
for (int i = 0; i < keys.length; i++)
   if (ARGB.alpha(keys[i]) != 0) palette.put(ARGB.transparent(keys[i]), values[i]);

pixel -> {
   int pixelAlpha = ARGB.alpha(pixel);
   if (pixelAlpha == 0) return pixel;
   int value = palette.getOrDefault(ARGB.transparent(pixel), ARGB.opaque(pixel));
   return ARGB.color(pixelAlpha * ARGB.alpha(value) / 255, value);
}
```

The match is on **RGB with the alpha masked off**, both when building the map
and when looking up — so a half-transparent pixel of a palette colour still
maps, and takes `pixelAlpha * valueAlpha / 255`. And an **unmatched pixel is
not dropped**: `getOrDefault` hands back `opaque(pixelRGB)`, whose alpha is 255,
so the pixel survives untouched. Working in RGBA bytes rather than packed ints
sidesteps whether `NativeImage.getPixels` is ARGB or ABGR — keys, values and
source all come from one decoder, so a consistent channel order cancels.

#### Two more datapack registries

`trim_material` and `trim_pattern` are **datapack** registries, so — exactly as
for M42's enchantments — their contents *and their id order* are the server's,
and the vector index is the protocol id. Both are parsed out of Configuration's
`registry_data` (11 materials, 18 patterns from a vanilla server). Both carry
`MapCodec`s that **inline** into the entry compound, so `asset_name` and
`override_armor_assets` are top-level fields and not nested — the same rule M42
found for `max_level`.

The sprite name is `ArmorTrim.layerAssetId`:

```java
MaterialAssetGroup.AssetInfo materialAsset = material.assets().assetId(equipmentAsset);
return pattern.assetId().withPath(p -> layerAssetPrefix + "/" + p + "_" + materialAsset.suffix());
```

**`assetId(equipmentAsset)` is what stops a trim disappearing.** It is
`overrides.getOrDefault(equipmentAssetId, base)`, and iron/gold/diamond/
netherite/copper each declare an override to `<material>_darker` **for their own
armour** — without it an iron trim on iron armour paints iron onto iron and
vanishes. Note the key is the *equipment asset*, not the material.

#### Depth EQUAL, because the geometry is identical

`ARMOR_DECAL_CUTOUT_NO_CULL` is `DepthStencilState(CompareOp.EQUAL, false)` —
depth-test equal, write nothing. That is the same trick M43's glint uses, and
it is the only sane way to paint a decoration onto geometry it is coplanar
with: Rewo's world pass is reversed-Z with a strict `GREATER`, so a second draw
of the same triangles would be rejected outright.

Vanilla has two pipelines here — `decal` selects the EQUAL one and the ordinary
armour pipeline is `LEQUAL`. **Both collapse to the same result** because the
trim's geometry is the armour's geometry to the bit, so "equal passes" and
"less-or-equal passes" select the same fragments. `decal` is decoded and
recorded rather than acted on, and the trim gets one pipeline.

The trim is a **fourth vertex range** — `solid | text | glint | trim` — drawn
after the solid pass and before the glint, which is where vanilla puts it: it is
another armour layer, submitted under the foil rather than over it.

#### A demand-filled pool, and an atlas that grew at the top

18 patterns x 17 palettes x 2 layer types is 612 sheets, which no band of the
entity atlas can hold — the same arithmetic that sent M22's items to a pool.
Trims get one too: 64 slots of 64x32, filled on first sighting, keyed by sprite
path so a pattern worn by two entities is permuted once.

`ATLAS_H` grew 1280 -> 1408 and the pool went at the **top**, with the skin and
item pools redefined *downward* from it. Every existing region kept its exact
address — `SKIN_POOL_Y` is still 1152 and `ITEM_POOL_Y` still 896 — so no mob,
item or skin UV moved, and `mobshot` stayed 243/243 without needing to be
re-verified for placement.

#### A leak the gates caught

The new pipeline was created and never destroyed, which no witness tests for
directly — `VUID-vkDestroyDevice-device-05137` fired in three gates at once
(`itemshot`, `hurtshot`, `lightmapshot`) with **zero failed witnesses**. The
project's "0 VUIDs" bar is what caught it; a green witness count would not have.

#### Verified

`itemshot --check` **46 -> 51**, **633 tests**, all seventeen gates green with
validation ON and 0 VUIDs, demo PNG byte-identical to M15 onward. Live: an iron
chestplate with a `gold`/`coast` trim renders the gold banding across the chest
and shoulders, beside a plain iron one that does not.

#### Open

- ~~**The trim is not on GUI icons**~~ — **RESOLVED in M49.** Variants bake
  under a composed `"<item>#<material id>"` key and the GUI pipeline's depth
  test became `GREATER_OR_EQUAL`, without which a coplanar layer1 was rejected
  and every trimmed icon rendered as its plain base.
- **No `humanoid_baby` layer.** Vanilla has a third layer type with its own
  sources; Rewo's baby mobs already use the adult armour parts (M46).
- **The trim does not glint.** `renderLayers` draws the foil after the first
  layer that draws and then clears the flag, so a foil trim is one more pass
  that armour glinting (M45's fourth surface) would need.
- **`decal` is decoded, not acted on** — see above; it is a no-op here by
  construction, not an oversight.

### M47 — the leather dye (2026-07-28)

M46 shipped with leather rendering grey, and called it "the dyeable base drawn
untinted". Both halves of that were wrong: the greyscale is not what an undyed
piece looks like, and the tint is not an afterthought applied to one layer — it
is the mechanism that decides **whether each layer draws at all**.

The whole rule is four lines of `EquipmentLayerRenderer`:

```java
int dyeColor = DyedItemColor.getOrDefault(itemStack, 0);
for (Layer layer : layers) {
   int color = getColorForLayer(layer, dyeColor);
   if (color != 0) { ...submitModel(..., color, ...); }
}

private static int getColorForLayer(Layer layer, int dyeColor) {
   Optional<Dyeable> dyeable = layer.dyeable();
   if (dyeable.isPresent()) {
      int colorWhenUndyed = dyeable.get().colorWhenUndyed().map(ARGB::opaque).orElse(0);
      return dyeColor != 0 ? dyeColor : colorWhenUndyed;
   } else {
      return -1;
   }
}
```

**Zero is not a black tint, it is "do not draw this layer".** That is the entire
implementation of `Layer.onlyIfDyed`, which builds a `Dyeable` carrying *no*
`color_when_undyed`: undyed it returns 0 and the layer vanishes; dyed it returns
the dye. Three distinct states hide behind one `Optional<Dyeable>` — absent
draws untinted always, present-with-a-colour draws tinted always, and
present-without-one draws only when dyed — so the field survives as
`Option<Option<u32>>` rather than collapsing to a bool.

**An undyed leather piece is brown, not grey.** `DyedItemColor.LEATHER_COLOR` is
`-6265536` = `0xA06540`, and every leather layer in the jar declares it as
`color_when_undyed`. The sheet is authored greyscale precisely *because* it is
always tinted — there is no code path that draws it untinted. M46's grey boots
were the tint being skipped, not the base being correct.

**A layer type maps to a list.** Surveyed on the real jar: 20 humanoid lists of
one layer, 3 of two, and all three of the twos are leather's — a dyeable base
plus an untinted overlay, which is what keeps the studs and stitching their own
colour on a dyed piece. Only leather carries `dyeable` at all. The list is read
generally anyway: the per-layer rule above is what decides whether a layer
draws, and hard-coding "base plus optional overlay" would bury it in a shape.
The renderer caps at `MAX_ARMOR_SUBLAYERS = 2` and **logs** a piece that names
more, so the cap stays a statement about the shipped data rather than a silent
truncation.

**`ByteBufCodecs.INT`** — `DyedItemColor`'s stream codec is a fixed big-endian
i32 among the var-ints, the same trap `container_set_slot`'s signed short is
(M34). Read as a var-int, `0xB02E26` consumes three bytes and leaves the fourth
to be parsed as the next component's type id. The component holds an **RGB**,
which is why `getOrDefault` is the thing that calls `ARGB.opaque` — and why an
absent dye is `0` while a *black* dye is `0xFF000000`. Those are different
values and they render differently.

The tint is a **vertex colour**, which is where vanilla puts it:
`submitModel(..., color, ...)`, and `entity.fsh` multiplies
`texture * vertexColor`. It rides in the same channel and the same space as the
directional shade, so an untinted layer is exactly `tint = 1` and no branch is
needed.

#### The pixel witness caught a key-format break

`d4` renders the same sheet twice, once with `color_when_undyed` and once with a
red dye, and measures the red/green and red/blue ratios **over the armour's own
pixels** — so it cannot be satisfied by drawing anything at all. Its first run
measured **zero armour pixels**, which was correct: M47 changed the atlas key
from `<asset>/<layer>` to `<layer>/<texture>` (two assets can name one sheet,
and one asset's two layers name two), and the renderer's slot filter still
looked for `"/humanoid"` as a substring. Nothing matched, and **all** armour had
gone invisible — not just leather. A witness that only checked the resolution
arithmetic would have passed.

#### Verified

`itemshot --check` **42 -> 46**, **631 tests** (two new: the fixed-i32 decode,
and that absence is not a black dye), all seventeen gates green, demo PNG
byte-identical to M15 onward. Live, in one frame: an undyed leather chestplate
renders brown and a `dyed_color` red one renders red — and the leather boots on
M46's mixed-material zombie, grey in that screenshot, are now brown.

#### Open

- ~~**No trims.**~~ — **RESOLVED in M48.** The pattern sprites are permuted
  from a greyscale source through the material's palette and drawn as a fourth
  vertex range with depth EQUAL.
- The **glint order** is transcribed but unreachable: `renderLayers` draws the
  foil after the *first* layer that draws and then sets `renderFoil = false`,
  which matters only once armour glints (M45's fourth surface).
- `usePlayerTexture` is read as data and never honoured — it is for the elytra's
  cape texture, which Rewo does not render.

### M46 — worn armour (2026-07-28)

Every mob that could hold a sword has been able to since M22, and every one of
them has been standing there naked. This dresses them.

26.x splits a worn piece the same way it splits a held one. The **item** names
an asset — `Equippable.assetId()`, `minecraft:diamond` — which is in the item
prototype and never on the wire, so `tools/gen_item_props.py` extracts it
alongside the max stack size and durability it already pulled. The **asset**
then names its layers:

```text
assets/minecraft/equipment/diamond.json
  { "layers": { "humanoid":          [ { "texture": "minecraft:diamond" } ],
                "humanoid_leggings": [ { "texture": "minecraft:diamond" } ],
                "horse_body":        [ ... ] } }
```

and each layer's texture is `entity/equipment/<layer>/<texture>.png` — a
**64x32** sheet in the classic armour layout, not a 64x64 skin. Fifteen of them
load from the jar. Only the two humanoid layers are read: `horse_body`,
`llama_body` and the saddles describe geometry Rewo does not render, and a table
nothing can draw is worse than no table.

**Two humanoid layers rather than four, and the split is not per slot.**
`HumanoidArmorLayer.usesInnerModel` is `slot == LEGS`, so the leggings get
`humanoid_leggings` and the helmet, chestplate and boots share `humanoid`. The
leggings sit *inside* the chestplate — `INNER_ARMOR_DEFORMATION` 0.5 against
`OUTER` 1.0 — and that thinner inflation, on its own sheet, is what stops the
two z-fighting where they overlap.

**The body is in two pieces at once.** `ADULT_ARMOR_PARTS_PER_SLOT` gives CHEST
`{body, left_arm, right_arm}` and LEGS `{left_leg, right_leg, body}`, so a
chestplate covers both arms and the leggings cover the torso. The legs are not
the humanoid's own leg boxes either: `createBaseArmorMesh` **replaces** them
with `texOffs(0, 16)`, box `(-2, 0, -2)` 4x12x4 at `g.extend(-0.1)`, a tenth
thinner again so a boot and a legging do not fight.

The armour is posed from the **same `xf` the body just used**, not derived a
second time. `HumanoidArmorLayer` is a render layer over a model whose angles
are already set; recomputing them would drift the moment an arm swung.

#### The armour layer follows the renderer, not the mesh

The first build hung armour off any model that looked humanoid. That is
*nearly* the right set and wrong at both edges. Every mention of
`HumanoidArmorLayer` in 26.2 is one of eight renderers:

```text
AvatarRenderer                       player
AbstractZombieRenderer               zombie, husk, drowned
AbstractSkeletonRenderer             skeleton, stray, bogged, wither_skeleton, parched
ZombieVillagerRenderer               zombie_villager
ZombifiedPiglinRenderer              zombified_piglin
PiglinRenderer                       piglin, piglin_brute
ArmorStandRenderer, GiantMobRenderer (Rewo models neither)
```

An **allay** has arms and no legs; an **illager** and a **creaking** have the
full humanoid limb set. None of the three has an armour layer, so equipping one
renders nothing in vanilla — and a mesh test dresses all three. The set is
transcribed as `mobs::wears_humanoid_armor` and checked before anything is
emitted.

#### Only the player has a `body` part

`humanoid_head_body` puts every **mob's** torso cube straight on the static
root. Only the player model has a real named `body`, and only because M19 gave
it one so `setupAttackAnimation` could rotate the torso. So a chestplate's body
box resolved to nothing on every mob, and they wore armoured arms over a bare
chest.

`armor_part` now falls back to the root for the torso — safe unconditionally,
because the kind gate above has already run. It is the same space: the cube the
fallback stands in for is on that root.

**The witness passed while the render was broken.** It asked the *player* model,
the one humanoid that has the named part. A helper tested in isolation proves
nothing about the models the client actually draws, so it now walks every
registered mob and the two directions are separate witnesses: `r4` that all
fourteen wearers resolve all six parts, `r5` that the seventy-five others
resolve none.

The plain piglin swings the *generic* `ArmRight`/`ArmLeft` rather than a
humanoid arm animation, so those are in the fallback list too. Widening it is
safe precisely because the kind gate runs first — the enderman and the villager
also carry `ArmRight` and are never asked.

#### A trace beat four screenshots

Whether the arms were armoured survived several rounds of squinting at crops,
one of which was a husk rather than the zombie under test and another of which
compared two live runs whose scenes had drifted apart. Logging which part each
armour box resolved to answered it in one run: every arm and leg resolved,
`body` did not. The bare green mass in the middle of every crop was the
**torso** — the opposite of what the pictures had been read as saying.

#### A wrong texture that was not this milestone's

An armoured zombie rendered with a villager's brown head and magenta legs,
which looked exactly like an atlas collision caused by the fifteen new sheets.
It was not. Logging the packed slot table showed placements **byte-identical**
with and without them (`zombie -> (768, 512, 64, 64)` either way); a single
unarmoured mob rendered correctly *with* the sheets present; and the same scene
on a stashed pre-M46 build reproduced the fault exactly. It is a pre-existing
bug that needs more than one entity in the scene to show, unrelated to armour,
and it is recorded in §0.0 rather than fixed here.

`mobshot` cannot see it: its check substitutes per-face debug colours, so it
proves UV/face correspondence and is blind to sampling the wrong sheet.

#### Verified

`itemshot --check` **37 -> 42**, **629 tests**, all seventeen gates green, demo
PNG byte-identical to M15 onward. Live: a zombie in a diamond helmet, a diamond
chestplate, **golden** leggings and leather boots renders each slot from its own
sheet.

#### Open

- ~~**Leather is not dyed.**~~ — **RESOLVED in M47.** Both layers are read and
  each goes through `getColorForLayer`, so an undyed piece is brown
  (`LEATHER_COLOR`) and a dyed one takes its dye while the overlay stays
  untinted.
- **No trims.** `ArmorTrim` is a third layer with its own palette.
- The **inventory preview** does not wear the armour it is carrying.
- **Baby** mobs use the adult armour parts; vanilla has a separate
  `BABY_ARMOR_PARTS_PER_SLOT` with its own deformations.

### M45 — the glint on world-space items (2026-07-28)

The third and last reachable surface: a stack lying on the ground, and one a
mob is holding. `ENTITY_GLINT_TEXTURING`'s scale is **0.5** against the item
contexts' 8.0 — a factor of sixteen, which is why a dropped sword wears a few
broad bands where an icon wears a fine weave.

**Worn armour is the fourth surface and it is not reachable.** Rewo renders no
armour on any entity — there is no armour geometry in the entity pass at all —
so `ARMOR_ENTITY_GLINT_TEXTURING` at 0.16 has nothing to apply to. That is
worth saying rather than shipping a constant nothing calls: the glint is
complete for everything Rewo actually draws.

#### Emitted at the same moment, not derived twice

The glint quads are pushed **from inside the two item emitters**, beside the
vertex they shadow, rather than rebuilt afterwards. M44 records why for the
hand — the pipeline depth-tests `EQUAL`, so a vertex a fraction off is rejected
fragment by fragment — and it matters more here: a dropped stack carries a
death topple, a bob, a spin and a per-copy jitter, so a parallel derivation
would have four more chances to disagree. A `GlintSink` threaded through
`emit_held_item`, `emit_ground_item` and `emit_model` takes the position the
item just used and substitutes only the UV.

The glint is a **third vertex range** in the entity pass's single buffer —
solid, then text, then glint — drawn after the solid pass whose depth it tests
against, and before the translucent ones because it is additive over opaque
pixels. And it has **no lightmap term**: vanilla's glint shader multiplies by
`GlintAlpha` and the fog fade and nothing else, so a dropped enchanted sword
shimmers as brightly in a cave as in daylight.

`hasFoil` rides in with the stack — on `HeldItem` for equipment and in the
`DATA_ITEM` metadata tuple for a dropped one — because it exists only in the
component patch. There is nothing about an item *id* that says whether a
particular stack is enchanted.

#### The gate measured zero, and was right to

`itemshot` calls `init_entities` directly rather than through the app's
`init_entities_maybe_cem`, so it never installed the glint and both pixel
witnesses read 0 while the live render was visibly correct. Same shape as the
`swingshot`/`install_shapes` gap M41 hit: **a gate that reimplements a slice of
the app's setup will miss whatever the app adds to it.** The fix is one call
and a comment saying why it is there.

`entities.rs` is also one of the **mixed CRLF/LF** files §0.0 warns about —
1,969 CRLF lines against 3,763 LF. The scripted edits had to match either
ending and emit the one the matched region used; normalising would have buried
a 40-line change in a 3,000-line diff.

#### Verified

`itemshot --check` **33 -> 37**, **629 tests**, all seventeen gates green, demo
PNG byte-identical to M15 onward. Live: a dropped enchanted sword glints beside
an identical plain one that does not, and a zombie holding an enchanted sword
glints beside a zombie holding a plain one — both pairs in a single frame, so
the comparison is controlled.

### M44 — the glint on the first-person hand (2026-07-28)

M43 put the shimmer on the GUI icons; this puts it on the item you are holding.
The transform, the blend, the depth rule and the sampler are all M43's — the
item scale is 8.0 in both contexts — so the milestone is almost entirely about
where the second pass hangs.

**The glint geometry has to be the item geometry, to the bit.** The pass
depth-tests `EQUAL` against what the hand pass just wrote, so a vertex a
fraction of a unit away is rejected fragment by fragment and draws nothing. The
glint builder therefore repeats the pose derivation — the use branch, the swing
branch, the display transform, the left-hand mirror — rather than re-deriving
it a second, subtly different way. That is the property `handshot`'s `n2`
pins, and it is the one a refactor is most likely to break.

**Only items glint.** The bare arm is skin, and `submitArmWithItem` takes the
arm branch before any foil is considered — so a hand with the flag forced on
and no item still produces nothing.

`hasFoil` comes from the **inventory**, not the equipment feed: a server never
sends a player their own equipment, which is the same reason M38's swing
duration reads the inventory.

#### The bug, and why it was invisible

The first build drew no shimmer at all, and the two frames — enchanted and
plain — came out identical. `init_hand` **destroys and rebuilds the pass**
whenever the held item's atlas changes, and the glint was being installed
*before* it, so every rebuild threw it away. The GUI path had the two calls the
other way round and worked from the first frame; this one had grown in the
opposite order and never once drew.

It is worth naming because nothing failed: no error, no warning, no validation
message. A rebuilt pass with no glint is a perfectly valid pass. The only
signal was two frames that should have differed and did not.

#### Verified

`handshot --check` **29 -> 34**, **635 tests**, all seventeen gates green, demo
PNG byte-identical to M15 onward. Live, the same sword held twice: enchanted it
carries the diagonal sheen, plain it is clean.

**Open.** Ground and mob-held items want `ENTITY_GLINT_TEXTURING` at scale 0.5
and worn armour `ARMOR_ENTITY_GLINT_TEXTURING` at 0.16, both through the entity
pass — the last two of the four surfaces.

### M43 — the enchantment glint (2026-07-28)

M42 left the client knowing an item is enchanted and drawing it plain. The
shimmer is a **second pass over the same geometry**, and almost all of it is
three pieces of state rather than any new maths.

#### The transform

`TextureTransform.setupGlintTexturing`, entire:

```java
long millis = (long)(Util.getMillis() * glintSpeed * 8.0);
float o0 = (millis % 110000L) / 110000.0F;
float o1 = (millis %  30000L) /  30000.0F;
Matrix4f m = new Matrix4f().translation(-o0, o1, 0.0F);
m.rotateZ((float)(Math.PI / 18)).scale(scale);
```

Two periods, 110 s and 30 s, so the pattern never visibly repeats; the u
offset runs **negative** and the v offset positive, which is what sends the
sheen diagonally rather than straight across. The cast to `long` happens
**before** the modulo — doing the remainder in floating point drifts once a
session has been up a few hours.

**JOML post-multiplies**, so `translation().rotateZ().scale()` builds
`T · Rz · S` and the shader applies it to a column vector: read as operations
on the coordinate it is **scale, then rotate, then translate** — the reverse of
the order the calls appear in. The three scales are the only difference between
contexts: **8.0** for an item (a GUI icon or a hand), 0.5 for an entity, 0.16
for armour.

**The UV fed in is the quad's own `0..1` texture coordinate**, not its place in
Rewo's atlas. Vanilla gives the glint pass the same `UV0` the item pass uses,
which is a coordinate in the item's own sprite; feeding an atlas coordinate
would make the pattern depend on where the packer happened to put the item.

#### Three pieces of pipeline state, each load-bearing

- **`BlendFunction.GLINT` is `(SRC_COLOR, ONE, ZERO, ONE)`.** The colour source
  factor is the source colour itself, so a dark glint texel contributes nothing
  and a bright one blooms, and it only ever adds. Alpha takes the
  destination's, which leaves the frame's alpha alone — that matters because
  every headless gate reads it back.
- **`DepthStencilState(EQUAL, false)`** — test equal, do not write. That is what
  lands the sheen exactly on the item's own fragments and nowhere else; a
  `LESS`/`GREATER` test would paint it over faces the item itself had hidden.
  It works because the glint geometry *is* the item geometry, so the depths
  match exactly.
- **REPEAT and LINEAR sampling.** The matrix scales the UV by 8, so the sheet
  is sampled far outside `0..1` and clamping would smear one edge texel across
  the whole item. `blur: true` in the texture's own `.mcmeta` is the one place
  Minecraft asks for a filtered GUI texture.

The phase is **wall-clock**, not the game tick: vanilla reads `Util.getMillis()`
directly, so the glint keeps scrolling on a paused screen and never stutters
with the tick rate.

#### The bug the render caught

`hasFoil()` is **not** `isEnchanted()`:

```java
Boolean override = get(ENCHANTMENT_GLINT_OVERRIDE);
return override != null ? override : getItem().isFoil(this);
```

The override wins in **both** directions — a golden apple can be told to glint
and a Sharpness V sword can be told not to. The first build read the flag
straight off the enchantment list, which gets the common case right and both of
those wrong; a frame with all four cases side by side is what showed it.

#### And the fixtures rotted again

Three `item_stack` tests used a hard-coded component id as their "unknown
codec", and M43 gave that id one — so they silently began asserting the
opposite of their names. Same shape as the `swingshot` fixture M41 fixed, and
fixed the same way: an **impossible** id, because the property is "an id with
no shape stops the walk" and not "this component happens to be uncovered
today". That is twice now; a fixture that names a real-but-uncovered thing is a
fixture with an expiry date.

#### Verified

`inventoryshot --check` **85 -> 91**, **630 tests**, all seventeen gates green,
demo PNG byte-identical to M15 onward. Live, four stacks side by side: an
enchanted sword glints, a plain one does not, a golden apple with the override
on does, and an enchanted sword with the override off does not. The sheen
**moves** — 311 of a slot's 2,500 pixels differ between two frames seven
seconds apart, over an item that is otherwise static.

**Open.** The glint is on the GUI icons only. The **first-person hand** is the
same item scale (8.0) through a sibling pass and is the natural next step;
**ground and mob-held** items want `ENTITY_GLINT_TEXTURING` at 0.5, and worn
armour `ARMOR_ENTITY_GLINT_TEXTURING` at 0.16, through the entity pass.

### M42 — the enchantment registry, and the tooltip lines it unlocks (2026-07-28)

M41 decoded the component patch, so an enchanted stack yielded `(registry id,
level)` pairs — and nothing to translate them with. This is the other half, and
it is split across two sources for a reason worth writing down.

**The registry has to come from the wire.** `minecraft:enchantment` is a
**datapack** registry, so both its contents and its id order are whatever the
server's packs say; deriving an id from bootstrap order is the mistake §0.0
gotcha 5 warns about, one registry further down. It arrives in Configuration's
`registry_data`, and `rewo-net/src/enchantment_parse.rs` keeps it in wire order
because **the index is the protocol id** — the same rule M16 records for
dimension types.

Two fields matter, and one is not where it looks. `description` is a chat
component; `max_level` is **top-level in the entry compound, not nested under
`definition`**, because `EnchantmentDefinition.CODEC` is a `MapCodec` and its
fields are inlined into the parent map.

**The strings and the tags come from the client jar**, because they are what the
*client* ships: `assets/minecraft/lang/en_us.json` for the names and the level
numerals, and `data/minecraft/tags/enchantment/{curse,tooltip_order}.json`. The
tags living under `data/` is not a mistake — the client jar carries the vanilla
datapack, which is where M19 already reads `ItemTags.SPEARS`.

#### The three rules in `getFullname`

- **The level numeral is suppressed only when `level == 1 && maxLevel == 1`.**
  A level-1 Mending (max 1) reads "Mending"; a level-1 Sharpness (max 5) reads
  "Sharpness I". Suppressing on `level == 1` alone loses the numeral from every
  single-level enchantment a player actually applies — which is why `max_level`
  is parsed at all.
- **A curse is red**, everything else grey.
- **The order is the `minecraft:tooltip_order` tag**, then whatever the stack
  carries that the tag does not mention, appended after in the stack's own
  order. Not the ids, and not the order the patch listed them in.

An id the registry never synced yields **no line**: the server sent an
enchantment this session does not know, and inventing a name would be worse
than the omission. A level past ten has no `enchantment.level.N` key, and
vanilla renders the raw key there; Rewo prints the number instead, which is
this milestone's one deliberate divergence.

#### The bug the render caught

The first build showed "Diamond Sword" and nothing else. `SlotText::is_empty`
gates whether a stack's text is recorded at all, and it had not been taught
about the new field — so a stack carrying **only** enchantments looked empty
and was dropped. A field missing from that method is a whole class of stack
whose tooltip silently loses its lines, which is now said in the doc comment
above it.

#### Verified

`inventoryshot --check` **79 -> 85**, **629 tests**, all seventeen gates green,
demo PNG byte-identical to M15 onward, `play` 30 s with CORRECTIONS 0 and both
build actions server-accepted. Live, a sword with four enchantments renders:

```
Diamond Sword          white
Curse of Vanishing     RED, and first — the curses lead the tooltip_order tag
Sharpness V
Unbreaking III
Mending                no numeral: level 1 and max level 1
```

**Open.** The **glint** — an enchanted item's shimmer is a second render pass
with its own texture and matrix, not a tooltip concern, and Rewo has the
`isEnchanted` bit for it already. Armour trim *models*, which need the trim's
material and pattern resolved to asset ids rather than merely walked past. And
the seven syncable components still without codecs.

### M41 — decoding the `DataComponentPatch` (2026-07-28)

The blocker every milestone since M35 has named. Durability bars, enchantment
and lore tooltip lines, armour trim models and an exact
`isSameItemSameComponents` were all waiting on the same thing: Rewo could see
*whether* a stack carried a component patch and never what was in it.

#### Why it was hard, and why it is a table

The patch encodes each entry's value with that component's **own stream codec
and no length prefix**. So a codec you have not transcribed cannot be skipped —
the reader parks mid-value and every stack after it in the packet is parsed out
of garbage. That is why M19 transcribed three codecs and treated the other 108
as fatal, and why one enchanted sword in an equipment update cost the whole
rest of the packet.

26.2 registers **111** components, 104 network-synchronised. Nearly all of them
are built from a dozen primitives by the same handful of combinators, so
`rewo-net/src/component_wire.rs` writes the codecs as **data** — a `Shape` tree
per component — and one interpreter walks them. A new component is a table row,
and the coverage is a number a gate can read rather than a claim.

**97 of 111 transcribed. Of the 14 left, 7 are never network-synchronised**
(`custom_data`, `lock`, `recipes`, `map_decorations`, `container_loot`,
`debug_stick_state`, `intangible_projectile`) and so cannot appear on the wire
at all. The seven real gaps are `bees`, `blocks_attacks`, `can_break`,
`can_place_on`, `equippable`, `jukebox_playable` and `kinetic_weapon`. Reaching
one still fails closed — and now says so, naming the component id, which turns
"an item is missing from my inventory" into a table row.

#### Wire facts that read backwards

- **A chat component is one NBT tag.** `ComponentSerialization.STREAM_CODEC` is
  `fromCodecWithRegistries`, which writes a tag and parses it with a Codec — so
  `custom_name`, `item_name` and `lore` are walkable through the NBT reader
  Rewo already had, without transcribing the chat codec at all. That one fact
  covers three components and every future `fromCodec*` one.
- **`Unit` is zero bytes.** `unbreakable` is a marker: its presence *is* the
  value. Reading even one byte for it shifts everything after.
- **`holderSet`'s var-int is not a count.** It is `count + 1`, and a literal
  `0` means a **tag name follows as a string** rather than any entries. So `0`
  is one string, `1` is the empty set, and `n` is `n - 1` ids.
- **`holder` is `id + 1` with `0` meaning an inline value; `holderRegistry` is
  the raw id.** M14 recorded the same distinction for a different codec; it
  bites again here, and reading one as the other shifts every id by one before
  desynchronising on the first direct holder.
- **`either` writes `true` for the *left*** alternative, which is the opposite
  of the intuition that a flag marks the special case.

#### The fingerprint, and what it fixes

Every entry's raw value bytes are digested with its type id, **sorted by type
id**, into one 64-bit number. Sorted because the patch is written from a map
and its iteration order is not part of its meaning; a removal folds in its own
id, because `getOrDefault` answers a removal with the *type's* default rather
than the item's prototype, so "damage removed" and "damage absent" are
different stacks.

That makes `isSameItemSameComponents` **exact**. M35 could only ask "does
either side carry components at all", so every patched stack swapped rather
than merging and two identically-enchanted books could not stack. It also fixes
`isStackable`, which is `maxStackSize > 1 && !isDamaged()` — M35 read the
second half as "carries any component", which made a custom-named stack of dirt
unstackable when vanilla stacks it happily.

The remaining error is a digest collision, which would merge two stacks vanilla
keeps apart — the direction M35's approximation was built to avoid. At 64 bits
over the few dozen stacks in an inventory that is far less likely than the
approximation it replaces was to be wrong.

#### Durability bars

`Item.getBarWidth` is `clamp(round(13 - damage * 13 / maxDamage), 0, 13)` — it
**counts down from 13**, and computing it as `13 * remaining / max` rounds the
other way through the middle of the range. The colour is
`hsvToRgb(health / 3, 1, 1)`: a third of the hue circle, red through yellow to
green. The draw is a 13x2 black bed with `getBarWidth()` x **1** of colour on
top, so the bottom row of black reads as the bar's shadow. And
`isBarVisible()` is `isDamaged()`, so a pristine tool has **no bar** rather
than a full one.

Only the numerator is on the wire. `minecraft:max_damage` lives in the item's
prototype — every diamond pickaxe has the same 1561 — so
`tools/gen_item_props.py` grew a third column and the generated table now
carries it for the 84 damageable items. A patch that overrides it still wins.

#### Tooltips

The lines are now `getStyledHoverName` (custom_name over item_name over the
translated id, coloured by the rarity component), the lore, and `Unbreakable` —
vanilla's order, and the gap after the **first** line only.

The enchantment lines are still missing, and deliberately: they need each
enchantment's display name, and `minecraft:enchantment` is a **datapack**
registry sent at runtime in `registry_data`, which Rewo does not decode. The
patch gives ids and levels and nothing to translate them with. Printing
"Enchanted" instead would be inventing a line vanilla never shows.

The tooltip text is kept beside `Inventory` keyed by the **component
fingerprint**, not by slot — `ItemSlot` is `Copy` and the click arithmetic
moves it through a dozen expressions. Keying by the fingerprint means a
locally-predicted click that moves a stack carries its text along for free.

#### Two witnesses caught real bugs

The **tooltip box was drawn in panel space while its text was drawn in screen
space**, putting the box a panel's width from its own words. `t4` had passed
throughout, because I wrote its expectation to match the implementation instead
of to bracket the text. It now asserts that the box contains the text's origin,
which is the property a wrong coordinate space fails.

And `swingshot`'s "an unwalkable patch suppresses the pose" fixture named
`minecraft:enchantments` as its untranscribed codec — which M41 transcribes, so
the witness quietly stopped testing its own claim and started asserting the
opposite. It now uses an **impossible** component id, which cannot rot the same
way: the property is "an id with no shape suppresses", not "this component
happens to be uncovered today".

Both are the same shape as the detector errors M38 hit: **a witness written
against the implementation rather than against the property.**

#### Verified

`inventoryshot --check` **70 -> 79**, `swingshot` 97/97, **628 tests**, all
seventeen gates green, demo PNG byte-identical to M15 onward. Live against a
real 26.2 server:

```
enchanted + named + damaged sword   walks, damage 100, enchanted
written book, firework, tool, food  walk (each needed a codec M41 added)
player head, compass, shulker box   walk, including nested container stacks
two identically-named dirt stacks   MERGE   (M35 swapped them)
a differently-named one             SWAPS
                                    both accepted, 0 container resyncs
```

**Open.** The seven syncable components without codecs; the enchantment
registry, which unblocks the enchantment tooltip lines and is its own decode;
armour trim *models*, which need the trim material and pattern resolved to
asset ids rather than merely walked.

### M40 — the rest of the inventory screen: icons for armour, tooltips, and every remaining interaction (2026-07-28)

Three things §0.0 listed as open, and the first of them turned out not to be
about armour at all.

#### The suppressed items were suppressed for the wrong reason

M22 resolved the 1,390 items whose definition is a plain `minecraft:model` and
suppressed the other 147, on the grounds that they "branch on stack state this
client does not track". That is true of the **tree** and false of the
**outcome**. Surveying the real jar: **all 71 `select` definitions carry a
`fallback`**, and every `condition` carries an `on_false`. For a stack with no
components those are not defaults to fall back on — they are the answer. An
untrimmed helmet has no `TRIM` component, `SelectItemModel` finds no case to
match, and vanilla renders the fallback, which is the plain helmet sprite.

So the rule is not "suppress the type", it is **suppress the property you
cannot evaluate**. Rewo can answer `trim_material` (absent), `charge_type`
(none), `block_state` (absent), the boolean conditions (`broken`,
`has_component`, `fishing_rod/cast` — all false on a plain stack),
`using_item` (M38 gave it a use clock) and `display_context` (it knows which
pass is drawing). It cannot answer `local_time` or `context_dimension`, and
those two stay suppressed along with `composite`, which layers several models
into one draw, and `special`, which hands the stack to a bespoke renderer.
**1,390 → 1,438 resolved, 147 → 99 suppressed**, and the armour icons the
inventory screen has been drawing blank since M35 now have geometry.

The reduction is recursive, and it has to be: a bow is a `condition` whose
`on_true` is a `range_dispatch`, so reducing only the top level would leave it
where it was.

**`display_context` selects different *geometry*, not a different transform.**
A spear is a flat sprite in a slot and a 3D `_in_hand` model in the hand, so
one baked model cannot serve both and `HeldItemModel` grew a `gui_quads`. In
26.2 every item that consults the property splits the same way, `gui` against
the rest, but that is a fact about the data rather than a rule, so the match is
by name and the bake compares the two resolutions instead of assuming.

**A witness corrected the diagnostics.** I wrote one asserting that `condition`
and `range_dispatch` would vanish from the suppressed buckets, and it failed:
the bucket recorded the definition's **root** type, so a `condition` whose
chosen branch is a `special` still bucketed as `condition` — which reads as
"the reduction cannot do conditions", the opposite of true. The reason string
now names the node the walk **stopped at** and the property it could not
answer, so a bucket says `minecraft:select (minecraft:context_dimension)`.

#### Tooltips

One line, the item's display name, white. That is the whole of what Rewo can
show and exactly what vanilla shows for a plain stack: `getTooltipFromItem`
starts with the hover name and appends what the **components** say —
enchantments, lore, durability, attribute modifiers — and Rewo sees only
whether a patch was present. Rarity rides on a component too, hence the white.

The names come from the jar's own `en_us.json`, read during the bake so a
generated table cannot drift from the assets. `Item.getDescriptionId` is
`item.minecraft.<id>`, but **`BlockItem` overrides it to the block's**
`block.minecraft.<id>` — and `item.minecraft.dirt` does not exist, so reading
only the item spelling loses every block in the game. Seven items carry both
keys and all seven spell the name identically, so the preference for the block
form is unobservable in 26.2; it is written down anyway.

Layout facts, each of which is a way to be two pixels wrong:

- The text block's height starts at **-2 for a single line** and 0 otherwise,
  and the draw loop adds 2 after the first. A one-line tooltip is 8 px of box
  for a 10 px line.
- The horizontal recovery is a **flip, not a clamp** — `max(x - 24 - w, 4)`
  puts the tooltip on the cursor's other side. **And the `x` it subtracts from
  is the already-offset one**, cursor + 12, not the raw cursor. My first
  witness expected 306 and the answer is 318; reading it as the cursor puts
  every edge tooltip twelve pixels too far left.
- The vertical recovery *is* a clamp, and it uses `h + 3` — the padding but not
  the margin — so a tooltip near the bottom is allowed to hang its border art
  off the screen. It also fires later than it looks: at `h = 8` the cursor has
  to be past 301 on a 300-tall screen.

The background is **two nine-slice sprites blitted at the same rect**,
`tooltip/background` (border 9, tiled middles) then `tooltip/frame` (border 10,
`stretch_inner`). Both middles are one flat colour, so a stretch reproduces a
tile exactly here; the flag is threaded through anyway.

#### The interactions

Four `ContainerInput`s, each with something that reads backwards.

**`SWAP`'s button is a third coordinate system.** `slotIndex` is a menu slot as
everywhere else, but `buttonNum` indexes `player.getInventory()` — `0..9` for
the hotbar and a literal **40** for the off-hand. Pressing `1` over the helmet
slot is `slot = 5, button = 0`, and the two numbers are counted from different
origins. The guard `0 <= b < 9 || b == 40` **rejects rather than clamps**: 9
through 39 do nothing at all.

**`THROW`'s trailing `while` loop never runs twice.** With `button == 1` the
amount is the slot's whole count, so the first `safeTake` empties it and the
loop's `isSameItem` compares against an empty stack. It reads like a repeat and
is a no-op. It is also gated on the cursor being empty — Q while dragging drops
neither.

**`PICKUP_ALL` runs two passes, and the first skips full stacks.** So a double
click gathers the partial stacks first and only breaks into a full one if it
still has room, which is why it leaves tidy stacks alone when it can. Its guard
is that the clicked slot is **empty or unpickable** — that is the slot the
first click of the double click just emptied. Without it, any second click on a
full slot would hoover up the inventory.

**`QUICK_CRAFT` packs two fields into one byte**: `type << 2 | header`, read
back by `getQuickcraftType` (`mask >> 2 & 3`) and `getQuickcraftHeader`
(`mask & 3`). Send a bare header and the server reads type 0, so a one-per-slot
drag spreads evenly instead. It is three packets — a begin, one add per slot,
and an end that carries the whole changed-slot map — and **a one-slot drag is
not a drag**: vanilla resets the state and re-dispatches it as `PICKUP` with
`buttonNum = quickcraftType`, which maps type 0 to a primary click and type 1
to a secondary one. The even spread divides the stack by the **slot count** and
floors, so three items over two slots is one each with one left on the cursor.

#### Verified

Gate: `inventoryshot --check` **44 → 70**, `itemshot --check` **28 → 33**.
Live against a real 26.2 server, every one accepted with **0 container
resyncs** — the server replays the click and compares, so that is the check
rather than a claim about bytes:

```
SWAP        slot 5 button 1   → 2 changed slots
THROW       slot 36 button 0  → 1 changed slot
PICKUP_ALL  slot 20           → took the 7 first, then 47 of the 64 → cursor 64
QUICK_CRAFT 40 dirt over 3    → 3 + 3 packets, 13 each, 1 left on the cursor
```

`REWO_CLICK` became semicolon-separated to make the last two testable at all —
a drag needs a stack on the cursor, and no single click can both leave one
there and use it. `d:<slot>,<slot>,…[,one]` is a drag.

**Open, with reasons.** *Durability bars* are blocked, not skipped: the bar is
`stack.getDamageValue()` against `getMaxDamage()`, both components, and Rewo
knows only whether a patch was present. Drawing a full bar on a worn pickaxe
would be worse than drawing none. *Enchantment, lore and attribute tooltip
lines* are blocked the same way. The *recipe book* is a separate screen with
its own data. The 99 items still suppressed are the two `composite`/`special`
families plus `local_time`, `context_dimension` and `compass` — real work or
genuinely unanswerable, not oversights.

### M39 — shift-click, the quick-move (2026-07-28)

The most-used inventory action after picking a stack up, and the one §0.0 named
first. `ContainerInput.QUICK_MOVE` is a *different input*, not a modifier on
PICKUP — `doClick` branches on it before it reads the button.

**The routing is not "the other half of the inventory".** `quickMoveStack`
checks armour and the off-hand **first**, for an item that fits them and whose
target slot is *empty*, which is why shift-clicking a helmet equips it rather
than moving it — and why shift-clicking a second helmet does not swap the first
out. Only after that does it fall through to the hotbar-to-grid and
grid-to-hotbar swap. The crafting result is the one destination walked
**backwards**, so a craft fills the hotbar from the right.

**`moveItemStackTo` is two passes, and they are asymmetric.** The first merges
into every slot already holding the same item, across the whole range; the
second places the remainder into the first empty slot that will take it and
then **breaks**. So a stack tops up a partial one before it ever takes an empty
slot, but a stack too large for one empty slot leaves the rest behind rather
than spreading. The outer loop in `doClick` is what repeats it — `while
(!clicked.isEmpty() && isSameItem(...))` — so a full stack across several empty
slots takes several passes.

Rewo's one approximation carries over from M35: `isStackable()` is
`maxStackSize > 1 && !isDamaged()`, and Rewo sees only *whether* a stack has
components, so a patched stack is treated as unstackable. Same one-directional
caution, erring the same way — a damaged tool is never merged into another,
which is what vanilla does anyway.

Gate: `inventoryshot --check` **44 → 49**, five witnesses on the routing, the
merge-before-place order, the armour exception and the decline. Verified live
as well: a shift-click and a plain click on the same stack, each accepted by
the server with **0 container resyncs** — the server replays the click and
compares, so that is the real check rather than a claim about bytes.

**Open.** Tooltips are the most visible thing still missing, and they need a
new data source — `en_us.json` for the display names — plus text layout, so
they are their own piece of work. Also out: drag / quick-craft, number-key
swap, Q-to-drop, double-click pickup-all, the recipe book, durability bars, and
armour icons (their `select` trim definitions are among M22's 147 suppressed).

### M38 — the first-person hand (2026-07-28)

The blocker §0.0 named for this was M34's inventory model, and it was real: you
cannot draw what the client does not know it is holding. With that gone, the
hand is the last thing standing between Rewo and looking like a Minecraft
client from the inside.

**Two rules in the bake, both invertible.** An absent `firstperson_lefthand`
falls back to the **right-hand** entry, and *only* in first person —
`ItemTransforms`' builder has a literal
`if (left == NO_TRANSFORM) left = right` with no equivalent for the
third-person pair, which is why the third-person left still records absence.
`item/generated` declares only the right hand, so the fallback fires for every
sprite that is not `handheld`. And the left/right **mirror is applied at draw
time, not baked**: `ItemTransform.apply` negates `translation.x`, `rotation.y`
and `rotation.z` for whichever transform was selected, including one that
arrived through that fallback. `handheld` authors its left entry pre-mirrored
(`[0, 90, -25]` against the right's `[0, -90, 25]`) so the two cancel; baking
the mirror as well would double it.

**The swing clock is not a new machine.** `LocalPlayer` is an ordinary
`LivingEntity`, and `Player.aiStep` calls `updateSwingTime`, so giving it an id
in M19's swing table models the same object the remote-player path already
does. `tick_swings` iterates the swing map rather than the entity map, so it
ticks even though Rewo never tracks the local player as an entity. The one
missing input was the held item — the server never sends you your own
equipment — and M34's inventory supplies it. That is the actual join between
the two milestones.

**Two clocks, and conflating them is the easy mistake.** `attackAnim` belongs
to the entity and runs on the swing machine. The equip height belongs to
`ItemInHandRenderer` and does not exist on the entity at all: it falls to zero
when the held item changes, swaps the visible item at the bottom *where nothing
is on screen*, and climbs back at 0.4 a tick. Running it per frame rather than
per tick would make the dip three frames long and invisible at any sane frame
rate.

**The hand has its own projection, and vanilla clears depth before it.**
`GameRenderer` does `hudProjection.setupPerspective(0.05, 100, cameraState.hudFov, …)`
then `clearDepthTexture(…, 0.0)` — and `calculateHudFov` returns a **hard-coded
70**, vertical, independent of the player's FOV option. That clear is not an
optimisation: the hand lives a fraction of a block from the eye, so without it
a wall in front of you slices your arm off. Rewo already used 70 and 0.05, so
nothing needed adjusting — but establishing that is what ruled the projection
out later.

**Three things were measured rather than assumed**, and all three matched. The
arm chain's `3.6`/`5.6` translates are in **block** units with cube vertices
divided by 16: with that divide the arm part lands 1.1 blocks below the eye and
0.7 in front; without it, ten blocks away. The baked item quads really are
0..16. And the pass turned out to be the GUI-item pass with exactly two
differences — a view-projection push constant instead of a screen mapping, and
the world's flipped viewport instead of the HUD's top-left — so they share a
vertex layout and nothing else moved.

**The 1.36x that was not there.** The render was first committed with an
unresolved discrepancy: a held block measured 603 px wide against a predicted
442, while its vertical extent matched exactly. Bisecting it offline settled it
— a unit test drove the production geometry builder with a synthetic cube and
projected the result, matching a hand derivation to a tenth of a pixel, which
eliminated the geometry. The fault was the measurement: the brown-pixel
detector was also finding the hotbar's dirt icons and the dirt-coloured
terrain. Re-measured with a diamond block in a window excluding the sky and the
hotbar, every edge lands within a pixel — 837/838, 1279/1279, 524/524, 719/719.

That was the third detector error of the milestone, all the same shape: a test
matching more than its subject. Non-black pixels against a painted sky (M34),
brown against a brown hotbar, then cyan against a blue sky on the first
re-measurement. **The gate is built around avoiding the class**: it draws a
synthetic magenta cube, asserts an empty frame contains no magenta at all, and
only then trusts a count.

Gate: **`rewo handshot --check`** — serverless, validation-required,
fail-closed, **29 witnesses**. Four on the bake against the real jar, eighteen
on the pose chain, the bare arm, the spear thrust and the use rigs against a
derivation done inside the gate, seven on pixels. The
strongest is `g2`: every magenta pixel the pass draws falls inside the rectangle
the CPU builder predicted, computed independently in the same run — which is
the join between the milestone's two halves. Two of its own witnesses were
wrong first: the fallback check used a **stick**, which parents `item/handheld`
and authors both hands, so it failed against correct code (an apple is
`item/generated`; the witness now asserts both, separating the fallback from
the authored case), and the fail-closed count caught 19 witnesses declared
as 17.

Measured: **623 tests**; seventeen serverless gates green with validation ON;
demo SHA-256 `2cc56b4a…` byte-identical to M15 onward.


**The bare arm, and the bug that hid it.** An empty hand draws the player's own
arm — `renderHand` submits *one named part* with `resetPose()` and a fixed
`zRot` of ±0.1 rad, so it is the rest pose plus one nudge rather than whatever
the body animation left behind, and only ever for the main hand. The atlas is
seeded with the jar's `entity/player/wide/steve.png`, which is what vanilla
shows on an offline server too.

It rendered as nothing at first, and the reason is worth keeping: **the model's
UVs are texels, not fractions.** An arm's span 16..56 of the 64-px skin, and
remapping them into the atlas without dividing by the skin size sends them to
16..56 in a 0..1 space, where the sampler clamps to a transparent edge. The
geometry was there the whole time — 72 vertices, built and uploaded — which is
why looking at the frame proved nothing and printing the UV range settled it in
one run. `handshot`'s `a2` now pins it: every arm vertex must land inside the
skin's rect, which the un-normalised form cannot do.


**The use-driven poses, and the plumbing they were blocked on.** Right-click is
a hold: press with nothing to place against sends `use_item` — the no-target
form Rewo never had — and release sends `RELEASE_USE_ITEM`. The local use clock
needed **no new machine**, for the same reason the swing did not:
`LivingEntity.startUsingItem` sets shared-flag bit 0 (bit 1 for the off hand),
which is exactly what `set_living_flags` already decodes for remote entities, so
driving the local id through that door gets M23's clock and every rule it
encodes — a repeat does not restart it, an unresolvable stack latches nothing, a
hand swap mid-use is caught by the tick rather than the start.

Seven poses from `submitArmWithItem`'s switch, and three invertible details in
them. **`hasCustomArmTransform` does not add a transform, it moves one**: true
for exactly `EAT`, `DRINK` and `SPEAR`, and for those the resting arm offset is
skipped up front and applied *after* the pose instead. **The brush cycles on
`remaining % 10`**, not on progress through a duration — brushing runs as long
as you hold it and has no progress to key on. And **`case BLOCK` excepts a real
shield**, which carries its own `display` transform for the context and would
otherwise be posed twice. Spyglass is not a pose at all but the absence of one:
vanilla guards the whole of `submitArmWithItem` on `!isScoping()`.

Two of the five gate witnesses failed first, both for the same reason —
`transform_point3(ZERO)` sees only **translation**. The brush's sweep is a
trailing rotation, so the origin never moves and the witness measured it as
motionless; sampling an offset point shows 0.97 blocks of travel. The other had
the eat jiggle backwards: `scaledUsageTime` is `remaining / duration` and
remaining counts *down*, so the item swings aside at the **start** of the use
and stays there.

**Open.** `SPEAR`'s *use* rig reads a kinetic-hit-feedback counter the wire does
not carry, and the crossbow's charge pose needs the same kind of unsynced
input; both are left at the resting pose and recorded rather than approximated.
The two map holds are component-driven rather than use-driven and are out. The
arm also wears the default skin rather than the player's own — the hand atlas
takes one, but nothing yet feeds it the local player's.

### M37 — particles, and the verification approach they needed first (2026-07-27)

§16 listed particles under "deliberately not next", with a reason rather than a
shrug: *every gate here is geometry-based; it needs a verification approach
invented first — don't pick it casually.* That is the right worry. Every gate in
this project puts the renderer in a known state and asserts a **value** against a
number derived independently. Particles resist that shape — spawned in bulk by a
random process, then integrated over time, so no single frame's contents are a
stated fact. "Render some smoke and see if it looks like smoke" is exactly the
proxy the mob-texture lesson was written about.

So the first deliverable was an argument, not code.

**Vanilla's particle system is not actually stochastic. It is a deterministic
function of a seed.** `Particle.tick()` contains no randomness at all — pure f64
arithmetic over position, velocity, gravity, friction, age and lifetime. Every
generator is a `LegacyRandomSource`, which is bit-for-bit `java.util.Random`'s
48-bit LCG and ports to Rust exactly. Fix the seed and spawn offset, velocity,
lifetime, colour, quad size and sprite index all become assertable numbers.

The claim is not universal and is not rounded to make it so: `WaterDropParticle`
overrides `tick` entirely — counting the lifetime **down** rather than the age
up, applying gravity undivided where the base applies `0.04 * gravity`, using a
hard-coded 0.98 friction, and drawing `nextFloat()` when it lands. Splash is
built on it.

**Two anchors keep the argument from being circular, and they retire different
failure modes.** Minecraft's `BitRandomSource` reimplements `next(bits)`,
`nextInt(bound)` and `nextFloat()` with formulas identical to the JDK's, so
`java.util.Random`'s own output is genuinely independent ground truth for those
three — asserted bit-for-bit from six seeds. That proves the generator. It
cannot prove the *reading*: a second implementation written from the same
misreading of the decompile would agree with the first. So a Java harness whose
class bodies are **copied verbatim** out of the decompile emits per-tick
trajectory vectors, checked in at
`crates/rewo-world/src/particles_oracle_26_2.txt`. It runs in an empty world, so
`collideBoundingBox` is the identity and no collision code executes on either
side — that isolates the constructor and tick arithmetic, which is what these
vectors grade.

**It caught a bug on the first run.** Four of six kinds failed, all on `yd`, all
low-bits-only. `+ 0.1` where vanilla writes `+ 0.1F` — widened, that float is
`0.10000000149011612`. An error of ~1.5e-9: invisible in any screenshot, and it
shifted every subsequent tick. Note *which* tests caught it. The RNG KATs
passed. `splash` passed, because it overwrites `yd` a few lines later; `poof`
passed, because it uses the 4-argument base and never runs that line. Only the
four kinds routing through the 6-argument base failed. Every float literal now
goes through a `w(v: f32) -> f64` helper so the widening is visible in the
source.

**Where bit-exactness is not available, and why that is correct.**
`nextGaussian` evaluates `sqrt(-2 * log(r²) / r²)`. `sqrt` is IEEE-754
correctly-rounded — measured 0 ULP divergence over 2M samples. `log` is not:
vanilla calls `Math.log`, which the JLS specifies only to within 1 ULP and which
HotSpot implements as an intrinsic. Measured on Temurin 25, `Math.log` and
`StrictMath.log` differ on ~7% of inputs in (0,1) by ≤1 ULP, amplifying to ≤3
ULP in the gaussian — so vanilla's own `nextGaussian` disagrees with
`java.util.Random.nextGaussian` on ~3% of draws. **Vanilla's spawn scatter is
therefore not bit-reproducible even between two JVMs**, and a gate demanding
bit-equality there would assert something stronger than vanilla itself
guarantees. Rust's `f64::ln` against the JVM over a 30,000-draw sweep: 22 draws
(0.073%) differ, worst **2 ULP**. The bound is 8 — a 4× margin — and is scoped
to that one primitive; everything else is graded to the bit.

**A wrong theory, tested rather than shipped.** MC declares `DOUBLE_MULTIPLIER`
as the *float* literal `1.110223E-16F` where the JDK uses `0x1.0p-53`, which
looks like a real divergence and was written up as one before being checked. It
is not: 2⁻⁵³ is a power of two and therefore exactly representable as an f32, so
the literal rounds to precisely 2⁻⁵³. Pinned by a test rather than left as a
comment.

**One deliberate divergence.** Vanilla seeds each particle from
`RandomSupport.generateUniqueSeed()`. Those seeds are *arbitrary* — no value is
more correct than another — so Rewo derives them from a system-level master
generator instead. Not an approximation: it draws from the same distribution and
picks a *nameable* sample, which is what makes the gate exist.

**A second defect, found by reading the authoritative data.** After the gate was
green, checking the jar's own `assets/minecraft/particles/*.json` — rather than
inferring sprite sets from the texture directory — turned up `splash.json`
listing **four** textures, picked by `sprite.get(random)`. The trajectory
witnesses could not catch it: they grade the particle's own generator, which the
test hands in directly. What was wrong was the **engine** stream. Flame, Crit and
Splash call `get(random)`; Smoke and Poof pass `sprites.first()` and animate by
age; Terrain takes its sprite from the block model. The pick is a constructor
*argument*, so it lands before any of the particle's own draws — and `nextInt(1)`
looks like a no-op worth skipping but still advances the LCG. The texture
directory is a proxy for the sprite set; the particle JSON is the property.

**Rendering.** `SingleQuadParticle` billboards, through vanilla's
`core/particle.vsh` — *the same shader its weather uses*, which is why the pass
reads so close to `weather.rs`. Facing is `FacingCameraMode.LOOKAT_XYZ`; Rewo
takes the right/up basis out of the view matrix (for a rotation matrix the
**rows** are the world axes) rather than carrying a second quaternion that could
drift. Positions are emitted in **world space** — the M33 weather trap, recorded
in the module header.

A block-break shard samples the **block** texture
(`getParticleMaterial(state).sprite()`) while a flame samples the particle strip.
Vanilla splits that across six `SingleQuadParticle.Layer` variants over three
atlases; Rewo puts both in one `sampler2DArray` and selects per-vertex — one
pipeline, one draw. The sprites are 8×8 against the block array's 16×16 and are
point-upscaled 2×, with the helper refusing any non-integer ratio rather than
silently blurring. The block half needed `BakedAssets::particle_layer`, which
resolves each state's model `#particle` slot through the merged parent chain —
**not** a face texture, because that gets the interesting case exactly wrong: a
broken grass_block throws *dirt*-coloured shards despite its green top face.

**Two more bugs the live wiring exposed**, both caught by numbers rather than by
looking. The particle clock was anchored at 0 while `session.ticks` was already
in the hundreds, so the first frame's catch-up ran every tick since connect and
aged a 400-flame burst past its lifetime before it was drawn — the log read
`alive=0` with the event correctly decoded. Anchored on first use now, and capped
at 4 ticks: vanilla's `ParticleEngine.tick` runs once per client tick and a
stalled client simply misses ticks. And the windowed path never built its
`ParticleAssets`, so particles would have been invisible in an actual window
while every headless check passed.

**Gate: `rewo particleshot --check`** — serverless, CPU-only, fail-closed on both
a failing witness and a witness that silently stopped running. **34 witnesses.**
`w1`–`w10` the wire, from packet bodies hand-assembled out of the decompiled
write methods (not a Rewo encoder: if the same code wrote and read the packet, a
transposed field would round-trip happily). `f1`–`f13` the fan-out. `s1`–`s11`
the simulation against the verbatim-source oracle. Witnesses worth naming: `w5`,
that `count` is a plain big-endian i32 and not a VarInt; `w8`, that all 47
truncated prefixes decode to `None` without panicking; `f4`, the `count == 0`
inversion, where the three `*_dist` fields stop being a scatter radius and become
a **direction** with `max_speed` its magnitude; `f1`/`f2`, that a seeded system
is byte-identical across runs *and* that a different seed differs, because the
first would also pass on a constant; `s9`, that `FlameParticle.move` bypasses
collision **by design** so a "fix" would be the regression; `s10`, that
`CritParticle` ticks inside its own constructor.

**Mutation-tested**, because a witness that has never failed has not been shown
to work. Restoring the `+ 0.1F` bug fails 4 trajectory witnesses — and splash and
poof correctly still pass. Reading `count` as a VarInt fails 9, starting at `w5`.
Transposing `x_dist`/`y_dist` fails `w4` and `f4`. Coarsening the destroy-block
density 0.25 → 0.5 fails `f7`/`f8`/`f10` at 8 shards instead of 64. Declaring
`Splash` single-frame again fails `f12`.

**Live verification** against a real 26.2 server on a freshly wiped flat world,
re-run after each rebase, every render diffed against a particle-free control
frame of the same scene:

| trigger | quads | mean RGB of the changed pixels |
|---|---|---|
| `/particle flame` ×400 | 400 | (246, 163, 33) orange |
| `/particle block{redstone_block}` ×400 | 400 | (120, 25, 12) red |
| `/particle block{lapis_block}` ×400 | 400 | (31, 59, 110) **blue** |
| `/particle block{gold_block}` ×400 | 400 | (193, 170, 54) yellow |
| `/setblock … air destroy` | **64** / 384 verts | — see below |

The three `block{…}` rows are the load-bearing ones: the shard colour **tracks
the block state**, which is what proves the per-state `particle_layer` lookup and
the array sampling are real rather than a constant fallback. Reproduced across
three separate runs; the means drift by a couple of units run to run (spawn
position and world time differ per connect), so the assertion is the dominant
channel and the ordering, not the exact triple.

The break row's claim is the **count**: a genuine server-side break sends
`level_event` 2001 and spawns exactly the 4×4×4 grid `particleshot`'s `f7`
asserts from the other direction. Reproduced exactly, twice.

**What the break row deliberately does NOT claim, and why.** An earlier run
measured its shards at within 0.04 chromaticity of an explicit
`block{grass_block}` particle and that figure was written down as fact. It did
not survive a second run, which put the same comparison at 0.16 and rising with
the threshold. **Frame-diffing cannot cleanly measure this trigger**, because
`/setblock … air destroy` *mutates the world*: the removed block covers thousands
of pixels, the shards spawn inside the volume it vacated, and the two frames also
differ in lighting *history* (one relit incrementally from a client-side edit,
the other received the server's light at chunk load). A control rendered in the
same world state removes the largest term but not all of them. The tell, both
times, was that restricting to strongly-changed pixels made the discrepancy
*worse* rather than better — the signature of a contaminated control rather than
of edge blending.

So the destroy path's *texture* resolution is left to the evidence that does hold
it: it is the same `particle_layer` lookup the three `block{…}` rows exercise,
and `grass_block` reaches `block/dirt` through its model's literal `#particle`
while `dirt` reaches the same texture through `#all` — a source fact from the
jar, not a pixel measurement. The first figure is retracted rather than defended.

**Measured after rebasing onto the M0–M36 main.** 610 tests (586 + 24: 20 in
`rewo-world`, 4 in `rewo-gpu`). All sixteen serverless gates green with Vulkan
validation ON and 0 VUIDs — `mobshot` 243/243, `blockentityshot` 172,
`swingshot` 97, `inventoryshot` 44, `hurtshot` 38, `weathershot` 35,
`particleshot` 34, `eventshot` 28, `itemshot` 28, `danceshot` 24, `portalshot`
12, plus `skyshot`, `lightmapshot`, `tintshot`, `meshshot`, `dimensioncheck`.
Canonical demo PNG still
`2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` — checked
deliberately rather than assumed, since resolving `#particle` can allocate new
texture-array layers for sprites no face happened to use, which would reorder the
array. It did not.

**Open.** Six kinds (block, smoke, flame, splash, crit, poof), not 125 —
unsupported types are dropped rather than guessed at, which is safe because
packets are length-framed, and most of the other 119 carry option payloads whose
codecs are untranscribed. No translucent sorting; particles blend, depth-test and
do not depth-write, where vanilla sorts its translucent layers. Splash's
fluid-height removal is half-implemented: vanilla removes a water drop below the
max of the block's collision height *and its fluid height*, and Rewo has no
fluid-height query on the collision seam, so a splash over still water lives
marginally longer. Terrain shards use vanilla's flat 0.6 grey without the
per-block tint multiply. `roll` is unapplied — none of the six kinds sets it. And
the gate does not grade pixels: a particle quad is a camera-facing billboard the
existing passes already cover, and what was novel about particles was the
simulation.

### M35 — the inventory screen (2026-07-27)

M34 decoded all 46 slots and drew nine of them. M35 opens the screen: the
panel, every slot, the hover highlight, the stack on the cursor — and clicking,
which is the half that has to be *exact* rather than merely plausible.

**The click is a prediction, and the server grades it.**
`ServerboundContainerClickPacket` carries the client's own belief about every
slot the click changed, as a `HashedStack` each. `handleContainerClick` replays
the click server-side, records what the client claimed, and then — this is the
only thing that triggers a resynchronisation —

```java
boolean fullResyncNeeded = packet.stateId() != this.player.containerMenu.getStateId();
```

So the state id is the whole contract, and the hashes are how the server knows
what the client already believes for its *next* `broadcastChanges`. A stale id
re-sends the entire container. The first live click was rejected for exactly
that reason, and the cause was the harness rather than the code: it clicked the
instant the first stack arrived, while `/give` was still sending one container
update per item, each advancing the id behind the packet already in flight.

**Two per-item facts had to be extracted, because neither is on the wire.**
`tools/gen_item_props.py` reads the datagen per-item component report:
`minecraft:max_stack_size` (**295 of 1537** items differ from the default 64 —
246 at 1, 49 at 16) and `minecraft:equippable`'s slot (**83** items). Both feed
`doClick`'s PICKUP branch: the first caps every transfer, and the second is
`ArmorSlot.mayPlace`. A wrong value predicts a wrong slot and the container
resynchronises, which looks like clicks bouncing back.

**The arithmetic, transcribed from `doClick` + `Slot`:**

- a primary click takes the whole stack, a secondary takes `(count + 1) / 2` —
  rounded **up** onto the cursor;
- placing is `safeInsert`, `min(amount, count, cap - occupied)`, where a
  secondary click's amount is a literal **1**, not half;
- an unlike item swaps, but only if `carried.getCount() <= slot.getMaxStackSize()`
  — which is what stops a 64-stack being swapped into an armour slot;
- and when the slot refuses the placement but holds the same item, the else-arm
  *takes* from it, which is how a crafting result is collected onto a partial
  stack. `tryRemove`'s `maxAmount` is what makes that take all-or-nothing there.

**The one honest approximation.** `ItemStack.isSameItemSameComponents` gates
every merge arm, and Rewo decodes *whether* a stack carried components but never
what they were — comparing them needs the per-type codecs, and only three of 111
are transcribed. So a patched stack is treated as unique and swaps rather than
merging. The error is one-directional by construction: a missed merge is
corrected by the server, a wrong merge would have fused two different tools. In
practice it barely arises, because the components that travel on a stack —
damage, enchantments — belong to items that stack to one, where no merge arm is
reached at all.

**Layout details that are not guessable.** `isHovering` is
`x >= left - 1 && x < left + w + 1` with `w = 16` — an **18x18** box, so slots
tile without a dead column between them; testing the 16 px icon rect leaves a
one-pixel cross the cursor falls through. The hotbar row is `top + 58` below the
main grid, a named `topToHotbar` local, not three rows of 18. The highlight
sprites are drawn at `slot - 4` at 24x24, bracketing the item — back sprite,
icon, front sprite. And the panel is centred by **integer** division in GUI
space, which is what keeps its sprite art on whole texels.

**The backdrop is a gradient, not a fill** — `0xC0101010` to `0xD0101010`. A
flat wash at either value looks almost right, which is why the value is pinned.

**Measured, not squinted at.** The panel looked washed out and had a black
rectangle in it. Sampling the frame against `inventory.png` showed six of seven
probes byte-identical — the seventh was the F3 overlay drawing over it — and the
black rectangle is the texture's *own*, the window vanilla covers with the 3D
player. Both first impressions were wrong.

Gate: **`rewo inventoryshot --check`** grew from 16 to **39 witnesses** — the
menu layout against an independent transcription of the constructor, the 18 px
hover box and its tiling, nine arithmetic cases, the slot rules (a helmet only
in the helmet slot, dirt in none of them, anything in the off-hand, the result
slot's asymmetry), the two declines, the packet bytes, and five pixel witnesses
including the byte-exact panel and the highlight's placement. Plus a live gate:
`REWO_CLICK=<slot>[,<button>]` clicks headlessly and counts container resyncs
across it, which is the container equivalent of `CORRECTIONS`.

Measured: **586 tests** (proto 11, data 72, world 188, net 136, mesh 38, gpu 80
= 525 lib; app 61); all fifteen serverless gates green with validation ON and 0
VUIDs; demo SHA-256 `2cc56b4a…` byte-identical to M15 onward; live
`play --light-check --no-relight --no-build` 884,736 cells / 0 mismatches,
physics **CORRECTIONS 0**, place and dig server-observed; four headless clicks
(take-all, take-half, a 16-cap stack, a single) all **0 resyncs**.

**Open.** The **player preview** is not drawn — `inventory.png` paints that
window black and vanilla renders the 3D player over it, so the screen has a
black rectangle where the model belongs. It is the most visible remaining gap
and a self-contained next step: the entity pass, an ortho projection, a
scissored viewport. Beyond it: no shift-click quick-move, drag/quick-craft,
number-key swap, Q-to-drop, double-click pickup-all, no tooltips, no recipe
book, no durability bars, and no crafting (the 2x2 grid renders and accepts
items, but the result is whatever the server puts there). Armour items still
draw nothing, because their `minecraft:select` trim definitions are among the
147 M22 suppresses.

### M34 — the inventory, and icons in the hotbar (2026-07-27)

The hotbar has been drawing its nine empty frames since the HUD landed. M34
fills them: the client now knows what it is carrying, and draws it.

**Two coordinate systems, and the conversion between them.** The wire speaks
*menu slots* — 46 of them, in the order `ContainerMenu` lists: crafting, armour
from index 5, the main grid, then the hotbar from **36**, then the offhand at
**45**. The game logic speaks *inventory indices*, where the hotbar is simply
0..8. Every packet in this milestone crosses that boundary, and the two never
line up: `container_set_content` and `container_set_slot` carry menu slots,
while `set_held_slot` carries an inventory index. Reading either in the other's
terms puts a pickaxe in a boot.

**Three decode rules that are not obvious.** `set_held_slot` is guarded by
`Inventory.isHotbarSlot`, which **ignores** an out-of-range value — it does not
clamp it and does not reset to 0, so a bad packet leaves your selection where
it was. `container_set_slot` carries its index as a **signed short** where its
neighbours in the same packet use var-ints. And any container id but **0**
belongs to an open screen — a chest, a crafting table — which this client does
not have, so those updates are ignored entirely rather than written into the
player's own inventory.

**The icons needed one new thing from the bake and one new pass.** M22 already
produced every item as quads in 0..16 model units with UVs into its own
texture, through two very different sources — an extruded sprite and a reused
block bake — that converge on the same shape. What it did not keep was
`display.gui`, the transform that decides where those quads land in a slot.
That is now accumulated through the same parent chain as the rest, and it is
the whole difference between the two cases:

- a **sprite** has no `display.gui` at all, and the identity transform maps
  0..16 model units onto exactly −8..8 px — the 16 px slot, edge to edge. The
  absence is *correct*, not missing.
- a **block** inherits `scale 0.625` with `rotation [30, 225, 0]` from
  `block/block`, which tilts it and reaches **8.37 px** against the slot's 8 px
  half-width. It overflows a little, as vanilla's does.

Lighting is a third model, distinct from both the ones Rewo already had.
The world shades by `Direction` ordinal and so does the held-item pass; the GUI
uses `minecraft_mix_light_separate` — `min(1, (max(0,n·L0) + max(0,n·L1)) *
0.6 + 0.4)` — with `DIFFUSE_LIGHT_0/1` posed differently for flat and 3D items
(`Lighting.Entry.ITEMS_FLAT` against `ITEMS_3D`). This is why a block looks lit
from a different angle in the hotbar than in your hand.

**Two bugs the gate found before it graded anything.** `init_gui_items` is
called again whenever the hotbar needs a texture the resident atlas does not
hold, and it was assigning over the old pass — whose Vulkan handles are plain
integers rather than RAII types, so an image, a sampler and a pipeline leaked
per hotbar change. And the atlas was repacked *every frame* rather than only on
that change, rebuilding a megabyte for a hotbar that changes once a second —
the same "never construct per frame what is logically static" rule the
2026-05-31 leak pass established for Skia shaders.

**The first measurement was meaningless.** The pixel witnesses counted
"non-black" pixels, but the world pass paints a sky behind everything, so every
pixel was lit in every frame and both slots measured a difference of exactly
zero — while the PNG showed both icons rendering perfectly. They now count
pixels that **differ from the same scene with an empty draw list**, which is
the only thing that isolates the item pass.

**One witness had its reasoning backwards.** It claimed a sprite covers more of
its slot than a tilted block, on the grounds that the sprite fills the slot
edge to edge. A sword sprite is mostly transparent; the block measured nearly
twice its area. Coverage was never the property — *placement* is. It was
replaced by a mutation: the same block rendered with an identity `display.gui`
lands at exactly `(144, 96)–(192, 144)`, its slot to the pixel, while the baked
transform puts it at `(112, 96)–(193, 144)`. The one-pixel overflow at 193
agrees with the analytic 8.37-against-8, which is two independent routes to the
same number.

Gate: **`rewo inventoryshot --check`** — serverless, validation-required,
fail-closed, **16/16 witnesses**: six driving synthetic packets through the
production `route_inventory`, four grading placement and lighting on the CPU,
six rendering real baked items into real hotbar slots and reading them back.
Measured: **578 tests** (proto 11, data 67, world 188, net 136, mesh 38, gpu 77
= 517 lib; app 61), every prior gate green with validation ON and 0 VUIDs —
`weathershot` 35/35, `blockentityshot` 172/172, `swingshot` 97/97, `hurtshot`
38/38, `mobshot` 243/243, `eventshot` 28/28, `itemshot` 28/28, `danceshot`
24/24, `portalshot` 12/12, plus `skyshot`, `lightmapshot`, `tintshot`,
`meshshot`, `dimensioncheck`; canonical demo SHA-256 `2cc56b4a…` byte-identical
to M15 onward; bench replay GPU avg **0.213 ms** (the replay has no HUD, so it
does not exercise this pass — the number says nothing regressed elsewhere);
`git diff --check` clean.

**Open.** The inventory is decoded whole but only the hotbar is drawn — there
is no inventory *screen*, so the other 37 slots are held and never shown. Stack
counts and durability bars are not rendered. Enchantment glint, per-layer tint
and the 147 state-dependent item definitions M22 suppresses all stay
suppressed here too, on the same "nothing rather than something wrong" rule.

### M32b — `portalshot`, the end-portal pixel oracle (2026-07-26)

M32 shipped the pass and recorded an honest gap: its witnesses graded the
pass's *inputs* and geometry, never its output, leaving it one level short of
the Vulkan oracles that pin `skyshot` and `lightmapshot`.
`rewo portalshot --check` closes it — serverless, validation-required,
**12/12 witnesses**, two consecutive identical runs, 0 VUIDs.

**What makes an exact prediction possible.** The shader is fifteen projective
texture fetches summed in linear space; predicting that pixel-for-pixel would
mean reproducing fifteen matrices, which would only prove the oracle agrees
with itself. Two properties of the shader avoid it entirely:

- **Uniform textures collapse the matrices.** If both samplers hold one
  constant colour, every `textureProj` returns that constant *whatever the
  matrices do*, and the frame is exactly
  `sky*COLORS[0] + portal*sum(COLORS[0..layers])` — a number the CPU computes
  from an independent transcription of the table, with no matrix anywhere. That
  is `v1`–`v6`: the sixteen constants, the 15-vs-16 layer count, the double
  application of `COLORS[0]` (base *and* `i = 0`), the additive accumulation,
  and the opaque non-blended write. Both textures are synthetic — an oracle
  whose expectation depends on an asset's contents is grading the asset.
- **One layer isolates one sample, and only then is the matrix observable.**
  `textureProj` divides by `p.w`, every layer matrix's column 3 is `(0,0,0,1)`
  so that divide cancels, and the composed column 0 has a zero depth term. The
  sampled `u` is therefore an *affine function of the screen UV alone*. `v9`/
  `v10` render a left-red/right-blue texture through a single layer and grade a
  7×7 grid of pixel centres against that prediction, skipping any pixel whose
  predicted `u` lands within 0.12 of a seam where bilinear filtering makes
  "which half" genuinely ambiguous.

**`v7` was written backwards, and the failure is the finding.** The first
version asserted that moving the camera must *change* the pixels. It does not:
`texProj0` is `projection_from_position(gl_Position)`, so the sampled texel
depends on the pixel's screen position and nothing else. For a portal covering
the viewport, sliding the quad 1.5 blocks through the world or rolling the
camera leaves the image **identical** — measured at 175 and 51 differing bytes
of 65,536, every one of them a single quantisation step, against the 40-step
deltas `v8` sees from a clock change. The starfield is welded to the screen and
swims against the world. A model-space sampler — the mistake the mesh's unused
UVs invited — would have dragged its pattern along with the geometry.

**Sensitivity, measured not asserted.** Mutating the shipped shader's
`texProj0 * matrix` into `matrix * texProj0` (the transposed reading M32 nearly
shipped) drops `v10` from 21/21 to **9/21** and the gate to exit 1 — while
`v1`–`v8` all still pass, because the uniform-texture witnesses are
matrix-blind by construction. That is the point: `v10` is doing work no other
witness in the file could do. `v9` guards it in turn by requiring the two
readings to actually disagree somewhere (24 of 49 pixels), and `v11`/`v12`
guard against grading a black clear or a multi-sample blur.

Gates after M32b: **503 tests** (442 lib + 61 app), `portalshot` 12/12,
`blockentityshot` 172/172, `itemshot` 28/28, `hurtshot` 38/38, `swingshot`
97/97, `eventshot` 28/28, `danceshot` 24/24, `mobshot` 243/243;
`lightmapshot`, `skyshot`, `tintshot`, `meshshot`, `dimensioncheck` green with
validation ON and 0 VUIDs; canonical demo SHA-256 `2cc56b4a…` byte-identical to
M15 onward; `git diff --check` clean.

### The block-entity arc, in one place

M25 measured eleven block-entity types whose block models bake to nothing.
Eleven render. `blockentityshot` grew from 21 witnesses to **172**.

What the arc is actually worth recording for is not the feature list but the
**five times a gate witness corrected something already written down as fact**:

| what was wrong | how it was caught |
|---|---|
| `a4` asserted "nothing is Rendered yet" and passed through four renderers | rewritten to derive the set from the resolver, in both directions |
| a pot's side plane baked six quads, not one | `k14` — `EnumSet.of(NORTH)` builds *only* that face |
| a banner base texture path baked no pole while every pattern loaded | `k19`, which now prints its counts rather than only asserting them |
| the conduit's frame shell was 42 positions, not the 48 I derived | a unit test; 42 is also the hunting threshold, so the eye opens on a *complete* frame |
| the spawner's inner translate does **not** make the mob orbit | `r6` — it lies along the spin axis and commutes |

Plus two rest poses wrong since M28 (a piglin's ears ~10° off, a dragon's jaw
drawn shut) that only building the animation could expose, and a GLSL
column-major transcription that a "helpful" rewrite nearly broke.

They share a shape. **The claims that survive unchallenged are the ones nothing
in the render moves against** — a wrong rest pose, a miscounted static set, an
ordering that happens not to matter, a witness whose subject moved. None of
them produce anything that *looks* broken. Only asserting the property directly
makes them falsifiable, which is the argument for witnesses that pin values
rather than shapes.

### M68 — the sheep's undercoat, and the tropical fish properly (2026-07-29)

Two mob-rendering items M64 left open. Both turned out to be a layer or a mesh
that the *class name* mis-describes, and in both cases the decompile's
registration table settled it where the class did not.

#### The sheep's undercoat

26.x added `SheepWoolUndercoatLayer` beside the fleece, and M64 recorded it as a
gap with two facts: it draws `sheep_wool_undercoat.png` over the body mesh, and
it is **not gated on `isSheared`**. Both hold. What M64 could not have known
without reading `LayerDefinitions` is the third:

> `SheepWoolUndercoatLayer` wraps its model in **`SheepFurModel`** — and
> `ModelLayers.SHEEP_WOOL_UNDERCOAT` maps to **`sheepBodyLayer`**, i.e.
> `SheepModel.createBodyLayer()` at `CubeDeformation.NONE`.

The class supplies only `setupAnim`; the geometry comes from the baked layer.
So the undercoat is not a third fleece at all — it is the **sheep's own body
mesh, repeated**, exactly coplanar with it. The sheet's layout confirms it
independently: its head block is 12 px wide over 8 rows (w=6, d=8), the *body*
head's box unwrap, where the fur sheet's is 12 px over 6.

And it is more literal than that. All **467** of the sheet's opaque texels are
byte-identical to `sheep.png` at the same UV — the undercoat sheet is the wool
region of the base sheep sheet, cut out. That is what makes `u4` an *absolute*
witness rather than a ratio one: the pixel behind every undercoat pixel is the
same texel at the same face shade, so their quotient **is** the tint.

**It takes the same dye.** `getWoolColor()`, the identical call
`SheepWoolLayer` makes — `ColorLerper.Type.SHEEP`, which is
`floor(getTextureDiffuseColor() * 0.75)` with WHITE overridden outright to
`0xE6E6E6`. Not the raw diffuse table. Worth stating because a *ratio* witness
cannot tell the two apart between two coloured dyes (a uniform scale cancels);
only measuring against the untinted body underneath separates them, and the
mutation reads ≈1.95× too bright in linear.

**Coplanar geometry needed a pipeline, not a fudge.** Rewo's solid entity pass
depth-tests strict `GREATER` (reversed-Z), so a layer at exactly the base's
depth is rejected fragment for fragment and draws *nothing*. Every other layer
Rewo bakes as a texture slot is inflated over the body — the fleece
0.6/1.75/0.5, the fish pattern 0.008 — and wins on its own. The undercoat's
quads therefore leave the solid range for M48's armour-trim range:
`CompareOp::EQUAL`, no depth write, drawn after it. That is the reversed-Z
reading of vanilla's `entityCutout` (`LEQUAL` + write) for geometry coplanar by
construction — at equal depth both pass, and the write is a no-op because the
value already there is the one it would write. Nudging the layer outward
instead would have been a guess, and a visible one.

The ordering is what makes the fleece still occlude it: an unshorn sheep's
depth buffer holds the *inflated* wool's nearer depth, which the undercoat then
fails. Not everywhere, though — the fur boxes are shorter than the body's (a
6×6×6 head against 6×6×8, 4×6×4 legs against 4×12×4) and the sheet is
alpha-cutout, so a woolly sheep's snout and lower legs still show undercoat, as
they do in vanilla. Measured: **6,966 px shorn, 1,634 woolly, none outside the
shorn set.**

**M64's `t6` was stating something false**, and this is the honest part of the
milestone. It read "a shorn sheep is inert to the dye that moved a woolly one —
the only tinted texture is the one the layer stopped submitting." Vanilla's
shorn dyed sheep answers the dye; that is the undercoat's entire visible point.
`t6` now renders with the second fleece suppressed and says so, keeping the
tint-versus-geometry discrimination it was written for, and `u1` carries the
corrected claim. The row had gone on passing after the layer shipped only
because the gate never enabled it.

#### The tropical fish

M64 excluded it and was right to: the packed int does not select a texture. It
selects a **mesh**, a **pattern layer** and **two dye colours**.

`TropicalFish.packVariant` is
`pattern.getPackedId() & 65535 | (baseColor.getId() & 0xFF) << 16 |
(patternColor.getId() & 0xFF) << 24`, and `Pattern`'s own packed id is
`base.id | index << 8`. Four fields, low to high:

```text
  bit  0      Base       0 SMALL (tropical_a) / 1 LARGE (tropical_b)
  bits 8..15  pattern    the index 0..5 *within* that Base
  bits 16..23 body dye   DyeColor id  (getModelTint -> state.baseColor)
  bits 24..31 pattern dye DyeColor id (TropicalFishPatternLayer)
```

**Bits 1..7 belong to no field**, which is why `Pattern.byId` is
`ByIdMap.**sparse**`: the id space is not dense, so an unrecognised packed id
falls back to a *named default, KOB*, rather than clamping or wrapping into a
neighbour. `FishVariant::unpack` reproduces that by reconstructing
`base | index << 8` and comparing — a mask over "the bits the fields use" would
accept the seven stray ones and read a valid pattern out of a bogus id.

The **shape** is one wire name with two `EntityModelKind`s, chosen by the
caller from bit 0 — the `PlayerSlim` shape, because
`TropicalFishRenderer.submit` assigns `this.model` from `state.pattern.base()`
before every submission. `TropicalFishLargeModel` is its own mesh, not a
rescale: a 2×6×6 body against 2×3×6, a 5-deep tail against 6, fins a block
higher, and a **bottom fin** the small plan has no part for.

The **pattern** is a second texture slot on the same mesh at
`CubeDeformation(0.008)`, and which of its six sheets it samples is an ordinary
per-draw variant id — so it lands in `mob_variants` beside M64's forty-two and
inherits that whole machinery (`n1`, `n5`, `n6` grade it for free once they
walk every texture key rather than only a mob's first).

The **two dyes** come from `getTextureDiffuseColor()` with no `ColorLerper` in
sight — the undimmed table, *not* the sheep's. The two agree closely as ratios
between two coloured dyes and disagree sharply against WHITE (44 of 45
channel-entries by more than 8%), which is why `f4` references dye 0.

#### Two gate findings

**`n5` was a false positive waiting for a second mob to vary the same slot.**
It kept one global set of per-slot UV offsets, which worked while each of
M64's six varied its only texture. The two fish plans' pattern bases are
adjacent 32×32 sheets and their alternates pack consecutively, so
`tropical_a_pattern_6` and `tropical_b_pattern_2` land on the *same relative
offset* while addressing different atlas slots. The offset is relative to a
mob's own base, so the set is now keyed by kind.

**Framing a fish head-on measures almost nothing.** `frame_kind` looks along
−Z and a tropical fish is **2 model-px wide**: the only body face in view is a
2×3 rect. `tropical_a_pattern_3` has no marks in that one rect, and an early
build of this gate read its 0 px as a broken bake — the atlas offsets, the
packing order and the decoded sheet were all checked before the render was
looked at. Yawed 90° every pattern renders thousands of pixels. The general
lesson is the M37 one from the other direction: a detector that measures
almost nothing will report a real feature as absent.

#### The atlas

Four sheets added to `MOB_TEXTURE_SPECS` and ten to `VARIANT_TEXTURES`; total
new area 16 KB of texels against ~900 KB of shelf. **`ATLAS_H` is unchanged at
1600** — M64's growth left room, and its note that the shelf ceiling is defined
by subtraction (so a grow-at-the-top slides every pool below) did not need
re-testing. The base table's sort is `(h desc, w desc, key)`, so the new keys
*do* reorder the shelf packing; that is safe because every consumer computes
its origins from the pack and the atlas is rebuilt at startup, and it is
covered by `mobshot --check`, which recomputes UVs from the same origins.

`mobshot --check` goes **243 → 246 mob-views, zero failures** — three views for
the new `TropicalFishLarge` kind, which is a real model and belongs in the
gate. The count is descriptive; the gate has no hard-coded expectation.

#### Measured

**1,104 tests** (was 1,098: +3 `mob_variants`, +3 `mobs`), zero failures. Every
gate green with Vulkan validation ON and **0 VUIDs**: `capeshot` 69, `itemshot`
62, `inventoryshot` 143, `healthbarshot` 33, `attributeshot` 43, `captureshot`
17, `blockentityshot` 172, `swingshot` 97, `hurtshot` 38, `weathershot` 35,
`handshot` 34, `particleshot` 34, `eventshot` 28, `danceshot` 24, `portalshot`
12, `hudshot` 41, `mobshot` **246/246** + emissive 5 + etf 8 + **tint 11** +
**variant 13**, plus `skyshot`, `lightmapshot`, `tintshot`, `meshshot`,
`dimensioncheck`. Demo PNG SHA-256
`2cc56b4acbfb92cb91398c27e5c4735885abff9331f66b7dc83bdbc002246635` —
byte-identical since M15. `git diff --check` exits 0.

**Every mutation partner was run.** U1 (`&& !isSheared` on the gate) fails
`u1`; U3a (drop `woolColor != WHITE`) fails `u3`; U3b (drop `!isBaby`) fails
`u3`; U4 (`DYE_DIFFUSE_COLORS` for the undercoat) fails `u4`; U5 (the coplanar
range depth-tests `ALWAYS`) fails `u5`; F1 (base/index sub-fields swapped)
fails `f1`; F2 (both plans from the small mesh) fails `f2`; F3 (both fish
layers from one dye field) fails `f3`; F3b (the two colour bytes transposed)
fails `f3b`; F4 (the wool lerper for the fish) fails `f4`.

**One mutation did not flip its row, and the row's text was corrected rather
than the row.** `u2`'s partner was "build the undercoat from
`SheepFurModel.createFurLayer()`" — inflating it, run alone, makes the layer
*vanish* (it no longer sits at the body's depth, so `EQUAL` rejects it) instead
of growing the silhouette; `u5` caught it and `u2` did not. The mutation that
shows what `u2` claims is the pair — fur inflation **and** leaving the layer in
the solid range — which takes the shorn silhouette 21,290 → 23,522. `u2` now
says so, including why the second half is load-bearing.

**Not verified:** no live sighting. Nobody has watched a shorn dyed sheep or a
school of tropical fish on a real server; everything here is headless.

**Open.** The undercoat's `isJebSheep` disjunct is deliberately absent — it
selects `ColorLerper.getLerpedColor`'s rainbow, which Rewo does not render, so
including it would draw the layer in a colour the fleece beside it is not
wearing. The fish's `Base` also drives nothing else Rewo models (both plans
share the tail animation verbatim), and the twenty-two `COMMON_VARIANTS` and
their predefined names are a tooltip concern, not a rendering one.

### M72 — passenger positioning: where a rider actually sits (2026-07-29)

M70 decoded `ClientboundSetPassengersPacket` into a riding graph and consumed
it for `Entity.isVehicle()` alone. The positional half was missing, so a player
on a horse kept rendering at its own last-reported position. M72 is that half.

**The seat is entity-type data, not a constant.** 26.x has no
`getPassengersRidingOffset()`. `Entity.getPassengerRidingPosition` is
`position().add(attachments.getClamped(PASSENGER, indexOf(passenger), yRot))`,
and `attachments` comes from `EntityDimensions`, which the **`EntityType`
builder** declares:

```java
EntityType.Builder.of(Pig::new, …).sized(0.9F, 0.9F).passengerAttachments(0.86875F)
```

So it is a table, extracted by **`tools/gen_entity_attachments.py`** into
`crates/rewo-data/src/entity_attachments_table.rs` — 158 types, **57 declaring
seats, 24 declaring a vehicle point**. Three conventions in that builder invert
if you assume them:

- **`passengerAttachments(float…)` takes Y offsets**, and a type may declare
  several (the happy ghast declares four full `Vec3` seats).
- **`ridingOffset(r)` is negated** into `attach(VEHICLE, 0, -r, 0)`. A zombie's
  `ridingOffset(-0.7F)` is a VEHICLE point of `(0, +0.7, 0)`.
- **PASSENGER's fallback is `AT_HEIGHT`, not `AT_FEET`** — `(0, height, 0)`,
  the *top* of the bounding box, which is why `sized(…)` has to be captured
  too. VEHICLE's fallback is the zero vector.

**There are two tables, keyed by two different types.** `positionRider` is

```java
Vec3 position = this.getPassengerRidingPosition(passenger);  // vehicle's PASSENGER point
Vec3 offset   = passenger.getVehicleAttachmentPoint(this);   // rider's own VEHICLE point
moveFunction.accept(passenger, position.x - offset.x, …);
```

The second is the rider's, rotated by the **rider's** yaw. A player's is
`(0, 0.6, 0)` — `Avatar.DEFAULT_VEHICLE_ATTACHMENT` — and it is the whole
reason a mounted player sits in a saddle instead of standing on the horse's
head. Dropping it raises every rider by 0.6 blocks, silently and constantly.

**A passenger does not interpolate, and vanilla is unambiguous about it.**
`ClientLevel.tickEntities` skips passengers outright
(`… && !entity.isPassenger() && …`); a rider is reached only through its
vehicle, by `tickNonPassenger` → `tickPassenger` → `rideTick()`, which ticks it
and *then* calls `getVehicle().positionRider(this)` — an unconditional
`setPos`. Its own synced position and its own three-step lerp are computed and
overwritten every tick and never reach the screen. Nor does the renderer
re-derive per frame: `EntityRenderer.extractRenderState` is
`Mth.lerp(partialTicks, entity.xOld, entity.getX())` for every entity alike,
which for a rider blends two *derived* positions because `tickPassenger` calls
`setOldPosAndRot()` first. Rewo therefore derives into `cur` at the end of
`tick_lerp`, after every entity's own step has moved `prev = cur` — one line of
ordering that makes `render_pos` correct at every sub-tick fraction for free.
**That equality is checkable**, and it is what "follows without jitter" means:
with the yaw constant, the rider's offset from its vehicle is identical at
every fraction, measured at 4.8e-8 blocks over a three-tick lerp across 37
blocks. The pre-M72 error was never a constant offset — it was a **lag**.

**The overrides are a virtual-dispatch problem, so the selector is the class.**
`tools/gen_entity_classes.py` gained fourteen ancestry sets (a pure addition —
the existing tables came back byte-identical), and `VehicleClass` resolves
most-derived-first because that is what `super` does: a camel **is** an
`AbstractHorse`, and its own override replaces the horse's rather than
composing with it. Shipped exactly: the boat (which **replaces** the lookup —
it declares no seats at all, and its `rideHeight` splits by *leaf* class, so
`Raft` and `ChestRaft` share `height × 0.8888889` **across** the chest
boundary while `Boat` and `ChestBoat` share `height / 3`), the chest boat's
0.15 forward shift, the two-seat fore/aft split and its `+0.2`
`instanceof Animal` bump, the minecart's `Vec3.ZERO` for a villager or
wandering trader, the cube mob's size term, the strider's walk bob, the
spider's rider-side width test, and the camel's body anchor. `Llama`'s
override is a no-op that restores the plain default, which is why it and a
resting horse coincide here.

**Every VEHICLE point in 26.2 lies on the y axis**, so the rider-side rotation
is *unobservable* in vanilla data — every one comes from `ridingOffset(float)`
or `Avatar.DEFAULT_VEHICLE_ATTACHMENT`, both of which build `(0, y, 0)`, and a
pure-y vector is invariant under `yRot`. Rather than fake a discriminating
sample, `r1` asserts the invariant over the whole generated table: if a future
version declares an off-axis point the witness fails and says to go build the
case, which is exactly when it starts to matter.

**Two rotation facts.** `EntityAttachments.transformPoint` rotates by
**negated** degrees, which is what puts a `+z` seat behind a vehicle facing
`+z`; and `AbstractHorse`/`Chicken.positionRider` end with
`if (passenger instanceof LivingEntity l) l.yBodyRot = this.yBodyRot`, which
assigns the **body** yaw and nothing else — that is why a player on a horse can
look sideways, and why a boat riding a chicken is not turned at all.

**Gate: `rewo rideshot --check`, 24 witnesses**, serverless and GPU-less,
fail-closed. Raw `set_passengers` bodies → `route_set_passengers` →
`EntityTable::tick_lerp` → `render_pos`. Every witness measures the rider's
position **relative to its vehicle**, never "the rider moved" — a rider moving
for its own reasons is precisely the bug. **All eighteen mutations were run and
seventeen bit on the first attempt.**

**The eighteenth did not, and it repeated M70's `b4` in a new shape.**
`r4.a_rider_that_moved_on` named `position_riders`' per-rider `vehicle_of`
re-check as its partner. Running that mutation left the gate green: with
`set_passengers` maintaining both maps together — it *detaches* a rider from
its previous vehicle before adding it here — an inconsistent pair is
**unreachable by construction**, so the re-check is a belt, exactly like the
cycle walk's visited set. The witness now names the detach that is actually
load-bearing (and asserts the old vehicle reads un-ridden, which that mutation
does flip), and both the code and the gate say plainly that the re-check is not
what makes it work. *A named mutation partner that cannot be reached is not a
partner.*

Two other samples were placed for the same reason and both bit: the spider's
`<=` width bound is exercised from a **polar bear**, whose 1.4 width is exactly
the spider's, and `getClamped` is sampled at index 3 **and** 4 on a four-seat
vehicle.

**Measured.** **1193 tests** (was 1180: +4 in `rewo-data`'s
`entity_attachments`, +9 in `rewo-world`'s `riding`). Every gate green with
validation ON and **0 VUIDs**: rideshot 24, labelshot 32, capeshot 69, itemshot
62, inventoryshot 152, healthbarshot 33, attributeshot 43, captureshot 17,
blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35, handshot 34,
particleshot 34, eventshot 28, danceshot 24, portalshot 12, hudshot 41, mobshot
**246/246** (+ emissive 5, etf 8, tint 11, variant 13), skyshot, lightmapshot,
tintshot, meshshot, dimensioncheck. Release `demo` PNG SHA-256 byte-identical
to M15 onward (`2cc56b4a…`).

**Not verified:** no live sighting. Nobody has watched a player ride a horse or
a boat on a real server; everything here is headless.

**Open, and each is blocked on something real rather than skipped.**

- **The seated pose.** `HumanoidModel.setupAnim`'s `if (state.isPassenger)`
  block **assigns** `rightLeg.xRot = -1.4137167F` (plus yRot/zRot) and *adds*
  `-π/5` to both arms. It is not a positioning problem but a model-selection
  one: Rewo's `Anim::LegRight` is shared by body plans that do not run
  `HumanoidModel`, and the undead/skeleton/illager arm overrides run **after**
  the passenger block, so their arms must not be lifted. Until that selection
  exists, a mounted rider sits in the right place with straight legs.
- **The animation-driven seat offsets.** The horse's rearing lift is
  `standAnimO`-scaled and reduces to zero at rest, so it costs nothing; the
  camel's sit and pose-transition arms need `LAST_POSE_CHANGE_TICK`, a synced
  LONG this client does not decode. The camel's *standing* anchor is
  implemented, because its fallback would be 0.375 blocks above its own back.
- **`Minecart.positionRider`'s player rotation** and `EntityRenderer`'s
  `passengerOffset` both need `NewMinecartBehavior`, an unmodelled client
  simulation.
- **The `minecraft:scale` attribute** is half of vanilla's scale factor and is
  deliberately not applied: Rewo's renderer does not scale a model by it
  either, so honouring it on the seat alone would place a rider off the mount
  it is drawn on. `getAgeScale()` — exactly `isBaby() ? 0.5F : 1.0F` — is
  applied, and it matches Rewo's existing baby model scaling exactly.

### M74 — the coverage re-audit, and six packets it turned up (2026-07-29)

Two jobs. Re-derive `REWO_PACKET_COVERAGE.md` from the code, because it had
drifted; then implement the class-A gaps the re-derivation turned up.

**Ten of the 141 rows were wrong, all in one direction** — `absent` about code
that was present: `explode`, `move_vehicle`, `set_entity_motion` (M68),
`set_chunk_cache_center` / `_radius`, `set_simulation_distance` (M67's own),
and `set_cursor_item`, `set_passengers`, `set_player_inventory`, `update_tags`
(M69/M70/M72). The headline counts were wrong by the same ten: **56 / 85
published against 66 / 75 true**, class A 31 against 21.

**The mechanism is not neglect.** M67 wrote the table by grepping, and four
packets landed in `ids.rs` *the same day* — three from its own sibling M68. It
was a snapshot of a moving tree and began going stale within hours. M67 saw it
happening and worked around it twice, both of which made it worse: an "After
§7" column predicting where the counts would land (so the published table
described a moment that never existed), and milestone markers like `**M69**`
written into the **status** column, which put four rows outside any grammar a
future check could read. *Annotating decay is not fixing it.*

**The fix is `ids::coverage_table_tests::the_coverage_table_matches_the_code`**
— a unit test, deliberately not a `*shot` gate, because it should fire on the
event that *causes* the drift (someone editing `ids.rs`) and `cargo test -p
rewo-net` is what runs then. It `include_str!`s three files at compile time —
the document, `ids.rs`, and the dispatch chain — so it has no runtime
dependency on the datagen report, the network or the cwd, and there is no
"skip if missing" branch to fall through. It recomputes all 141 statuses, both
count tables and the class distribution; it is not a spot-check, because a
spot-check has the same failure mode as the grep it replaces.

Its limit is stated in the document: it verifies **status**, not correctness
and not completeness. A dispatched-but-half-decoded packet is `handled` to it,
which is exactly the gap §4 is maintained by hand to cover.

**M74's own instrument was stricter than M67's** and reached the same negative
finding: asking "does an incoming id get *compared* against this field inside
`rewo-net`" rather than "does the name appear anywhere under `crates/`" still
yields **zero** resolved-but-ignored packets.

#### The six packets

Final tally **72 handled / 0 ignored / 69 absent**; class A 21 → **15**.

**The chunk-batch pair is a live divergence, not a missing decode.** Rewo
replied `p.f32(64.0)` to every `chunk_batch_finished`. Vanilla replies
`ChunkBatchSizeCalculator.getDesiredChunksPerTick()`, which is `7e6 / agg` over
a seeded `agg = 2e6` — an opening bid of **3.5**. Rewo therefore over-bid the
server ~**18×** on the first batch of every session and never adapted, and the
server sizes its chunk batches to that number. Both halves were needed:
`chunk_batch_start` (12) is the clock stamp, and a calculator with no interval
to measure would only have produced a *differently* wrong constant. Four rules
each have a witness: the `batchSize > 0` guard covers the weight bump too
(dropping it divides by zero, and Java's `double / 0` is `Infinity`, so the
estimate is poisoned silently); the clamp window is `agg/3 ..= agg*3`,
recomputed per sample rather than fixed; the weight is used **before** it is
bumped; and the bid is a `double` divide narrowed to `f32`.

**`Difficulty` is a third enum convention.** The project's notes record two —
`readEnum` (out-of-range is an *error*) and `ByIdMap.continuous(…, ZERO)`
(out-of-range is the zero value). `Difficulty.STREAM_CODEC` is
`ByteBufCodecs.idMapper` over `ByIdMap.continuous` with **WRAP**. The three
readings disagree on real input: id `5` is `EASY` under WRAP, `PEACEFUL` under
ZERO, and a rejected packet under `readEnum`. And WRAP is `Math.floorMod`, not
`%` — a negative id is legal and indexes from the far end, where Rust's `%`
would panic. The id-4 sample is *not* discriminating (WRAP and ZERO agree
there); the witness samples **5**.

**`container_close`'s id is read and then ignored.** `handleContainerClose` is
one line with no comparison against `containerMenu.containerId`. Gating on it —
which is exactly what M34/M35 correctly do for `container_set_slot` — drops the
packet whose only job is to close the screen. Wired through to
`ScreenState::close_requests_seen`, a watermark rather than a consumed flag so
the session keeps owning the counter.

**`set_camera` closes a stub M70 left.** `LabelViewer::camera_entity` was
hard-wired to `session.player_id` under a comment reading "Rewo never detaches
the camera"; this is the packet that detaches it. An unresolvable entity leaves
the camera **where it was** rather than resetting to the player, and the
resolution must count the local player's own id as valid — vanilla's
`level.getEntity` finds the player and Rewo's `EntityTable` never contains it.

**`ticking_state` / `ticking_step`** are decode-and-state in M67's sense:
`TickRateManager` is transcribed in full, including `tick()`, but the 20 Hz
loop does not consult it — gating the loop would retime every existing harness
and wants its own live gate. Two edges are witnessed anyway because they are
free: `setTickRate`'s clamp is Java's `Math.max`, which **propagates NaN**
where Rust's `f32::max` swallows it (and the wire carries an unvalidated f32);
and `tick()` reads `frozenTicksToRun` *before* decrementing, so `/tick step 1`
on a frozen world runs exactly one tick — the witness uses a step count of
**1**, because 2 lets both readings run at least one tick and hides it.

**Excluded on purpose:** `player_rotation` (73) and `player_look_at` (71),
which rank second in the document's §3 — they write
`rewo_world::physics::PlayerState`, which a concurrent milestone owns while it
lands `player_abilities` and the flight / no-clip physics behind it.

#### A doc claim the decompile contradicted, and a witness that did not bite

I wrote in a doc comment that the login packet carries the difficulty. **It
does not** — `ClientboundLoginPacket` has no such field, and `handleLogin`
writes `new ClientLevelData(Difficulty.NORMAL, …)` with the constant in the
source. So `change_difficulty` is the only source and Rewo's default is
vanilla's literal. Checking it also turned up the reason not to reset it:
`handleRespawn` rebuilds from `this.levelData.getDifficulty()`, carrying the
value across a dimension change, so `ClientState` lives on `PlaySession` and is
untouched by `apply_respawn` — the rule `ViewArea` already follows.

**The first battery ran 37 mutations; 36 bit and one survived — and the
survivor exposed that the whole routing layer was unwitnessed.** Dropping
`|| local_player == Some(target)` from `set_camera`'s resolvability left the
suite green, because every witness tested a *reader* or a *state method* while
the rule that computes resolvability lived in a closure inside
`route_client_state`, which no test could reach without building a whole `Ids`.
A second mutation in the same layer (gating `container_close` on id 0) could
not even be *expressed* as a compiling change for the same reason.

The fix follows `view_area`'s existing precedent rather than inventing one:
both modules now expose `Ids` / `kind_for_id` / `apply`, so the routing
decisions are ordinary functions a witness can drive, and `route_*` in `lib.rs`
is four lines of wiring. That turned two unreachable mutations into seven
reachable ones — which then caught a third thing nobody had asked about,
`apply` reporting success on a malformed body. **Re-run: 42 mutations, 42
caught, every one by a named witness.**

*If a mutation cannot be reached from where the witness sits, the witness is
measuring something else — and the fix is to move the rule, not the witness.*

Four more mutations were run against the coverage check itself: flipping a
`handled` row, staling a §2 count, resolving a packet with no table row (the
case that actually happened), and resolving one nothing dispatches (which
reports the third status). All four were caught. A fifth — making
`is_dispatched` return `true` unconditionally — **passed the main check** and
was caught only by the vacuity witness beside it, which is what earns that
witness its place.

**Measured.** **1291 tests** (was 1247: +44 in `rewo-net` — 22 in
`chunk_batch`, 17 in `ticking`, 19 in `client_state`, 2 in the coverage check,
against 16 that moved out of the old inline routing). Every gate green with
validation ON and **0 VUIDs**: labelshot 47, rideshot 24, capeshot 69, itemshot
62, inventoryshot 152, healthbarshot 33, attributeshot 43, captureshot 17,
blockentityshot 172, swingshot 97, hurtshot 38, weathershot 35, handshot 34,
particleshot 34, eventshot 28, danceshot 24, portalshot 12, hudshot 41, mobshot
**246/246** (+ emissive 5, etf 8, tint 11, variant 13), skyshot, lightmapshot,
tintshot, meshshot, dimensioncheck. Release `demo` PNG SHA-256 byte-identical
to M15 onward (`2cc56b4a…`). `git diff --check` clean; every file touched was
already pure LF and stayed that way.

**Not verified:** no live server run. The chunk-batch reply is a behaviour
change on the wire, and the argument for it is that it now matches vanilla
exactly rather than that anyone watched a session stream chunks with it. Its
arithmetic is pinned against the decompile under a synthetic clock; the effect
on a real connection is unobserved. The same applies to the two things wired
into the app — `set_camera` reaching `LabelViewer` and `container_close`
closing the inventory screen are unit-true and un-eyeballed.

### M76 — the rotation the server writes, and the world spawn (2026-07-29)

Three class-A packets that write local player and level state:
`player_rotation` (73), `player_look_at` (71), `set_default_spawn_position`
(97). M74 ranked the first two second in `REWO_PACKET_COVERAGE.md` §3 and
deliberately did not take them, because they write the `PlayerState` that M75
owned while it landed flight; M75 landed, so they were free.

**The brief for this milestone was wrong about the headline fact, and so was
the coverage document.** Both said `player_rotation` "carries per-axis relative
bits" and pointed at `ClientboundPlayerPositionPacket`'s `RelativeMovement` set
as the pattern to follow. It does not. The packet is

```java
record ClientboundPlayerRotationPacket(float yRot, boolean relativeY,
                                       float xRot, boolean relativeX)
```

a four-field `StreamCodec.composite`: ten fixed bytes with each flag as a plain
`ByteBufCodecs.BOOL` sitting **after** the float it qualifies. There is no
`Relative.SET_STREAM_CODEC` in the packet at all. A reader written from the
description would have consumed the yaw's four bytes as the mask and then read
float payload as booleans — and it would have *decoded every packet without
error*, because the arity happens to work out.

The `Set<Relative>` is real, one layer up: `handleRotatePlayer` calls
`Relative.rotation(relativeY, relativeX)` to build it and hands it to
`PositionMoveRotation.calculateAbsolute`, the same function the positional
teleport uses. **So the two packets share their semantics and not their
layout**, which is the specific way the guess was wrong — checking that the
*meaning* matched would have confirmed it.

#### What the rotation packets actually do

`handleRotatePlayer` reduces, once the position and delta clauses of
`calculateAbsolute` are recognised as identities on its own call, to:

```
yaw   = (relativeY ? yaw : 0) + yRot                       -> setYRot
pitch = clamp((relativeX ? pitch : 0) + xRot, -90, 90)     -> setXRot
```

Four things there invert the obvious guess.

**The clamp is on the sum, not the step**, and it is applied twice — once in
`calculateAbsolute` and again inside `Entity.setXRot`, whose form is
`Math.clamp(xRot % 360.0F, -90, 90)`. The second is idempotent for this packet.

**The yaw gets neither a clamp nor a wrap.** `setYRot` is a bare assignment
behind a finiteness guard, so a server sending 720 leaves the player at 720 and
that value goes back out on the wire unwrapped. Wrapping it looks like tidying
and is a divergence.

**A non-finite rotation is discarded, not clamped.** Both setters test
`Float.isFinite` and, when it fails, log and return *without writing*. That is
reachable, because `Mth.clamp(NaN, -90, 90)` is NaN — its first test is
`value < min`, false for NaN, so it falls through to `Math.min`. A NaN pitch
therefore survives `calculateAbsolute` and leaves the previous pitch standing,
while the yaw half of the same packet still applies.

**Only one of the two answers the server.** `handleRotatePlayer` ends with an
unconditional `ServerboundMovePlayerPacket.Rot(yRot, xRot, false, false)`,
before any tick and whether or not the rotation changed; `handleLookAt` sends
nothing and lets the next movement report carry it. `RotationRoute` exists to
carry that distinction from the seam to the session. Neither has
`handleMovePlayer`'s `if (!player.isPassenger())` guard, so a rotational
teleport applies while mounted where a positional one does not.

#### `Mth.atan2`, not `Math.atan2`

`Entity.lookAt` calls vanilla's own `atan2` — a 257-entry `asin`/`cos` table
plus a Quake-style `fastInvSqrt` and a cubic correction, built at class-init
from `Math.asin`/`Math.cos`. It is transcribed rather than substituted, for
the reason M12 recorded for `Mth.sin`: the platform function agrees to eyeball
precision everywhere and is not what vanilla evaluated. Measured divergence
over a 3,600-point sweep: **7.0e-6 rad**, which is 4e-4 degrees — invisible in
a render and not zero, so the gate's witness is **two-sided**. It asserts the
transcription stays close to the platform (or it is simply broken) *and* that
it does not equal it (or someone quietly swapped the platform function back
in). A one-sided witness here would have passed on the substitution.

`libm` supplies the table's `asin`/`cos` because HotSpot does **not**
intrinsify `asin` — `Math.asin` delegates to `StrictMath.asin`, which is
fdlibm, so that column is exact. `Math.cos` *is* intrinsified and may differ by
up to 1 ULP; that is stated at the site rather than papered over, because it
means this reproduces `Mth.atan2`'s algorithm to the operation and not its
result to the bit.

Three Java details in `lookAt` are load-bearing and asymmetric: the pitch's
`(float)` cast encloses the **negation** while the yaw's encloses only the
division (`- 90.0F` is then a float subtraction); `(float)Math.PI` is π rounded
to `f32` and widened back, so the divide is by a slightly-wrong π *by
specification*; and the yaw's arguments are `(zd, xd)`, which with the `- 90.0F`
is what produces Minecraft's south-zero convention. Aiming at exactly your own
anchor is not special-cased and is not a NaN: `atan2(0, 0)` is 0, so the yaw
becomes -90 and the pitch 0.

The anchor is `EntityAnchorArgument.Anchor`, read by `readEnum` — an array
index, so ordinal 2 is a decode **error**. That is the third enum convention
this codebase has met, and all three are now reachable with a two-value enum:
`readEnum` errors, `ByIdMap.continuous(…, ZERO)` returns the zero value (M65),
`…, WRAP)` takes `Math.floorMod` (M74).

**An unknown target entity is not a no-op.** `getPosition` falls back to the
packet's own `x/y/z`, and those are not filler — the sending constructor sets
them to `toAnchor.apply(entity)` at send time. So the fallback is the correct
anchored point, stale only by the target's motion since. That is what makes
Rewo's scoping defensible rather than lazy: `Anchor::Eyes` on a *remote* entity
needs `EntityDimensions.eyeHeight`, a per-type field Rewo does not model, so
the production resolver declines and lands on the carried coordinates —
vanilla's own unknown-entity branch, and an error purely of staleness. `Feet`
resolves exactly, from `EntityTable`.

#### `set_default_spawn_position`, and the field that a dimension change resets

`LevelData.RespawnData` is a `GlobalPos` (a dimension **identifier string** —
`ResourceKey.streamCodec`, not a registry id — plus a packed `BlockPos` long)
and two floats. Stored verbatim: `RespawnData.of`'s `wrapDegrees`/`clamp` and
`MAP_CODEC`'s `floatRange` are both off this path, and `STREAM_CODEC` is a bare
composite over the record's accessors.

It lands on `ClientLevelData` beside the difficulty, and **the two behave
oppositely across a dimension change**, which is the finding worth keeping.
`handleRespawn` builds a replacement `ClientLevelData(getDifficulty(),
isHardcore(), isFlat)` — the difficulty is carried across *explicitly*, and the
respawn data is not a constructor parameter at all. What fills it is the
`ClientLevel` constructor one line later:
`setRespawnData(RespawnData.of(dimension, new BlockPos(8, 64, 8), 0, 0))`.

Two consequences, both inverting the guess. **`RespawnData.DEFAULT` — overworld,
`BlockPos.ZERO` — never appears on a client**; the constructor's `(8, 64, 8)`
*of the level being entered* is the real default, and it follows you into the
Nether. And a same-dimension respawn keeps the level data entirely, so **the
world spawn survives death and is discarded by travel** — the reverse of the
intuition that a respawn packet resets respawn state.

`ClientLevelData.respawnData` has no initialiser, so it is briefly null;
`Option<RespawnData>` models that window exactly, and it is unobservable in a
live session because `enter_level` runs on the login packet.

**One scoped exclusion, stated rather than silently skipped.** `setRespawnData`
actually stores `getWorldBorderAdjustedRespawnData(…)`, which relocates a spawn
outside the border onto its centre column via a `MOTION_BLOCKING` heightmap
lookup. Rewo has no world border — `initialize_border` and the five
`set_border_*` packets are all class B — and the default border is ±29,999,984,
which contains every position a world generates. The adjustment is unreachable
here; landing it needs the border packets first, not a guess at its bounds.

#### Verification

`rewo abilityshot --check` grows **47 → 76** witnesses (M75's 47 plus 29), each
naming a mutation partner, driving the production seams `route_player_rotation`
and `route_client_state` with a real resolved `Ids` rather than the readers
underneath — M45's rule, that a gate reimplementing a slice of the app's
dispatch misses whatever the app adds to it. The rotation packets share
`abilityshot` rather than getting a command because they are the subject it
already owns: M75 decided how the local player *moves*, and these decide where
it *looks*.

**32 mutations run, 31 caught, 1 equivalent.** Two did not fail on the first
pass and both were worth the run.

**The sample that did not sit where the mutation bites.** Reordering the pitch
clamp to `offset + clamp(x_rot)` instead of `clamp(offset + x_rot)` survived
every witness. The reason is that any step under 90° leaves `clamp(x_rot)` an
identity, and for an over-range step with a *positive* base both orders
saturate to 90 anyway — so separating them needs a base of the **opposite
sign** to the step. Base -80, step +400: `clamp(320) = 90` the right way round,
`-80 + 90 = 10` the wrong one. This is M75's recorded lesson repeating exactly
one milestone later, and the fix is a witness sitting on the case rather than
straddling it.

**One genuinely equivalent mutant.** Deleting `getPosition`'s `if
(this.atEntity)` test changes nothing, because `parse` sets `at_entity` and
`to_anchor` together and the `and_then` short-circuits to the same fallback for
the point form. The two guards are interchangeable *while that invariant
holds*, so the honest thing to pin is the invariant, not either guard — the
gate now witnesses `at_entity == to_anchor.is_some()` after `parse`, which a
mutation of `parse` does break. The redundant test is kept because it is what
`getPosition` is.

**One property is unreachable from these two packets, and the row says so.**
`setXRot`'s `% 360` before the clamp cannot fire from either: `player_rotation`
arrives already clamped to ±90 by `calculateAbsolute`, and `look_at`'s pitch is
bounded by construction. Dropping it is caught by the `rewo-world` unit test
and **not** by the gate, which is the correct division rather than a gap — the
modulo is a property of `Entity.setXRot`, whose other callers are out of scope
here.

**A witness caught an arithmetic error in my own module docs** on its first
run: the point-form `player_look_at` body is 26 bytes (a one-byte anchor
VarInt, three doubles, the flag), and the docs said 25.

Gates: `abilityshot` 76/76, and the coverage table's machine check
(`ids::coverage_table_tests`) flipped all three rows and both count tables —
it named them precisely, which is what it exists for.

**Not verified.** No live server was run against these three: `rewo play`'s
harness has no `/teleport … facing` step and no `/setworldspawn`, so the
`ServerboundMovePlayerPacket.Rot` reply and the `enter_level` reset on a real
dimension change are unit-true and unobserved on a connection. `CORRECTIONS 0`
is unchanged and says nothing about them — the server does not correct
rotation, so that meter is structurally blind to this milestone in the same way
§6 records it being blind to a dropped knockback.
