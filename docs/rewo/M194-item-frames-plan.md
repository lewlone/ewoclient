# M194 plan — item frames (the entity, its frame, its item, a map in it)

Written 2026-10-15 at the end of the overnight push, for a session that may
start it cold. Branch: `claude/item-frames` (from `main` at `9352e5d`).
See "Where it stopped" for what is built.

Rewo draws no item frames at all today: the entity is tracked like any other
(`EntityTable`) but nothing reads its facing, item or rotation, and no
renderer draws the frame. Maps are ready (M190 data, M192 held-map drawing,
M193 decorations); dropped items already draw in world space through the
`ItemEntityRenderer` path (`rewo-gpu/src/entities.rs` ~`:4807`, M24b).

All references are the 26.2 Vineflower decompile under
`%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/`. Verify before
transcribing.

## 1. Vanilla

### 1.1 The entity's data

- `ItemFrame extends HangingEntity extends BlockAttachedEntity extends Entity`.
  `Entity` defines slots 0-7, `HangingEntity` one (`DATA_DIRECTION`, slot 8,
  serializer DIRECTION), `BlockAttachedEntity` none, so `ItemFrame.DATA_ITEM`
  is **slot 9** (ITEM_STACK) and `DATA_ROTATION` **slot 10** (INT, `% 8` on
  set). Calibrate against a real server's metadata before trusting the count
  (the project rule for polymorphic indices, `.claude/rules/rewo-net.md`).
- `recreateFromPacket` (`ItemFrame.java:407-409`): the facing also comes from
  `ClientboundAddEntityPacket.data` via `Direction.from3DDataValue` (0 down,
  1 up, 2 north, 3 south, 4 west, 5 east). Both sources set the same field.
- Types: `minecraft:item_frame` and `minecraft:glow_item_frame` (glow: full
  brightness for the item and map, `ItemFrameRenderer.getLightCoords`).
- `getFramedMapId(stack)`: the stack's `MAP_ID` (M191 carries it on
  `StackDetail::map_id`).

### 1.2 `ItemFrameRenderer.submit` (`client/renderer/entity/ItemFrameRenderer.java:44-97`)

```
translate(-renderOffset)             // renderOffset = (stepX*0.3, -0.25, stepZ*0.3), :103-105
translate(step * 0.46875)            // toward the wall it hangs on
horizontal: xRot 0, yRot 180 - direction.toYRot()
vertical:   xRot -90 * axisDirection.step, yRot 180
rotX(xRot); rotY(yRot)
frame model (unless invisible): push; translate(-0.5,-0.5,-0.5); frame block model; pop
translate(0, 0, invisible ? 0.5 : 0.4375)
if map with data:
   rotZ((rotation % 4 * 2) * 360/8); rotZ(180); scale(1/128); translate(-64,-64,0); translate(0,0,-1)
   MapRenderer.render(state, ..., showOnlyFrame = TRUE, light)   // pixels + decorations with showOnItemFrame
else if item:
   rotZ(rotation * 360/8); scale(0.5); item with ItemDisplayContext.FIXED
```

Note the map rotates in quarter turns (`rotation % 4 * 2` eighths) and the
item in eighths. `showOnlyFrame = true` keeps only decorations whose type has
`showOnItemFrame` (M193's table carries the flag: player arrows are false,
frame markers and banners true).

### 1.3 The frame model (`extractRenderState`, `:121-145`)

`blockModelResolver.updateForItemFrame(frameModel, isGlowFrame, hasMap)`:
the block models `block/item_frame`, `block/glow_item_frame`,
`block/item_frame_map`, `block/glow_item_frame_map` (check the exact names in
the jar's `assets/minecraft/models/block/`). They are ordinary block-model
JSON (elements + textures), so Rewo's block-model baker can produce their
quads; they are drawn with the entity's light, not as world blocks.

## 2. Rewo design (proposed)

1. **Data** (`rewo-world` + `rewo-net`): an `item_frames` side map on
   `EntityTable` keyed by entity id: `ItemFrameState { facing: u8, rotation:
   u8, item: Option<ItemSlot fingerprint/id>, glow: bool }`, filled from
   `add_entity`'s data (facing) and metadata slots 8 (facing), 9 (item), 10
   (rotation) for the two frame types only. Classify it in
   `clear_side_state` and the test's `side_rows` (`.claude/rules/rewo.md`:
   every side map is cleared in one place). Unit-test the decode against the
   slot numbers above.
2. **Placement** (`rewo-gpu`, pure): `item_frame_pose(facing) -> Mat4` (the
   chain to the frame model's origin), `item_frame_item_pose(facing,
   rotation, invisible)`, `item_frame_map_pose(facing, rotation)`, each tested
   against hand-derived points (e.g. a north-facing frame's map centre is
   0.5 - 0.46875 - 1/128 from the block face).
3. **Frame model**: bake the four block models once (where the item models
   are baked) and draw them through the entity pass at the frame pose.
4. **The item**: reuse the dropped-item path with the FIXED display
   transform (bake `display.fixed` beside `ground` in `held.rs` if it is not
   there) at `item_frame_item_pose`.
5. **The map in a frame**: the entity pass needs the map's pixels; give it a
   per-frame map atlas like M192's hand band (or share one map atlas), and
   draw `MAP_PIXELS_QUAD` + decorations filtered by `showOnItemFrame`.
6. **Gates**: an offscreen `itemframeshot` (or an extension of an existing
   entity gate) that places a frame facing each of the six directions and
   checks the frame quads, an item and a map land where §1.2 says; pixel
   witnesses with literal positions (M187's lesson). A render-check row when
   a windowed run is possible.

## 3. Order of work

1. Data decode + tests. 2. Placement functions + tests. 3. Frame model bake +
draw. 4. Item in frame. 5. Map in frame (+ decorations with
`showOnItemFrame`). 6. Gate, §15 entry, full gate, merge.

## Where it stopped

Branch `claude/item-frames` (not merged).

| Commit | Slice | Plan step |
|---|---|---|
| `c47dffe` | This plan. | — |
| *(slice 1)* | `rewo-gpu/src/item_frame.rs`: `facing_step`, `frame_base`, `frame_model_pose`, `item_pose`, `map_pose` (the §1.2 chain, relative to the entity; the dispatcher's render offset and `submit`'s `-renderOffset` cancel), tests over all six facings (contents at `step/32`, a map `1/128` further out, flush when invisible; quarters vs eighths). | 2 |

| *(slice 2)* | Step 1, the data: `EntityState::spawn_data` (add_entity's trailing VarInt, read in `read_add_entity`); metadata slots `(8, DIRECTION=12)`, `(9, ITEM_STACK)` -> `FrameItem { item_id, map_id, foil }`, `(10, INT)`; `EntityTable::item_frames` side map (cleared in `clear_side_state`, dressed in the side-row test) with `set_frame_*` and `item_frame(id) -> (facing, rotation, item)` (facing falls back to spawn data); `EntityClasses::is_item_frame` gates the slots to `item_frame`/`glow_item_frame`. Tests in `rewo-world` and `rewo-net`. Not yet calibrated against a real server's metadata. | 1 |

| `a7be406` (2026-10-18) | Steps 3-6: `display.fixed` on every item model; the four frame models baked as `rewo:be/*`; `BlockEntityDraw::fixed_display`; the entity atlas's framed-map band (`ATLAS_H` 1600 -> 1728) with LRU map slots and the decoration sprites; `FrameMapDraw`; `live_cmd/item_frames.rs` (collector + draw builder, glow lights); no capsule for frames; the `itemframeshot` gate (21 witnesses). | 3-6 |
| *(next commit)* | render-check r69 (M192's held map, injected into the off-hand), r70 (an injected frame reaches the windowed collector, no capsule) and r71 (`render_check.py` summons a real frame: slots 8-10 calibrated against the server). | 6 |

**M194 is complete**; see `REWO_PLAN.md` §15. Deviations recorded there: no
foil on a framed item, seven framed maps on screen at most, the glow map's
light taken at block level 13, no name tag for a named framed item.
