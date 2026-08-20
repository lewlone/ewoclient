//! rewo-mesh — M4 mesher: full-cube fast path with ambient occlusion, plus
//! the general model-quad path for everything else (stairs, slabs, fences,
//! glass, plants, torches, …).
//!
//! Per-vertex color = directional face shade × AO × biome tint (white when
//! untinted). As of M15 that product is **not stored**: the 28-byte
//! [`MeshVertex`] carries the discrete shade and AO *codes* plus lossless tint
//! bytes, and `world.vert` reconstructs the identical float sequence. Block and
//! sky light stay packed *separately* in the same word (see [`pack_layer`]) and
//! are combined in the shader, so the time of day never forces a remesh.
//!
//! Biome tint is applied at mesh time (M14): a dynamic Grass/Foliage/DryFoliage/
//! Water face selects the *raw* (un-tinted) atlas layer and carries its resolved
//! biome color as RGB bytes; a `Constant` tint (spruce/birch) carries a fixed
//! color. Tint stays `u8` end to end — no float round-trip. Synthetic / no-biome
//! worlds deliberately keep the legacy pre-tinted layers with a white tint, so
//! the demo path stays byte-identical. A per-job cache (`TintCache`) memoizes
//! the expensive vanilla radius-2 resolution, so a block's tint is computed
//! once, not once per face.
//!
//! **Greedy meshing (M15).** The M4 note said greedy was impossible here
//! because per-vertex AO makes coplanar faces non-mergeable. That is true only
//! for faces whose four corner AO codes *differ*: across such a face the
//! rasterizer interpolates a gradient that a merged rectangle cannot reproduce.
//! A face whose four AO codes are all equal has no gradient at all, so replacing
//! N of them with one rectangle is mathematically identical — the same shade,
//! the same AO level, the same repeated texture. [`mesh_column`] therefore
//! splits the cube path in two: uniform-AO faces on the five non-up directions
//! enter a per-(face, plane) mask and are merged; up (+Y) faces and non-uniform
//! faces fall back to the legacy unit quad, byte-for-byte. The up carve-out is
//! empirical, not structural — the merge site in [`mesh_column`] records what
//! merging it measured. Models and fluids never merge — they run the untouched
//! [`emit_model`] / [`emit_fluid`] path.
//!
//! [`mesh_column_reference`] is the frozen pre-greedy mesher, kept as the
//! unit-face oracle: expanding the optimized output back into unit faces must
//! reproduce it exactly.

pub mod crumbling;
pub mod pool;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rewo_data::assets::{CarriedFluid, RenderKind, TintSource};
use rewo_world::World;

/// Vanilla `Options.biomeBlendRadius` default — the `(2r+1)²` block-tint window.
const BIOME_BLEND_RADIUS: i32 = 2;

/// A per-`mesh_column` memo of dynamic block-tint results.
///
/// The decompiled 26.2 `ClientLevel` wraps `calculateBlockTint` in a
/// `BlockTintCache` (one per `ColorResolver`) precisely because the call is
/// expensive: each result averages a `(2r+1)²` window of `BiomeManager.getBiome`
/// lookups, and every lookup runs 8 fiddled corner-distance evaluations. Our
/// mesher asks for a block's tint once per tinted cube face / model quad / fluid
/// face — a single leaf cube with a dynamic (non-constant) tint would repeat the
/// identical radius-2 average up to six times. This memo collapses those to one
/// computation.
///
/// Scope is a single mesh job (one `mesh_column` call). It is a plain local,
/// never shared and never locked, so a concurrent `chunks_biomes` / chunk
/// (re)load can never leave a stale entry behind — the cache is dropped when the
/// job returns, exactly when the snapshot it was computed against goes away.
#[derive(Default)]
struct TintCache {
    /// key = (canonical sampled block x, y, z, resolver code); value = lossless RGB.
    map: HashMap<(i32, i32, i32, u8), [u8; 3]>,
}

/// Dynamic biome tint (lossless RGB bytes) for a tinted face, or `None` to
/// fall back to the legacy pre-tinted layer (no biome context or an untinted
/// face). Stored verbatim in `MeshVertex::tint`; `world.vert` folds it in
/// alongside shade/AO when it reconstructs the color — the camera sky/fog is a
/// separate uniform, so this never re-runs on a time-of-day change.
///
/// Results for the four dynamic resolvers are memoized in `cache`, keyed by the
/// **actually sampled** block position + resolver. `GrassBelow` (doubleTallGrass
/// UPPER) samples `pos.below()` with the Grass resolver, so it canonicalizes to
/// Grass at `y-1` and shares that slot. `Constant` tints (spruce/birch) are a
/// fixed color with no window average, so they bypass the cache entirely.
fn biome_tint(
    world: &World,
    cache: &mut TintCache,
    x: i32,
    y: i32,
    z: i32,
    src: TintSource,
) -> Option<[u8; 3]> {
    use rewo_world::biome::ColorResolver;
    // No biome context (synthetic / demo world) → legacy path, byte-identical.
    world.biome_context()?;
    // Resolve to (sampled block pos, resolver, cache code); Constant / None
    // short-circuit without touching the cache.
    let (bx, by, bz, resolver, code) = match src {
        TintSource::None => return None,
        TintSource::Constant(c) => return Some(c),
        TintSource::Grass => (x, y, z, ColorResolver::Grass, 0u8),
        TintSource::GrassBelow => (x, y - 1, z, ColorResolver::Grass, 0u8),
        TintSource::Foliage => (x, y, z, ColorResolver::Foliage, 1u8),
        TintSource::DryFoliage => (x, y, z, ColorResolver::DryFoliage, 2u8),
        TintSource::Water => (x, y, z, ColorResolver::Water, 3u8),
    };
    let key = (bx, by, bz, code);
    if let Some(v) = cache.map.get(&key) {
        return Some(*v);
    }
    let rgb = world.block_tint(bx, by, bz, resolver, BIOME_BLEND_RADIUS)?;
    cache.map.insert(key, rgb);
    Some(rgb)
}

// -- packed vertex bit layout (M15) -----------------------------------------
// `light` word: layer[0..15] | block[16..19] | sky[20..23] | shade[24..26] |
// ao[27..28] | reserved[29..31]. The lower 24 bits are exactly `pack_layer`'s
// historical contract, untouched.
const SHADE_SHIFT: u32 = 24;
const SHADE_MASK: u32 = 0b111;
const AO_SHIFT: u32 = 27;
const AO_MASK: u32 = 0b11;

/// Pack the discrete shade/AO codes into the high bits of a `pack_layer` word.
pub fn pack_light_word(layer: u32, block: u8, sky: u8, shade_code: u8, ao_code: u8) -> u32 {
    // Release assert, not `debug_assert!`: `SHADE_MASK` is 3 bits (0..=7) while
    // `FACE_SHADE` has 7 entries, so the reserved code 7 fits the mask and would
    // pack silently — then index out of bounds in
    // `MeshVertex::reconstructed_color` (and in `world.vert`, where an
    // out-of-range constant-array index is undefined). 0..=6 are the valid
    // codes; 7 fails closed here, at the one boundary every emitter goes
    // through.
    assert!(
        (shade_code as usize) < FACE_SHADE.len(),
        "shade_code {} out of range: FACE_SHADE has {} entries (valid 0..={})",
        shade_code,
        FACE_SHADE.len(),
        FACE_SHADE.len() - 1
    );
    debug_assert!((ao_code as usize) < AO_LEVELS.len());
    pack_layer(layer, block, sky)
        | ((shade_code as u32 & SHADE_MASK) << SHADE_SHIFT)
        | ((ao_code as u32 & AO_MASK) << AO_SHIFT)
}

/// Pack lossless tint bytes + a reserved flag byte.
pub fn pack_tint(rgb: [u8; 3], flags: u8) -> u32 {
    (rgb[0] as u32) | ((rgb[1] as u32) << 8) | ((rgb[2] as u32) << 16) | ((flags as u32) << 24)
}

/// White tint — the legacy/untinted path. `255/255 == 1.0` exactly, so a
/// reconstructed color is bit-identical to the old `c * 1.0`.
pub const TINT_WHITE: [u8; 3] = [255, 255, 255];

/// **28 bytes.** World-space position stays full `f32` (the M4 invariant: the
/// mesher emits world space and the shader adds no column origin).
///
/// - `pos`   f32x3 (12) — world space, unchanged
/// - `uv`    f32x2 (8)  — **exact**; see below
/// - `light` u32 (4) — see the bit layout above
/// - `tint`  u32 (4) — lossless RGB bytes + reserved flags
///
/// A 24-byte variant storing UV as f16 was built and **rejected**: `emit_fluid`
/// emits surface UVs of the form `1.0 - k/9` (vanilla's 8/9 source height), and
/// `1/9` is not a dyadic rational, so no f16 represents it. The resulting
/// ~2.7e-5 UV error is only ~0.0004 texel, but it lands on a texel boundary and
/// flipped nearest-neighbour sampling on 6 demo pixels (max channel delta 25),
/// breaking byte-identical rendering. Baked *model* UVs are all k/16 and were
/// exact (2,982,592 components, 0 inexact) — the fluid family is the one f16
/// cannot carry, so UV stays f32.
///
/// The per-vertex color is **not** stored; the vertex shader reconstructs it as
/// `FACE_SHADE[shade] * AO_LEVELS[ao] * (tint_rgb / 255)`, in that order, which
/// is the identical float sequence the mesher used to run on the CPU.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq, Debug)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub light: u32,
    pub tint: u32,
}

impl MeshVertex {
    /// The one constructor. Takes ordinary f32 UV + discrete codes + lossless
    /// tint bytes, so no call site hand-assembles the layout.
    pub fn new(
        pos: [f32; 3],
        uv: [f32; 2],
        layer: u32,
        block: u8,
        sky: u8,
        shade_code: u8,
        ao_code: u8,
        tint_rgb: [u8; 3],
    ) -> Self {
        Self {
            pos,
            uv,
            light: pack_light_word(layer, block, sky, shade_code, ao_code),
            tint: pack_tint(tint_rgb, 0),
        }
    }

    /// UV as the vertex shader receives it. Exact — the stored value is the
    /// source value, with no quantization step anywhere in the path.
    pub fn uv_f32(&self) -> [f32; 2] {
        self.uv
    }

    pub fn layer_index(&self) -> u16 {
        (self.light & 0xFFFF) as u16
    }

    pub fn block_light(&self) -> u8 {
        ((self.light >> 16) & 15) as u8
    }

    pub fn sky_light(&self) -> u8 {
        ((self.light >> 20) & 15) as u8
    }

    pub fn shade_code(&self) -> u8 {
        ((self.light >> SHADE_SHIFT) & SHADE_MASK) as u8
    }

    pub fn ao_code(&self) -> u8 {
        ((self.light >> AO_SHIFT) & AO_MASK) as u8
    }

    pub fn tint_rgb(&self) -> [u8; 3] {
        [
            (self.tint & 0xFF) as u8,
            ((self.tint >> 8) & 0xFF) as u8,
            ((self.tint >> 16) & 0xFF) as u8,
        ]
    }

    pub fn tint_flags(&self) -> u8 {
        ((self.tint >> 24) & 0xFF) as u8
    }

    /// CPU mirror of `world.vert`'s reconstruction — the same operations in the
    /// same order, so oracles can compare against the legacy float formula.
    pub fn reconstructed_color(&self) -> [f32; 3] {
        let c = FACE_SHADE[self.shade_code() as usize] * AO_LEVELS[self.ao_code() as usize];
        let rgb = self.tint_rgb();
        [
            c * (rgb[0] as f32 / 255.0),
            c * (rgb[1] as f32 / 255.0),
            c * (rgb[2] as f32 / 255.0),
        ]
    }
}

pub struct ColumnMesh {
    pub cx: i32,
    pub cz: i32,
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    /// Translucent geometry (water) — drawn blended, after all opaque
    /// content, sorted per column back-to-front by the renderer.
    pub tvertices: Vec<MeshVertex>,
    pub tindices: Vec<u32>,
    pub y_min: f32,
    pub y_max: f32,
    /// Every visible (un-culled) cube unit face the scan saw. Models and fluids
    /// are not counted — only the cube path can merge.
    pub visible_cube_faces: u32,
    /// Of those, the ones eligible to enter a greedy mask: four equal corner AO
    /// codes *and* a direction other than up (see the merge site in
    /// [`mesh_column`] for why up is excluded).
    pub greedy_candidate_faces: u32,
    /// Rectangles actually emitted from the candidates. `<= greedy_candidate_faces`;
    /// the ratio is the compression the pass bought.
    pub greedy_quads: u32,
    /// Visible cube faces that did not merge — every up (+Y) face, plus any face
    /// with a non-uniform corner-AO pattern — emitted as legacy unit quads.
    /// `visible_cube_faces == greedy_candidate_faces + unit_fallback_faces`.
    pub unit_fallback_faces: u32,
    /// Cells where a fluid was emitted for a block that is **not** a
    /// `RenderKind::Fluid` — i.e. a waterlogged (or unconditionally watery)
    /// block's water (M161).
    ///
    /// It exists so the windowed client can be *asked* whether the feature
    /// reached it. `live --render-check`'s r48 reads it, because the way this
    /// feature fails is not a wrong pixel: it is `MeshTables.fluid` never being
    /// threaded through, which renders exactly as it did before and is the M86
    /// shape — a whole feature that only ever ran headlessly.
    pub carried_fluid_cells: u32,
}

/// [up(+Y), down(-Y), north(-Z), south(+Z), west(-X), east(+X)].
const FACE_OFFSETS: [(i32, i32, i32); 6] = [
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];
/// Directional face shade, indexed by the vertex's 3-bit shade code.
/// **Mirrored verbatim in `shaders/world.vert`** — keep both in sync.
///
/// Codes 0..=5 are the M15 assignment and are frozen: they are exactly
/// `CardinalLighting::DEFAULT` in the mesher's face order `[up, down, north,
/// south, west, east]`, so under the default table a face's code *is* its
/// direction and code 0 doubles as the `shade: false` identity
/// (`FACE_SHADE[0] == 1.0`). Nothing may renumber them — every Overworld mesh
/// byte and the greedy oracle's expansion (which reads the code back as a face
/// direction) depend on it.
///
/// M16 appends **code 6 = 0.9**, the one factor `CardinalLighting::NETHER`
/// introduces (its up *and* down). Code 7 stays reserved: it is inside the
/// 3-bit mask but outside this table, and [`pack_light_word`] rejects it in
/// release builds rather than letting it index out of bounds here or in the
/// shader.
pub const FACE_SHADE: [f32; 7] = [1.0, 0.5, 0.8, 0.8, 0.6, 0.6, 0.9];

/// **The one face → shade-code mapping**, driven by the world's dimension.
///
/// Both mesher paths, all three geometry kinds (cube, model, fluid) and the
/// greedy merge key resolve a face's shade through here, so a face's stored
/// code cannot depend on which emitter produced it.
///
/// The rule is a lookup, not a search: a face whose dimension leaves its factor
/// at the default keeps its historical code — which is what makes every
/// Overworld/default mesh byte-identical to pre-M16, including the two pairs
/// (north/south, west/east) that share a value and would otherwise collapse
/// onto one code. Only a face the dimension actually moves (Nether up/down,
/// 1.0/0.5 → 0.9) looks the new factor up, and finds code 6.
///
/// Fail-closed: a factor that is in no `FACE_SHADE` entry has no representable
/// code, so this panics rather than packing a wrong shade. Vanilla ships
/// exactly the two tables `CardinalLightType` names, so that is unreachable
/// today — it exists so a third table cannot be added silently.
pub fn face_shade_code(world: &World, face: usize) -> u8 {
    let want = world.cardinal_light().by_mesh_face(face);
    if FACE_SHADE[face] == want {
        return face as u8;
    }
    match FACE_SHADE.iter().position(|f| *f == want) {
        Some(code) => code as u8,
        None => panic!(
            "cardinal light factor {want} for face {face} has no FACE_SHADE code \
             (dimension {:?}); add it to FACE_SHADE *and* shaders/world.vert",
            world.cardinal_light_type()
        ),
    }
}

/// Unit-cube face corners + UV, matching asset face order.
const FACE_CORNERS: [[([f32; 3], [f32; 2]); 4]; 6] = [
    [
        ([0.0, 1.0, 0.0], [0.0, 0.0]),
        ([1.0, 1.0, 0.0], [1.0, 0.0]),
        ([1.0, 1.0, 1.0], [1.0, 1.0]),
        ([0.0, 1.0, 1.0], [0.0, 1.0]),
    ], // up
    [
        ([0.0, 0.0, 0.0], [0.0, 0.0]),
        ([0.0, 0.0, 1.0], [0.0, 1.0]),
        ([1.0, 0.0, 1.0], [1.0, 1.0]),
        ([1.0, 0.0, 0.0], [1.0, 0.0]),
    ], // down
    [
        ([1.0, 1.0, 0.0], [0.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0]),
        ([0.0, 0.0, 0.0], [1.0, 1.0]),
        ([1.0, 0.0, 0.0], [0.0, 1.0]),
    ], // north
    [
        ([0.0, 1.0, 1.0], [0.0, 0.0]),
        ([1.0, 1.0, 1.0], [1.0, 0.0]),
        ([1.0, 0.0, 1.0], [1.0, 1.0]),
        ([0.0, 0.0, 1.0], [0.0, 1.0]),
    ], // south
    [
        ([0.0, 1.0, 0.0], [0.0, 0.0]),
        ([0.0, 1.0, 1.0], [1.0, 0.0]),
        ([0.0, 0.0, 1.0], [1.0, 1.0]),
        ([0.0, 0.0, 0.0], [0.0, 1.0]),
    ], // west
    [
        ([1.0, 1.0, 1.0], [0.0, 0.0]),
        ([1.0, 1.0, 0.0], [1.0, 0.0]),
        ([1.0, 0.0, 0.0], [1.0, 1.0]),
        ([1.0, 0.0, 1.0], [0.0, 1.0]),
    ], // east
];

/// AO tangent axes per face (nonzero-component indices + the normal axis).
const FACE_AXES: [(usize, usize, (i32, i32, i32)); 6] = [
    (0, 2, (0, 1, 0)),  // up: u=x, v=z
    (0, 2, (0, -1, 0)), // down
    (0, 1, (0, 0, -1)), // north: u=x, v=y
    (0, 1, (0, 0, 1)),  // south
    (2, 1, (-1, 0, 0)), // west: u=z, v=y
    (2, 1, (1, 0, 0)),  // east
];
/// AO level 0..3 → brightness, indexed by the vertex's 2-bit AO code.
/// **Mirrored verbatim in `shaders/world.vert`** — keep both in sync.
pub const AO_LEVELS: [f32; 4] = [0.45, 0.65, 0.82, 1.0];
/// The AO code meaning "no occlusion" — `AO_LEVELS[AO_NONE] == 1.0`, so model
/// and fluid faces reproduce their old un-AO'd color exactly.
pub const AO_NONE: u8 = 3;

/// Pack a texture layer with the two light channels.
///
/// Vanilla keeps block and sky light **separate** all the way to the shader:
/// they are combined additively there, and the sky half is scaled by the time
/// of day (`SKY_LIGHT_FACTOR`). Collapsing them to one number in the mesh
/// would bake the time of day into the geometry and force a full remesh at
/// every sunrise.
///
/// The layer index needs 16 bits (a few thousand layers) and each channel
/// needs 4, so all three ride in the existing `u32` — the vertex does not
/// grow. Layout: `layer | block << 16 | sky << 20`.
pub fn pack_layer(layer: u32, block: u8, sky: u8) -> u32 {
    debug_assert!(layer <= 0xFFFF, "texture layer {layer} exceeds 16 bits");
    (layer & 0xFFFF) | ((block as u32 & 15) << 16) | ((sky as u32 & 15) << 20)
}

fn is_full_cube(table: &[RenderKind], state: u32) -> bool {
    matches!(table.get(state as usize), Some(RenderKind::Cube { .. }))
}

/// The **frozen pre-greedy mesher** — one unit quad per visible face, in scan
/// order. Kept verbatim as the oracle the optimized [`mesh_column`] is graded
/// against: expanding its rectangles back into unit faces must reproduce this
/// output exactly. Not on the hot path.
pub(crate) fn mesh_column_reference(
    world: &World,
    table: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
    carried: &[Option<CarriedFluid>],
    cx: i32,
    cz: i32,
) -> Option<ColumnMesh> {
    let col = world.column(cx, cz)?;
    let shape = world.shape;
    let base_x = cx * 16;
    let base_z = cz * 16;

    let mut vertices: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut tvertices: Vec<MeshVertex> = Vec::new();
    let mut tindices: Vec<u32> = Vec::new();
    // One tint memo for the whole job (see `TintCache`). Dropped on return, so a
    // later chunks_biomes / reload can never observe a stale entry.
    let mut tint_cache = TintCache::default();
    let mut y_min = f32::MAX;
    let mut y_max = f32::MIN;
    let mut visible_cube_faces = 0u32;
    let mut carried_fluid_cells = 0u32;
    let mut bump = |y: f32| {
        y_min = y_min.min(y);
        y_max = y_max.max(y + 1.0);
    };

    for si in 0..shape.section_count() {
        if col.section_is_trivial(si) {
            continue;
        }
        let sy = shape.min_y + (si as i32) * 16;
        for y in sy..sy + 16 {
            for lz in 0..16 {
                for lx in 0..16 {
                    let wx = base_x + lx;
                    let wz = base_z + lz;
                    let state = world.block_state_at(wx, y, wz);
                    // `SectionCompiler.compile:89-97` — the fluid first, then
                    // the block, as two independent draws at one position.
                    if let Some(f) = fluid_at(table, carried, state) {
                        // Water blends → translucent set; lava is opaque (and
                        // fullbright) → opaque set.
                        let (fv, fi) = if f.lava {
                            (&mut vertices, &mut indices)
                        } else {
                            (&mut tvertices, &mut tindices)
                        };
                        let fluid_verts_before = fv.len();
                        emit_fluid(
                            world,
                            table,
                            carried,
                            &mut tint_cache,
                            fv,
                            fi,
                            wx,
                            y,
                            wz,
                            f,
                        );
                        // Counted only when the draw produced geometry.
                        // `r48`'s claim is "meshed its water", and an increment
                        // beside the CALL would also fire for a cell whose six
                        // faces were all suppressed.
                        carried_fluid_cells +=
                            u32::from(f.carried && fv.len() > fluid_verts_before);
                        bump(y as f32);
                    }
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            visible_cube_faces += emit_cube(
                                world,
                                table,
                                &mut tint_cache,
                                &mut vertices,
                                &mut indices,
                                wx,
                                y,
                                wz,
                                faces,
                                raw_faces,
                                tint,
                            );
                            bump(y as f32);
                        }
                        Some(RenderKind::Model(idx)) => {
                            emit_model(
                                world,
                                table,
                                models,
                                &mut tint_cache,
                                &mut vertices,
                                &mut indices,
                                wx,
                                y,
                                wz,
                                *idx,
                            );
                            bump(y as f32);
                        }
                        // Already drawn above. `LiquidBlock.getRenderShape`
                        // is `RenderShape.INVISIBLE` (`:135-137`), so the block
                        // itself contributes nothing.
                        Some(RenderKind::Fluid { .. }) => {}
                        _ => {}
                    }
                }
            }
        }
    }

    if indices.is_empty() && tindices.is_empty() {
        return None;
    }
    Some(ColumnMesh {
        cx,
        cz,
        vertices,
        indices,
        tvertices,
        tindices,
        y_min,
        y_max,
        // The reference merges nothing: every visible face IS a unit fallback.
        visible_cube_faces,
        greedy_candidate_faces: 0,
        greedy_quads: 0,
        unit_fallback_faces: visible_cube_faces,
        carried_fluid_cells,
    })
}

// -- greedy meshing (M15) ---------------------------------------------------

/// The merge identity of a mergeable cube face.
///
/// `state` is in the key alongside the packed words on purpose: two different
/// blocks can legitimately share an atlas layer on one face, and `light`/`tint`
/// are lossy summaries of *appearance*, not of *material*. Keying on the block
/// state as well makes coalescence a deliberate "same block" decision rather
/// than a coincidence of packed bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FaceKey {
    state: u32,
    /// The exact `pack_light_word` output — layer, block/sky light, the face's
    /// shade code, and the (uniform) AO code.
    light: u32,
    /// The exact `pack_tint` output.
    tint: u32,
}

/// One face direction's plane masks.
///
/// Planes are a small dense integer range (`0..=height` for up/down, `0..=16`
/// for the four sides), so this is a plain `Vec` indexed by plane — iteration
/// order is index order, which is what keeps the emitted vertex stream
/// deterministic. Each plane's cell array is allocated on first use: a column
/// whose terrain occupies four sections must not pay for 24 sections of mask.
struct FaceMask {
    u_len: usize,
    v_len: usize,
    planes: Vec<Option<Vec<Option<FaceKey>>>>,
}

impl FaceMask {
    fn new(u_len: usize, v_len: usize, plane_count: usize) -> Self {
        Self {
            u_len,
            v_len,
            planes: (0..plane_count).map(|_| None).collect(),
        }
    }

    fn set(&mut self, plane: usize, u: usize, v: usize, key: FaceKey) {
        let (u_len, v_len) = (self.u_len, self.v_len);
        debug_assert!(u < u_len && v < v_len, "mask cell out of range");
        let cells = self.planes[plane].get_or_insert_with(|| vec![None; u_len * v_len]);
        cells[v * u_len + u] = Some(key);
    }
}

/// All six face directions' masks for one column, plus the vertical origin the
/// v/plane indices are measured from.
struct GreedyMasks {
    faces: [FaceMask; 6],
    /// `vy` (= `y - shape.min_y`) of the lowest nontrivial section's floor.
    vy_lo: i32,
}

impl GreedyMasks {
    /// `span` is the block height of the nontrivial-section range: masks only
    /// need to cover the part of the column that can hold geometry.
    fn new(span: usize) -> Self {
        // up/down: a 16×16 grid per horizontal plane. `vy` planes run
        // `vy_lo ..= vy_lo + span` (the up face of the topmost block sits one
        // plane above it), hence `span + 1`.
        let horizontal = || FaceMask::new(16, 16, span + 1);
        // north/south and west/east: a 16×span grid per vertical plane, and
        // there are 17 planes across a 16-wide column (0..=16).
        let vertical = || FaceMask::new(16, span, 17);
        Self {
            faces: [
                horizontal(),
                horizontal(),
                vertical(),
                vertical(),
                vertical(),
                vertical(),
            ],
            vy_lo: 0,
        }
    }

    /// Column-local cell address of a face. Bijective: each `(face, plane, u, v)`
    /// comes from exactly one block, so two blocks can never collide in a cell.
    fn cell(&self, face: usize, lx: i32, vy: i32, lz: i32) -> (usize, usize, usize) {
        let vv = vy - self.vy_lo;
        let (plane, u, v) = match face {
            0 => (vv + 1, lx, lz), // up:    plane = vy+1, u = lx, v = lz
            1 => (vv, lx, lz),     // down:  plane = vy
            2 => (lz, lx, vv),     // north: plane = lz,   u = lx, v = vy
            3 => (lz + 1, lx, vv), // south: plane = lz+1
            4 => (lx, lz, vv),     // west:  plane = lx,   u = lz, v = vy
            _ => (lx + 1, lz, vv), // east:  plane = lx+1
        };
        (plane as usize, u as usize, v as usize)
    }

    fn insert(&mut self, face: usize, lx: i32, vy: i32, lz: i32, key: FaceKey) {
        let (plane, u, v) = self.cell(face, lx, vy, lz);
        self.faces[face].set(plane, u, v, key);
    }
}

/// World-space coordinate of the block that owns the face at mask cell
/// `(face, plane, u, v)` — the inverse of [`GreedyMasks::cell`], and the anchor
/// the rectangle emitter grows from.
fn mask_base_block(
    face: usize,
    plane: i32,
    u: i32,
    v: i32,
    base_x: i32,
    base_z: i32,
    y0: i32,
) -> [i32; 3] {
    match face {
        // up/down index planes vertically, so `plane` (not `v`) carries y.
        0 => [base_x + u, y0 + plane - 1, base_z + v],
        1 => [base_x + u, y0 + plane, base_z + v],
        2 => [base_x + u, y0 + v, base_z + plane],
        3 => [base_x + u, y0 + v, base_z + plane - 1],
        4 => [base_x + plane, y0 + v, base_z + u],
        _ => [base_x + plane - 1, y0 + v, base_z + u],
    }
}

/// Emit one merged rectangle: `width` unit faces along the face's u axis,
/// `height` along its v axis, anchored at `base`.
///
/// The generic form works for all six orientations because each face's UV is an
/// affine function of its two tangent corner coordinates with slope ±1 (north,
/// east and west are mirrored; up, down and south are direct). Scaling the
/// position offsets and the UV by the *same* factor therefore preserves the
/// per-tile mapping exactly — the texture **repeats** across the rectangle
/// instead of stretching, and the winding is untouched. At `width == height == 1`
/// this reduces to the legacy unit quad bit-for-bit.
fn emit_rect(
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    face: usize,
    base: [i32; 3],
    width: i32,
    height: i32,
    light: u32,
    tint: u32,
) {
    let (au, av, _) = FACE_AXES[face];
    let base_idx = vertices.len() as u32;
    for (corner, uv) in FACE_CORNERS[face] {
        let mut pos = [
            base[0] as f32 + corner[0],
            base[1] as f32 + corner[1],
            base[2] as f32 + corner[2],
        ];
        if width > 1 && corner[au] == 1.0 {
            pos[au] += (width - 1) as f32;
        }
        if height > 1 && corner[av] == 1.0 {
            pos[av] += (height - 1) as f32;
        }
        vertices.push(MeshVertex {
            pos,
            uv: [uv[0] * width as f32, uv[1] * height as f32],
            light,
            tint,
        });
    }
    indices.extend_from_slice(&[
        base_idx,
        base_idx + 1,
        base_idx + 2,
        base_idx,
        base_idx + 2,
        base_idx + 3,
    ]);
}

/// Greedy-mesh one plane in place and emit its rectangles. Returns the count.
///
/// Deterministic v-major / u-minor: at the first unconsumed cell, grow the
/// maximal run of equal keys along u, then the maximal stack of rows whose full
/// width matches, clear the rectangle, emit. Scanning order is fixed, so the
/// vertex stream is reproducible run to run.
#[allow(clippy::too_many_arguments)]
fn greedy_plane(
    cells: &mut [Option<FaceKey>],
    u_len: usize,
    v_len: usize,
    face: usize,
    plane: i32,
    base_x: i32,
    base_z: i32,
    y0: i32,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    let mut rects = 0;
    for v0 in 0..v_len {
        for u0 in 0..u_len {
            let Some(key) = cells[v0 * u_len + u0] else {
                continue;
            };
            let mut w = 1;
            while u0 + w < u_len && cells[v0 * u_len + u0 + w] == Some(key) {
                w += 1;
            }
            let mut h = 1;
            'grow: while v0 + h < v_len {
                for du in 0..w {
                    if cells[(v0 + h) * u_len + u0 + du] != Some(key) {
                        break 'grow;
                    }
                }
                h += 1;
            }
            for dv in 0..h {
                for du in 0..w {
                    cells[(v0 + dv) * u_len + u0 + du] = None;
                }
            }
            let base = mask_base_block(face, plane, u0 as i32, v0 as i32, base_x, base_z, y0);
            emit_rect(
                vertices, indices, face, base, w as i32, h as i32, key.light, key.tint,
            );
            rects += 1;
        }
    }
    rects
}

/// The production mesher: greedy-merged cube faces, legacy models and fluids.
///
/// A cube face merges when its four corner AO codes agree *and* it is not an up
/// (+Y) face; those are collected into per-(face, plane) masks and merged into
/// rectangles after the scan. Every other face — up faces, non-uniform AO, and
/// every model/fluid quad — is emitted exactly as [`mesh_column_reference`]
/// would. Merging never crosses a column boundary (the masks are column-local)
/// but freely crosses *section* boundaries, because a mask spans the column's
/// whole occupied height.
pub fn mesh_column(
    world: &World,
    table: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
    carried: &[Option<CarriedFluid>],
    cx: i32,
    cz: i32,
) -> Option<ColumnMesh> {
    let col = world.column(cx, cz)?;
    let shape = world.shape;
    let base_x = cx * 16;
    let base_z = cz * 16;

    let mut vertices: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut tvertices: Vec<MeshVertex> = Vec::new();
    let mut tindices: Vec<u32> = Vec::new();
    let mut tint_cache = TintCache::default();
    let mut y_min = f32::MAX;
    let mut y_max = f32::MIN;
    let mut bump = |y: f32| {
        y_min = y_min.min(y);
        y_max = y_max.max(y + 1.0);
    };

    let mut visible_cube_faces = 0u32;
    let mut greedy_candidate_faces = 0u32;
    let mut unit_fallback_faces = 0u32;
    let mut carried_fluid_cells = 0u32;

    // Size the masks to the nontrivial-section range, not the whole dimension:
    // an overworld column is 24 sections tall and typically holds terrain in a
    // handful of them.
    let (mut si_lo, mut si_hi) = (usize::MAX, 0usize);
    for si in 0..shape.section_count() {
        if !col.section_is_trivial(si) {
            if si_lo == usize::MAX {
                si_lo = si;
            }
            si_hi = si;
        }
    }
    let span = if si_lo == usize::MAX {
        0
    } else {
        (si_hi - si_lo + 1) * 16
    };
    let mut masks = GreedyMasks::new(span);
    masks.vy_lo = if si_lo == usize::MAX {
        0
    } else {
        (si_lo * 16) as i32
    };

    for si in 0..shape.section_count() {
        if col.section_is_trivial(si) {
            continue;
        }
        let sy = shape.min_y + (si as i32) * 16;
        for y in sy..sy + 16 {
            let vy = y - shape.min_y;
            for lz in 0..16 {
                for lx in 0..16 {
                    let wx = base_x + lx;
                    let wz = base_z + lz;
                    let state = world.block_state_at(wx, y, wz);
                    // `SectionCompiler.compile:89-97` — the fluid first, then
                    // the block, as two independent draws at one position. A
                    // waterlogged stair runs BOTH arms; a `LiquidBlock` only
                    // this one (its render shape is INVISIBLE).
                    if let Some(f) = fluid_at(table, carried, state) {
                        let (fv, fi) = if f.lava {
                            (&mut vertices, &mut indices)
                        } else {
                            (&mut tvertices, &mut tindices)
                        };
                        let fluid_verts_before = fv.len();
                        emit_fluid(
                            world,
                            table,
                            carried,
                            &mut tint_cache,
                            fv,
                            fi,
                            wx,
                            y,
                            wz,
                            f,
                        );
                        // Counted only when the draw produced geometry.
                        // `r48`'s claim is "meshed its water", and an increment
                        // beside the CALL would also fire for a cell whose six
                        // faces were all suppressed.
                        carried_fluid_cells +=
                            u32::from(f.carried && fv.len() > fluid_verts_before);
                        bump(y as f32);
                    }
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            for face in 0..6 {
                                let Some(cf) = cube_face(
                                    world,
                                    table,
                                    &mut tint_cache,
                                    wx,
                                    y,
                                    wz,
                                    face,
                                    faces,
                                    raw_faces,
                                    tint,
                                ) else {
                                    continue;
                                };
                                visible_cube_faces += 1;
                                // A face whose four corner AO codes agree has no
                                // gradient to lose, so it may merge. Anything
                                // else must stay a unit quad — and so must every
                                // up (+Y) face, whatever its AO.
                                //
                                // The up carve-out is a measurement, not a
                                // structural limit, and the measurement does not
                                // resolve to a single cause. Merging the up
                                // direction changed 11 pixels of the canonical
                                // demo render under normal UV, max channel delta
                                // 35. Re-running the same all-six vs no-up
                                // comparison with the world UV forced to a
                                // constant left 1 pixel. So UV interpolation /
                                // nearest sampling owns 10 of the 11 — a
                                // rectangle repeats its texture by scaling the UV
                                // past 1.0, which moves a nearest-sampled
                                // atlas/interpolation boundary *inside* the quad
                                // instead of leaving it on the quad's edge where
                                // the unit faces put it — and 1 pixel survives
                                // with UV held constant, so it is a
                                // topology/coverage difference the UV explanation
                                // does not cover. That residual pixel is not
                                // diagnosed. The other five directions came out
                                // byte-identical under the same comparison, so
                                // they merge and up keeps the legacy unit quads.
                                if face != 0
                                    && cf.ao[0] == cf.ao[1]
                                    && cf.ao[0] == cf.ao[2]
                                    && cf.ao[0] == cf.ao[3]
                                {
                                    greedy_candidate_faces += 1;
                                    masks.insert(
                                        face,
                                        lx,
                                        vy,
                                        lz,
                                        FaceKey {
                                            state,
                                            light: pack_light_word(
                                                cf.layer as u32,
                                                cf.lb,
                                                cf.ls,
                                                cf.shade,
                                                cf.ao[0],
                                            ),
                                            tint: pack_tint(cf.tint_rgb, 0),
                                        },
                                    );
                                } else {
                                    push_cube_face(
                                        &mut vertices,
                                        &mut indices,
                                        wx,
                                        y,
                                        wz,
                                        face,
                                        &cf,
                                    );
                                    unit_fallback_faces += 1;
                                }
                            }
                            bump(y as f32);
                        }
                        Some(RenderKind::Model(idx)) => {
                            emit_model(
                                world,
                                table,
                                models,
                                &mut tint_cache,
                                &mut vertices,
                                &mut indices,
                                wx,
                                y,
                                wz,
                                *idx,
                            );
                            bump(y as f32);
                        }
                        // Already drawn above (see the `fluid_at` block).
                        Some(RenderKind::Fluid { .. }) => {}
                        _ => {}
                    }
                }
            }
        }
    }

    // World y of mask row 0 / plane 0.
    let y0 = shape.min_y + masks.vy_lo;
    let mut greedy_quads = 0u32;
    for (face, mask) in masks.faces.iter_mut().enumerate() {
        let (u_len, v_len) = (mask.u_len, mask.v_len);
        for (plane, cells) in mask.planes.iter_mut().enumerate() {
            let Some(cells) = cells else { continue };
            greedy_quads += greedy_plane(
                cells,
                u_len,
                v_len,
                face,
                plane as i32,
                base_x,
                base_z,
                y0,
                &mut vertices,
                &mut indices,
            );
        }
    }

    if indices.is_empty() && tindices.is_empty() {
        return None;
    }
    Some(ColumnMesh {
        cx,
        cz,
        vertices,
        indices,
        tvertices,
        tindices,
        y_min,
        y_max,
        visible_cube_faces,
        greedy_candidate_faces,
        greedy_quads,
        unit_fallback_faces,
        carried_fluid_cells,
    })
}

/// Mesher face index -> the bit [`rewo_data::assets::FACE_DIRS`] uses for the
/// same direction, for reading [`CarriedFluid::self_occludes`].
///
/// The two crates order their faces differently and always have —
/// `FACE_OFFSETS` is `[up, down, north, south, west, east]` while `FACE_DIRS`
/// is `[west, east, down, up, north, south]`. A remap table is exactly the kind
/// of thing that inverts silently, so `self_occlude_remap_is_the_two_direction_
/// tables` derives it from both arrays rather than trusting these literals.
const SELF_OCCLUDE_REMAP: [u32; 6] = [3, 2, 4, 5, 0, 1];

/// One block state's fluid, as vanilla's `BlockState.getFluidState()` answers
/// it, reduced to what [`emit_fluid`] reads.
///
/// **This is the single query.** 26.2 stores the answer in two unrelated
/// places — `LiquidBlock.getFluidState` reads its own `LEVEL`, while a
/// `SimpleWaterloggedBlock` returns a source — and Rewo mirrors that split
/// (`RenderKind::Fluid` vs [`CarriedFluid`]) because the two really are
/// different things: one block *is* the fluid, the other *carries* it. What
/// must not be split is the lookup, so everything that asks goes through
/// [`fluid_at`]. `same()` in particular decides face suppression for ordinary
/// pools too, and a version of it that only knew about `RenderKind::Fluid`
/// would leave a visible internal wall wherever a pool meets a waterlogged
/// block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FluidHere {
    layer: u16,
    raw_layer: u16,
    level: u8,
    lava: bool,
    /// `isFaceOccludedBySelf`'s input, in the **mesher's** face order.
    self_occludes: u8,
    /// The fluid came from the carried table rather than from a `LiquidBlock`.
    carried: bool,
}

/// What `blockState.getFluidState()` answers for `state`, or `None` for
/// `Fluids.EMPTY`.
fn fluid_at(
    table: &[RenderKind],
    carried: &[Option<CarriedFluid>],
    state: u32,
) -> Option<FluidHere> {
    if let Some(RenderKind::Fluid {
        layer,
        raw_layer,
        level,
        lava,
    }) = table.get(state as usize)
    {
        return Some(FluidHere {
            layer: *layer,
            raw_layer: *raw_layer,
            level: *level,
            lava: *lava,
            // Plain water never suppresses its own faces —
            // `isFaceOccludedByState` (`FluidRenderer.java:37`) returns false at
            // its first branch, because water's own face-occlusion shape is
            // `Shapes.empty()`.
            //
            // NOT because water is `noOcclusion`: it is not.
            // `Blocks.java:285-297` builds WATER with `.replaceable()
            // .noCollision().strength(100).pushReaction(DESTROY).noLootTable()
            // .liquid().sound(EMPTY)` and no `noOcclusion()`, so `canOcclude`
            // stays **true** — and Rewo agrees, `minecraft:water` is not in
            // `block_light::NO_OCCLUDE`. The empty shape arrives the other way:
            // `BlockBehaviour:514` is `canOcclude ? getOcclusionShape : empty`,
            // `getOcclusionShape` (`:287-288`) delegates to `getShape`, and
            // `LiquidBlock.getShape` (`:145-146`) is `Shapes.empty()` — so
            // `:516-517` files every face under `EMPTY_OCCLUSION_SHAPES`.
            // Stated at length because the wrong reason invites adding water to
            // `NO_OCCLUDE`, which would change the light path for no reason.
            self_occludes: 0,
            carried: false,
        });
    }
    let f = (*carried.get(state as usize)?)?;
    let mut mask = 0u8;
    for (face, bit) in SELF_OCCLUDE_REMAP.iter().enumerate() {
        if f.self_occludes & (1 << bit) != 0 {
            mask |= 1 << face;
        }
    }
    Some(FluidHere {
        layer: f.layer,
        raw_layer: f.raw_layer,
        level: f.level,
        lava: false,
        self_occludes: mask,
        carried: true,
    })
}

/// Fluid surface height within its block, from the `level` property:
/// source = 8/9, flowing 1..7 shrink toward 1/9, ≥8 = falling (full).
fn fluid_h(level: u8) -> f32 {
    match level {
        0 => 8.0 / 9.0,
        1..=7 => (8 - level) as f32 / 9.0,
        _ => 1.0,
    }
}

/// The fluid's level at (x,y,z) if it is the same fluid type, else None.
///
/// This is vanilla's `isNeighborSameFluid` and `getHeight` sharing one input:
/// `neighborFluidState.getType().isSame(fluidState.getType())`. A waterlogged
/// block's type IS water, so it answers here.
fn fluid_level(
    world: &World,
    table: &[RenderKind],
    carried: &[Option<CarriedFluid>],
    x: i32,
    y: i32,
    z: i32,
    want_lava: bool,
) -> Option<u8> {
    let f = fluid_at(table, carried, world.block_state_at(x, y, z))?;
    (f.lava == want_lava).then_some(f.level)
}

/// Vanilla-style fluid cell: top face at per-corner heights (max over the
/// four touching same-fluid cells — a simpler take on vanilla's weighted
/// average that still reads as a continuous sloped surface), trapezoid
/// side faces down to the block floor, bottom face against air.
///
/// `f` is the cell's own fluid ([`fluid_at`]) — a `LiquidBlock`'s or a
/// waterlogged block's, indistinguishable from here, which is the point.
///
/// **`f.self_occludes` is applied to the four sides and the bottom and NOT to
/// the top**, and that asymmetry is vanilla's, not an oversight.
/// `FluidRenderer.tesselate:77` computes `renderUp` as
/// `!isNeighborSameFluid(..)` alone, while `:78-83` route the other five
/// through `shouldRenderFace`, which is
/// `!isNeighborSameFluid(..) && !isFaceOccludedBySelf(blockState, direction)`
/// (`:56-60`). So a waterlogged **top** slab really does emit a water surface
/// at y+8/9, buried inside its own geometry; adding the self test to the up
/// face "for consistency" deletes surfaces vanilla draws.
#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    world: &World,
    table: &[RenderKind],
    carried: &[Option<CarriedFluid>],
    cache: &mut TintCache,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    f: FluidHere,
) {
    let lava = f.lava;
    // Water gets the biome water tint (raw layer + dynamic color); lava never.
    let (fluid_layer, tint_rgb) = if lava {
        (f.layer, TINT_WHITE)
    } else {
        match biome_tint(world, cache, wx, y, wz, TintSource::Water) {
            Some(rgb) => (f.raw_layer, rgb),
            None => (f.layer, TINT_WHITE),
        }
    };
    let same = |x: i32, yy: i32, z: i32| {
        fluid_level(world, table, carried, x, yy, z, lava).is_some()
    };
    // Corner height at grid point (wx+dx, wz+dz): max over the 4 cells
    // sharing that corner; a cell with the same fluid above it is a full
    // column (1.0).
    let corner = |dx: i32, dz: i32| -> f32 {
        let mut h = 0.0f32;
        for (cx, cz) in [
            (wx + dx - 1, wz + dz - 1),
            (wx + dx, wz + dz - 1),
            (wx + dx - 1, wz + dz),
            (wx + dx, wz + dz),
        ] {
            if let Some(lv) = fluid_level(world, table, carried, cx, y, cz, lava) {
                let ch = if same(cx, y + 1, cz) {
                    1.0
                } else {
                    fluid_h(lv)
                };
                h = h.max(ch);
            }
        }
        h
    };
    let (h00, h10, h01, h11) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
    let (x0, x1) = (wx as f32, wx as f32 + 1.0);
    let (z0, z1) = (wz as f32, wz as f32 + 1.0);
    let yf = y as f32;

    // Face light mirrors the cube path (the cell the face looks into); lava
    // emits its own block light, so it stays bright in an unlit cave.
    let light = |x: i32, yy: i32, z: i32| -> (u8, u8) {
        if lava {
            (15, 0)
        } else {
            world.light_at(x, yy, z)
        }
    };
    // Fluid faces carry flat directional shade and no AO, so the AO code is the
    // identity level (AO_LEVELS[AO_NONE] == 1.0). The shade is the active
    // dimension's code for the face's direction (M16) — identity under the
    // default table, so the five call sites below read as the old literals.
    let mut quad = |p: [([f32; 3], [f32; 2]); 4], shade_code: u8, l: (u8, u8)| {
        let base = vertices.len() as u32;
        for (pos, uv) in p {
            vertices.push(MeshVertex::new(
                pos,
                uv,
                fluid_layer as u32,
                l.0,
                l.1,
                shade_code,
                AO_NONE,
                tint_rgb,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    // Top — unless the same fluid sits above.
    if !same(wx, y + 1, wz) {
        quad(
            [
                ([x0, yf + h00, z0], [0.0, 0.0]),
                ([x1, yf + h10, z0], [1.0, 0.0]),
                ([x1, yf + h11, z1], [1.0, 1.0]),
                ([x0, yf + h01, z1], [0.0, 1.0]),
            ],
            face_shade_code(world, 0),
            light(wx, y + 1, wz),
        );
    }
    // Sides — skip against the same fluid or a full opaque cube.
    // (north -Z, south +Z, west -X, east +X; corner pairs per edge.)
    let sides: [((i32, i32), usize, [([f32; 3], [f32; 2]); 4]); 4] = [
        (
            (0, -1),
            2,
            [
                ([x0, yf + h00, z0], [0.0, 1.0 - h00]),
                ([x1, yf + h10, z0], [1.0, 1.0 - h10]),
                ([x1, yf, z0], [1.0, 1.0]),
                ([x0, yf, z0], [0.0, 1.0]),
            ],
        ),
        (
            (0, 1),
            3,
            [
                ([x0, yf + h01, z1], [0.0, 1.0 - h01]),
                ([x1, yf + h11, z1], [1.0, 1.0 - h11]),
                ([x1, yf, z1], [1.0, 1.0]),
                ([x0, yf, z1], [0.0, 1.0]),
            ],
        ),
        (
            (-1, 0),
            4,
            [
                ([x0, yf + h00, z0], [0.0, 1.0 - h00]),
                ([x0, yf + h01, z1], [1.0, 1.0 - h01]),
                ([x0, yf, z1], [1.0, 1.0]),
                ([x0, yf, z0], [0.0, 1.0]),
            ],
        ),
        (
            (1, 0),
            5,
            [
                ([x1, yf + h10, z0], [0.0, 1.0 - h10]),
                ([x1, yf + h11, z1], [1.0, 1.0 - h11]),
                ([x1, yf, z1], [1.0, 1.0]),
                ([x1, yf, z0], [0.0, 1.0]),
            ],
        ),
    ];
    for ((dx, dz), face, corners) in sides {
        let (nx, nz) = (wx + dx, wz + dz);
        // `shouldRenderFace` = !isNeighborSameFluid && !isFaceOccludedBySelf.
        // The `is_full_cube` term is Rewo's older stand-in for vanilla's THIRD
        // test, `isFaceOccludedByNeighbor(faceDir, max(hh0,hh1), faceState)`
        // (`FluidRenderer.java:289`) — a height-aware shape query where this is
        // a full-cube one. Left alone here on purpose: it is a pre-existing
        // divergence of the plain-water path, so changing it moves the demo PNG
        // and belongs to whoever takes that on.
        if same(nx, y, nz)
            || f.self_occludes & (1 << face) != 0
            || is_full_cube(table, world.block_state_at(nx, y, nz))
        {
            continue;
        }
        quad(corners, face_shade_code(world, face), light(nx, y, nz));
    }
    // Bottom — against anything that isn't fluid, isn't covered by our own
    // shape, and isn't a full cube. Vanilla's `renderDown` has TWO neighbour
    // gates (`:78-79`): `shouldRenderFace` — whose self half is the new term —
    // and `!isFaceOccludedByNeighbor(DOWN, 0.8888889F, blockStateDown)`, the
    // hard-coded MAX_FLUID_HEIGHT. `is_full_cube` stands in for the second, as
    // on the sides.
    if !same(wx, y - 1, wz)
        && f.self_occludes & (1 << 1) == 0
        && !is_full_cube(table, world.block_state_at(wx, y - 1, wz))
    {
        quad(
            [
                ([x0, yf, z0], [0.0, 0.0]),
                ([x0, yf, z1], [0.0, 1.0]),
                ([x1, yf, z1], [1.0, 1.0]),
                ([x1, yf, z0], [1.0, 0.0]),
            ],
            face_shade_code(world, 1),
            light(wx, y - 1, wz),
        );
    }
}

/// Everything the cube path resolves for one visible face of one block.
///
/// Both mesher paths go through here, so the greedy path's merge key and the
/// fallback path's vertices are computed by the *same* code — they cannot drift.
struct CubeFace {
    layer: u16,
    tint_rgb: [u8; 3],
    lb: u8,
    ls: u8,
    /// AO code per `FACE_CORNERS[face]` index.
    ao: [u8; 4],
    /// [`face_shade_code`] for this face under the world's dimension. Resolved
    /// once, here, so the greedy merge key and the unit quad cannot disagree.
    shade: u8,
}

/// Resolve one cube face, or `None` if a full-cube neighbour culls it.
#[allow(clippy::too_many_arguments)]
fn cube_face(
    world: &World,
    table: &[RenderKind],
    cache: &mut TintCache,
    wx: i32,
    y: i32,
    wz: i32,
    face: usize,
    faces: &[u16; 6],
    raw_faces: &[u16; 6],
    tint: &[TintSource; 6],
) -> Option<CubeFace> {
    let (dx, dy, dz) = FACE_OFFSETS[face];
    let (nx, ny, nz) = (wx + dx, y + dy, wz + dz);
    if is_full_cube(table, world.block_state_at(nx, ny, nz)) {
        return None;
    }
    let (lb, ls) = world.light_at(nx, ny, nz);
    // Dynamic biome tint (or the legacy pre-tinted layer + white). The same
    // (wx,y,wz)+resolver recurs across the 6 faces; the cache serves it once.
    let (layer, tint_rgb) = match biome_tint(world, cache, wx, y, wz, tint[face]) {
        Some(rgb) => (raw_faces[face], rgb),
        None => (faces[face], TINT_WHITE),
    };
    let (uu, vv, (fnx, fny, fnz)) = FACE_AXES[face];
    let mut ao = [0u8; 4];
    for (i, (corner, _uv)) in FACE_CORNERS[face].iter().enumerate() {
        // AO from the three neighbors around this corner, in the layer
        // just outside the face.
        let su = 2 * corner[uu] as i32 - 1;
        let sv = 2 * corner[vv] as i32 - 1;
        let mut off_u = [0i32; 3];
        off_u[uu] = su;
        let mut off_v = [0i32; 3];
        off_v[vv] = sv;
        let np = [wx + fnx, y + fny, wz + fnz];
        let s1 = solid(
            world,
            table,
            [np[0] + off_u[0], np[1] + off_u[1], np[2] + off_u[2]],
        );
        let s2 = solid(
            world,
            table,
            [np[0] + off_v[0], np[1] + off_v[1], np[2] + off_v[2]],
        );
        let sc = solid(
            world,
            table,
            [
                np[0] + off_u[0] + off_v[0],
                np[1] + off_u[1] + off_v[1],
                np[2] + off_u[2] + off_v[2],
            ],
        );
        ao[i] = if s1 && s2 {
            0
        } else {
            3 - (s1 as u8 + s2 as u8 + sc as u8)
        };
    }
    Some(CubeFace {
        layer,
        tint_rgb,
        lb,
        ls,
        ao,
        shade: face_shade_code(world, face),
    })
}

/// Emit one resolved cube face as a legacy unit quad.
fn push_cube_face(
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    face: usize,
    f: &CubeFace,
) {
    let base_idx = vertices.len() as u32;
    for (i, (corner, uv)) in FACE_CORNERS[face].iter().enumerate() {
        vertices.push(MeshVertex::new(
            [
                wx as f32 + corner[0],
                y as f32 + corner[1],
                wz as f32 + corner[2],
            ],
            *uv,
            f.layer as u32,
            f.lb,
            f.ls,
            f.shade,
            f.ao[i],
            f.tint_rgb,
        ));
    }
    indices.extend_from_slice(&[
        base_idx,
        base_idx + 1,
        base_idx + 2,
        base_idx,
        base_idx + 2,
        base_idx + 3,
    ]);
}

/// Emit every visible face of one cube as unit quads. Returns the face count.
#[allow(clippy::too_many_arguments)]
fn emit_cube(
    world: &World,
    table: &[RenderKind],
    cache: &mut TintCache,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    faces: &[u16; 6],
    raw_faces: &[u16; 6],
    tint: &[TintSource; 6],
) -> u32 {
    let mut visible = 0;
    for face in 0..6 {
        let Some(f) = cube_face(world, table, cache, wx, y, wz, face, faces, raw_faces, tint)
        else {
            continue;
        };
        push_cube_face(vertices, indices, wx, y, wz, face, &f);
        visible += 1;
    }
    visible
}

#[allow(clippy::too_many_arguments)]
fn emit_model(
    world: &World,
    table: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
    cache: &mut TintCache,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    model_idx: u32,
) {
    let Some(quads) = models.get(model_idx as usize) else {
        return;
    };
    // Model quads get flat (per-face) shading; AO on arbitrary quads is an
    // M4-followon. Light is sampled from the cell the quad FACES, not the
    // block's own — vanilla's `renderModelFaceFlat` does the same. Sampling
    // the block's own cell reads the inside of a solid block, which is always
    // dark: grass_block renders as a Model (cube + overlay), so the whole
    // ground plane of an overworld would light at zero.
    let own_light = world.light_at(wx, y, wz);
    for quad in quads {
        if quad.cull >= 0 {
            let (dx, dy, dz) = FACE_OFFSETS[quad.cull as usize];
            if is_full_cube(table, world.block_state_at(wx + dx, y + dy, wz + dz)) {
                continue;
            }
        }
        // A shaded quad takes its direction's code under the active dimension
        // (M16), which is the identity `quad.dir` in every default-lit world.
        // `shade: false` means an unshaded quad and stays code 0 in *every*
        // dimension: FACE_SHADE[0] is exactly 1.0, so it reproduces the old
        // `1.0` multiplier bit-for-bit — vanilla's unshaded quads ignore
        // cardinal lighting the same way.
        let shade_code = if quad.shade {
            face_shade_code(world, quad.dir as usize)
        } else {
            0
        };
        // A quad facing into a solid neighbour (an interior face) has nothing
        // to sample, so it keeps the block's own cell.
        let (odx, ody, odz) = FACE_OFFSETS[quad.dir as usize];
        let (nbx, nby, nbz) = (wx + odx, y + ody, wz + odz);
        let (own_block, own_sky) = if is_full_cube(table, world.block_state_at(nbx, nby, nbz)) {
            own_light
        } else {
            world.light_at(nbx, nby, nbz)
        };
        // Dynamic biome tint (raw layer + tint color) or the legacy pre-tinted
        // layer. The tint is per-quad; AO/shade still vary per-vertex. A model's
        // quads at one block share a resolver+position, so the cache serves them
        // all from one computation.
        let (layer, tint_rgb) = match biome_tint(world, cache, wx, y, wz, quad.tint) {
            Some(rgb) => (quad.raw_layer, rgb),
            None => (quad.layer, TINT_WHITE),
        };
        let base_idx = vertices.len() as u32;
        for i in 0..4 {
            vertices.push(MeshVertex::new(
                [
                    wx as f32 + quad.verts[i][0],
                    y as f32 + quad.verts[i][1],
                    wz as f32 + quad.verts[i][2],
                ],
                quad.uv[i],
                layer as u32,
                own_block,
                own_sky,
                shade_code,
                AO_NONE,
                tint_rgb,
            ));
        }
        indices.extend_from_slice(&[
            base_idx,
            base_idx + 1,
            base_idx + 2,
            base_idx,
            base_idx + 2,
            base_idx + 3,
        ]);
    }
}

fn solid(world: &World, table: &[RenderKind], p: [i32; 3]) -> bool {
    is_full_cube(table, world.block_state_at(p[0], p[1], p[2]))
}

// -- M15 greedy oracle: emitted-geometry decoder ----------------------------
//
// A merged rectangle is deliberately *not* byte-comparable to the N unit quads
// it replaces, so diffing raw buffers against the reference could only ever say
// "different". The oracle therefore inverts the emitter: it re-derives each
// quad's face, plane, anchor block and extent purely from the emitted vertex
// positions, checks every corner against the generic scaling rule, and then
// enumerates the unit faces the rectangle covers. Comparing *that* expansion
// against `mesh_column_reference`'s is what proves the merge lost nothing.
//
// This lives in production (not `mod tests`) because `check_greedy_oracle` is a
// release-compiled gate. `mod tests` wraps these with `.expect(...)`, so both
// graders run the identical decode and cannot drift apart.

/// One unit face as both mesher paths must agree on it.
///
/// `light24` is the lower 24 bits (layer + block/sky light); the shade code
/// rides in `face` and AO is kept per corner, so nothing is smuggled through a
/// masked-off bit. Field order is the sort order — face, then block, then
/// appearance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(crate) struct UnitFace {
    pub face: u8,
    pub block: [i32; 3],
    pub light24: u32,
    pub ao: [u8; 4],
    pub tint: u32,
}

/// A unit face plus the dimensions of the rectangle it was emitted inside.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExpandedFace {
    pub f: UnitFace,
    pub w: i32,
    pub h: i32,
}

/// Split a vertex/index stream into quads, checking the `[0,1,2,0,2,3]` pattern
/// every emitter in this file uses.
pub(crate) fn split_quads(v: &[MeshVertex], idx: &[u32]) -> Result<Vec<[MeshVertex; 4]>, String> {
    if v.len() % 4 != 0 {
        return Err(format!(
            "geometry is not whole quads: {} vertices is not a multiple of 4",
            v.len()
        ));
    }
    if idx.len() != v.len() / 4 * 6 {
        return Err(format!(
            "index count {} is not 6 per quad for {} vertices",
            idx.len(),
            v.len()
        ));
    }
    for q in 0..v.len() / 4 {
        let b = (q * 4) as u32;
        if idx[q * 6..q * 6 + 6] != [b, b + 1, b + 2, b, b + 2, b + 3] {
            return Err(format!(
                "quad {q} winding: indices {:?} are not [{b},{},{},{b},{},{}]",
                &idx[q * 6..q * 6 + 6],
                b + 1,
                b + 2,
                b + 2,
                b + 3
            ));
        }
    }
    Ok(v.chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect())
}

/// Expand cube geometry back into the unit faces it covers.
pub(crate) fn expand_unit_faces(
    v: &[MeshVertex],
    idx: &[u32],
) -> Result<Vec<ExpandedFace>, String> {
    expand_quad_list(&split_quads(v, idx)?)
}

/// [`expand_unit_faces`] over an already-selected quad list. A mixed column's
/// opaque stream also carries model and fluid quads, which are not axis-aligned
/// unit faces and must be filtered out before this runs.
pub(crate) fn expand_quad_list(qs: &[[MeshVertex; 4]]) -> Result<Vec<ExpandedFace>, String> {
    let mut out = Vec::new();
    for q in qs.iter().copied() {
        // The decoder recovers a quad's direction from its shade code, which is
        // only a bijection under `CardinalLighting::DEFAULT` (M16: the Nether
        // maps up *and* down onto code 6). The oracle column is a default-lit
        // world, so this holds there; anything else is refused rather than
        // indexed out of `FACE_AXES`.
        let face = q[0].shade_code() as usize;
        if face >= FACE_OFFSETS.len() {
            return Err(format!(
                "shade code {face} is not a face direction — unit-face expansion only decodes DEFAULT cardinal lighting"
            ));
        }
        let (au, av, _) = FACE_AXES[face];
        let an = 3 - au - av; // the remaining axis: 0+1+2 == 3
        let plane = q[0].pos[an];
        for vtx in &q {
            if vtx.pos[an] != plane {
                return Err(format!(
                    "face {face}: plane axis {an} is not constant ({} vs {plane})",
                    vtx.pos[an]
                ));
            }
        }
        let span = |axis: usize| {
            let lo = q.iter().map(|x| x.pos[axis]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|x| x.pos[axis]).fold(f32::MIN, f32::max);
            (lo, hi)
        };
        let (umin, umax) = span(au);
        let (vmin, vmax) = span(av);
        let (w, h) = ((umax - umin) as i32, (vmax - vmin) as i32);
        if w < 1 || h < 1 {
            return Err(format!("face {face}: degenerate rectangle {w}x{h}"));
        }

        // Anchor block: the face's own axis sits at `plane` minus whichever side
        // of the block the face is on (up/south/east are the +1 side).
        let mut base = [0i32; 3];
        base[au] = umin as i32;
        base[av] = vmin as i32;
        base[an] = (plane - FACE_CORNERS[face][0].0[an]) as i32;

        // Every corner must match the generic emitter rule exactly, and the UV
        // must scale with the extent (repeat, not stretch).
        for (i, (corner, uv)) in FACE_CORNERS[face].iter().enumerate() {
            let mut expect = [
                base[0] as f32 + corner[0],
                base[1] as f32 + corner[1],
                base[2] as f32 + corner[2],
            ];
            if corner[au] == 1.0 {
                expect[au] += (w - 1) as f32;
            }
            if corner[av] == 1.0 {
                expect[av] += (h - 1) as f32;
            }
            if q[i].pos != expect {
                return Err(format!(
                    "face {face} corner {i} position {:?} != expected {expect:?} for a {w}x{h} rectangle at {base:?}",
                    q[i].pos
                ));
            }
            let expect_uv = [uv[0] * w as f32, uv[1] * h as f32];
            if q[i].uv != expect_uv {
                return Err(format!(
                    "face {face} corner {i} uv {:?} != expected {expect_uv:?} (a rectangle must repeat its texture, not stretch it)",
                    q[i].uv
                ));
            }
        }

        let light24 = q[0].light & 0x00FF_FFFF;
        let tint = q[0].tint;
        for vtx in &q {
            if vtx.light & 0x00FF_FFFF != light24 {
                return Err(format!(
                    "face {face} at {base:?}: layer/light is not per-quad ({:#08x} vs {light24:#08x})",
                    vtx.light & 0x00FF_FFFF
                ));
            }
            if vtx.tint != tint {
                return Err(format!(
                    "face {face} at {base:?}: tint is not per-quad ({:#010x} vs {tint:#010x})",
                    vtx.tint
                ));
            }
        }
        let ao = [
            q[0].ao_code(),
            q[1].ao_code(),
            q[2].ao_code(),
            q[3].ao_code(),
        ];
        for dv in 0..h {
            for du in 0..w {
                let mut block = base;
                block[au] += du;
                block[av] += dv;
                out.push(ExpandedFace {
                    f: UnitFace {
                        face: face as u8,
                        block,
                        light24,
                        ao,
                        tint,
                    },
                    w,
                    h,
                });
            }
        }
    }
    Ok(out)
}

// -- M15 greedy oracle: the adversarial cube gate ---------------------------

/// Vanilla's spruce/birch `BlockTintSources.constant` colors — a real pair of
/// distinct constants, used here as the tint-boundary material.
const ORACLE_SPRUCE_RGB: [u8; 3] = [97, 153, 97];
const ORACLE_BIRCH_RGB: [u8; 3] = [128, 167, 85];

// Fixture block states (indices into `oracle_table`).
const OS_GRANITE: u32 = 1;
const OS_DIORITE: u32 = 2;
const OS_CUTOUT: u32 = 3;
const OS_SPRUCE: u32 = 4;
const OS_BIRCH: u32 = 5;

/// The atlas layer `GRANITE`, `DIORITE` and `CUTOUT` all carry on their **down**
/// face. Deliberately shared: it makes the block state the only thing that can
/// separate them in a down plane.
const OS_SHARED_DOWN_LAYER: u16 = 11;
/// The layer `SPRUCE`/`BIRCH` resolve to once the Constant tint engages (their
/// `raw_faces`). Observing this rather than `OS_TINT_LEGACY_LAYER` is what
/// proves the tint path actually ran.
const OS_TINT_RAW_LAYER: u16 = 51;
const OS_TINT_LEGACY_LAYER: u16 = 50;

/// The fixture's render table.
///
/// Every cube carries *distinct* layers per direction, so a merge that crossed a
/// face boundary would show up in the layer index. The three opaque materials
/// share exactly one layer — the down face — which is the material-boundary
/// probe. `CUTOUT` stands in for an alpha-tested block: `RenderKind::Cube` is
/// the pass's only cube representation (there is no cutout flag in the mesh
/// path), so a cutout block is a `Cube` with its own layers, and what the gate
/// grades is that it stays semantically present and never coalesces with a
/// neighbouring material.
fn oracle_table() -> Vec<RenderKind> {
    let opaque = |faces: [u16; 6]| RenderKind::Cube {
        faces,
        raw_faces: faces,
        tint: [TintSource::None; 6],
    };
    let tinted = |rgb: [u8; 3]| RenderKind::Cube {
        faces: [OS_TINT_LEGACY_LAYER; 6],
        raw_faces: [OS_TINT_RAW_LAYER; 6],
        tint: [TintSource::Constant(rgb); 6],
    };
    vec![
        RenderKind::Invisible,
        opaque([10, OS_SHARED_DOWN_LAYER, 12, 13, 14, 15]), // GRANITE
        opaque([20, OS_SHARED_DOWN_LAYER, 22, 23, 24, 25]), // DIORITE
        opaque([30, OS_SHARED_DOWN_LAYER, 32, 33, 34, 35]), // CUTOUT (alpha-test)
        tinted(ORACLE_SPRUCE_RGB),
        tinted(ORACLE_BIRCH_RGB),
    ]
}

/// A minimal biome context. `TintSource::Constant` short-circuits *after*
/// `biome_tint`'s `world.biome_context()?` gate, so without a context attached
/// the tint materials would silently fall back to the legacy pre-tinted layer
/// and the tint-boundary probe would be vacuous. Nothing here is consulted — a
/// constant tint reads no registry and no colormap — it only opens the gate.
fn oracle_biome_context() -> rewo_world::biome::BiomeContext {
    use rewo_world::biome::{BiomeContext, BiomeDef, BiomeRegistry, Colormaps, GrassModifier};
    let def = BiomeDef {
        music_volume: None,
        name: "rewo:oracle".into(),
        temperature: 0.5,
        downfall: 0.5,
        water_color: 0,
        grass_override: None,
        foliage_override: None,
        dry_foliage_override: None,
        grass_modifier: GrassModifier::None,
        sky_color: None,
        fog_color: None,
            has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
    BiomeContext::new(
        std::sync::Arc::new(BiomeRegistry::new(vec![def])),
        Colormaps::neutral(),
        0,
    )
}

/// The adversarial column — one column at (0,0) holding every property the gate
/// grades. Regions are separated by at least one block of air on the axis that
/// matters, so each probe is independent of the others (AO reaches one cell past
/// a face, so a one-block gap is enough).
///
/// - **A** `x1..6 z1..6 y60..69` GRANITE — the merge core. Its four walls are
///   6×10 and straddle the `y=64` section floor; its floor is a 6×6; its 36 tops
///   are the up carve-out.
/// - **B** `x1..6 z=10 y64..69` GRANITE + an occluder at `(2,65,9)` — the AO
///   discontinuity. The occluder sits diagonally off the wall's north side, so
///   it darkens single corners of nearby faces without culling them.
/// - **C/D** `x8..15 z1..9 y=62` — three z-bands of GRANITE / DIORITE / CUTOUT
///   sharing one down layer: the material + cutout boundary.
/// - **E** `x8..15 z11..14` at `y=66` and `y=70` — the light boundaries; the
///   pokes land on the *sampled neighbour* cells (`y=65` block, `y=69` sky),
///   because a face reads light from the cell it faces, not from its own.
/// - **F** `x1..8 z11..14 y=62` — SPRUCE | BIRCH, the constant-tint boundary.
fn oracle_world() -> World {
    use rewo_world::dimension::DimensionShape;
    use rewo_world::light::Channel;

    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    w.set_biome_context(std::sync::Arc::new(oracle_biome_context()));

    // A — the merge core, crossing the y=64 section floor.
    for x in 1..=6 {
        for z in 1..=6 {
            for y in 60..=69 {
                w.set_block(x, y, z, OS_GRANITE);
            }
        }
    }

    // B — the AO discontinuity wall + its diagonal occluder.
    for x in 1..=6 {
        for y in 64..=69 {
            w.set_block(x, y, 10, OS_GRANITE);
        }
    }
    w.set_block(2, 65, 9, OS_GRANITE);

    // C/D — three materials, one shared down layer, one plane.
    for x in 8..=15 {
        for z in 1..=9 {
            let state = match z {
                1..=3 => OS_GRANITE,
                4..=6 => OS_DIORITE,
                _ => OS_CUTOUT,
            };
            w.set_block(x, 62, z, state);
        }
    }

    // E — one plate per light channel.
    for x in 8..=15 {
        for z in 11..=14 {
            w.set_block(x, 66, z, OS_GRANITE); // grades block light at y=65
            w.set_block(x, 70, z, OS_GRANITE); // grades sky light at y=69
        }
    }

    // F — the constant-tint boundary.
    for x in 1..=8 {
        for z in 11..=14 {
            w.set_block(x, 62, z, if x <= 4 { OS_SPRUCE } else { OS_BIRCH });
        }
    }

    // The light discontinuities. `Column::empty_lit` already carries a full
    // sky array (every nibble 15) and no block array (reads 0), so poking one
    // channel in one region cannot disturb any other face's light.
    let shape = w.shape;
    if let Some(col) = w.column_mut(0, 0) {
        for x in 12..=15 {
            for z in 11..=14 {
                col.set_light(&shape, Channel::Block, x, 65, z, 12);
                col.set_light(&shape, Channel::Sky, x, 69, z, 7);
            }
        }
    }
    w
}

// -- M15 greedy oracle: the legacy (non-cube) controls ----------------------
//
// Only the cube path can merge. Models and fluids run the untouched
// `emit_model` / `emit_fluid` code, so for them the optimized mesher must be
// *byte-identical* to the reference — a strictly stronger claim than the cube
// path's semantic equality, and the one that proves the greedy pass did not
// perturb scan order, buffer interleaving or the opaque/translucent split.
//
// These are the constructions `mod tests` already uses, hoisted into production
// so the controls and the unit tests are provably the same fixture (the tests
// delegate to them). The combined water+lava test column is split into two
// single-fluid controls so each stream is graded on its own.

/// States: 0 air, 1 an opaque cube (unplaced here), 2 water, 3 lava.
fn oracle_fluid_table() -> Vec<RenderKind> {
    vec![
        RenderKind::Invisible,
        RenderKind::Cube {
            faces: [0; 6],
            raw_faces: [0; 6],
            tint: [TintSource::None; 6],
        },
        RenderKind::Fluid {
            layer: 1,
            raw_layer: 1,
            level: 0,
            lava: false,
        },
        RenderKind::Fluid {
            layer: 2,
            raw_layer: 2,
            level: 0,
            lava: true,
        },
    ]
}

fn oracle_model_table() -> Vec<RenderKind> {
    vec![RenderKind::Invisible, RenderKind::Model(0)]
}

/// Two quads: an unculled shaded one facing north, and a culled unshaded one
/// facing south — so the control covers the cull branch, the `shade: false`
/// branch and the raw/legacy layer split.
fn oracle_model_quads() -> Vec<Vec<rewo_data::assets::Quad>> {
    use rewo_data::assets::Quad;
    vec![vec![
        Quad {
            verts: [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 7,
            raw_layer: 8,
            cull: -1,
            dir: 2,
            tint: TintSource::None,
            shade: true,
        },
        Quad {
            verts: [
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 9,
            raw_layer: 9,
            cull: 3,
            dir: 3,
            tint: TintSource::None,
            shade: false,
        },
    ]]
}

/// A patch of models spanning the `y=64` section boundary, to exercise ordering.
fn oracle_model_world() -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    for x in 3..7 {
        for z in 3..7 {
            w.set_block(x, 63, z, 1);
            w.set_block(x, 64, z, 1);
        }
    }
    w
}

/// Water only — a two-deep pool, so the control covers the submerged-cell
/// full-height branch as well as the surface.
fn oracle_water_world() -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    for x in 3..7 {
        for z in 3..7 {
            w.set_block(x, 62, z, 2);
            w.set_block(x, 63, z, 2);
        }
    }
    w
}

/// Lava only — opaque and fullbright, so it lands in the *opaque* stream.
fn oracle_lava_world() -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    for x in 3..7 {
        for z in 3..7 {
            w.set_block(x + 8, 62, z, 3);
        }
    }
    w
}

/// Geometry a legacy (non-cube) control produced. Identical in both meshers by
/// construction, so one set of counts describes both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyControlCounts {
    pub opaque_vertices: usize,
    pub opaque_indices: usize,
    pub translucent_vertices: usize,
    pub translucent_indices: usize,
}

// -- M161: the waterlogged controls ----------------------------------------
//
// Face indices below are the MESHER's, except inside `CarriedFluid`, whose
// `self_occludes` is `rewo_data::assets::FACE_DIRS`-ordered (see
// `SELF_OCCLUDE_REMAP`). `DOWN` is `FACE_DIRS` index 2 and `UP` is 3.

/// `self_occludes` for a shape flush with the bottom of its block (a bottom
/// slab, a stair's lower step) — `FACE_DIRS` order.
const ORACLE_OCC_DOWN: u8 = 1 << 2;
/// `self_occludes` for a shape flush with the top of its block (a top slab).
const ORACLE_OCC_UP: u8 = 1 << 3;
/// All six — a `type=double` slab, and the ONLY shape in the fixture that
/// occludes a SIDE. Without it the side half of `shouldRenderFace` has no
/// witness at all, which is how M161's battery first found it SURVIVING.
///
/// Measured against the real bake, the 68 full-cube carriers are exactly the
/// waterlogged double slabs (`blockentityshot` names them) — but **that state
/// is command-only**, and an earlier version of this comment called it "the
/// common real case", which is backwards. `SlabBlock.getStateForPlacement:72`
/// writes `WATERLOGGED, false` the moment a slab becomes DOUBLE, and
/// `placeLiquid` / `canPlaceLiquid` (`:106-113`) both refuse a DOUBLE slab, so
/// `type=double, waterlogged=true` is reachable only through `/setblock`, a
/// structure or a datapack. It is a real state that vanilla renders this way,
/// so the fixture stays; what was wrong was the reason given for it. The side
/// rule's *ordinary* case is a waterlogged stair — 1,536 of the 2,560
/// waterlogged stair states carry a side bit, measured in `blockentityshot`.
const ORACLE_OCC_ALL: u8 = 0b11_1111;

/// A DRY `Model(0)`; the two waterlogged states below share its geometry.
const WL_DRY: u32 = 4;
/// The same model, waterlogged, with a bottom-slab occlusion shape.
const WL_BOTTOM_SLAB: u32 = 5;
/// The same model, waterlogged, with a top-slab occlusion shape.
const WL_TOP_SLAB: u32 = 6;
/// The same model, waterlogged, occluding on every face — a double slab.
const WL_DOUBLE_SLAB: u32 = 7;
/// A plain water source (`oracle_fluid_table`'s).
const WL_WATER: u32 = 2;

/// [`oracle_fluid_table`] (0 air, 1 an opaque cube, 2 water, **3 LAVA**) plus
/// three `Model(0)` states.
///
/// The three models share one model index deliberately: the only input that
/// differs between the dry control and the two waterlogged blocks is the
/// carried-fluid table, so a difference in the output cannot come from the
/// geometry. `oracle_waterlogged_states_are_what_they_are_named` pins the
/// indices — an earlier draft of this fixture counted the base table as three
/// entries rather than four and graded LAVA as its "dry model", and the only
/// thing that noticed was the opaque-byte-identity witness.
fn oracle_waterlogged_table() -> Vec<RenderKind> {
    let mut t = oracle_fluid_table();
    debug_assert_eq!(t.len(), WL_DRY as usize);
    for _ in WL_DRY..=WL_DOUBLE_SLAB {
        t.push(RenderKind::Model(0));
    }
    t
}

fn oracle_waterlogged_carried() -> Vec<Option<CarriedFluid>> {
    let wl = |self_occludes| {
        Some(CarriedFluid {
            layer: 1,
            raw_layer: 1,
            level: 0,
            falling: false,
            self_occludes,
        })
    };
    let mut c: Vec<Option<CarriedFluid>> = vec![None; WL_DOUBLE_SLAB as usize + 1];
    c[WL_BOTTOM_SLAB as usize] = wl(ORACLE_OCC_DOWN);
    c[WL_TOP_SLAB as usize] = wl(ORACLE_OCC_UP);
    c[WL_DOUBLE_SLAB as usize] = wl(ORACLE_OCC_ALL);
    c
}

/// One block of `state` in mid-air at (2, 62, 2) — nothing below, so the
/// fluid's down face is decided by the block's OWN occlusion shape alone.
fn oracle_waterlogged_world(state: u32) -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    w.set_block(2, 62, 2, state);
    w
}

/// A water source at (8, 62, 2) with `east` at (9, 62, 2) — the `same()`
/// witness. Everything else is air, so the only face in the plane x = 9 is the
/// pool's east side and (if `east` carries water) that block's west side.
fn oracle_pool_beside(east: u32) -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    w.set_block(8, 62, 2, WL_WATER);
    w.set_block(9, 62, 2, east);
    w
}

/// Classify a translucent stream's fluid quads by geometry alone — `(top, side,
/// bottom)`. Reads the emitted vertices, not the emitter: a bottom face has all
/// four vertices on the block floor, a top face none, a side face two.
fn fluid_face_counts(v: &[MeshVertex], floor: f32) -> (usize, usize, usize) {
    let (mut top, mut side, mut bottom) = (0, 0, 0);
    for q in v.chunks_exact(4) {
        match q.iter().filter(|x| (x.pos[1] - floor).abs() < 1e-6).count() {
            4 => bottom += 1,
            0 => top += 1,
            _ => side += 1,
        }
    }
    (top, side, bottom)
}

/// Quads whose four vertices all sit on the plane `x`.
fn quads_on_x_plane(v: &[MeshVertex], x: f32) -> usize {
    v.chunks_exact(4)
        .filter(|q| q.iter().all(|p| (p.pos[0] - x).abs() < 1e-6))
        .count()
}

/// What [`check_waterlogged`] measured. Every number is observed — the checker
/// returns `Err` before a report exists if any of them is wrong.
#[derive(Clone, Copy, Debug, Default)]
pub struct WaterloggedFaceCounts {
    /// `(top, side, bottom)` for the bottom-slab-shaped waterlogged block.
    pub bottom_slab: (usize, usize, usize),
    /// The same for the top-slab-shaped one.
    pub top_slab: (usize, usize, usize),
    /// And for the one that occludes on all six faces.
    pub double_slab: (usize, usize, usize),
    /// Translucent quads the DRY twin produced. Must be 0.
    pub dry_translucent_quads: usize,
    /// `ColumnMesh::carried_fluid_cells` for the bottom-slab world / the dry one.
    pub carried_cells: (u32, u32),
    /// Quads in the plane x = 9 with a waterlogged / a dry east neighbour.
    pub pool_plane_faces: (usize, usize),
}

/// Grade the M161 waterlogged path. Serverless, asset-free, and driven through
/// the production [`mesh_column`] — the fixture supplies only the two tables the
/// bake supplies in production.
fn check_waterlogged() -> Result<WaterloggedFaceCounts, String> {
    let table = oracle_waterlogged_table();
    let carried = oracle_waterlogged_carried();
    let models = oracle_model_quads();
    // The fixture's own indices, asserted before anything is graded against
    // them: `WL_DRY` must be a model that carries nothing, and the two
    // waterlogged states must be the SAME model that does. Without this the
    // checker happily grades whatever happens to sit at those indices.
    for (st, want_carried) in [
        (WL_DRY, false),
        (WL_BOTTOM_SLAB, true),
        (WL_TOP_SLAB, true),
        (WL_DOUBLE_SLAB, true),
    ] {
        if !matches!(table.get(st as usize), Some(RenderKind::Model(0))) {
            return Err(format!(
                "waterlogged fixture: state {st} is {:?}, not Model(0)",
                table.get(st as usize)
            ));
        }
        if carried.get(st as usize).copied().flatten().is_some() != want_carried {
            return Err(format!(
                "waterlogged fixture: state {st} carries {:?}, expected carried={want_carried}",
                carried.get(st as usize)
            ));
        }
    }
    let mesh = |state: u32| {
        mesh_column(
            &oracle_waterlogged_world(state),
            &table,
            &models,
            &carried,
            0,
            0,
        )
    };

    // 1. One state, two draws (`SectionCompiler.compile:89-97`). The dry twin is
    //    the same `RenderKind::Model` with the same model index, so the only
    //    input that differs is the carried-fluid table.
    let wl = mesh(WL_BOTTOM_SLAB).ok_or("waterlogged: the bottom-slab world meshed to nothing")?;
    let dry = mesh(WL_DRY).ok_or("waterlogged: the dry world meshed to nothing")?;
    if wl.vertices.is_empty() {
        return Err("waterlogged: the block's own model produced no opaque geometry".into());
    }
    if wl.tvertices.is_empty() {
        return Err(
            "waterlogged: the block carried water and produced NO translucent geometry — this is the whole feature"
                .into(),
        );
    }
    if !dry.tvertices.is_empty() {
        return Err(format!(
            "waterlogged: the DRY twin produced {} translucent vertices — the carried-fluid table, not the render kind, must decide",
            dry.tvertices.len()
        ));
    }
    if bytemuck::cast_slice::<_, u8>(&wl.vertices) != bytemuck::cast_slice::<_, u8>(&dry.vertices) {
        return Err(
            "waterlogged: the block's own opaque geometry changed when it carried water — the two draws must be independent"
                .into(),
        );
    }
    if (wl.carried_fluid_cells, dry.carried_fluid_cells) != (1, 0) {
        return Err(format!(
            "waterlogged: carried_fluid_cells is {} / {}, expected 1 / 0",
            wl.carried_fluid_cells, dry.carried_fluid_cells
        ));
    }

    // 2. `isFaceOccludedBySelf` on the down face, and its ABSENCE on the up one.
    //    The two worlds differ in exactly one bit of `self_occludes`.
    let top = mesh(WL_TOP_SLAB).ok_or("waterlogged: the top-slab world meshed to nothing")?;
    let a = fluid_face_counts(&wl.tvertices, 62.0);
    let b = fluid_face_counts(&top.tvertices, 62.0);
    if a != (1, 4, 0) {
        return Err(format!(
            "waterlogged: a block whose own DOWN face is covered emitted (top,side,bottom) {a:?}, expected (1,4,0) — `shouldRenderFace` (FluidRenderer:56-60) must suppress the down face"
        ));
    }
    if b != (1, 4, 1) {
        return Err(format!(
            "waterlogged: a block whose own UP face is covered emitted (top,side,bottom) {b:?}, expected (1,4,1) — `renderUp` (FluidRenderer:77) does NOT go through `shouldRenderFace`, so the top face survives, and the self mask must not touch the sides"
        ));
    }
    // The real case: a `type=double` slab occludes on every direction, so
    // `shouldRenderFace` suppresses all four sides AND the bottom, and the ONE
    // face vanilla still draws is the top — which is the whole point of
    // `renderUp` skipping the self test. The two witnesses above cannot see the
    // side half of the test at all (their masks name only up and down), which
    // M161's mutation battery found by watching a dropped side test survive.
    let d = mesh(WL_DOUBLE_SLAB).ok_or("waterlogged: the double-slab world meshed to nothing")?;
    let dbl = fluid_face_counts(&d.tvertices, 62.0);
    if dbl != (1, 0, 0) {
        return Err(format!(
            "waterlogged: a block that occludes on ALL SIX faces emitted (top,side,bottom) {dbl:?}, expected (1,0,0) — every face but the top goes through `shouldRenderFace`, and the top does not"
        ));
    }

    // 3. `isNeighborSameFluid` — an ordinary pool must stop drawing toward a
    //    waterlogged block, because that block's fluid type IS water.
    let near = |east: u32| {
        mesh_column(&oracle_pool_beside(east), &table, &models, &carried, 0, 0)
            .ok_or("waterlogged: a pool world meshed to nothing")
    };
    let toward_wl = quads_on_x_plane(&near(WL_BOTTOM_SLAB)?.tvertices, 9.0);
    let toward_dry = quads_on_x_plane(&near(WL_DRY)?.tvertices, 9.0);
    if (toward_wl, toward_dry) != (0, 1) {
        return Err(format!(
            "waterlogged: the plane x=9 holds {toward_wl} quads beside a waterlogged block and {toward_dry} beside a dry one, expected 0 and 1 — `same()` decides face suppression for ORDINARY pools too, so a version that only knows `RenderKind::Fluid` leaves a visible internal wall in every underwater build"
        ));
    }

    Ok(WaterloggedFaceCounts {
        bottom_slab: a,
        top_slab: b,
        double_slab: dbl,
        dry_translucent_quads: dry.tvertices.len() / 4,
        carried_cells: (wl.carried_fluid_cells, dry.carried_fluid_cells),
        pool_plane_faces: (toward_wl, toward_dry),
    })
}

/// Grade one legacy control: the optimized mesher must reproduce the reference
/// **byte for byte** on both streams, touch no cube metrics, and actually
/// produce the stream(s) the fixture exists to cover.
#[allow(clippy::too_many_arguments)]
fn oracle_check_legacy_control(
    world: &World,
    table: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
    carried: &[Option<CarriedFluid>],
    what: &str,
    want_opaque: bool,
    want_translucent: bool,
) -> Result<LegacyControlCounts, String> {
    let r = mesh_column_reference(world, table, models, carried, 0, 0)
        .ok_or_else(|| format!("{what} control: the reference mesher produced nothing"))?;
    let o = mesh_column(world, table, models, carried, 0, 0)
        .ok_or_else(|| format!("{what} control: the optimized mesher produced nothing"))?;

    // Non-vacuity first: a control that emitted nothing would "match" trivially.
    if want_opaque && o.vertices.is_empty() {
        return Err(format!(
            "{what} control is vacuous: the opaque stream is empty, but this fixture exists to populate it"
        ));
    }
    if !want_opaque && !o.vertices.is_empty() {
        return Err(format!(
            "{what} control: {} vertices reached the opaque stream, which this fixture must leave empty",
            o.vertices.len()
        ));
    }
    if want_translucent && o.tvertices.is_empty() {
        return Err(format!(
            "{what} control is vacuous: the translucent stream is empty, but this fixture exists to populate it"
        ));
    }
    if !want_translucent && !o.tvertices.is_empty() {
        return Err(format!(
            "{what} control: {} vertices reached the translucent stream, which this fixture must leave empty",
            o.tvertices.len()
        ));
    }

    // Byte equality on both streams — models and fluids never merge, so nothing
    // about their geometry may change.
    let (ov, rv) = (
        bytemuck::cast_slice::<_, u8>(&o.vertices),
        bytemuck::cast_slice::<_, u8>(&r.vertices),
    );
    if ov != rv {
        let at = ov
            .iter()
            .zip(rv)
            .position(|(a, b)| a != b)
            .map(|i| format!("first differing byte at {i}"))
            .unwrap_or_else(|| format!("lengths {} vs {}", ov.len(), rv.len()));
        return Err(format!(
            "{what} control: opaque vertex bytes are not identical to the reference ({at}) — a non-cube path must pass through the greedy mesher untouched"
        ));
    }
    if o.indices != r.indices {
        return Err(format!(
            "{what} control: opaque indices differ from the reference ({} vs {} entries)",
            o.indices.len(),
            r.indices.len()
        ));
    }
    let (otv, rtv) = (
        bytemuck::cast_slice::<_, u8>(&o.tvertices),
        bytemuck::cast_slice::<_, u8>(&r.tvertices),
    );
    if otv != rtv {
        let at = otv
            .iter()
            .zip(rtv)
            .position(|(a, b)| a != b)
            .map(|i| format!("first differing byte at {i}"))
            .unwrap_or_else(|| format!("lengths {} vs {}", otv.len(), rtv.len()));
        return Err(format!(
            "{what} control: translucent vertex bytes are not identical to the reference ({at})"
        ));
    }
    if o.tindices != r.tindices {
        return Err(format!(
            "{what} control: translucent indices differ from the reference ({} vs {} entries)",
            o.tindices.len(),
            r.tindices.len()
        ));
    }
    if (o.y_min, o.y_max) != (r.y_min, r.y_max) {
        return Err(format!(
            "{what} control: vertical bounds differ ({}..{} vs {}..{})",
            o.y_min, o.y_max, r.y_min, r.y_max
        ));
    }

    // Nothing here is a cube, so every cube metric must be zero — otherwise the
    // greedy path is claiming faces it does not own.
    let metrics = (
        o.visible_cube_faces,
        o.greedy_candidate_faces,
        o.greedy_quads,
        o.unit_fallback_faces,
    );
    if metrics != (0, 0, 0, 0) {
        return Err(format!(
            "{what} control: cube metrics are {metrics:?}, expected all zero — models and fluids are not cube faces"
        ));
    }

    Ok(LegacyControlCounts {
        opaque_vertices: o.vertices.len(),
        opaque_indices: o.indices.len(),
        translucent_vertices: o.tvertices.len(),
        translucent_indices: o.tindices.len(),
    })
}

/// What [`check_greedy_oracle`] measured. Every number is observed, not
/// assumed — the checker returns `Err` before building one of these if any
/// property failed.
#[derive(Clone, Debug)]
pub struct GreedyOracleReport {
    /// Unit faces the frozen reference mesher emitted (one quad each).
    pub reference_unit_faces: usize,
    pub reference_quads: usize,
    /// Quads the optimized mesher emitted — for this cube-only fixture, every
    /// one is a cube rectangle (merged or 1×1 fallback).
    pub optimized_quads: usize,
    pub optimized_cube_rects: usize,
    pub vertex_reduction_percent: f32,
    pub index_reduction_percent: f32,
    pub visible_cube_faces: u32,
    pub greedy_candidate_faces: u32,
    pub greedy_quads: u32,
    pub unit_fallback_faces: u32,
    /// Largest rectangle area emitted per face direction, indexed by face
    /// (0=up, 1=down, 2=north, 3=south, 4=west, 5=east).
    pub max_rect_area_per_face: [i32; 6],
    pub up_faces: usize,
    pub max_up_rect_area: i32,
    /// Rectangles whose vertical span strictly crosses the `y=64` section floor.
    pub section_crossing_rects: usize,
    pub nonuniform_ao_faces: usize,
    /// The largest rectangle in the same plane as the AO fallbacks.
    pub ao_plane_max_rect_area: i32,
    /// The three shared-layer material strips, in scan order.
    pub material_boundary_rects: [(i32, i32); 3],
    pub material_boundary_layer: u16,
    /// Unit faces owned by the cutout material (identical in both meshers).
    pub cutout_faces: usize,
    pub block_light_boundary_rects: [(i32, i32); 2],
    pub sky_light_boundary_rects: [(i32, i32); 2],
    pub tint_boundary_rects: [(i32, i32); 2],
    pub tint_boundary_layer: u16,
    pub tint_boundary_words: [u32; 2],
    pub deterministic_runs: usize,

    // -- legacy (non-cube) controls: byte-identical to the reference ---------
    //
    // The three booleans are always `true` in a returned report — the checker
    // fails before constructing one otherwise. They exist so a caller printing
    // the report can state plainly that each control ran and matched, and the
    // counts show it was not vacuous.
    pub model_only_identical: bool,
    pub model_only: LegacyControlCounts,
    pub water_only_identical: bool,
    pub water_only: LegacyControlCounts,
    pub lava_only_identical: bool,
    pub lava_only: LegacyControlCounts,
    /// M161 — a waterlogged model, which produces BOTH streams from one state.
    pub waterlogged_identical: bool,
    pub waterlogged: LegacyControlCounts,
    pub waterlogged_faces: WaterloggedFaceCounts,
}

/// Locate the single expanded entry covering one unit face.
fn oracle_face_at(
    exp: &[ExpandedFace],
    face: u8,
    block: [i32; 3],
    what: &str,
) -> Result<ExpandedFace, String> {
    let hits: Vec<_> = exp
        .iter()
        .filter(|e| e.f.face == face && e.f.block == block)
        .collect();
    match hits.len() {
        1 => Ok(*hits[0]),
        n => Err(format!(
            "{what}: face {face} of {block:?} is covered {n} times, expected exactly once"
        )),
    }
}

fn oracle_rect_at(
    exp: &[ExpandedFace],
    face: u8,
    block: [i32; 3],
    what: &str,
) -> Result<(i32, i32), String> {
    oracle_face_at(exp, face, block, what).map(|e| (e.w, e.h))
}

/// Assert a rectangle's exact extent.
fn oracle_expect_rect(
    exp: &[ExpandedFace],
    face: u8,
    block: [i32; 3],
    want: (i32, i32),
    what: &str,
) -> Result<(i32, i32), String> {
    let got = oracle_rect_at(exp, face, block, what)?;
    if got != want {
        return Err(format!(
            "{what}: face {face} of {block:?} merged into a {}x{} rectangle, expected {}x{}",
            got.0, got.1, want.0, want.1
        ));
    }
    Ok(got)
}

/// **The M15 acceptance gate.** Grades the production [`mesh_column`] against
/// the frozen [`mesh_column_reference`] on one adversarial cube column.
///
/// The comparison is *semantic*, not byte-wise: the optimized rectangles are
/// expanded back into the unit faces they cover (see [`expand_unit_faces`]) and
/// that set — face direction, owning block, lower-24 layer/light, all four AO
/// codes, tint word — must equal the reference's exactly. A byte diff cannot
/// express this, because a merged rectangle is *supposed* to differ from the N
/// unit quads it replaces.
///
/// Serverless and asset-free: no jar, no network, no Vulkan. Compiled into
/// release builds on purpose — it is the gate, not a test fixture, and it never
/// calls the test harness (`mod tests` calls *it*).
///
/// Returns `Err` with a property-specific message on the first failure.
pub fn check_greedy_oracle() -> Result<GreedyOracleReport, String> {
    let world = oracle_world();
    let table = oracle_table();

    let r = mesh_column_reference(&world, &table, &[], &[], 0, 0)
        .ok_or("reference mesher produced nothing for the oracle column")?;
    let o = mesh_column(&world, &table, &[], &[], 0, 0)
        .ok_or("optimized mesher produced nothing for the oracle column")?;

    // The fixture is cube-only, so nothing may reach the translucent stream and
    // every opaque quad must be a cube rectangle.
    if !o.tvertices.is_empty() || !r.tvertices.is_empty() {
        return Err(format!(
            "cube-only fixture produced translucent geometry ({} optimized / {} reference vertices)",
            o.tvertices.len(),
            r.tvertices.len()
        ));
    }

    let exp = expand_unit_faces(&o.vertices, &o.indices)?;
    let exp_ref = expand_unit_faces(&r.vertices, &r.indices)?;

    // -- 1. metrics invariants ------------------------------------------------
    if o.visible_cube_faces != r.visible_cube_faces {
        return Err(format!(
            "visible cube faces disagree: optimized {} vs reference {} — the two scans no longer see the same surface",
            o.visible_cube_faces, r.visible_cube_faces
        ));
    }
    if o.visible_cube_faces != o.greedy_candidate_faces + o.unit_fallback_faces {
        return Err(format!(
            "metrics: {} visible cube faces != {} candidates + {} fallbacks",
            o.visible_cube_faces, o.greedy_candidate_faces, o.unit_fallback_faces
        ));
    }
    if o.greedy_quads > o.greedy_candidate_faces {
        return Err(format!(
            "metrics: {} rectangles from only {} candidates — a rectangle must consume at least one",
            o.greedy_quads, o.greedy_candidate_faces
        ));
    }
    if o.greedy_quads == 0 {
        return Err("metrics: the fixture merged nothing — greedy_quads is 0".into());
    }
    if o.unit_fallback_faces == 0 {
        return Err(
            "metrics: the fixture produced no fallback — up faces and AO discontinuities must fall back"
                .into(),
        );
    }
    let optimized_quads = o.vertices.len() / 4;
    if optimized_quads != (o.greedy_quads + o.unit_fallback_faces) as usize {
        return Err(format!(
            "metrics: {optimized_quads} emitted quads != {} rectangles + {} fallbacks (a model/fluid quad leaked into a cube-only fixture?)",
            o.greedy_quads, o.unit_fallback_faces
        ));
    }
    if exp.len() != o.visible_cube_faces as usize {
        return Err(format!(
            "the optimized output expands to {} unit faces but the scan saw {} visible cube faces",
            exp.len(),
            o.visible_cube_faces
        ));
    }

    // -- 2. the merge actually paid off --------------------------------------
    if o.vertices.len() >= r.vertices.len() || o.indices.len() >= r.indices.len() {
        return Err(format!(
            "no reduction: optimized {} verts / {} indices vs reference {} / {}",
            o.vertices.len(),
            r.vertices.len(),
            o.indices.len(),
            r.indices.len()
        ));
    }
    let pct = |opt: usize, refr: usize| (1.0 - opt as f32 / refr as f32) * 100.0;
    let vertex_reduction_percent = pct(o.vertices.len(), r.vertices.len());
    let index_reduction_percent = pct(o.indices.len(), r.indices.len());

    // -- 3. every enabled direction really merged ----------------------------
    let mut max_rect_area_per_face = [0i32; 6];
    for e in &exp {
        let slot = &mut max_rect_area_per_face[e.f.face as usize];
        *slot = (*slot).max(e.w * e.h);
    }
    const DIR_NAME: [&str; 6] = ["up", "down", "north", "south", "west", "east"];
    for face in 1..6 {
        if max_rect_area_per_face[face] <= 1 {
            return Err(format!(
                "direction {} ({face}) never merged: its largest rectangle covers {} unit face(s), expected >1",
                DIR_NAME[face], max_rect_area_per_face[face]
            ));
        }
    }
    // The five walls/floors the fixture is built to produce, pinned exactly.
    oracle_expect_rect(&exp, 1, [1, 60, 1], (6, 6), "region A floor")?;
    oracle_expect_rect(&exp, 2, [1, 60, 1], (6, 10), "region A north wall")?;
    oracle_expect_rect(&exp, 3, [1, 60, 6], (6, 10), "region A south wall")?;
    oracle_expect_rect(&exp, 4, [1, 60, 1], (6, 10), "region A west wall")?;
    oracle_expect_rect(&exp, 5, [6, 60, 1], (6, 10), "region A east wall")?;

    // -- 4. the up carve-out --------------------------------------------------
    let ups: Vec<_> = exp.iter().filter(|e| e.f.face == 0).collect();
    if ups.is_empty() {
        return Err("the fixture emitted no up faces — the up carve-out is untested".into());
    }
    for e in &ups {
        if e.w != 1 || e.h != 1 {
            return Err(format!(
                "up face of {:?} merged into a {}x{} rectangle — every up (+Y) face must stay a unit quad (see the merge site in mesh_column for the canonical-pixel measurement)",
                e.f.block, e.w, e.h
            ));
        }
    }

    // -- 5. rectangles cross the section floor -------------------------------
    let section_crossing_rects = split_quads(&o.vertices, &o.indices)?
        .into_iter()
        .filter(|q| {
            let lo = q.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
            lo < 64.0 && hi > 64.0
        })
        .count();
    if section_crossing_rects == 0 {
        return Err(
            "no rectangle crossed the y=64 section floor — masks must span the column's whole occupied height"
                .into(),
        );
    }

    // -- 6. AO discontinuity falls back, its neighbours still merge -----------
    let mut nonuniform_ao_faces = 0usize;
    let mut nonuniform_vertical = 0usize;
    for e in &exp {
        if e.f.ao.iter().any(|a| *a != e.f.ao[0]) {
            nonuniform_ao_faces += 1;
            if e.f.face != 0 {
                nonuniform_vertical += 1;
            }
            if e.w != 1 || e.h != 1 {
                return Err(format!(
                    "face {} of {:?} has non-uniform corner AO {:?} yet merged into a {}x{} rectangle — a rectangle cannot reproduce a corner gradient",
                    e.f.face, e.f.block, e.f.ao, e.w, e.h
                ));
            }
        }
    }
    if nonuniform_vertical == 0 {
        // Two ways to land here, and the caller needs to tell them apart: either
        // the fixture stopped generating a discontinuity (the probe went vacuous
        // — up faces fall back regardless, so a top plane cannot grade this
        // rule), or the mesher merged the non-uniform faces and *flattened* their
        // gradient to one corner's code, which erases the evidence. The
        // surface-equality check below catches the second case explicitly.
        return Err(
            "no non-uniform-AO vertical face reached the output — either the fixture stopped producing a discontinuity, or the mesher merged one and flattened its corner gradient"
                .into(),
        );
    }
    // The fallback must be local to the discontinuity, not a plane-wide bail-out:
    // the far half of that same north plane is one rectangle.
    let ao_plane_max_rect_area = exp
        .iter()
        .filter(|e| e.f.face == 2 && e.f.block[2] == 10)
        .map(|e| e.w * e.h)
        .max()
        .unwrap_or(0);
    if ao_plane_max_rect_area <= 1 {
        return Err(format!(
            "the AO-discontinuity plane (north, z=10) merged nothing (largest rectangle covers {ao_plane_max_rect_area} unit face(s)) — the fallback must be local to the discontinuity, not plane-wide"
        ));
    }
    oracle_expect_rect(&exp, 2, [4, 64, 10], (3, 6), "AO plane, unoccluded half")?;

    // -- 7. material boundary with a shared layer ----------------------------
    let strips = [
        oracle_face_at(&exp, 1, [8, 62, 1], "material strip GRANITE")?,
        oracle_face_at(&exp, 1, [8, 62, 4], "material strip DIORITE")?,
        oracle_face_at(&exp, 1, [8, 62, 7], "material strip CUTOUT")?,
    ];
    let material_boundary_layer = (strips[0].f.light24 & 0xFFFF) as u16;
    if material_boundary_layer != OS_SHARED_DOWN_LAYER {
        return Err(format!(
            "material boundary probe is vacuous: the down layer is {material_boundary_layer}, expected the shared {OS_SHARED_DOWN_LAYER}"
        ));
    }
    for (i, s) in strips.iter().enumerate() {
        if s.f.light24 != strips[0].f.light24 || s.f.tint != strips[0].f.tint {
            return Err(format!(
                "material strip {i} differs in layer/light/tint ({:#08x}/{:#010x} vs {:#08x}/{:#010x}) — the probe must isolate the block state as the only discriminator",
                s.f.light24, s.f.tint, strips[0].f.light24, strips[0].f.tint
            ));
        }
        if s.f.ao != strips[0].f.ao {
            return Err(format!(
                "material strip {i} differs in AO {:?} vs {:?} — the probe must isolate the block state",
                s.f.ao, strips[0].f.ao
            ));
        }
        if (s.w, s.h) != (8, 3) {
            return Err(format!(
                "material strip {i} is a {}x{} rectangle, expected 8x3 — with an identical layer, light, tint and AO, the only thing that may keep these three states apart is the block state in the merge key",
                s.w, s.h
            ));
        }
    }

    // -- 8. the cutout material stays present and separate -------------------
    let is_cutout = |b: [i32; 3]| b[1] == 62 && (8..=15).contains(&b[0]) && (7..=9).contains(&b[2]);
    let cutout_faces = exp.iter().filter(|e| is_cutout(e.f.block)).count();
    let cutout_faces_ref = exp_ref.iter().filter(|e| is_cutout(e.f.block)).count();
    if cutout_faces == 0 {
        return Err("the cutout material emitted no faces — it must stay semantically present through the greedy path".into());
    }
    if cutout_faces != cutout_faces_ref {
        return Err(format!(
            "the cutout material lost faces: {cutout_faces} optimized vs {cutout_faces_ref} reference"
        ));
    }

    // -- 9. light discontinuities are not crossed ----------------------------
    let block_lo = oracle_face_at(&exp, 1, [8, 66, 11], "block-light plate, unlit half")?;
    let block_hi = oracle_face_at(&exp, 1, [12, 66, 11], "block-light plate, lit half")?;
    if block_lo.f.light24 == block_hi.f.light24 {
        return Err(format!(
            "block-light probe is vacuous: both halves carry light word {:#08x} — the poke did not reach the sampled neighbour cells at y=65",
            block_lo.f.light24
        ));
    }
    for (s, half) in [(block_lo, "unlit"), (block_hi, "lit")] {
        if (s.w, s.h) != (4, 4) {
            return Err(format!(
                "block-light {half} half is a {}x{} rectangle, expected 4x4 — a rectangle must not span a block-light discontinuity",
                s.w, s.h
            ));
        }
    }
    let sky_lo = oracle_face_at(&exp, 1, [8, 70, 11], "sky-light plate, bright half")?;
    let sky_hi = oracle_face_at(&exp, 1, [12, 70, 11], "sky-light plate, dim half")?;
    if sky_lo.f.light24 == sky_hi.f.light24 {
        return Err(format!(
            "sky-light probe is vacuous: both halves carry light word {:#08x} — the poke did not reach the sampled neighbour cells at y=69",
            sky_lo.f.light24
        ));
    }
    for (s, half) in [(sky_lo, "bright"), (sky_hi, "dim")] {
        if (s.w, s.h) != (4, 4) {
            return Err(format!(
                "sky-light {half} half is a {}x{} rectangle, expected 4x4 — a rectangle must not span a sky-light discontinuity",
                s.w, s.h
            ));
        }
    }

    // -- 10. the constant-tint boundary is not crossed -----------------------
    //
    // A tint is a property of the block state, so `TintSource::Constant` cannot
    // produce a *same-state* tint change — the two sides necessarily differ in
    // state too. What this grades is therefore the observable property (no
    // rectangle spans the tint change) plus the fact that the split cannot be
    // attributed to layer or light: both sides carry an identical `light24`.
    // Check 7 above separately proves the state field splits a shared layer.
    let spruce = oracle_face_at(&exp, 1, [1, 62, 11], "tint boundary, spruce half")?;
    let birch = oracle_face_at(&exp, 1, [5, 62, 11], "tint boundary, birch half")?;
    let tint_boundary_layer = (spruce.f.light24 & 0xFFFF) as u16;
    if tint_boundary_layer != OS_TINT_RAW_LAYER {
        return Err(format!(
            "tint boundary probe is vacuous: the face resolved to layer {tint_boundary_layer}, not the raw {OS_TINT_RAW_LAYER} — the Constant tint path did not engage (is the biome context attached?)"
        ));
    }
    if spruce.f.light24 != birch.f.light24 {
        return Err(format!(
            "tint boundary probe is impure: the halves also differ in layer/light ({:#08x} vs {:#08x}), so a split would not prove the tint was honoured",
            spruce.f.light24, birch.f.light24
        ));
    }
    if spruce.f.tint == birch.f.tint {
        return Err(format!(
            "tint boundary probe is vacuous: both halves carry tint word {:#010x}",
            spruce.f.tint
        ));
    }
    if spruce.f.tint != pack_tint(ORACLE_SPRUCE_RGB, 0)
        || birch.f.tint != pack_tint(ORACLE_BIRCH_RGB, 0)
    {
        return Err(format!(
            "tint boundary: the emitted tint words {:#010x}/{:#010x} are not the fixture's constants {:#010x}/{:#010x} — the constant tint is not carried losslessly",
            spruce.f.tint,
            birch.f.tint,
            pack_tint(ORACLE_SPRUCE_RGB, 0),
            pack_tint(ORACLE_BIRCH_RGB, 0)
        ));
    }
    for (s, half) in [(spruce, "spruce"), (birch, "birch")] {
        if (s.w, s.h) != (4, 4) {
            return Err(format!(
                "tint boundary {half} half is a {}x{} rectangle, expected 4x4 — a rectangle must not span a tint change",
                s.w, s.h
            ));
        }
    }

    // -- 11. the whole surface, face for face --------------------------------
    let mut got: Vec<UnitFace> = exp.iter().map(|e| e.f).collect();
    let mut want: Vec<UnitFace> = exp_ref.iter().map(|e| e.f).collect();
    got.sort();
    want.sort();
    if got.len() != want.len() {
        return Err(format!(
            "expanded surface size differs: optimized {} unit faces vs reference {}",
            got.len(),
            want.len()
        ));
    }
    if let Some((i, (g, w))) = got.iter().zip(&want).enumerate().find(|(_, (g, w))| g != w) {
        return Err(format!(
            "expanded surface differs at sorted face {i}: optimized {g:?} vs reference {w:?}"
        ));
    }

    // -- 12. determinism ------------------------------------------------------
    let deterministic_runs = 4;
    for run in 0..deterministic_runs {
        let again = mesh_column(&world, &table, &[], &[], 0, 0)
            .ok_or("determinism: a rerun produced nothing")?;
        if bytemuck::cast_slice::<_, u8>(&again.vertices)
            != bytemuck::cast_slice::<_, u8>(&o.vertices)
        {
            return Err(format!(
                "determinism: rerun {run} emitted different vertex bytes — the masks must be index-ordered arrays, not hash maps"
            ));
        }
        if again.indices != o.indices {
            return Err(format!(
                "determinism: rerun {run} emitted different indices"
            ));
        }
        if (again.greedy_quads, again.unit_fallback_faces)
            != (o.greedy_quads, o.unit_fallback_faces)
        {
            return Err(format!(
                "determinism: rerun {run} reported {}/{} rectangles/fallbacks vs {}/{}",
                again.greedy_quads,
                again.unit_fallback_faces,
                o.greedy_quads,
                o.unit_fallback_faces
            ));
        }
    }

    // -- 13. legacy controls: models and fluids pass through untouched -------
    //
    // The cube path is graded semantically because a rectangle is *supposed* to
    // differ from the faces it replaces. Models and fluids merge nothing, so
    // they are held to byte equality — which is what proves the greedy pass
    // left scan order, buffer layout and the opaque/translucent split alone.
    let model_only = oracle_check_legacy_control(
        &oracle_model_world(),
        &oracle_model_table(),
        &oracle_model_quads(),
        &[],
        "model-only",
        true,  // models land in the opaque stream
        false, // and never in the translucent one
    )?;
    let fluid_table = oracle_fluid_table();
    let water_only = oracle_check_legacy_control(
        &oracle_water_world(),
        &fluid_table,
        &[],
        &[],
        "water-only",
        false, // water blends, so the opaque stream must stay empty
        true,
    )?;
    let lava_only = oracle_check_legacy_control(
        &oracle_lava_world(),
        &fluid_table,
        &[],
        &[],
        "lava-only",
        true, // lava is opaque and fullbright
        false,
    )?;
    // M161 — one waterlogged block. The only control that must populate BOTH
    // streams, because it is the only fixture where one block state runs both
    // of `SectionCompiler`'s draws.
    let waterlogged = oracle_check_legacy_control(
        &oracle_waterlogged_world(WL_BOTTOM_SLAB),
        &oracle_waterlogged_table(),
        &oracle_model_quads(),
        &oracle_waterlogged_carried(),
        "waterlogged",
        true, // the block's own model
        true, // and the water it carries
    )?;
    let waterlogged_faces = check_waterlogged()?;

    Ok(GreedyOracleReport {
        reference_unit_faces: exp_ref.len(),
        reference_quads: r.vertices.len() / 4,
        optimized_quads,
        optimized_cube_rects: optimized_quads,
        vertex_reduction_percent,
        index_reduction_percent,
        visible_cube_faces: o.visible_cube_faces,
        greedy_candidate_faces: o.greedy_candidate_faces,
        greedy_quads: o.greedy_quads,
        unit_fallback_faces: o.unit_fallback_faces,
        max_rect_area_per_face,
        up_faces: ups.len(),
        max_up_rect_area: max_rect_area_per_face[0],
        section_crossing_rects,
        nonuniform_ao_faces,
        ao_plane_max_rect_area,
        material_boundary_rects: [
            (strips[0].w, strips[0].h),
            (strips[1].w, strips[1].h),
            (strips[2].w, strips[2].h),
        ],
        material_boundary_layer,
        cutout_faces,
        block_light_boundary_rects: [(block_lo.w, block_lo.h), (block_hi.w, block_hi.h)],
        sky_light_boundary_rects: [(sky_lo.w, sky_lo.h), (sky_hi.w, sky_hi.h)],
        tint_boundary_rects: [(spruce.w, spruce.h), (birch.w, birch.h)],
        tint_boundary_layer,
        tint_boundary_words: [spruce.f.tint, birch.f.tint],
        deterministic_runs,
        model_only_identical: true,
        model_only,
        water_only_identical: true,
        water_only,
        lava_only_identical: true,
        lava_only,
        waterlogged_identical: true,
        waterlogged,
        waterlogged_faces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_world::dimension::DimensionShape;

    /// The fluid table lives in production so `check_greedy_oracle`'s water and
    /// lava controls and these tests are provably the *same* fixture.
    fn fluid_table() -> Vec<RenderKind> {
        oracle_fluid_table()
    }

    #[test]
    fn water_source_meshes_translucent_at_vanilla_height() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 9, 4, 1); // floor cube
        w.set_block(4, 10, 4, 2); // water source on it
        let mesh = mesh_column(&w, &fluid_table(), &[], &[], 0, 0).expect("meshed");
        assert!(
            !mesh.vertices.is_empty(),
            "floor cube goes to the opaque set"
        );
        assert!(
            !mesh.tvertices.is_empty(),
            "water goes to the translucent set"
        );
        let top = mesh
            .tvertices
            .iter()
            .map(|v| v.pos[1])
            .fold(f32::MIN, f32::max);
        assert!(
            (top - (10.0 + 8.0 / 9.0)).abs() < 1e-5,
            "source surface sits at 8/9: {top}"
        );
    }

    // -- M161: waterlogged blocks ------------------------------------------
    //
    // The face-emission rules and the two-draws property are graded by
    // `check_greedy_oracle`'s waterlogged control (production code, run by
    // `rewo meshshot --check` AND by the test below it). These cover the pieces
    // that control cannot reach.

    /// The remap between the two crates' face orders, derived from both
    /// direction tables rather than trusted as literals. A wrong entry here
    /// reads a bottom slab's occlusion as a north face's and is invisible in
    /// every fixture whose block occludes on more than one side.
    #[test]
    fn self_occlude_remap_is_the_two_direction_tables() {
        use rewo_data::assets::FACE_DIRS;
        for (face, &bit) in SELF_OCCLUDE_REMAP.iter().enumerate() {
            let (dx, dy, dz) = FACE_OFFSETS[face];
            assert_eq!(
                FACE_DIRS[bit as usize],
                (dx, dy, dz),
                "mesher face {face} maps to FACE_DIRS[{bit}]"
            );
        }
        // A permutation, so no direction is dropped or read twice.
        let mut seen: Vec<u32> = SELF_OCCLUDE_REMAP.to_vec();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 5]);
    }

    /// The two storage sites, one query. `RenderKind::Fluid` wins where both
    /// could answer, and a `LiquidBlock` never suppresses its own faces —
    /// `getFaceOcclusionShape` is `Shapes.empty()`, so `isFaceOccludedByState`
    /// returns at its first branch. Water is **not** `noOcclusion` (see
    /// [`fluid_at`]); the empty shape comes from `LiquidBlock.getShape`.
    #[test]
    fn fluid_at_reads_the_liquid_block_then_the_carried_table() {
        let table = oracle_waterlogged_table();
        let carried = oracle_waterlogged_carried();
        let water = fluid_at(&table, &carried, WL_WATER).expect("water is a fluid");
        assert!(!water.carried && !water.lava && water.self_occludes == 0);
        let wl = fluid_at(&table, &carried, WL_BOTTOM_SLAB).expect("the slab carries water");
        assert!(wl.carried && !wl.lava, "carried water is never lava");
        assert_eq!(wl.level, 0, "every carried fluid in 26.2 is a source");
        assert_eq!(
            wl.self_occludes,
            1 << 1,
            "FACE_DIRS' down bit must land on the mesher's face 1"
        );
        assert!(fluid_at(&table, &carried, WL_DRY).is_none());
        // An out-of-range state, and a carried table shorter than the render
        // table (every pre-M161 caller passes an empty one).
        assert!(fluid_at(&table, &carried, 999).is_none());
        assert!(fluid_at(&table, &[], WL_BOTTOM_SLAB).is_none());
        assert!(fluid_at(&table, &[], WL_WATER).is_some());
    }

    /// `fluid_at` hands each of the two layers to the field of the same name —
    /// on BOTH branches.
    ///
    /// Its own fixtures cannot ask: `oracle_fluid_table`'s water is
    /// `layer: 1, raw_layer: 1` and `oracle_waterlogged_carried`'s carrier is
    /// the same pair, so `layer: f.raw_layer, raw_layer: f.layer` is
    /// **indistinguishable** there — and it is not a cosmetic swap, because
    /// `emit_fluid` picks between them by whether the world has a biome
    /// (`f.raw_layer` + the dynamic M14 colour, else the pre-tinted `f.layer`),
    /// so swapping them double-tints one path and un-tints the other. Distinct
    /// sentinels are the whole point of this test; a shared value grades
    /// nothing. The real bake's pair is graded in `blockentityshot`.
    #[test]
    fn fluid_at_keeps_the_pre_tinted_and_raw_layers_apart() {
        const POOL_TINTED: u16 = 11;
        const POOL_RAW: u16 = 22;
        const CARRIED_TINTED: u16 = 33;
        const CARRIED_RAW: u16 = 44;
        let table = vec![
            RenderKind::Invisible,
            RenderKind::Fluid {
                layer: POOL_TINTED,
                raw_layer: POOL_RAW,
                level: 0,
                lava: false,
            },
        ];
        let carried = vec![
            None,
            None,
            Some(CarriedFluid {
                layer: CARRIED_TINTED,
                raw_layer: CARRIED_RAW,
                level: 0,
                falling: false,
                self_occludes: 0,
            }),
        ];
        let pool = fluid_at(&table, &carried, 1).expect("state 1 is a pool");
        assert_eq!((pool.layer, pool.raw_layer), (POOL_TINTED, POOL_RAW));
        let wl = fluid_at(&table, &carried, 2).expect("state 2 carries water");
        assert_eq!((wl.layer, wl.raw_layer), (CARRIED_TINTED, CARRIED_RAW));
    }

    /// A carried fluid is a SOURCE — `getOwnHeight` is `amount / 9.0` and a
    /// source's amount is 8 — so its surface sits at 8/9 like any other source,
    /// not at the top of the block it is inside.
    #[test]
    fn carried_water_surfaces_at_eight_ninths() {
        let mesh = mesh_column(
            &oracle_waterlogged_world(WL_TOP_SLAB),
            &oracle_waterlogged_table(),
            &oracle_model_quads(),
            &oracle_waterlogged_carried(),
            0,
            0,
        )
        .expect("meshed");
        let top = mesh
            .tvertices
            .iter()
            .map(|v| v.pos[1])
            .fold(f32::MIN, f32::max);
        assert!((top - (62.0 + 8.0 / 9.0)).abs() < 1e-5, "surface at {top}");
    }

    /// `getHeight`'s `hasSameAbove` arm reaches ACROSS the two storage sites:
    /// an ordinary water block above a waterlogged one makes the waterlogged
    /// cell a full column, and the waterlogged cell stops emitting a top face
    /// at all. A `same()` that only knew `RenderKind::Fluid` would leave a
    /// surface buried inside the pool.
    #[test]
    fn a_pool_above_a_waterlogged_block_makes_it_a_full_column() {
        use rewo_world::dimension::DimensionShape;
        let table = oracle_waterlogged_table();
        let carried = oracle_waterlogged_carried();
        let models = oracle_model_quads();
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(2, 62, 2, WL_TOP_SLAB);
        w.set_block(2, 63, 2, WL_WATER);
        let m = mesh_column(&w, &table, &models, &carried, 0, 0).expect("meshed");
        // `fluid_face_counts` is per-CELL and this world has two, so the claim
        // is made directly on the plane a buried surface would sit in. (The
        // first draft of this test asked `fluid_face_counts(.., 62.0)` for
        // `top == 0` and failed, because the POOL's quads have no vertex at
        // y=62 either and so counted as tops.)
        let surface_at = |y: f32| {
            m.tvertices
                .chunks_exact(4)
                .filter(|q| q.iter().all(|v| (v.pos[1] - y).abs() < 1e-5))
                .count()
        };
        assert_eq!(
            surface_at(62.0 + 8.0 / 9.0),
            0,
            "the buried cell must not emit a surface"
        );
        assert_eq!(surface_at(63.0 + 8.0 / 9.0), 1, "the pool's own surface");
        // The pool's own sides run from 63 down to 63 + 8/9; the waterlogged
        // cell's run the full block height, which is what "full column" means.
        let low = m
            .tvertices
            .iter()
            .filter(|v| v.pos[1] > 62.0 && v.pos[1] < 63.0)
            .count();
        assert_eq!(low, 0, "a full column has no vertex between 62 and 63");
        assert_eq!(m.carried_fluid_cells, 1);
    }

    /// `carried_fluid_cells` counts cells that **emitted**, not cells whose
    /// lookup found a carrier — which is what `r48`'s label claims.
    ///
    /// The two forms differ in exactly one situation and it is a real one: a
    /// carrier whose six faces are ALL suppressed. `renderUp` skips
    /// `shouldRenderFace`, so the top survives self-occlusion and only a same
    /// fluid ABOVE can take it — i.e. a waterlogged double slab submerged in a
    /// pool. Every other fixture in this file emits at least that top face, so
    /// this is the only place the gate is observable, and without it the gate
    /// could be deleted and `r48` would go back to proving that the TABLE
    /// reached the mesher rather than that any water was meshed.
    #[test]
    fn a_fully_occluded_submerged_carrier_is_not_counted_as_meshed() {
        use rewo_world::dimension::DimensionShape;
        let table = oracle_waterlogged_table();
        let carried = oracle_waterlogged_carried();
        let models = oracle_model_quads();
        let mesh_at = |above: Option<u32>| {
            let mut w = World::new(DimensionShape::OVERWORLD);
            w.ensure_column(0, 0);
            w.set_block(2, 62, 2, WL_DOUBLE_SLAB);
            if let Some(a) = above {
                w.set_block(2, 63, 2, a);
            }
            mesh_column(&w, &table, &models, &carried, 0, 0).expect("meshed")
        };
        // In open air the same block DOES emit its top face, so the zero below
        // is the gate and not a lookup that found nothing.
        let open = mesh_at(None);
        assert_eq!(open.carried_fluid_cells, 1, "an exposed double slab meshes its top");
        let buried = mesh_at(Some(WL_WATER));
        let own_cell = buried
            .tvertices
            .chunks_exact(4)
            .filter(|q| q.iter().any(|v| v.pos[1] < 63.0 - 1e-5))
            .count();
        assert_eq!(own_cell, 0, "all six of the buried carrier's faces are suppressed");
        assert_eq!(
            buried.carried_fluid_cells, 0,
            "a carrier that emitted no geometry did not mesh its water"
        );
    }

    /// The production gate, run by `cargo test` as well as by
    /// `rewo meshshot --check`.
    #[test]
    fn the_waterlogged_oracle_passes() {
        let f = check_waterlogged().expect("waterlogged oracle");
        assert_eq!(f.bottom_slab, (1, 4, 0));
        assert_eq!(f.top_slab, (1, 4, 1));
        assert_eq!(f.double_slab, (1, 0, 0));
        assert_eq!(f.pool_plane_faces, (0, 1));
    }

    #[test]
    fn lava_meshes_opaque() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 10, 4, 3);
        let mesh = mesh_column(&w, &fluid_table(), &[], &[], 0, 0).expect("meshed");
        assert!(!mesh.vertices.is_empty(), "lava is opaque geometry");
        assert!(mesh.tvertices.is_empty());
    }

    #[test]
    fn submerged_water_column_is_full_height() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 10, 4, 2);
        w.set_block(4, 11, 4, 2); // water above → lower cell is a full column
        let mesh = mesh_column(&w, &fluid_table(), &[], &[], 0, 0).expect("meshed");
        // Lower cell contributes no top face; the surface is the upper
        // cell's 8/9 → max y = 11 + 8/9.
        let top = mesh
            .tvertices
            .iter()
            .map(|v| v.pos[1])
            .fold(f32::MIN, f32::max);
        assert!((top - (11.0 + 8.0 / 9.0)).abs() < 1e-5, "top {top}");
    }

    #[test]
    fn side_faces_have_texture_top_at_block_top() {
        for face in 2..6 {
            for (pos, uv) in FACE_CORNERS[face] {
                if uv[1] == 0.0 {
                    assert_eq!(pos[1], 1.0, "face {face} v=0 must sit at block top");
                } else {
                    assert_eq!(pos[1], 0.0);
                }
            }
        }
    }

    /// A Water-tinted model quad: with no biome context it takes the legacy
    /// pre-tinted layer + white tint bytes (byte-identical demo path); with a
    /// biome context it takes the RAW layer + the biome water color into
    /// `MeshVertex::tint`, which `reconstructed_color` then mirrors.
    #[test]
    fn dynamic_biome_tint_vs_legacy_path() {
        use rewo_data::assets::Quad;
        use rewo_world::biome::{BiomeContext, BiomeDef, BiomeRegistry, Colormaps, GrassModifier};
        use std::sync::Arc;

        let table = vec![RenderKind::Invisible, RenderKind::Model(0)];
        let models = vec![vec![Quad {
            verts: [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            uv: [[0.0, 0.0]; 4],
            layer: 7,     // legacy pre-tinted layer
            raw_layer: 8, // raw layer for the biome path
            cull: -1,
            dir: 2, // north
            tint: TintSource::Water,
            shade: false, // c = 1.0 → color is exactly the tint
        }]];

        // Legacy: no biome context → white color, pre-tinted layer 7.
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(2, 64, 2, 1);
        let m = mesh_column(&w, &table, &models, &[], 0, 0).expect("meshed");
        let v = m.vertices[0];
        assert_eq!(v.layer_index(), 7, "legacy path uses the pre-tinted layer");
        assert_eq!(
            v.tint_rgb(),
            TINT_WHITE,
            "legacy path carries white tint bytes"
        );
        assert_eq!(
            v.reconstructed_color(),
            [1.0, 1.0, 1.0],
            "legacy path reconstructs exact white"
        );

        // Biome path: attach a registry whose water_color = (100, 0, 0). The
        // empty column's single-value biome container = index 0.
        let biome = BiomeDef {
            music_volume: None,
            name: "x".into(),
            temperature: 0.5,
            downfall: 0.5,
            water_color: (0xFFu32 << 24 | (100u32 << 16)) as i32,
            grass_override: None,
            foliage_override: None,
            dry_foliage_override: None,
            grass_modifier: GrassModifier::None,
            sky_color: None,
            fog_color: None,
                has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
        let ctx = BiomeContext::new(
            Arc::new(BiomeRegistry::new(vec![biome])),
            Colormaps::neutral(),
            0,
        );
        w.set_biome_context(Arc::new(ctx));
        let m2 = mesh_column(&w, &table, &models, &[], 0, 0).expect("meshed");
        let v2 = m2.vertices[0];
        assert_eq!(v2.layer_index(), 8, "biome path uses the RAW layer");
        assert_eq!(
            v2.tint_rgb(),
            [100, 0, 0],
            "tint bytes are carried losslessly"
        );
        let c2 = v2.reconstructed_color();
        assert!(
            (c2[0] - 100.0 / 255.0).abs() < 1e-6,
            "red channel = biome water color: {}",
            c2[0]
        );
        assert_eq!(c2[1], 0.0);
        assert_eq!(c2[2], 0.0);
    }

    // -- M15 packed vertex ABI ---------------------------------------------

    #[test]
    fn packed_vertex_is_exactly_28_bytes_with_expected_offsets() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 28, "packed vertex size");
        assert_eq!(std::mem::align_of::<MeshVertex>(), 4, "packed vertex align");
        // Offsets the Vulkan attribute descriptions hard-code.
        let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, 0, 0, AO_NONE, TINT_WHITE);
        let base = &v as *const _ as usize;
        assert_eq!(&v.pos as *const _ as usize - base, 0, "pos offset");
        assert_eq!(&v.uv as *const _ as usize - base, 12, "uv offset");
        assert_eq!(&v.light as *const _ as usize - base, 20, "light offset");
        assert_eq!(&v.tint as *const _ as usize - base, 24, "tint offset");
    }

    /// Every shade code × every AO level must reconstruct the *legacy* float
    /// formula bit-for-bit: `FACE_SHADE[face] * AO_LEVELS[ao] * (rgb/255)`.
    #[test]
    fn reconstruction_matches_legacy_formula_for_every_shade_and_ao() {
        for face in 0..FACE_SHADE.len() {
            for ao in 0..AO_LEVELS.len() {
                for rgb in [
                    [255u8, 255, 255],
                    [100, 0, 0],
                    [1, 2, 3],
                    [0, 0, 0],
                    [91, 163, 163],
                ] {
                    let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, 0, face as u8, ao as u8, rgb);
                    assert_eq!(v.shade_code(), face as u8);
                    assert_eq!(v.ao_code(), ao as u8);
                    // The legacy CPU sequence, verbatim.
                    let c = FACE_SHADE[face] * AO_LEVELS[ao];
                    let legacy = [
                        c * (rgb[0] as f32 / 255.0),
                        c * (rgb[1] as f32 / 255.0),
                        c * (rgb[2] as f32 / 255.0),
                    ];
                    let got = v.reconstructed_color();
                    for i in 0..3 {
                        assert_eq!(
                            got[i].to_bits(),
                            legacy[i].to_bits(),
                            "face {face} ao {ao} rgb {rgb:?} channel {i}: {got:?} vs {legacy:?}"
                        );
                    }
                }
            }
        }
    }

    /// White tint + shade 0 + AO_NONE must be EXACTLY 1.0 (the lightmapshot
    /// probe quad depends on this).
    #[test]
    fn white_untinted_vertex_reconstructs_exact_one() {
        let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, 0, 0, AO_NONE, TINT_WHITE);
        assert_eq!(v.reconstructed_color(), [1.0, 1.0, 1.0]);
        assert_eq!(FACE_SHADE[0], 1.0);
        assert_eq!(AO_LEVELS[AO_NONE as usize], 1.0);
    }

    #[test]
    fn tint_bytes_roundtrip_losslessly_for_every_value() {
        for c in 0u16..=255 {
            let rgb = [c as u8, (255 - c) as u8, (c as u8).wrapping_mul(7)];
            let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, 0, 0, AO_NONE, rgb);
            assert_eq!(v.tint_rgb(), rgb, "tint byte roundtrip");
            assert_eq!(v.tint_flags(), 0, "flags default clear");
        }
    }

    /// The packed word must preserve `pack_layer`'s lower-24-bit contract for
    /// every layer index and every light nibble pair.
    #[test]
    fn light_word_preserves_pack_layer_lower_24_bits() {
        for layer in [0u32, 1, 7, 499, 500, 4095, 0xFFFF] {
            for block in 0u8..16 {
                for sky in [0u8, 1, 7, 15] {
                    for shade in 0..FACE_SHADE.len() as u8 {
                        for ao in 0..AO_LEVELS.len() as u8 {
                            let v = MeshVertex::new(
                                [0.0; 3], [0.0; 2], layer, block, sky, shade, ao, TINT_WHITE,
                            );
                            assert_eq!(
                                v.light & 0x00FF_FFFF,
                                pack_layer(layer, block, sky),
                                "lower 24 bits must equal pack_layer"
                            );
                            assert_eq!(v.layer_index(), layer as u16);
                            assert_eq!(v.block_light(), block);
                            assert_eq!(v.sky_light(), sky);
                            assert_eq!(v.shade_code(), shade);
                            assert_eq!(v.ao_code(), ao);
                        }
                    }
                }
            }
        }
    }

    /// An out-of-range shade code is rejected **at the packing boundary**, in
    /// release builds too. `SHADE_MASK` is 3 bits, so the reserved code 7
    /// survives masking intact — nothing downstream of `pack_light_word` would
    /// notice until `MeshVertex::reconstructed_color` indexed `FACE_SHADE[7]`
    /// out of bounds (and `world.vert` did the same, where it is undefined
    /// rather than a panic).
    #[test]
    fn pack_light_word_rejects_the_reserved_shade_code() {
        // The mask alone cannot catch 7: it round-trips through SHADE_MASK.
        assert_eq!(
            7u32 & SHADE_MASK,
            7,
            "mask would accept 7 — the assert must not"
        );

        let reserved = std::panic::catch_unwind(|| pack_light_word(0, 0, 0, 7, AO_NONE));
        assert!(reserved.is_err(), "shade_code 7 must panic at pack time");

        // 0..=6 are the valid codes, and the boundary is exactly FACE_SHADE.len().
        for code in 0..FACE_SHADE.len() as u8 {
            let ok = std::panic::catch_unwind(move || pack_light_word(0, 0, 0, code, AO_NONE));
            assert!(ok.is_ok(), "shade_code {code} is valid and must not panic");
        }
        assert_eq!(FACE_SHADE.len(), 7, "codes 0..=6 exist, 7 is reserved");
    }

    /// UV transport is **exact for every family the mesher emits** — there is no
    /// quantization step at all. This is the regression guard for the rejected
    /// f16 variant: the fluid family below (`1 - k/9`) is precisely what f16
    /// could not represent, and it flipped 6 demo pixels at a texel boundary.
    #[test]
    fn uv_is_stored_exactly_for_every_emitted_family() {
        let store =
            |x: f32| MeshVertex::new([0.0; 3], [x, x], 0, 0, 0, 0, AO_NONE, TINT_WHITE).uv_f32()[0];

        // Integer spans 0..=16 (what greedy will emit).
        for i in 0..=16u32 {
            let x = i as f32;
            assert_eq!(store(x).to_bits(), x.to_bits(), "integer uv {x}");
        }
        // The k/16 model grid — every baked MC model UV.
        for k in 0..=16u32 {
            let x = k as f32 / 16.0;
            assert_eq!(store(x).to_bits(), x.to_bits(), "k/16 uv {x}");
        }
        // Representative fractional model UVs.
        for x in [0.5f32, 0.25, 0.75, 0.0625, 0.1875, 0.4375, 0.9375, 1.0, 0.0] {
            assert_eq!(store(x).to_bits(), x.to_bits(), "dyadic uv {x}");
        }
        // THE fluid family: `emit_fluid` emits `1 - fluid_h(level)`, i.e. 1-k/9.
        // Non-dyadic; f16 lost these. f32 storage must be bit-exact.
        for k in 0..=9u32 {
            let h = k as f32 / 9.0;
            for x in [h, 1.0 - h] {
                assert_eq!(store(x).to_bits(), x.to_bits(), "fluid uv {x}");
            }
        }
        // The exact value the rejected f16 build corrupted.
        let one_ninth = 1.0f32 - 8.0 / 9.0;
        assert_eq!(store(one_ninth).to_bits(), one_ninth.to_bits());
    }

    /// The constructor introduces no quantization: stored UV == source UV.
    #[test]
    fn vertex_uv_is_the_identity_of_its_input() {
        for uv in [
            [0.0f32, 0.0],
            [1.0, 1.0],
            [0.5, 0.25],
            [0.0625, 0.9375],
            [1.0 - 8.0 / 9.0, 4.0 / 9.0],
        ] {
            let v = MeshVertex::new([0.0; 3], uv, 0, 0, 0, 0, AO_NONE, TINT_WHITE);
            assert_eq!(v.uv_f32(), uv, "uv must round-trip exactly");
            assert_eq!(v.uv, uv, "uv is stored verbatim");
        }
    }

    /// The per-job `TintCache`: repeated requests at one canonical key reuse a
    /// single entry (incl. GrassBelow → Grass@y-1 canonicalization), distinct
    /// resolver/position keys allocate their own, and Constant bypasses it.
    #[test]
    fn tint_cache_reuses_canonical_key_and_does_not_alias() {
        use rewo_world::biome::{BiomeContext, BiomeDef, BiomeRegistry, Colormaps, GrassModifier};
        use std::sync::Arc;

        // One biome, distinct override per resolver (so distinct resolvers give
        // distinct values, not just distinct keys).
        let argb = |rgb: u32| (0xFFu32 << 24 | rgb) as i32;
        let biome = BiomeDef {
            music_volume: None,
            name: "x".into(),
            temperature: 0.5,
            downfall: 0.5,
            water_color: argb(0x0000FF),            // blue
            grass_override: Some(argb(0x00FF00)),   // green
            foliage_override: Some(argb(0xFF0000)), // red
            dry_foliage_override: Some(argb(0x0000AA)),
            grass_modifier: GrassModifier::None,
            sky_color: None,
            fog_color: None,
                has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_biome_context(Arc::new(BiomeContext::new(
            Arc::new(BiomeRegistry::new(vec![biome])),
            Colormaps::neutral(),
            0,
        )));

        let mut cache = TintCache::default();
        // First Grass request → one entry.
        let g1 = biome_tint(&w, &mut cache, 5, 8, 5, TintSource::Grass).unwrap();
        assert_eq!(cache.map.len(), 1);
        // Same position + resolver → cache hit, no new entry, identical value.
        let g2 = biome_tint(&w, &mut cache, 5, 8, 5, TintSource::Grass).unwrap();
        assert_eq!(cache.map.len(), 1, "same key reuses one entry");
        assert_eq!(g1, g2);
        // GrassBelow at y=9 canonicalizes to Grass at y=8 → the SAME entry.
        let gb = biome_tint(&w, &mut cache, 5, 9, 5, TintSource::GrassBelow).unwrap();
        assert_eq!(cache.map.len(), 1, "GrassBelow@y+1 aliases Grass@y");
        assert_eq!(gb, g1);
        // Distinct resolver at the same position → a new, distinct entry.
        let f = biome_tint(&w, &mut cache, 5, 8, 5, TintSource::Foliage).unwrap();
        assert_eq!(cache.map.len(), 2, "distinct resolver does not alias");
        assert_ne!(f, g1);
        // Distinct position, same resolver → a new entry.
        biome_tint(&w, &mut cache, 6, 8, 5, TintSource::Grass).unwrap();
        assert_eq!(cache.map.len(), 3, "distinct position does not alias");
        // Water + DryFoliage each add their own slot.
        biome_tint(&w, &mut cache, 5, 8, 5, TintSource::Water).unwrap();
        biome_tint(&w, &mut cache, 5, 8, 5, TintSource::DryFoliage).unwrap();
        assert_eq!(cache.map.len(), 5);
        // Constant tint bypasses the cache (fixed color, no window average).
        let before = cache.map.len();
        let c = biome_tint(&w, &mut cache, 5, 8, 5, TintSource::Constant([10, 20, 30])).unwrap();
        assert_eq!(cache.map.len(), before, "constant tint bypasses the cache");
        // M15: the tint flow carries lossless u8 RGB — no float round-trip.
        assert_eq!(c, [10, 20, 30]);
    }

    // -- M15 greedy meshing -------------------------------------------------

    /// A cube whose six faces carry *distinct* atlas layers, so a merge that
    /// crossed a face boundary would be visible in the layer index.
    const STONE: RenderKind = RenderKind::Cube {
        faces: [10, 11, 12, 13, 14, 15],
        raw_faces: [10, 11, 12, 13, 14, 15],
        tint: [TintSource::None; 6],
    };
    /// A second, materially different cube — same geometry, different layers.
    const SLATE: RenderKind = RenderKind::Cube {
        faces: [20, 21, 22, 23, 24, 25],
        raw_faces: [20, 21, 22, 23, 24, 25],
        tint: [TintSource::None; 6],
    };

    fn cube_table() -> Vec<RenderKind> {
        vec![RenderKind::Invisible, STONE, SLATE]
    }

    /// The emitted-geometry decoder is production code (`UnitFace`,
    /// `ExpandedFace`, `split_quads`, `expand_unit_faces`, `expand_quad_list`),
    /// because `check_greedy_oracle` is a release-compiled gate. These wrappers
    /// keep the tests panicking on a malformed stream — and keep both graders on
    /// the *same* decode, so they cannot drift apart.
    type Expanded = ExpandedFace;

    fn quads(v: &[MeshVertex], idx: &[u32]) -> Vec<[MeshVertex; 4]> {
        split_quads(v, idx).expect("quad stream")
    }

    fn expand(v: &[MeshVertex], idx: &[u32]) -> Vec<Expanded> {
        expand_unit_faces(v, idx).expect("unit-face expansion")
    }

    fn expand_quads(qs: &[[MeshVertex; 4]]) -> Vec<Expanded> {
        expand_quad_list(qs).expect("unit-face expansion")
    }

    fn unit_faces(m: &ColumnMesh) -> Vec<UnitFace> {
        let mut v: Vec<UnitFace> = expand(&m.vertices, &m.indices)
            .into_iter()
            .map(|e| e.f)
            .collect();
        v.sort();
        v
    }

    /// The rectangle dimensions the given unit face was emitted inside.
    fn rect_of(m: &ColumnMesh, face: u8, block: [i32; 3]) -> (i32, i32) {
        let hit: Vec<_> = expand(&m.vertices, &m.indices)
            .into_iter()
            .filter(|e| e.f.face == face && e.f.block == block)
            .collect();
        assert_eq!(
            hit.len(),
            1,
            "face {face} of {block:?} must be covered once"
        );
        (hit[0].w, hit[0].h)
    }

    /// A flat plate of `state` at `y`, spanning `[x0,x1] × [z0,z1]` inclusive.
    #[allow(clippy::too_many_arguments)]
    fn plate(w: &mut World, x0: i32, x1: i32, y: i32, z0: i32, z1: i32, state: u32) {
        for x in x0..=x1 {
            for z in z0..=z1 {
                w.set_block(x, y, z, state);
            }
        }
    }

    /// Metrics that must hold for every optimized mesh.
    fn assert_metrics(m: &ColumnMesh) {
        assert_eq!(
            m.visible_cube_faces,
            m.greedy_candidate_faces + m.unit_fallback_faces,
            "every visible cube face is either a candidate or a fallback"
        );
        assert!(
            m.greedy_quads <= m.greedy_candidate_faces,
            "a rectangle consumes at least one candidate: {} > {}",
            m.greedy_quads,
            m.greedy_candidate_faces
        );
    }

    /// The rectangle emitter, for all six faces at 3×2: winding (hence the
    /// geometric normal) survives scaling, the UV reaches exactly 3×2 with the
    /// legacy per-face corner orientation, and positions span the face's own two
    /// tangent axes while its normal axis stays pinned to the plane.
    #[test]
    fn rect_emitter_preserves_orientation_winding_and_uv_scale_on_every_face() {
        let cross = |q: &[MeshVertex]| {
            let e1 = [
                q[1].pos[0] - q[0].pos[0],
                q[1].pos[1] - q[0].pos[1],
                q[1].pos[2] - q[0].pos[2],
            ];
            let e2 = [
                q[2].pos[0] - q[0].pos[0],
                q[2].pos[1] - q[0].pos[1],
                q[2].pos[2] - q[0].pos[2],
            ];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            [n[0] / len, n[1] / len, n[2] / len]
        };

        let base = [7i32, -3, 11];
        for face in 0..6 {
            let (au, av, _) = FACE_AXES[face];
            let an = 3 - au - av;

            let mut uv1 = Vec::new();
            let mut iv1 = Vec::new();
            emit_rect(&mut uv1, &mut iv1, face, base, 1, 1, 0, 0);
            let mut v = Vec::new();
            let mut i = Vec::new();
            emit_rect(&mut v, &mut i, face, base, 3, 2, 0, 0);

            // Winding: the 3×2 rect must face the same way as the unit rect,
            // and both must agree with FACE_OFFSETS (the mesher winds so the
            // CCW normal points *into* the block — consistent across all six).
            let n_unit = cross(&uv1);
            let n_rect = cross(&v);
            let off = FACE_OFFSETS[face];
            let expect = [-off.0 as f32, -off.1 as f32, -off.2 as f32];
            assert_eq!(n_unit, expect, "face {face}: unit normal");
            assert_eq!(n_rect, expect, "face {face}: 3x2 normal matches the unit");
            assert_eq!(
                i,
                vec![0, 1, 2, 0, 2, 3],
                "face {face}: index pattern unchanged"
            );

            // UV: exactly the legacy corner orientation, scaled to 3×2.
            for (k, (_c, uv)) in FACE_CORNERS[face].iter().enumerate() {
                assert_eq!(
                    v[k].uv,
                    [uv[0] * 3.0, uv[1] * 2.0],
                    "face {face} corner {k} uv"
                );
            }
            let mut set: Vec<[u32; 2]> =
                v.iter().map(|x| [x.uv[0] as u32, x.uv[1] as u32]).collect();
            set.sort();
            assert_eq!(
                set,
                vec![[0, 0], [0, 2], [3, 0], [3, 2]],
                "face {face}: uv set reaches 3x2"
            );

            // Positions: pinned on the normal axis, spanning 3 and 2 on the
            // face's u and v axes respectively.
            let plane = v[0].pos[an];
            let sp = |axis: usize| {
                let lo = v.iter().map(|x| x.pos[axis]).fold(f32::MAX, f32::min);
                let hi = v.iter().map(|x| x.pos[axis]).fold(f32::MIN, f32::max);
                (lo, hi)
            };
            for x in &v {
                assert_eq!(x.pos[an], plane, "face {face}: normal axis is the plane");
            }
            assert_eq!(
                plane, uv1[0].pos[an],
                "face {face}: plane is width-invariant"
            );
            let (ulo, uhi) = sp(au);
            let (vlo, vhi) = sp(av);
            assert_eq!(
                (uhi - ulo, vhi - vlo),
                (3.0, 2.0),
                "face {face}: spans 3 on u and 2 on v"
            );
            assert_eq!(ulo, base[au] as f32, "face {face}: anchored at the base");
            assert_eq!(vlo, base[av] as f32);
        }
    }

    /// A 1×1 rectangle must be the legacy unit quad, bit-for-bit — that is what
    /// lets the greedy path own faces that happen not to merge.
    #[test]
    fn unit_rect_is_bit_identical_to_the_legacy_unit_quad() {
        for face in 0..6 {
            let cf = CubeFace {
                layer: 12,
                tint_rgb: [3, 200, 71],
                lb: 5,
                ls: 9,
                ao: [2; 4],
                shade: face as u8,
            };
            let (mut lv, mut li) = (Vec::new(), Vec::new());
            push_cube_face(&mut lv, &mut li, 4, -9, 6, face, &cf);
            let (mut gv, mut gi) = (Vec::new(), Vec::new());
            emit_rect(
                &mut gv,
                &mut gi,
                face,
                [4, -9, 6],
                1,
                1,
                pack_light_word(12, 5, 9, face as u8, 2),
                pack_tint([3, 200, 71], 0),
            );
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&gv),
                bytemuck::cast_slice::<_, u8>(&lv),
                "face {face}"
            );
            assert_eq!(gi, li, "face {face} indices");
        }
    }

    /// A flat plate merges hard: far fewer quads and vertices than the
    /// reference, with at least one rectangle covering many unit faces — and
    /// the expansion still reproduces the reference face-for-face.
    #[test]
    fn flat_plate_merges_and_expands_back_to_the_reference() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        plate(&mut w, 4, 9, 64, 4, 8, 1); // 6 × 5 blocks
        let table = cube_table();

        let r = mesh_column_reference(&w, &table, &[], &[], 0, 0).expect("reference meshed");
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized meshed");
        assert_metrics(&o);

        // 30 blocks: 30 tops + 30 bottoms + a 22-face perimeter.
        assert_eq!(r.visible_cube_faces, 82);
        assert_eq!(o.visible_cube_faces, r.visible_cube_faces);
        assert_eq!(r.greedy_quads, 0, "the reference merges nothing");
        assert_eq!(r.unit_fallback_faces, r.visible_cube_faces);

        assert!(
            o.indices.len() < r.indices.len() && o.vertices.len() < r.vertices.len(),
            "optimized must be strictly smaller: {} idx / {} verts vs {} / {}",
            o.indices.len(),
            r.indices.len(),
            o.vertices.len(),
            r.vertices.len()
        );

        let biggest = expand(&o.vertices, &o.indices)
            .iter()
            .map(|e| e.w * e.h)
            .max()
            .unwrap();
        assert!(
            biggest > 1,
            "at least one rectangle must cover more than one unit face"
        );
        // The 6×5 bottom plane is one rectangle. (Face 1 shares face 0's u/v
        // axes, so this is the same merge shape the top would have formed —
        // except up faces never merge; see `up_faces_never_merge_and_stay_unit_quads`.)
        assert_eq!(
            rect_of(&o, 1, [4, 64, 4]),
            (6, 5),
            "the whole bottom merges"
        );

        assert_eq!(
            unit_faces(&o),
            unit_faces(&r),
            "expanded optimized output must equal the reference face set"
        );
    }

    /// Up (+Y) faces never merge, however uniform their AO — see the merge site
    /// in [`mesh_column`] for the measurement that carved them out.
    ///
    /// A bare plate is the sharpest fixture for it: nothing occludes anything,
    /// so *every* visible face has uniform AO and the direction is the only
    /// thing separating a candidate from a fallback.
    #[test]
    fn up_faces_never_merge_and_stay_unit_quads() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        plate(&mut w, 4, 9, 64, 4, 8, 1); // the same 6 × 5 plate
        let table = cube_table();

        let r = mesh_column_reference(&w, &table, &[], &[], 0, 0).expect("reference");
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized");
        assert_metrics(&o);

        // 82 = 30 tops + 30 bottoms + a 22-face perimeter. The 30 that fall back
        // are exactly the tops, and they fall back purely for facing up.
        assert_eq!(
            (
                o.visible_cube_faces,
                o.greedy_candidate_faces,
                o.unit_fallback_faces
            ),
            (82, 52, 30),
            "with uniform AO everywhere, only the up faces fall back"
        );

        let ups: Vec<_> = expand(&o.vertices, &o.indices)
            .into_iter()
            .filter(|e| e.f.face == 0)
            .collect();
        assert_eq!(ups.len(), 30, "every top is still covered exactly once");
        for e in &ups {
            assert_eq!(
                (e.w, e.h),
                (1, 1),
                "up face of {:?} merged into a {}x{} rectangle",
                e.f.block,
                e.w,
                e.h
            );
        }

        assert_eq!(unit_faces(&o), unit_faces(&r));
    }

    /// Distinct materials must never coalesce, even sharing a plane, light and
    /// tint — the block state is in the merge key for exactly this.
    #[test]
    fn distinct_block_states_do_not_coalesce_in_one_plane() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        plate(&mut w, 4, 7, 64, 4, 7, 1); // stone half
        plate(&mut w, 8, 11, 64, 4, 7, 2); // slate half, same plane
        let table = cube_table();
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("meshed");
        assert_metrics(&o);

        // Graded on the bottom plane: it shares the top's u/v axes, and up faces
        // never merge at all.
        assert_eq!(
            rect_of(&o, 1, [4, 64, 4]),
            (4, 4),
            "stone bottoms merge alone"
        );
        assert_eq!(
            rect_of(&o, 1, [8, 64, 4]),
            (4, 4),
            "slate bottoms merge alone"
        );
        assert_eq!(
            unit_faces(&o),
            unit_faces(&mesh_column_reference(&w, &table, &[], &[], 0, 0).unwrap())
        );
    }

    /// Models never enter the greedy path: a model-only column must come out of
    /// the optimized mesher byte-identical to the reference.
    #[test]
    fn model_only_column_is_byte_identical_to_the_reference() {
        // Fixture lives in production — `check_greedy_oracle`'s model control
        // grades the identical construction.
        let table = oracle_model_table();
        let models = oracle_model_quads();
        let w = oracle_model_world();
        let r = mesh_column_reference(&w, &table, &models, &[], 0, 0).expect("reference");
        let o = mesh_column(&w, &table, &models, &[], 0, 0).expect("optimized");

        assert!(!o.vertices.is_empty());
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&o.vertices),
            bytemuck::cast_slice::<_, u8>(&r.vertices),
            "model vertices must be byte-identical"
        );
        assert_eq!(o.indices, r.indices, "model indices must be identical");
        assert_eq!((o.y_min, o.y_max), (r.y_min, r.y_max));
        assert_eq!(
            (
                o.visible_cube_faces,
                o.greedy_candidate_faces,
                o.greedy_quads
            ),
            (0, 0, 0),
            "models are not cube faces"
        );
    }

    /// Fluids never enter the greedy path either — both the opaque (lava) and
    /// translucent (water) streams must be byte-identical to the reference.
    #[test]
    fn fluid_only_column_is_byte_identical_to_the_reference() {
        let table = fluid_table();
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        for x in 3..7 {
            for z in 3..7 {
                w.set_block(x, 62, z, 2); // water
                w.set_block(x, 63, z, 2);
                w.set_block(x + 8, 62, z, 3); // lava
            }
        }
        let r = mesh_column_reference(&w, &table, &[], &[], 0, 0).expect("reference");
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized");

        assert!(!o.vertices.is_empty(), "lava populates the opaque stream");
        assert!(
            !o.tvertices.is_empty(),
            "water populates the translucent one"
        );
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&o.vertices),
            bytemuck::cast_slice::<_, u8>(&r.vertices),
            "opaque fluid bytes"
        );
        assert_eq!(o.indices, r.indices, "opaque fluid indices");
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&o.tvertices),
            bytemuck::cast_slice::<_, u8>(&r.tvertices),
            "translucent fluid bytes"
        );
        assert_eq!(o.tindices, r.tindices, "translucent fluid indices");
        assert_eq!(
            (
                o.visible_cube_faces,
                o.greedy_candidate_faces,
                o.greedy_quads
            ),
            (0, 0, 0)
        );
    }

    /// A face whose four corner AO codes disagree must fall back to a unit quad
    /// with its per-corner gradient intact, and must not be swallowed by the
    /// rectangle its uniform neighbours form.
    ///
    /// The fixture is a vertical **north**-facing wall rather than a floor: up
    /// faces never merge regardless of AO, so grading the AO rule on a top plane
    /// would pass no matter what the rule did.
    #[test]
    fn ao_discontinuity_falls_back_and_is_not_merged() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        // A 7-wide, 6-tall wall standing in the z = 6 plane.
        for x in 3..10 {
            for y in 64..70 {
                w.set_block(x, y, 6, 1);
            }
        }
        // One block floating off the wall's north side (z = 5, the air side those
        // faces look into), diagonally out from (6,64,6): it occludes exactly one
        // corner of that face's AO. It is *not* adjacent to (6,64,6), so it also
        // culls nothing there.
        w.set_block(5, 65, 5, 1);
        let table = cube_table();

        let r = mesh_column_reference(&w, &table, &[], &[], 0, 0).expect("reference");
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized");
        assert_metrics(&o);

        let disc = expand(&o.vertices, &o.indices)
            .into_iter()
            .find(|e| e.f.face == 2 && e.f.block == [6, 64, 6])
            .expect("north face of (6,64,6) must exist");
        assert!(
            disc.f.ao.iter().any(|a| *a != disc.f.ao[0]),
            "this face's AO must actually be non-uniform: {:?}",
            disc.f.ao
        );
        assert_eq!(
            (disc.w, disc.h),
            (1, 1),
            "a non-uniform-AO face must stay a 1x1 quad, not merge"
        );

        // The rest of the wall still merges hard — including the part of the very
        // same north plane the occluder never reached, so the fallback is local to
        // the discontinuity rather than a plane-wide bail-out.
        assert_eq!(
            rect_of(&o, 2, [7, 64, 6]),
            (3, 6),
            "the untouched north faces merge into one rectangle"
        );
        assert_eq!(
            rect_of(&o, 3, [3, 64, 6]),
            (7, 6),
            "the unoccluded south plane is a single rectangle"
        );

        // And the whole expansion still matches the reference exactly, so the
        // gradient survived verbatim.
        assert_eq!(unit_faces(&o), unit_faces(&r));
    }

    /// Masks span the column's whole occupied height, so a wall merges straight
    /// through a 16-block section boundary.
    #[test]
    fn wall_merges_across_a_section_boundary() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        // y=64 is a section floor (sections are [-64,-48), … , [48,64), [64,80)).
        for y in 58..=69 {
            w.set_block(8, y, 8, 1);
        }
        let table = cube_table();
        let r = mesh_column_reference(&w, &table, &[], &[], 0, 0).expect("reference");
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized");
        assert_metrics(&o);

        // The north face plane holds one 1-wide, 12-tall run.
        assert_eq!(
            rect_of(&o, 2, [8, 58, 8]),
            (1, 12),
            "the whole wall face is one rectangle"
        );
        let spans = quads(&o.vertices, &o.indices).into_iter().any(|q| {
            let lo = q.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
            lo < 64.0 && hi > 64.0
        });
        assert!(spans, "at least one rectangle must cross the y=64 boundary");
        assert_eq!(unit_faces(&o), unit_faces(&r));
    }

    /// Merging is column-local: a full-width plate emits its own west and east
    /// faces on the column's own boundary planes, and no rectangle reaches past
    /// them into a neighbouring column.
    #[test]
    fn merging_never_crosses_a_column_boundary() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(1, 1);
        let (base_x, base_z) = (16, 16);
        plate(&mut w, base_x, base_x + 15, 64, base_z, base_z + 15, 1);
        let table = cube_table();
        let r = mesh_column_reference(&w, &table, &[], &[], 1, 1).expect("reference");
        let o = mesh_column(&w, &table, &[], &[], 1, 1).expect("optimized");
        assert_metrics(&o);

        for v in &o.vertices {
            assert!(
                v.pos[0] >= base_x as f32 && v.pos[0] <= (base_x + 16) as f32,
                "x {} escapes the column's span",
                v.pos[0]
            );
            assert!(
                v.pos[2] >= base_z as f32 && v.pos[2] <= (base_z + 16) as f32,
                "z {} escapes the column's span",
                v.pos[2]
            );
        }
        // The boundary faces are emitted by this column, independently.
        assert_eq!(rect_of(&o, 4, [base_x, 64, base_z]), (16, 1), "west wall");
        assert_eq!(
            rect_of(&o, 5, [base_x + 15, 64, base_z]),
            (16, 1),
            "east wall"
        );
        assert!(
            o.vertices.iter().any(|v| v.pos[0] == (base_x + 16) as f32),
            "the east boundary plane itself is reached"
        );
        assert_eq!(unit_faces(&o), unit_faces(&r));
    }

    /// The whole-column equivalence claim, on a fixture that mixes every path:
    /// merged cubes, AO fallbacks, two materials, models and both fluids.
    #[test]
    fn mixed_column_expands_to_the_reference_and_keeps_other_paths_intact() {
        use rewo_data::assets::Quad;
        let mut table = cube_table();
        table.push(RenderKind::Fluid {
            layer: 30,
            raw_layer: 30,
            level: 0,
            lava: false,
        }); // 3
        table.push(RenderKind::Fluid {
            layer: 31,
            raw_layer: 31,
            level: 0,
            lava: true,
        }); // 4
        table.push(RenderKind::Model(0)); // 5
        let models = vec![vec![Quad {
            verts: [
                [0.2, 0.0, 0.2],
                [0.8, 0.0, 0.2],
                [0.8, 1.0, 0.2],
                [0.2, 1.0, 0.2],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 40,
            raw_layer: 40,
            cull: -1,
            dir: 2,
            tint: TintSource::None,
            shade: true,
        }]];

        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        plate(&mut w, 1, 14, 62, 1, 14, 1); // stone floor
        plate(&mut w, 4, 9, 63, 4, 9, 2); // slate slab on top
        w.set_block(2, 63, 2, 1); // AO discontinuity source
        w.set_block(12, 63, 12, 5); // a model
        w.set_block(11, 63, 3, 3); // water
        w.set_block(3, 63, 11, 4); // lava

        let r = mesh_column_reference(&w, &table, &models, &[], 0, 0).expect("reference");
        let o = mesh_column(&w, &table, &models, &[], 0, 0).expect("optimized");
        assert_metrics(&o);

        assert!(
            o.greedy_quads > 0 && o.unit_fallback_faces > 0,
            "both paths run"
        );
        assert_eq!((o.y_min, o.y_max), (r.y_min, r.y_max), "bounds unchanged");
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&o.tvertices),
            bytemuck::cast_slice::<_, u8>(&r.tvertices),
            "water is untouched by the cube path"
        );
        assert_eq!(o.tindices, r.tindices);

        // Layers 10..=25 are the two cube materials; 30/31 are fluids and 40 is
        // the model. Split the opaque stream on that so each path is graded by
        // the right oracle.
        let split = |m: &ColumnMesh| {
            let (cube, other): (Vec<_>, Vec<_>) = quads(&m.vertices, &m.indices)
                .into_iter()
                .partition(|q| q[0].layer_index() < 30);
            (cube, other)
        };
        let (o_cube, o_other) = split(&o);
        let (r_cube, r_other) = split(&r);

        // The non-cube opaque quads (lava + model) keep their scan order and
        // their exact bytes — the cube path did not disturb them.
        assert!(!o_other.is_empty(), "lava and the model land here");
        assert_eq!(o_other.len(), r_other.len(), "no model/fluid quad lost");
        for (a, b) in o_other.iter().zip(&r_other) {
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(a),
                bytemuck::cast_slice::<_, u8>(b),
                "model/fluid quads must be byte-identical"
            );
        }

        let (mut oc, mut rc) = (
            expand_quads(&o_cube)
                .into_iter()
                .map(|e| e.f)
                .collect::<Vec<_>>(),
            expand_quads(&r_cube)
                .into_iter()
                .map(|e| e.f)
                .collect::<Vec<_>>(),
        );
        oc.sort();
        rc.sort();
        assert_eq!(oc, rc, "the cube stream expands to the reference face set");
        assert!(
            o.vertices.len() < r.vertices.len(),
            "the merge still paid off: {} vs {}",
            o.vertices.len(),
            r.vertices.len()
        );
    }

    /// Two runs of the same column must produce byte-identical geometry: the
    /// masks are index-ordered arrays, not hash maps.
    #[test]
    fn output_is_deterministic_across_runs() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        plate(&mut w, 2, 13, 64, 2, 13, 1);
        plate(&mut w, 5, 8, 65, 5, 8, 2);
        let table = cube_table();
        let a = mesh_column(&w, &table, &[], &[], 0, 0).expect("meshed");
        for _ in 0..4 {
            let b = mesh_column(&w, &table, &[], &[], 0, 0).expect("meshed");
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&a.vertices),
                bytemuck::cast_slice::<_, u8>(&b.vertices)
            );
            assert_eq!(a.indices, b.indices);
            assert_eq!(a.greedy_quads, b.greedy_quads);
        }
    }

    /// The production gate ([`check_greedy_oracle`]) runs green, and its report
    /// carries counts that show the adversarial fixture actually exercised every
    /// property — a checker that silently graded an empty column would pass its
    /// own assertions, so the *report* is what proves the fixture bit.
    #[test]
    fn production_greedy_oracle_passes_and_measures_a_real_fixture() {
        let rep = check_greedy_oracle().expect("greedy oracle");

        // A substantial surface, meaningfully compressed.
        assert!(
            rep.reference_unit_faces > 500,
            "fixture is too small to be adversarial: {} unit faces",
            rep.reference_unit_faces
        );
        assert_eq!(
            rep.reference_unit_faces, rep.visible_cube_faces as usize,
            "the reference emits exactly one quad per visible face"
        );
        assert_eq!(rep.reference_quads, rep.reference_unit_faces);
        assert!(
            rep.optimized_quads < rep.reference_quads,
            "no compression: {} vs {} quads",
            rep.optimized_quads,
            rep.reference_quads
        );
        assert!(
            rep.vertex_reduction_percent > 20.0 && rep.index_reduction_percent > 20.0,
            "reduction too small to be a real merge: {:.1}% verts / {:.1}% indices",
            rep.vertex_reduction_percent,
            rep.index_reduction_percent
        );

        // Both paths ran, and the metrics balance.
        assert!(rep.greedy_quads > 0 && rep.unit_fallback_faces > 0);
        assert_eq!(
            rep.visible_cube_faces,
            rep.greedy_candidate_faces + rep.unit_fallback_faces
        );
        assert_eq!(
            rep.optimized_quads,
            (rep.greedy_quads + rep.unit_fallback_faces) as usize
        );

        // All five safe directions merged; up never did.
        for face in 1..6 {
            assert!(
                rep.max_rect_area_per_face[face] > 1,
                "direction {face} never merged"
            );
        }
        assert_eq!(rep.max_up_rect_area, 1, "an up face merged");
        // One up face per block with air above: region A's 6×6 roof, region B's
        // 6-block wall top, the lone occluder, the 8×9 material plate, both 8×4
        // light plates and the 8×4 tint plate.
        assert_eq!(
            rep.up_faces,
            36 + 6 + 1 + 72 + 32 + 32 + 32,
            "up-face population"
        );
        assert!(rep.section_crossing_rects > 0);

        // The boundary probes each split into the expected rectangles.
        assert!(rep.nonuniform_ao_faces > 0);
        assert!(rep.ao_plane_max_rect_area > 1);
        assert_eq!(rep.material_boundary_rects, [(8, 3); 3]);
        assert_eq!(rep.material_boundary_layer, OS_SHARED_DOWN_LAYER);
        assert!(rep.cutout_faces > 0);
        assert_eq!(rep.block_light_boundary_rects, [(4, 4); 2]);
        assert_eq!(rep.sky_light_boundary_rects, [(4, 4); 2]);
        assert_eq!(rep.tint_boundary_rects, [(4, 4); 2]);
        assert_eq!(rep.tint_boundary_layer, OS_TINT_RAW_LAYER);
        assert_ne!(rep.tint_boundary_words[0], rep.tint_boundary_words[1]);
        assert_eq!(rep.deterministic_runs, 4);

        // -- legacy controls: byte-identical, and each one non-vacuous -------
        assert!(rep.model_only_identical && rep.water_only_identical && rep.lava_only_identical);

        // 16 model blocks × 2 quads, minus the 12 whose south quad is culled by
        // the model above/beside it — whatever the exact figure, both streams
        // must be whole quads and the model must own the opaque stream alone.
        assert!(
            rep.model_only.opaque_vertices > 0,
            "model control emitted nothing"
        );
        assert_eq!(rep.model_only.opaque_vertices % 4, 0);
        assert_eq!(
            rep.model_only.opaque_indices,
            rep.model_only.opaque_vertices / 4 * 6
        );
        assert_eq!(
            (
                rep.model_only.translucent_vertices,
                rep.model_only.translucent_indices
            ),
            (0, 0),
            "a model must not reach the translucent stream"
        );

        // Water blends: translucent only.
        assert!(
            rep.water_only.translucent_vertices > 0,
            "water control emitted nothing"
        );
        assert_eq!(rep.water_only.translucent_vertices % 4, 0);
        assert_eq!(
            rep.water_only.translucent_indices,
            rep.water_only.translucent_vertices / 4 * 6
        );
        assert_eq!(
            (
                rep.water_only.opaque_vertices,
                rep.water_only.opaque_indices
            ),
            (0, 0),
            "water must not reach the opaque stream"
        );

        // Lava is opaque and fullbright: opaque only.
        assert!(
            rep.lava_only.opaque_vertices > 0,
            "lava control emitted nothing"
        );
        assert_eq!(rep.lava_only.opaque_vertices % 4, 0);
        assert_eq!(
            rep.lava_only.opaque_indices,
            rep.lava_only.opaque_vertices / 4 * 6
        );
        assert_eq!(
            (
                rep.lava_only.translucent_vertices,
                rep.lava_only.translucent_indices
            ),
            (0, 0),
            "lava must not reach the translucent stream"
        );

        // The two fluids are distinct fixtures, not the same column twice.
        assert_ne!(
            rep.water_only.translucent_vertices, rep.lava_only.opaque_vertices,
            "the water and lava controls look like the same geometry"
        );
    }

    // -- M16 dimension cardinal lighting ------------------------------------

    /// A Nether-lit world: the `the_nether` shape *and* `CardinalLighting::NETHER`.
    fn nether_world() -> World {
        use rewo_world::dimension::CardinalLightType;
        let mut w = World::new(DimensionShape::NETHER);
        w.set_cardinal_light_type(CardinalLightType::Nether);
        w.ensure_column(0, 0);
        w
    }

    /// The face direction of a **fluid** quad, from the plane it lies in
    /// relative to its own block. `emit_fluid` winds its four sides identically
    /// (unlike the cube emitter), so their normals cannot tell north from south
    /// — the plane can, and it is still independent of the shade code.
    fn fluid_face_of_quad(q: &[MeshVertex; 4], block: [i32; 3]) -> usize {
        let constant = |axis: usize| q.iter().all(|v| v.pos[axis] == q[0].pos[axis]);
        if constant(0) {
            if q[0].pos[0] == block[0] as f32 {
                4
            } else {
                5
            }
        } else if constant(2) {
            if q[0].pos[2] == block[2] as f32 {
                2
            } else {
                3
            }
        } else {
            assert!(constant(1), "a fluid quad must be planar on one axis");
            // The bottom sits exactly on the block floor; the surface rides
            // above it at the fluid height.
            if q[0].pos[1] == block[1] as f32 {
                1
            } else {
                0
            }
        }
    }

    /// The face direction a **cube or model** quad faces, recovered from its
    /// winding — independent of the shade code, so it can grade the shade code.
    fn face_of_quad(q: &[MeshVertex; 4]) -> usize {
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let e1 = sub(q[1].pos, q[0].pos);
        let e2 = sub(q[2].pos, q[0].pos);
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = [n[0] / len, n[1] / len, n[2] / len];
        // The mesher winds every face so the CCW normal points *into* the block.
        FACE_OFFSETS
            .iter()
            .position(|off| n == [-off.0 as f32, -off.1 as f32, -off.2 as f32])
            .unwrap_or_else(|| panic!("quad normal {n:?} is not axis-aligned"))
    }

    /// The mapping itself, against the transcribed tables, for both dimensions.
    #[test]
    fn face_shade_code_is_the_dimension_cardinal_table() {
        use rewo_world::dimension::CardinalLighting;

        // DEFAULT: the code IS the direction, and codes 0..=5 keep their exact
        // M15 values — this is what makes every Overworld mesh byte unchanged.
        let d = World::new(DimensionShape::OVERWORLD);
        for face in 0..6 {
            assert_eq!(
                face_shade_code(&d, face),
                face as u8,
                "default face {face} must keep its historical code"
            );
            assert_eq!(
                FACE_SHADE[face_shade_code(&d, face) as usize],
                CardinalLighting::DEFAULT.by_mesh_face(face),
                "default face {face} factor"
            );
        }
        assert_eq!(FACE_SHADE[..6], [1.0, 0.5, 0.8, 0.8, 0.6, 0.6]);

        // NETHER: up and down move to the appended code 6; the four sides are
        // unchanged, so they must keep their own codes rather than collapsing
        // onto whichever entry happens to share their value first.
        let n = nether_world();
        assert_eq!(face_shade_code(&n, 0), 6, "nether up");
        assert_eq!(face_shade_code(&n, 1), 6, "nether down");
        for face in 2..6 {
            assert_eq!(
                face_shade_code(&n, face),
                face as u8,
                "nether side {face} is not moved by the dimension"
            );
        }
        for face in 0..6 {
            assert_eq!(
                FACE_SHADE[face_shade_code(&n, face) as usize],
                CardinalLighting::NETHER.by_mesh_face(face),
                "nether face {face} factor"
            );
        }
        assert_eq!(FACE_SHADE[6], 0.9, "code 6 is the Nether up/down factor");
    }

    /// Code 6 reconstructs exactly the legacy float sequence at 0.9 — the same
    /// claim `reconstruction_matches_legacy_formula_for_every_shade_and_ao`
    /// makes for 0..=5, stated on its own for the new entry.
    #[test]
    fn shade_code_six_reconstructs_the_nether_factor() {
        for ao in 0..AO_LEVELS.len() {
            for rgb in [[255u8, 255, 255], [100, 0, 0], [1, 2, 3], [91, 163, 163]] {
                let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, 0, 6, ao as u8, rgb);
                assert_eq!(v.shade_code(), 6, "code 6 survives packing");
                let c = 0.9f32 * AO_LEVELS[ao];
                let legacy = [
                    c * (rgb[0] as f32 / 255.0),
                    c * (rgb[1] as f32 / 255.0),
                    c * (rgb[2] as f32 / 255.0),
                ];
                let got = v.reconstructed_color();
                for i in 0..3 {
                    assert_eq!(
                        got[i].to_bits(),
                        legacy[i].to_bits(),
                        "ao {ao} rgb {rgb:?} channel {i}: {got:?} vs {legacy:?}"
                    );
                }
            }
        }
        // And it is distinguishable from every frozen code, so it cannot be a
        // renumbering of one of them.
        for code in 0..6 {
            assert_ne!(FACE_SHADE[code], FACE_SHADE[6], "code {code} vs 6");
        }
    }

    /// A Nether cube: its top *and* bottom carry code 6 (0.9), its four sides
    /// keep their own codes, and both mesher paths agree. The bottom plane still
    /// merges — the resolved code rides in the merge key, so faces that share it
    /// coalesce and faces that do not, cannot.
    #[test]
    fn nether_cube_top_and_bottom_use_code_six() {
        let mut w = nether_world();
        plate(&mut w, 4, 5, 10, 4, 5, 1); // a 2×2 plate of STONE
        let table = cube_table();

        for (what, m) in [
            (
                "reference",
                mesh_column_reference(&w, &table, &[], &[], 0, 0).expect("reference"),
            ),
            (
                "optimized",
                mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized"),
            ),
        ] {
            let qs = quads(&m.vertices, &m.indices);
            assert!(!qs.is_empty(), "{what}: nothing meshed");
            let mut seen = [0usize; 6];
            for q in &qs {
                let face = face_of_quad(q);
                seen[face] += 1;
                let want = if face <= 1 { 6 } else { face as u8 };
                for v in q {
                    assert_eq!(
                        v.shade_code(),
                        want,
                        "{what}: face {face} must carry shade code {want}"
                    );
                    assert_eq!(
                        FACE_SHADE[v.shade_code() as usize],
                        if face <= 1 {
                            0.9
                        } else if face <= 3 {
                            0.8
                        } else {
                            0.6
                        },
                        "{what}: face {face} shade factor"
                    );
                }
            }
            for face in 0..6 {
                assert!(seen[face] > 0, "{what}: face {face} emitted no quad");
            }
        }

        // The optimized path still merges the bottom plane into one 2×2
        // rectangle, all of it at code 6.
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized");
        let downs: Vec<_> = quads(&o.vertices, &o.indices)
            .into_iter()
            .filter(|q| face_of_quad(q) == 1)
            .collect();
        assert_eq!(downs.len(), 1, "the four bottoms merge into one rectangle");
        assert_eq!(downs[0][0].shade_code(), 6);
        let span = |q: &[MeshVertex; 4], axis: usize| {
            let lo = q.iter().map(|v| v.pos[axis]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|v| v.pos[axis]).fold(f32::MIN, f32::max);
            hi - lo
        };
        assert_eq!((span(&downs[0], 0), span(&downs[0], 2)), (2.0, 2.0));
        // Up faces never merge (M15 carve-out), and each still carries code 6.
        let ups: Vec<_> = quads(&o.vertices, &o.indices)
            .into_iter()
            .filter(|q| face_of_quad(q) == 0)
            .collect();
        assert_eq!(ups.len(), 4, "one unit quad per top");
        for q in &ups {
            assert_eq!(q[0].shade_code(), 6);
            assert_eq!((span(q, 0), span(q, 2)), (1.0, 1.0));
        }
    }

    /// Model quads and fluid faces take the same mapping: a shaded quad follows
    /// the dimension, an unshaded one stays code 0 in *every* dimension.
    #[test]
    fn nether_model_and_fluid_faces_use_the_dimension_mapping() {
        use rewo_data::assets::Quad;

        let quad_at = |dir: u8, shade: bool, y: f32| Quad {
            verts: [[0.0, y, 0.0], [1.0, y, 0.0], [1.0, y, 1.0], [0.0, y, 1.0]],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 7,
            raw_layer: 7,
            cull: -1,
            dir,
            tint: TintSource::None,
            shade,
        };
        // Three quads: shaded up (moved by the Nether), shaded north (not
        // moved), unshaded up (never shaded at all).
        let models = vec![vec![
            quad_at(0, true, 0.0),
            quad_at(2, true, 0.5),
            quad_at(0, false, 1.0),
        ]];
        let table = oracle_model_table();

        for (what, mut w, want) in [
            (
                "default",
                {
                    let mut w = World::new(DimensionShape::OVERWORLD);
                    w.ensure_column(0, 0);
                    w
                },
                [0u8, 2, 0],
            ),
            ("nether", nether_world(), [6, 2, 0]),
        ] {
            w.set_block(4, 10, 4, 1);
            let m = mesh_column(&w, &table, &models, &[], 0, 0).expect("model meshed");
            let codes: Vec<u8> = quads(&m.vertices, &m.indices)
                .iter()
                .map(|q| q[0].shade_code())
                .collect();
            assert_eq!(codes, want, "{what}: model quad shade codes");
            // Every quad's four vertices agree (the code is per-quad).
            for q in quads(&m.vertices, &m.indices) {
                assert!(q.iter().all(|v| v.shade_code() == q[0].shade_code()));
            }
        }

        // Fluids: a water source's top and bottom move to code 6 in the Nether,
        // its sides do not; in a default world every face keeps its direction.
        let ftable = fluid_table();
        for (what, mut w, up_down) in [
            (
                "default",
                {
                    let mut w = World::new(DimensionShape::OVERWORLD);
                    w.ensure_column(0, 0);
                    w
                },
                [0u8, 1],
            ),
            ("nether", nether_world(), [6, 6]),
        ] {
            w.set_block(4, 10, 4, 2); // a lone water source: all six faces visible
            let m = mesh_column(&w, &ftable, &[], &[], 0, 0).expect("fluid meshed");
            let qs = quads(&m.tvertices, &m.tindices);
            assert_eq!(qs.len(), 6, "{what}: a lone source emits six faces");
            let mut seen = [false; 6];
            for q in &qs {
                let face = fluid_face_of_quad(q, [4, 10, 4]);
                assert!(!seen[face], "{what}: face {face} emitted twice");
                seen[face] = true;
                let want = match face {
                    0 => up_down[0],
                    1 => up_down[1],
                    f => f as u8,
                };
                for v in q {
                    assert_eq!(v.shade_code(), want, "{what}: fluid face {face}");
                }
            }
        }
    }

    /// **The default-world byte guarantee.** In a `CardinalLighting::DEFAULT`
    /// world the shade byte of every emitted vertex is exactly the pre-M16 rule
    /// — the face's own direction index for cube and fluid faces, `quad.dir` (or
    /// 0 when unshaded) for model quads — on a fixture that runs all three
    /// paths through both meshers. The direction is recovered from the quad's
    /// winding, so nothing about it comes from the shade code being graded.
    ///
    /// The shade code is the only vertex field M16 touches, so this pins the
    /// whole default output byte-identical to M15. `check_greedy_oracle`'s
    /// unchanged report (see `production_greedy_oracle_passes_and_measures_a_real_fixture`)
    /// is the second half of that claim: its decoder reads the shade code back
    /// *as* a face direction on the adversarial column.
    #[test]
    fn default_world_shade_bytes_are_the_legacy_per_direction_codes() {
        use rewo_data::assets::Quad;

        let mut table = cube_table();
        table.push(RenderKind::Fluid {
            layer: 30,
            raw_layer: 30,
            level: 0,
            lava: false,
        }); // 3
        table.push(RenderKind::Fluid {
            layer: 31,
            raw_layer: 31,
            level: 0,
            lava: true,
        }); // 4
        table.push(RenderKind::Model(0)); // 5
        let models = vec![vec![Quad {
            verts: [
                [0.2, 0.0, 0.2],
                [0.8, 0.0, 0.2],
                [0.8, 1.0, 0.2],
                [0.2, 1.0, 0.2],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 40,
            raw_layer: 40,
            cull: -1,
            dir: 2,
            tint: TintSource::None,
            shade: true,
        }]];

        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        plate(&mut w, 1, 14, 62, 1, 14, 1);
        plate(&mut w, 4, 9, 63, 4, 9, 2);
        w.set_block(2, 63, 2, 1); // an AO discontinuity, so fallbacks run too
        w.set_block(12, 63, 12, 5); // model
        w.set_block(11, 63, 3, 3); // water
        w.set_block(3, 63, 11, 4); // lava

        // The mapping is the identity here, which is precisely the pre-M16
        // constant every emitter used to write.
        for face in 0..6 {
            assert_eq!(face_shade_code(&w, face), face as u8);
        }

        for (what, m) in [
            (
                "reference",
                mesh_column_reference(&w, &table, &models, &[], 0, 0).expect("reference"),
            ),
            (
                "optimized",
                mesh_column(&w, &table, &models, &[], 0, 0).expect("optimized"),
            ),
        ] {
            let mut checked = 0usize;
            for (stream, qs) in [
                ("opaque", quads(&m.vertices, &m.indices)),
                ("translucent", quads(&m.tvertices, &m.tindices)),
            ] {
                for q in &qs {
                    // Three emitters, three ways to recover the direction the
                    // shade byte is supposed to encode. Layer 40 is the model
                    // quad, whose direction is declared (`dir: 2`) rather than
                    // geometric; 30/31 are the two fluid cells, graded by plane;
                    // everything else is a cube face, graded by winding.
                    let want = match q[0].layer_index() {
                        40 => 2,
                        30 => fluid_face_of_quad(q, [11, 63, 3]) as u8,
                        31 => fluid_face_of_quad(q, [3, 63, 11]) as u8,
                        _ => face_of_quad(q) as u8,
                    };
                    for v in q {
                        assert_eq!(
                            v.shade_code(),
                            want,
                            "{what}/{stream}: shade byte is not the legacy per-direction code"
                        );
                        assert!(
                            v.shade_code() < 6,
                            "{what}/{stream}: a default world must never emit an M16 code"
                        );
                    }
                    checked += 1;
                }
            }
            assert!(
                checked > 100,
                "{what}: fixture is too small: {checked} quads"
            );
        }
    }

    /// An all-air column yields nothing from both mesher paths.
    #[test]
    fn empty_column_returns_none_from_both_paths() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        let table = cube_table();
        assert!(mesh_column_reference(&w, &table, &[], &[], 0, 0).is_none());
        assert!(mesh_column(&w, &table, &[], &[], 0, 0).is_none());
    }
}
