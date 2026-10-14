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
- **Reversed-Z depth**: clear 0.0, compare `GREATER`. Coplanar multi-layer
  geometry (item sprite layers) needs **`GREATER_OR_EQUAL`**. Second passes
  over the same geometry (glint, trims) use depth **EQUAL, no write**, and must
  reproduce the geometry *to the bit*: reuse the same pose derivation, don't
  re-derive it.
- `MeshVertex` is a packed 28 bytes. Greedy meshing merges only uniform cube
  faces and **never +Y**. Look for hardcoded `len * N` beside `VERTEX_STRIDE`
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
- **Terrain arena** (mega-buffer, `world/arena.rs`): `place_column` frees its regions on any
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
