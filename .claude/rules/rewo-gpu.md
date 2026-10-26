---
paths:
  - "crates/rewo-gpu/**"
  - "crates/rewo-mesh/**"
  - "crates/rewo-app/src/*shot_cmd.rs"
  - "crates/rewo-app/src/live_cmd/**"
  - "crates/rewo-app/src/demo_cmd.rs"
  - "crates/rewo-app/src/bench_cmd.rs"
  - "REWO_VELVET_UI_PLAN.md"
---

# Rewo rendering — Vulkan passes, meshing, models, HUD

Reasoning per rule: `docs/rewo/milestones-*.md`. Measurements: `REWO_PLAN.md` §0.0.

**`WorldRenderer` is a façade over per-subsystem modules** in
`crates/rewo-gpu/src/world/` (`arena`, `cull`, `terrain`, `textures`,
`lightmap`, `sky`, `gamma`, `frame`, `lines`, `scene_passes`, `entity_passes`,
`gui_passes`, `depth`, `resources`). `world.rs` holds the struct, `SceneFrame`
and the module map in its header; a subsystem's state is a sub-struct owned by
its module (`Arena`, `Culler`, `TerrainPass`, `TextureArray`, `LightmapRing`,
…), and `rewo_gpu::world::X` paths are kept by `pub use` re-exports. New work
goes into the module that owns the subsystem, not back into `world.rs`.

## Coordinate, depth and geometry conventions

- **The mesher emits WORLD-space vertices.** The shader must not add a column
  origin; the double-add was the real M4 "far-field holes" bug.
- **Collision uses `baked.solid`, not `matches!(Cube)`.** `grass_block` renders
  as a Model, and keying off the render fast path drops the player through the
  ground.
- 26.x model textures can be `{sprite, force_translucent}` objects as well as
  strings.
- **`rotationXYZ` is not `rotateZYX`.** JOML `Quaternionf.rotationXYZ(x, y,
  z)` (item display transforms, `Direction.getRotation()`) is `M = Rx·Ry·Rz`,
  turning a vector about Z first; a `ModelPart`/`PartPose` rotation
  (`rotateZYX`) is `M = Rz·Ry·Rx`. Three helpers had the part order under the
  other name until M242. Use `be_transform::rot_xyz` / `rot_zyx`,
  `held::rotate_xyz` or `mobs::rotate_zyx` by what the Java calls.
- **GUI item depth**: in item space +Z faces the viewer, so a larger z is a
  greater (nearer) reversed-Z depth (`gui_item.vert`, fixed M242).
- **GUI item light** (M245): the normal goes through the display transform
  and the slot's y flip (`gui_normal`), and vanilla's gamma-space diffuse is
  decoded before it multiplies linear colour.
- **The entity atlas pages two sizes** (M246): 16×16 item sprites into
  the item pool, anything else up to 64×64 (every block-entity sheet) into
  the large band. A texture that fits neither draws nothing. A new texture
  path needs a pixel witness, not a draw-list one: block entities drew no
  pixels from M25b to M246 with every witness green.
- **`BakedAssets::models` is indexed by model, not by state**: a state
  reaches its quads through `render[state]` (`RenderKind::Model(i)`, or a
  full-cube fast path with no quads stored). Use
  `BakedAssets::state_quads`. M260 read `models[state]` and drew another
  block's model (an oak stair in cinnabar bricks) while r119, a quad
  count, passed; a scratch render caught it.
- **A block sprite wider than 16 is tiled, not dropped.** The terrain layer
  array is 16 × 16, and `layer_for` refused 26.x's 32 × 32 signs, hanging
  signs, shelves and dried-ghast tentacles, so every such state baked to no
  quads (a sign was only its text; 26.2 has no `SignModel`, the board is a
  block model). `Baker::big_sprite` cuts a sprite into layer-sized tiles and
  `split_quad_over_tiles` cuts each face over them; the sign text transform
  is `sign_states::SignState::text_transform` (a hanging sign has its own).
- **Entity shadows** (`ShadowFeatureRenderer`): `rewo_world::shadow` ports
  `extractShadow` behind the `ShadowGround` trait, the per-renderer radius and
  strength are the generated `shadow_radius_table` (`tools/gen_shadow_radius.py`)
  plus `rewo_data::shadow`'s rules, and `EntityPass` draws them as the last
  range (gamma space, `GREATER_OR_EQUAL`, no depth write, vertices pulled
  toward the camera by `1 - 1/4096` for `VIEW_OFFSET_Z_LAYERING`). The
  `entityShadows` option gates it.
- **Villager outfits** (`VillagerProfessionLayer`): the type, profession and
  badge sheets are `rewo_data::villager::SHEETS`, packed after every other
  atlas texture and found by name; `emit_model` redraws the base model's quads
  on each into the layer range (the no-hat model skips the head subtree),
  drawn by `draw_layers` with `GREATER_OR_EQUAL` AND depth writes. Not
  `EQUAL`: the hat and rim are texels the base sheet leaves transparent, so
  the base never wrote their depth (M-playtest: a farmer with no hat).
  `mobtexshot`'s `vo1` checks the hat widens the head. Adding atlas textures
  means growing `ATLAS_H` and the cape gate's `*_SHELF_GROWTH` sum, and moves
  every atlas V denominator (a witness that rests on one or two edge texels
  can flip: `m77`'s puppy did).
- **World text frames are y-down** (a sign's and a text display's
  transform end in a negative y scale, as vanilla's font layout does), so
  `emit_world_text` maps a glyph's top to its cell's top (`glyph_uv`'s
  `v1`); the nametag's frame is y-up and takes `v0`. Every sign read upside
  down from M25e to M263 with every string witness green;
  `itemshot c3f` checks the pixels.
- The entity atlas's large slots are 64×64: a bigger entity sheet (the
  end crystal's 128×64) is halved at bake (`halve_texture`, M257).
- **Face numbering differs by bake**: the block bake's `dir` is
  `FACE_NAMES` order (up 0, down 1, ...); `HeldQuad::dir` is vanilla's
  `Direction` ordinal (down 0, up 1, ...). Convert at the boundary
  (`bake_block_item`), and test a label against its geometry.
- **Reversed-Z depth**: clear 0.0, compare `GREATER`. Coplanar multi-layer
  geometry (item sprite layers) needs **`GREATER_OR_EQUAL`**. Second passes
  over the same geometry (glint, trims) use depth **EQUAL, no write**, and must
  reproduce the geometry *to the bit*: reuse the same pose derivation, don't
  re-derive it.
- `MeshVertex` is a packed 28 bytes. **Greedy meshing is off in production**
  (M208, `MERGE_FACES`): its rectangles left T-junction cracks. Any return
  must pass `the_production_mesh_has_no_t_junctions`. Look for hardcoded `len * N` beside `VERTEX_STRIDE`
  whenever a vertex grows (M21, M109). Every pass's hand-written stride has a
  `const _: () = assert!(size_of::<Vertex>() == VERTEX_STRIDE)` beside it, so a
  struct that grows fails the build; add the same to a new pass. `rewo-mesh`
  and `rewo-gpu` cannot see each other, so `live_cmd/view.rs`
  (`mesh_vertex_size_is_the_world_vertex_stride`) pins the pair.
- **`Renderer::render` checks the depth extent against the swapchain** and
  returns `NeedsRecreate` on a mismatch, and also when a failed recreate left
  no swapchain (null handle, 0x0 extent). Never discard a `recreate` /
  `ensure_depth` error with `let _ =`: log it and let the next frame retry
  through `NeedsRecreate`; the app's `RecreateGuard` exits only after 10
  consecutive failures spanning 2 s. Reporting the lost-swapchain state as
  `Skipped` once left a blank window that never retried.
- **Terrain lighting is vanilla's** (`rewo-mesh/src/smooth_light.rs`, a port of
  `BlockModelLighter`): the mesher writes the finished 8-bit vertex colour and
  smooth light coordinates, and `world.vert` samples the lightmap per vertex.
  Don't move lighting back to the fragment stage or store float colours; both
  drift from vanilla.
- Item display transforms: `ItemTransform.Deserializer` multiplies translation
  by 0.0625 and clamps **before** apply. Shade with the rotated normal.

## Colour space

- Shader colour constants are authored in sRGB and **must convert to linear**
  (SRGB attachments encode on store). UI passes **mask alpha writes** so
  read-back PNGs stay opaque.
- The text pass takes **linear** colour (`color_linear`); nine callers once
  passed sRGB bytes. White and black hide this, so test with a mid colour.
- The Velvet passes must be built with **`world::unorm_of(target_format)`** and
  drawn inside `WorldRenderer::with_gamma_space` (both in `world/gamma.rs`), or
  the pipeline format mismatches the attachment.
- **Glint blends in gamma space** through a UNORM view of the same image
  (`MUTABLE_FORMAT`). Squaring isn't invariant under the sRGB transfer, so in
  linear space the glint quantised to nothing. Without
  `VK_KHR_swapchain_mutable_format` no glint draws at all.
- **The gamma scope splits the frame's dynamic rendering** (`world/gamma.rs`).
  `WorldRenderer::in_gamma_space` ends the instance and reopens it against the
  UNORM view with `LOAD`, then reopens the sRGB one. So the first instance must
  **`STORE` depth** (the glint and the other gamma-space passes test `EQUAL`
  against it; `DONT_CARE` leaves it undefined), and `scope_barrier` orders each
  reopen against the previous store (a global barrier, since the colour image
  is written through one view and read through its twin). Sync validation
  cannot see a missing `STORE`.

## GPU resource lifetime

**`ring >= fif + 1` for a buffer ring written before `render`, `ring >= fif`
for one written inside it** (`buf_ring.rs`). A `set_*` that frees its buffer at
the top destroys memory in-flight command buffers reference (40,532 VUIDs in
M86). A pass rebuilt by an `init_*` throws away state installed on it before
the rebuild (M44), so install after. Destroy every pipeline you create: a leak
shows as `VUID-vkDestroyDevice-device-05137` with every witness green.

- **Glyph atlas (Velvet text)**: persistent image, updated by dirty-rect
  region copy (`upload_region_r8`, the no-wait submit the skin atlas uses). Only
  a grow builds a new image (`create_texture_r8_async`, deferred destruction of
  the old one). Never rebuild the atlas through a fence-waiting upload:
  `sync_atlas` runs on the gameplay path.
- **Terrain arena** (mega-buffer, `world/arena.rs`): it grows when full (M205:
  drain, copy into a buffer of double size, extend the free lists) up to
  `ARENA_*_LIMIT` capped by the device's largest buffer (M210); size those
  against `tools/load_check.py --rd 32 --heavy` (several GB of host RAM; not beside other heavy jobs), not a flat world. `place_column` frees its regions on any
  staging error; a failed `flush_uploads` unregisters the batch's columns and
  reports them via `take_lost_columns` (`pump_meshing` re-dirties them); a
  "mega-buffer full" column is kept in a retry queue and re-uploaded
  nearest-first once removals free space; over `MAX_COLUMNS`, `sync_meta`
  (`world/cull.rs`) keeps the columns nearest the camera.
- **Per-player texture slots are LRU** (`LruSlots` in `entities.rs`): skin,
  cape and tab-face pools evict the least recently drawn, never a slot drawn
  this frame; when all are in use the claim is refused and the player keeps the
  default texture. Uploads take the owner's UUID, and evictions come back via
  `take_evicted_textures` so the app forgets the stale address. Never index a
  pool with `next % N`.
- **`WorldRenderer`'s per-frame inputs are two structs**: `GuiFrame` (GUI pass
  inputs, `world/gui_passes.rs`) and `SceneFrame` (`world.rs`: camera,
  lightmap, sky/fog tints, fog band, sky mode, celestial state, End flash,
  end-portal time, selected block). Its own fields are GPU resources and
  passes: a new `set_*` input goes into one of them.
- Swapchain: creation cleans up the swapchain and views on a partial failure;
  the format pick prefers B8G8R8A8_SRGB, then any sRGB format in the
  sRGB-nonlinear space, and warns otherwise (every pass writes linear colour and
  relies on the attachment's encode). Timestamps are written only when the
  graphics family reports `timestampValidBits`, and deltas are masked to them.
- `rewo-mesh`'s pool runs each job under `catch_unwind` and reports
  `panicked: true`, so `in_flight` clears and the column is treated as empty
  until re-meshed (rayon would otherwise abort the process).

Debug knobs this crate reads (`REWO_FOG`, `REWO_MOB_DEBUG_TEX`,
`REWO_CEM_NOANIM`, `REWO_NO_SYNC_VALIDATION`) go through `src/knobs.rs` and
read as unset unless the crate's `gates` feature is on (rewo-app's `gates`
enables it). Add new ones there, not as `std::env::var`.

## HUD / screen coordinates

`HudFill` is in GUI pixels (the pass multiplies by GUI scale); `OwnedTextLine`
takes screen pixels. Use `rewo_gpu::hud::gui_scale` instead of recomputing it.
**An agreement witness has to model whatever sits between producer and
screen**: both sides in the same wrong space agree perfectly (M135).

## Models, CEM and mob gates

- Mob meshes are verbatim ports of vanilla `ModelPart.Cube`/`Polygon` with the
  exact entity transform. `rewo mobshot --check` (face-coloured debug
  textures) verifies UV/face correspondence, **not** which sheet is sampled;
  `mobtexshot` covers the real-texture multi-entity case.
- Keyframe rigs are machine-extracted (`tools/gen_anim_defs.ps1` →
  `anim_defs.rs`, deterministic LF). Never hand-edit generated files; re-run
  the generator.
- CEM (OptiFine `.jem`): a **top-level** `translate` is the rotation pivot (not
  static position), `pivot = to_model(-translate)`. The model bakes through a
  180° Z rotation (`invertAxis:"xy"`), so animation X/Y rotations and
  translations are **negated**. A submodel's pivot is its accumulated position.
  OptiFine translation **replaces** a bone's translate, so subtract a per-bone
  rest baseline.
- Trig must match Java: `Mth.sin` is the 65,536-entry table, `libm::sin`
  matches `(float)Math.sin`, and JOML's `fma` is non-fused.

## Visual freeze

HUD widget transcription is deliberately stopped at one widget pending a
visual overhaul. The chrome palette is a `ShellStyle` data table, so a redesign
is a data edit. See `REWO_VELVET_UI_PLAN.md` §8/§9 before touching HUD visuals.
