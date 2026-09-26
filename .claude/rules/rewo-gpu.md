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
  whenever a vertex grows (M21, M109).
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
  drawn inside `WorldRenderer::with_gamma_space`, or the pipeline format
  mismatches the attachment.
- **Glint blends in gamma space** through a UNORM view of the same image
  (`MUTABLE_FORMAT`). Squaring isn't invariant under the sRGB transfer, so in
  linear space the glint quantised to nothing. Without
  `VK_KHR_swapchain_mutable_format` no glint draws at all.

## GPU resource lifetime

**`ring >= fif + 1` for a buffer ring written before `render`, `ring >= fif`
for one written inside it** (`buf_ring.rs`). A `set_*` that frees its buffer at
the top destroys memory in-flight command buffers reference (40,532 VUIDs in
M86). A pass rebuilt by an `init_*` throws away state installed on it before
the rebuild (M44), so install after. Destroy every pipeline you create: a leak
shows as `VUID-vkDestroyDevice-device-05137` with every witness green.

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
