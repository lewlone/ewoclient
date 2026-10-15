# M192 plan — the held map in first person

Written 2026-10-15 at the end of the overnight push, for a session that starts
it cold. **Started the same night: see "Where it stopped" directly below.** M190 (`6ff0640`) supplies the data:
`PlaySession::maps` (`rewo_world::maps::MapStore`), `ClientMap::rgba()`
(128 x 128 RGBA, the vanilla palette) and `ClientMap::version` (bumped on every
colour patch). M191 (`cb9dd4a`) put the stack's map id on
`StackComponents::map_id` / `StackDetail::map_id`.

Scope: the map **held in first person**, one- and two-handed, with its
background, its pixels and the arms. **Not** in scope (separate milestones):
item frames, the decoration sprites (`MapRenderer`'s second half), a map's
name labels. Each is named under "Later" so it is not lost.

All line numbers are the 26.2 Vineflower decompile under
`%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/client/renderer/`.
Re-check them before transcribing; the rule in `CLAUDE.md` is to verify a doc
claim before acting on it.

## Where it stopped (2026-10-15, end of the overnight session)

Branch **`claude/m192-held-map`** (not merged into `main`; the orchestrator's
rule is to merge only when M192 is complete and the full gate passes). Six
green slices, each passing clippy `-D warnings` and its tests:

| Commit | Slice | Plan step |
|---|---|---|
| `4b4a072` | `rewo-gpu/src/hand.rs`: `map_tilt`, `map_frame`, `one_handed_map`, `map_hand`, `two_handed_map`, `MAP_BACKGROUND_QUAD`, `MAP_PIXELS_QUAD`, tests `m192_held_map` (hand-derived literals). r69 and the atlas rows claimed in §0.0. | 1, 2 |
| `5e58cc4` | `live_cmd/gui.rs`: `HAND_MAP_*` rects, `HeldMapTextures`, `HeldMapUv`, `pack_hand_atlas(.., map)` (items move to y 128). Test `a_held_map_takes_the_top_band_of_the_hand_atlas`. | 3 (layout) |
| `823c0b3` | `rewo-data` bake: `BakedAssets::map_background` / `map_background_checkerboard`, tested on the real jar. | 3 (textures) |
| `01cb138` | `hand::MapDraw` + `build_map_vertices` (arms via `append_arm`, background quad with vanilla's backwards-looking texture choice, pixel quad when data is present). | 4 (geometry) |
| `fd78676` | `apply_hand` wiring: map branch from the inventory (`has_map_id`; two-handed = main hand + empty off-hand), `session.maps` via `StackDetail::map_id`, repack key `(map id, version, data present)`, the map's hand filtered out of `build_vertices`. | 4 (wiring) |
| *(slice 7)* | Region re-upload: `WorldRenderer::update_hand_region` (+ `hand_atlas_installs` / `hand_region_updates` counters); `apply_hand` re-uploads only the 128x128 band when `map_version_only_changed`. `handshot` gm4 (38 witnesses): a second patch reaches the frame by region update with no full install. | 3 (re-upload), 5.3 |
| `0056380` | `handshot` gm1-gm3 (`EXPECTED_WITNESSES` 34 -> 37), production packer + geometry under validation: palette texels reach the frame, one-handed placement, unknown map = plain background only. | 5 (partly) |

**Still to do, in order:**

1. ~~Region re-upload~~ **done in slice 7.**
2. **Pull the branch decision out of `apply_hand`** into a pure function
   (`held_map_branch(main, off) -> Option<(Arm, two_handed)>`) and test it:
   map + off-hand item = one-handed in the main hand, the item still drawn;
   map in the off-hand = one-handed left; map + empty off-hand = two-handed.
   Then the plan's §3 witness 4 (branch) and witness 3 (re-upload, counting
   `init_hand` calls).
3. **Known deviation to fix or record:** the map is resolved from the held
   stack, not from the equip clock's visible item, so during the swap dip the
   map appears at once where vanilla's `mainHandItem` lags. Route it through
   `EquipHeight::visible_item` plus the stack.
4. **render-check r69** (claimed): count frames whose hand vertices include a
   map quad, read back from the renderer; the owner runs
   `tools/render_check.py` (it opens a window).
5. gm1-gm3 run without a skin, so the map arms are not pixel-checked; add a
   skin-bearing witness that the two map arms land left and right of the map.
6. Full `cargo test`, `python tools/gates.py`, `tools/numbers.py`, a §15
   entry, then merge the branch into `main` (`--no-ff`) and note it in the
   overnight summary.

Later, unchanged: item frames, decoration sprites, lighting (see the end of
this file).

## 1. Vanilla, in the order it runs

### 1.1 The branch (`ItemInHandRenderer.java:440-452`)

```java
if (itemStack.isEmpty()) { ...bare arm... }
else if (itemStack.has(DataComponents.MAP_ID)) {
   if (isMainHand && this.offHandItem.isEmpty())
      this.renderTwoHandedMap(poseStack, ..., xRot, inverseArmHeight, attack);
   else
      this.renderOneHandedMap(poseStack, ..., inverseArmHeight, arm, attack, itemStack);
}
```

The test is `has(MAP_ID)`, not "is a filled_map": use `ItemSlot::has_map_id`
(M93f). Two-handed only for the **main** hand with an **empty off-hand**;
the off-hand's own pass then draws nothing (its stack is empty). A map in the
off-hand, or in the main hand with something in the off-hand, is one-handed.
This replaces the ordinary item path entirely: no item model, no glint.

### 1.2 `renderOneHandedMap` (`:173-203`)

```
invert = arm == RIGHT ? 1 : -1
translate(invert * 0.125, -0.125, 0)
if !invisible: push; rotZ(invert * 10°); renderPlayerArm(..., inverseArmHeight, attack, arm); pop
push
translate(invert * 0.51, -0.08 + inverseArmHeight * -1.2, -0.75)
s = sqrt(attack); xSwing = sin(s*PI)
translate(invert * (-0.5 * xSwing), 0.4*sin(s*2PI) - 0.3*xSwing, -0.3*sin(attack*PI))
rotX(xSwing * -45°); rotY(invert * xSwing * -30°)
renderMap(...)
pop
```

`renderPlayerArm` is the existing bare-arm pose (`hand::player_arm`), drawn
inside the extra `rotZ(invert * 10°)` and the leading translate.

### 1.3 `renderTwoHandedMap` (`:205-232`)

```
s = sqrt(attack)
translate(0, -(-0.2*sin(attack*PI)) / 2, -0.4*sin(s*PI))
tilt = calculateMapTilt(xRot)
translate(0, 0.04 + inverseArmHeight * -1.2 + tilt * -0.5, -0.72)
rotX(tilt * -85°)
if !invisible: push; rotY(90°); renderMapHand(RIGHT); renderMapHand(LEFT); pop
rotX(sin(s*PI) * 20°)
scale(2, 2, 2)
renderMap(...)
```

`calculateMapTilt` (`:145-149`):
`tilt = clamp(1 - xRot/45 + 0.1, 0, 1); return -cos(tilt*PI)*0.5 + 0.5`.
`xRot` is the camera pitch (the same one the view bob uses).

### 1.4 `renderMapHand` (`:151-171`)

```
invert = arm == RIGHT ? 1 : -1
push
rotY(92°); rotX(45°); rotZ(invert * -41°)
translate(invert * 0.3, -1.1, 0.45)
avatarRenderer.renderRightHand / renderLeftHand(..., skin, sleeve shown)
pop
```

`renderRightHand`/`renderLeftHand` draw the arm box (and the sleeve layer
when the model part is shown) at the model's rest pose; Rewo's bare-arm
geometry in `hand.rs` (`ArmGeometry`, the arm quads near `:800`) already draws
that box from the skin — reuse its quad builder under this pose rather than
re-deriving the box. Check how the existing bare arm handles the sleeve
layer and do the same.

### 1.5 `renderMap` (`:234-250`)

```
rotY(180°); rotZ(180°); scale(0.38); translate(-0.5, -0.5, 0); scale(1/128)
background quad: (-7,135)..(135,-7) at z 0, UV 0..1,
   texture = data == null ? map_background.png : map_background_checkerboard.png
if data != null: MapRenderer.render(state, poseStack, ..., showOnlyFrame=false)
```

Note the texture choice reads **backwards** and is transcribed as written:
the plain background when there is **no** data, the checkerboard when there
is. `map_background.png` is 64 x 64, `map_background_checkerboard.png` is
128 x 128 (measured from the jar's PNG headers).

`MapRenderer.render` (`MapRenderer.java:32-44`): the map texture as one quad
(0,128)..(128,0) at **z -0.01**, UV 0..1. Decorations follow (`:45-...`) and
are out of scope here.

Both quads use `RenderTypes.text(...)`: textured, alpha-tested, lit by
`lightCoords`. Rewo's hand pass carries a per-vertex `shade` and no lightmap;
match whatever the bare arm does today and record the difference if any.

## 2. Rewo design

### 2.1 Atlas layout (the hand atlas, `live_cmd/gui.rs:250-252, 364`)

The hand atlas is 512 x 512, packed in 64 px slots (`GUI_ATLAS_SLOT`), the
bottom row (`HAND_SKIN_Y` = 448) reserved for the skin. While a map is held,
pack it as:

| Region | Rect (x, y, w, h) | Content |
|---|---|---|
| map pixels | (0, 0, 128, 128) | `ClientMap::rgba()`, or transparent when the map is unknown |
| checkerboard | (128, 0, 128, 128) | `textures/map/map_background_checkerboard.png` |
| plain background | (256, 0, 64, 64) | `textures/map/map_background.png` |
| item slots | from (0, 128) on, 64 px | the off-hand item (one-handed case), as today |
| skin | (0, 448, 64, 64) | unchanged |

Claim the layout in `REWO_PLAN.md` §0.0's allocation table (it is a
merge-silent resource) in the same commit that adds it.

**Re-upload rule.** Today the atlas is rebuilt when the wanted item set
changes (`gui.rs:440-455`, `wanted != state.resident`). Add the held map's
`(map_id, version)` to that residency key, and for a version change alone
re-upload only the 128 x 128 region with `rewo_gpu::entities::upload_region`
(async, no fence; `entities.rs:5761`) instead of rebuilding the pass. A
rebuild destroys the glint setup, which `gui.rs` already re-installs after
`init_hand`; keep that order.

### 2.2 Geometry (`rewo-gpu/src/hand.rs`)

- Add `HandDraw::map: Option<HeldMap>` with `HeldMap { two_handed: bool,
  data_present: bool, uv_map, uv_background }` (atlas rects), resolved by the
  caller like `item` is.
- New pure functions beside `item_hand_kind`, each a `Mat4` chain in vanilla's
  call order (`glam` post-multiplies like `PoseStack`):
  `map_tilt(x_rot)`, `one_handed_map(arm, inv_h, attack) -> (arm_pose,
  map_pose)`, `two_handed_map(x_rot, inv_h, attack) -> (map_pose, [right_arm,
  left_arm])`, `map_hand(arm)`, `map_quads(map_pose) -> background, pixels`.
- `build_vertices` gets a `map` branch ahead of the item branch, mirroring
  §1.1; the two-handed case must make the off-hand draw nothing.
- No glint for a map (`build_glint_vertices` skips it).

### 2.3 Wiring (`rewo-app`)

Where the frame resolves the main- and off-hand stacks for the hand pass
(`live_cmd/gui.rs`, the `HandDraw` construction): if the stack
`has_map_id`, look up `session.maps.get(map_id)` via the stack's
`StackDetail::map_id` and build `HeldMap`. `xRot` for the tilt is the camera
pitch already passed to the view bob.

## 3. Verification (offscreen; no window needed)

Extend `rewo handshot --check` (`rewo-app/src/handshot_cmd.rs`, currently 34
witnesses, pinned by `EXPECTED_WITNESSES`) with a map section, following its
three layers:

1. **Placement**, no GPU: drive `two_handed_map` / `one_handed_map` and
   compare against an independent derivation written in the gate from
   §1.2-§1.5's literals (not from the functions under test; M187's lesson,
   where a witness reading the builder's own constant let a mutant survive).
   Pin `map_tilt` at pitch -90, 0, 45 and 90 (clamps at both ends).
2. **Pixels**: render a two-handed map whose `MapStore` holds a known patch
   (e.g. a 2 x 2 block of GRASS NORMAL = (109, 153, 48), M190's test value)
   through the production pass, and find those exact texels in the frame; a
   control render with the map unknown must show the plain background
   instead and none of the patch colour.
3. **Re-upload**: apply a second patch (bump `version`), render again, and
   require the new colour where the old one was, with the atlas not rebuilt
   (count `init_hand` calls).
4. **Branch**: map in the main hand with an item in the off-hand draws the
   one-handed map plus the off-hand item; map in the off-hand is one-handed;
   both through the production `build_vertices`.

Then add one `live --render-check` row (claim the next free `rNN`, currently
**r69**) that a held map's quads reached the windowed hand pass, read back
from the renderer, and note it for the owner to run (`tools/render_check.py`
opens a window). Run `python tools/gates.py` (pinned set) and `cargo test`;
`tools/numbers.py` refreshes §0.0.

## 4. Order of work

1. §1 re-read against the decompile; claim the atlas rows and `r69` in §0.0.
2. `hand.rs` pure pose functions + their unit tests (literal derivations).
3. `pack_hand_atlas` layout + the `(map_id, version)` residency key + the
   region re-upload.
4. `build_vertices` map branch; `gui.rs` wiring from `session.maps`.
5. `handshot` map witnesses (placement, pixels, re-upload, branch); bump
   `EXPECTED_WITNESSES`.
6. render-check r69; `REWO_PLAN.md` §15 entry; gates + tests + numbers.

## Later (not M192)

- Item frames holding a map (`ItemFrameRenderer`, which also needs item-frame
  entities drawn at all; check first).
- Decoration sprites (`MapRenderer.java:45-...`, the `map/decorations/*`
  atlas, `rot * 360 / 16`, the per-decoration z step of -0.001) and banner
  names (scaled text at z -0.025).
- `ItemInHandRenderer`'s lighting: `RenderTypes.text` samples the lightmap at
  `lightCoords`; Rewo's hand pass has no lightmap input today.
