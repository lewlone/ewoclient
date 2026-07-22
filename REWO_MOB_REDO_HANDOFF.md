# REWO — Mob Model Redo: Handoff

**Status:** the in-game mob models render with **scrambled textures**. Silhouette,
proportions, and dominant colour are roughly right; the actual texture-to-face
mapping is wrong (the cow has no readable face, sides/top are rotated/mirrored).
This document is a self-contained brief to redo **all** mob models correctly.

**Why this exists:** the previous pass verified mobs by eyeballing *silhouette +
colour* ("pink & squat = pig") and called that "verified." That check cannot
catch a scrambled UV map — a cow can have the right outline and a garbled face and
still pass. The redo must verify **texture-face correspondence**, not just shape.
See "Verification (mandatory)" below — that section is the point of the whole doc.

---

## 1. The bug, precisely

All mob geometry funnels through one function that unwraps a box into 6 textured
quads:

- `crates/rewo-gpu/src/entities.rs :: box_uv_faces(min, max, size, uv, off_x, off_y)`

Every mob uses it:
- **humanoid** (player / zombie / husk / drowned): `humanoid_cuboids()` → `cuboid_quads()` → `box_uv_faces`
- **quadruped** (cow / pig / sheep): `quadruped_model_quads()` / `sheep_model_quads()` → `build_quad_parts()` → `box_uv_faces`
- **slime**: `slime_model_quads()` → `box_uv_faces`

`box_uv_faces` is a **hand-rolled approximation** of Minecraft's box-UV unwrap. It
is wrong in at least three independent ways, and because everything shares it,
**every mob is wrong**:

1. **Face → texture-sub-rect assignment is wrong.** In vanilla the *front face*
   (where a cow/pig's eyes+nose live) is **NORTH = −Z**, sampling the sub-rect
   `[u1..u2] × [v1..v2]` (see §3). `box_uv_faces` puts that rect on its **+Z**
   quad and calls it "front". Combined with the crate's `(-x, 24-y, -z)` vanilla→
   local conversion (which negates Z), the front/back and the face texture land on
   the wrong world-faces.
2. **Per-face vertex → UV-corner ordering is wrong.** Vanilla assigns each of a
   quad's 4 vertices to a *specific* UV corner, and the assignment **differs per
   face** (that is how it encodes each face's rotation/flip — e.g. the UP face is
   vertically flipped, `v1→v0`). `box_uv_faces` uses one uniform TL/TR/BR/BL
   mapping for all six faces, so side/top/bottom faces are rotated or mirrored.
3. **No mirror handling, no UP-face flip.** Vanilla's `mirror` reverses vertex
   order and swaps X; UP uses `(u2,v1,u22,v0)` (note `v1` before `v0`). None of
   that is reproduced.

The geometry (vertex *positions*) is approximately correct — that's why
silhouettes look plausible. It's the **UV mapping** that's scrambled.

---

## 2. Recommended fix — "guaranteed to work"

Do **not** patch `box_uv_faces` incrementally. Port Minecraft's actual box builder
faithfully — it *is* the spec, it's ~40 lines, and it makes geometry+UV consistent
by construction:

1. Port `ModelPart.Cube` (§3) as a function that, given `(texOffX, texOffY, minX,
   minY, minZ, width, height, depth, grow, mirror, texW, texH)`, returns **8
   vertices** and **6 quads** with the *exact* vertex arrays and UV rects vanilla
   uses. Keep MC's coordinate system (feet-DOWN +Y, front −Z, +X east). Do the UV
   remap exactly as `Polygon` does (`vertices[0..3]` → `(u1,v0)(u0,v0)(u0,v1)(u1,v1)`).
2. Build each model in **MC-local coordinates** from its vanilla source (§5): the
   `PartPose.offset/offsetAndRotation` and `CubeListBuilder` calls translate
   directly. Apply each part's local rotation about its pivot, then the part
   offset — same as `PoseStack`.
3. Convert to this renderer's world space **once, at the very end** (or better:
   change the world/entity vertex convention to match MC and drop the conversion).
   The current `(-x, 24-y, -z)` hack scattered inside model builders is a big part
   of why faces flip — centralise it.
4. **Delete** `box_uv_faces`, `cuboid_quads`, `quadruped_model_quads`,
   `sheep_model_quads`, `slime_model_quads`, `humanoid_cuboids` and rebuild each
   model on the faithful `Cube`. The animation/rotation layer (`emit_model`,
   `LimbPart`, the walk-swing + head-look) is **correct and reusable** — it
   operates on already-built quads and is orthogonal to the UV bug. Keep it.

The one subtlety to preserve: vanilla builds inflated overlay layers (hat/jacket
for humanoid, wool for sheep) as separate cubes with a `grow`/`CubeDeformation` —
the port must support `grow` (inflate min/max, keep the base UV size).

---

## 3. Ground truth — vanilla `ModelPart.Cube` (26.2, verbatim)

`%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/client/model/geom/ModelPart.java`,
class `Cube` (line ~239) and record `Polygon` (line ~342):

```java
// grow = CubeDeformation (inflate). mirror swaps X.
maxX = minX+width; maxY=minY+height; maxZ=minZ+depth;
minX-=growX; minY-=growY; minZ-=growZ; maxX+=growX; maxY+=growY; maxZ+=growZ;
if (mirror) { swap(minX, maxX); }

// 8 corners. t* = minZ (NORTH side), l* = maxZ (SOUTH side).
t0=(minX,minY,minZ) t1=(maxX,minY,minZ) t2=(maxX,maxY,minZ) t3=(minX,maxY,minZ)
l0=(minX,minY,maxZ) l1=(maxX,minY,maxZ) l2=(maxX,maxY,maxZ) l3=(minX,maxY,maxZ)

// UV columns/rows
u0=texX;  u1=texX+d;  u2=texX+d+w;  u22=texX+d+2w;  u3=texX+d+w+d;  u4=texX+d+2w+d;
v0=texY;  v1=texY+d;  v2=texY+d+h;

// 6 faces: Polygon(verts[4], u0,v0,u1,v1, texW,texH, mirror, facing)
DOWN : {l1,l0,t0,t1}  rect (u1,v0,u2,v1)
UP   : {t2,t3,l3,l2}  rect (u2,v1,u22,v0)   // note v1 then v0 → vertical flip
WEST : {t0,l0,l3,t3}  rect (u0,v1,u1,v2)    // -X
NORTH: {t1,t0,t3,t2}  rect (u1,v1,u2,v2)    // -Z  ← the FRONT / face
EAST : {l1,t1,t2,l2}  rect (u2,v1,u3,v2)    // +X
SOUTH: {l0,l1,l2,l3}  rect (u3,v1,u4,v2)    // +Z  ← the BACK

// Polygon remap: each quad's 4 verts → these UV corners (u/texW, v/texH):
verts[0] = (u1, v0)   // passed-in (u0,v0,u1,v1) = sub-rect top-left..bottom-right
verts[1] = (u0, v0)
verts[2] = (u0, v1)
verts[3] = (u1, v1)
if (mirror) reverse(verts)
```

Reproduce this **exactly** (columns, per-face vertex arrays, remap order, UP flip,
mirror). MC textures are authored to this unwrap; nothing else will line up.

---

## 4. What exists today (inventory — file:symbol)

Renderer (crate `rewo-gpu`):
- `src/entities.rs` — the whole entity pass. Key symbols:
  - `EntityPass::new(gpu, fmt, font, EntityTextures)` — bakes the atlas, builds all model quad lists once.
  - `EntityTextures { skin, slime, zombie, cow, pig, sheep, sheep_wool }` — borrowed mob skins.
  - Atlas: `ATLAS_W=256, ATLAS_H=256`. Slots: font `(0,0)-(128,128)`; player skin `(128,0)`; slime `(128,64)` 64×32; zombie `(192,0)`; cow `(192,64)`; pig `(0,128)` 64×64; sheep body `(64,128)` 64×32; sheep wool `(64,160)` 64×32. `blit_tex(atlas, px, x, y, w, h)`.
  - `EntityModelKind { Player, Zombie, Cow, Pig, Sheep, Slime, Capsule }`.
  - `set_draws(&[EntityDraw], cam_right, cam_up)` — dispatches kind→model→`emit_model`, appends nametags, uploads to a 2-slot ring.
  - `emit_model(verts, d, quads, scale, arm_forward)` — **KEEP**: applies per-part pivot rotation (walk-swing + head pitch + head-yaw) then whole-model yaw, then `d.pos + p*scale`. This is correct (the head-look + limb-swing live here). `LimbPart` enum drives the pivots.
  - **BROKEN, replace:** `box_uv_faces`, `cuboid_quads`, `humanoid_cuboids`, `quadruped_model_quads`, `build_quad_parts`, `sheep_model_quads`, `slime_model_quads`.
  - Pipeline facts: `cull_mode = NONE`; reversed-Z depth (GREATER, clear 0.0), solid writes depth; **alpha-test / discard** on the texel alpha (transparent texels dropped — this is how overlay layers show the base beneath); vertex = `pos[3] + uv[2] + rgba[4]`; shade is a baked per-face constant in `PlayerQuad.shade`.
  - Model scales (model-px → world blocks): player `0.9375/16`, zombie/cow/pig/sheep `1/16`, slime `1/8`.
- `src/world.rs` — `WorldRenderer::init_entities(gpu, font, EntityTextures)`, `set_entities`, `set_camera`. Entities are **live-only** (never called by `demo`/`view`/`bench`), so those regression gates are unaffected by mob work.
- `shaders/entity.vert` / `entity.frag` — the entity shader pair (glGLSL, compiled by build.rs via glslc).

Assets (crate `rewo-data`):
- `src/assets.rs :: bake()` loads mob skins: `bake_entity_tex(jar, "entity/<mob>/<file>.png", w, h)`. Returns `BakedAssets { player_skin, slime_tex, zombie_tex, cow_tex, pig_tex, sheep_tex, sheep_wool_tex, … }`. Textures are RGBA, indexed-PNG expanded.

App (crate `rewo-app`):
- `src/live_cmd.rs :: collect_entities()` — maps `minecraft:<name>` → `EntityModelKind`, builds `EntityDraw { pos, width, height, color, name, kind, yaw, head_yaw, pitch, limb_swing, limb_amount }`.
- `entity_textures(&baked)` — packs skins into `EntityTextures`.

Coordinate conventions (current, and the source of much confusion):
- **World/mesh** vertices: standard MC-ish world space, +Y up.
- **Model builders**: authored in vanilla local coords (feet-DOWN +Y, front −Z) then converted per-vertex to "feet-UP +Y, front +Z" via `[-v[0], 24.0 - v[1], -v[2]]`. `emit_model` then scales + places. **Recommendation:** pick ONE convention (ideally MC's own) and convert exactly once.

---

## 5. Per-mob vanilla model sources (decompile paths)

Base: `%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/client/model/`
- **Humanoid** (player/zombie/husk/drowned): `HumanoidModel.java` — 6 base cubes + 6 inflated overlay cubes (hat/jacket/sleeves/pants). Player uses `PlayerModel` (wide vs slim). Texture 64×64.
- **Quadruped base**: `QuadrupedModel.java` (`createBodyMesh(legHeight, cubeDeformation)`), head added per-mob.
  - **Cow**: `animal/CowModel.java` (head 8×8×8, texOffs 0,0; horns; legSize 12). Texture `cow_temperate.png` 64×64.
  - **Pig**: `animal/PigModel.java` (head 8×8×8 + snout texOffs 16,16 addBox(-2,0,-9,4,3,1); legSize 6). Texture 64×64.
  - **Sheep**: `animal/sheep/SheepModel.java` (head 6×6×8 texOffs 0,0 @ (0,6,-8); body 8×16×6 texOffs 28,8; legSize 12) + `SheepFurModel.java` wool overlay (head 6×6×6 grow 0.6; body 8×16×6 grow 1.75; legs 4×6×4 grow 0.5). Textures `sheep.png` + `sheep_wool.png`, each 64×32. Wool is dye-tinted in vanilla; white is fine for v1.
- **Slime**: `SlimeModel.java` — outer 8³ cube (translucent shell) + inner cube + eyes/mouth. Texture `slime.png` 64×32.

Get any model's exact boxes with:
`grep -A3 "texOffs\|addBox\|PartPose" <Model>.java`

Cross-check the texture layouts on the wiki (minecraft.wiki entity model pages) or
by opening the `.png` next to a face-labeled render.

---

## 6. Verification (MANDATORY — this is the part that failed before)

**Silhouette + colour is NOT sufficient.** You must confirm each world-face shows
the correct texture region. Use one or both:

1. **Debug-texture pass (definitive, automated).** Temporarily replace each mob's
   atlas region with a **face-labeled test texture** (each of the 6 box-UV
   sub-rects filled with a distinct solid colour, or the letters F/B/L/R/U/D).
   Render the mob from front, left, and top; assert the front quad is the "front"
   colour, etc. This catches every rect/rotation error deterministically and needs
   no human eye.
2. **Reference comparison.** Render each mob from a fixed front and side angle;
   compare face-by-face against a real vanilla client screenshot (or the wiki
   model). The cow's face (eyes/muzzle) must be on the front of the head, upright.

Headless render harness (already built, in `rewo-app/src/live_cmd.rs`):
- `rewo live --out X.png` renders one headless frame. Env knobs:
  - `REWO_USERNAME=RewoOp` (op'd on the test server, so `/summon` works)
  - `REWO_SUMMON=<mob>` `REWO_SUMMON_NBT="{NoAI:1b,Rotation:[180f,0f]}"` `REWO_SUMMON_DIST=4` `REWO_SUMMON_DY=<float>`
  - `REWO_PRECMD="kill @e[type=!player]"` (clear the scene first)
  - `REWO_LOOK_AT="x,y,z"` (deterministic camera aim) or `REWO_LOOK_ENTITY=1 REWO_LOOK=<kind> [REWO_LOOK_HIGH]`
  - `REWO_FORCE_HEAD=<deg>` `REWO_FORCE_LIMB="swing,amt"` (pin pose deterministically)
- **Always rebuild the profile you run** (`cargo build --release -p rewo-app` then run `target/release/rewo.exe`) — a stale debug/release binary silently runs old code (this bit twice already).

Test server (offline 26.2, already set up):
- Dir `%APPDATA%/EwoClient/rewo/26.2/testserver/`, jar `../server.jar`, JDK
  `%APPDATA%/EwoClient/jdks/temurin-25/bin/java.exe`, port 25599, `online-mode=false`,
  op `RewoOp`. Start: `(cd testserver && java -Xmx2G -jar ../server.jar --nogui)`.
  Currently `spawn-monsters/animals/npcs=false` for clean summon shots (a fresh
  regen still world-gens some passive mobs; `kill @e[type=!player]` once + spawning
  off keeps it clean).

Regression gates that must stay green (mob work is live-only, so these should be
untouched — if they move, something leaked out of the entity pass):
- `cargo test -p rewo-gpu -p rewo-data -p rewo-app -p rewo-world -p rewo-net`
- `rewo demo --out d.png` → **byte-identical** md5 `ee6e26f475178bbfbe9df418a7f5b6db`
- `rewo bench --replay <m1-soak.rewo> --frames 400` → flat percentiles
- 0 Vulkan validation errors (VUIDs) in any run

---

## 7. Keep / don't-touch (working infra) + traps

**Correct and reusable — do NOT rewrite:**
- `emit_model` rotation layer: per-part pivot (`LimbPart`), diagonal walk-swing,
  head pitch, head-yaw (mobs look at players). Operates on built quads; orthogonal
  to the UV bug.
- `EntityTextures` struct, the atlas bake/`blit_tex`, the 2-slot upload ring, the
  entity pipeline (cull NONE + reversed-Z + alpha-test), nametag billboards.
- The `EntityModelKind` dispatch + `collect_entities` name mapping.

**Traps (learned the hard way):**
- **`sin(yaw)` scale shadow (already fixed, don't reintroduce):** in `emit_model`
  the yaw sin/cos must NOT be named `s`/`c` — `s` is the model scale used in
  `d.pos + p*s`. It's `(cyaw, syaw)` now. Shadowing it scales every mob by
  `sin(yaw)` (mobs vanish near yaw 0/±180).
- **cull NONE** means winding won't hide a back-facing model, but it also means a
  wrong-winding face won't self-cull — you can't rely on culling to catch errors.
- **Reversed-Z**: overlay layers (wool/jacket) sit *outside* the base via `grow`;
  they win the depth test because closer = larger depth. Keep the inflate.
- **Indexed PNGs**: entity textures may be palette-indexed; `bake_entity_tex`
  already expands to RGBA — keep that.
- **Stale binary**: see §6. Rebuild before every verification render.

---

## 8. Suggested order of work

1. Port `Cube` + `Polygon` faithfully (§3). Unit-test it: build a 1×1×1 cube at a
   known texOffs and assert the 6 faces' UVs equal the hand-computed vanilla rects.
2. Bring up **one** mob end-to-end (cow is the reference the user called out) on
   the new `Cube`, and verify with the §6 debug-texture pass — front face upright,
   all six faces correct — before touching the others.
3. Roll the remaining models (pig, sheep+wool, humanoid+overlay, slime) onto the
   new builder; verify each the same way.
4. Re-confirm the regression gates in §6 (should be untouched).

Ground truth is the decompiled `.java` under
`%APPDATA%/EwoClient/rewo/26.2/decompiled/…` — it is on-disk and authoritative.
Match it exactly; do not approximate.
