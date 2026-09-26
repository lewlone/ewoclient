//! rewo-mesh — M4 mesher: full-cube fast path, plus the general model-quad
//! path for everything else (stairs, slabs, fences, glass, plants, torches, …).
//!
//! **Lighting is vanilla's**, per `ModelBlockRenderer.tesselateBlock`: with
//! smooth lighting on, a block that emits no light and whose model uses ambient
//! occlusion goes through `BlockModelLighter.prepareQuadAmbientOcclusion`
//! ([`smooth_light`]); everything else through `prepareQuadFlat`. Each vertex
//! stores what vanilla's vertex stores: the 8-bit RGB color (`ARGB.gray` of the
//! AO brightness, scaled by the cardinal face shade, times the tint) and the
//! two light channels in `LightCoordsUtil` smooth units (vanilla's `UV2`). The
//! shader samples the lightmap per vertex from those, so the time of day never
//! forces a remesh. See [`MeshVertex`].
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
//! because per-vertex lighting makes coplanar faces non-mergeable. That is true
//! only for faces whose four vertices *differ*: across such a face the
//! rasterizer interpolates a gradient that a merged rectangle cannot reproduce.
//! A face whose four vertices carry the same light and color has no gradient at
//! all, so replacing N of them with one rectangle is mathematically identical.
//! [`mesh_column`] therefore splits the cube path in two: uniform faces on the
//! five non-up directions enter a per-(face, plane) mask and are merged; up
//! (+Y) faces and non-uniform faces fall back to the unit quad, byte-for-byte. The up carve-out is
//! empirical, not structural — the merge site in [`mesh_column`] records what
//! merging it measured. Models and fluids never merge — they run the untouched
//! [`emit_model`] / [`emit_fluid`] path.
//!
//! [`mesh_column_reference`] is the frozen pre-greedy mesher, kept as the
//! unit-face oracle: expanding the optimized output back into unit faces must
//! reproduce it exactly.

pub mod crumbling;
pub mod pool;
pub mod smooth_light;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rewo_data::assets::{CarriedFluid, CullInfo, RenderKind, TintSource};
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
/// face). Multiplied into the vertex color the way vanilla's `multiplyColor`
/// does (see [`block_vertex_color`]).
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

// -- packed vertex -----------------------------------------------------------

/// `LightCoordsUtil.FULL_BRIGHT` = `pack(15, 15)`.
pub const FULL_BRIGHT: i32 = 15 << 4 | 15 << 20;

/// `LightCoordsUtil.pack(block, sky)` — whole light levels in vanilla's packed
/// light coordinates (block in bits 4..7, sky in bits 20..23).
pub const fn light_coords(block: u8, sky: u8) -> i32 {
    (block as i32) << 4 | (sky as i32) << 20
}

/// `LightCoordsUtil.max` — the brighter of two coordinates, per channel.
fn light_max(a: i32, b: i32) -> i32 {
    let block = ((a >> 4) & 15).max((b >> 4) & 15);
    let sky = ((a >> 20) & 15).max((b >> 20) & 15);
    block << 4 | sky << 20
}

/// The vertex's `light` word: `layer[0..15] | block[16..23] | sky[24..31]`.
///
/// The two light channels are vanilla's `UV2` pair: `LightCoordsUtil` smooth
/// units, i.e. `smoothBlock(coords)` / `smoothSky(coords)`, a light level ×16
/// plus the fraction smooth lighting blends in (0..=240). They stay separate to
/// the shader, which is what lets the time of day re-light the world without a
/// remesh.
pub fn pack_light_word(layer: u32, coords: i32) -> u32 {
    debug_assert!(layer <= 0xFFFF, "texture layer {layer} exceeds 16 bits");
    let block = (coords & 0xFF) as u32;
    let sky = ((coords >> 16) & 0xFF) as u32;
    (layer & 0xFFFF) | block << 16 | sky << 24
}

/// White tint — the untinted path; multiplying by it is the identity.
pub const TINT_WHITE: [u8; 3] = [255, 255, 255];

/// `ARGB.as8BitChannel(value)` = `Mth.floor(value * 255.0F)`.
fn as_8bit_channel(value: f32) -> i32 {
    (value * 255.0f32).floor() as i32
}

/// The vertex color vanilla's block renderer produces, bit for bit:
/// `ARGB.gray(brightness)` (`prepareQuadAmbientOcclusion` / `prepareQuadFlat`),
/// then `QuadInstance.scaleColor(shade)` (`ARGB.scaleRGB`: `(int)(c * shade)`
/// clamped), then `multiplyColor(tint)` (`ARGB.multiply`: `a * b / 255`).
///
/// The flat path is `brightness = shade, shade = 1.0`: vanilla's
/// `prepareQuadFlat` puts the cardinal shade into `gray` directly and never
/// scales. [`TINT_WHITE`] is the identity of the last step, which is what an
/// untinted quad (`tintIndex == -1`) skips.
pub fn block_vertex_color(brightness: f32, shade: f32, tint: [u8; 3]) -> [u8; 3] {
    let gray = as_8bit_channel(brightness);
    let scaled = ((gray as f32 * shade) as i32).clamp(0, 255);
    tint.map(|t| (scaled * t as i32 / 255) as u8)
}

/// `ARGB.scaleRGB(tintColor, shade)` on an opaque tint — the fluid renderer's
/// vertex color (`FluidRenderer`, which never goes through `gray`).
fn scale_rgb(tint: [u8; 3], shade: f32) -> [u8; 3] {
    tint.map(|t| ((t as f32 * shade) as i32).clamp(0, 255) as u8)
}

/// **28 bytes**, the same fields vanilla's block vertex carries.
///
/// - `pos`   f32x3 (12) — world space (the mesher emits world space and the
///   shader adds no column origin)
/// - `uv`    f32x2 (8)  — exact; see below
/// - `light` u32 (4)   — see [`pack_light_word`]
/// - `color` u32 (4)   — `r[0..7] g[8..15] b[16..23]`, byte 3 = 255; the GPU
///   reads it as `R8G8B8A8_UNORM`, the same conversion vanilla's `Color`
///   attribute gets
///
/// A 24-byte variant storing UV as f16 was built and **rejected**: `emit_fluid`
/// emits surface UVs of the form `1.0 - k/9` (vanilla's 8/9 source height), and
/// `1/9` is not a dyadic rational, so no f16 represents it. The resulting
/// ~2.7e-5 UV error is only ~0.0004 texel, but it lands on a texel boundary and
/// flipped nearest-neighbour sampling on 6 demo pixels (max channel delta 25),
/// breaking byte-identical rendering. So UV stays f32.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq, Debug)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub light: u32,
    pub color: u32,
}

impl MeshVertex {
    /// The one constructor: `coords` is vanilla packed light
    /// (`LightCoordsUtil`), `color` the finished 8-bit vertex color.
    pub fn new(pos: [f32; 3], uv: [f32; 2], layer: u32, coords: i32, color: [u8; 3]) -> Self {
        Self {
            pos,
            uv,
            light: pack_light_word(layer, coords),
            color: pack_color(color),
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

    /// Block light in smooth units (`LightCoordsUtil.smoothBlock`, 0..=240).
    pub fn block_smooth(&self) -> u8 {
        ((self.light >> 16) & 0xFF) as u8
    }

    /// Sky light in smooth units (`LightCoordsUtil.smoothSky`, 0..=240).
    pub fn sky_smooth(&self) -> u8 {
        ((self.light >> 24) & 0xFF) as u8
    }

    /// Whole block light level (`LightCoordsUtil.block`: the smooth value's
    /// integer part).
    pub fn block_light(&self) -> u8 {
        self.block_smooth() >> 4
    }

    /// Whole sky light level (`LightCoordsUtil.sky`).
    pub fn sky_light(&self) -> u8 {
        self.sky_smooth() >> 4
    }

    pub fn color_rgb(&self) -> [u8; 3] {
        [
            (self.color & 0xFF) as u8,
            ((self.color >> 8) & 0xFF) as u8,
            ((self.color >> 16) & 0xFF) as u8,
        ]
    }

    /// The color as the vertex shader sees it: `R8G8B8A8_UNORM`, `c / 255`.
    pub fn reconstructed_color(&self) -> [f32; 3] {
        self.color_rgb().map(|c| c as f32 / 255.0)
    }
}

/// Pack an opaque RGB color into the vertex's `color` word.
pub fn pack_color(rgb: [u8; 3]) -> u32 {
    (rgb[0] as u32) | ((rgb[1] as u32) << 8) | ((rgb[2] as u32) << 16) | 0xFF00_0000
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
    /// block's water (M164).
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
/// `CardinalLighting.byFace` for a mesher face (`[up, down, north, south,
/// west, east]`) under the world's dimension: 0.5/1.0/0.8/0.6 by default, the
/// Nether's 0.9 up and down.
pub fn face_shade(world: &World, face: usize) -> f32 {
    world.cardinal_light().by_mesh_face(face)
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
/// Everything the mesher reads per block state, borrowed.
///
/// `cull` may be empty (or shorter than `render`): a state with no entry falls
/// back to the legacy rule — a `RenderKind::Cube` occludes all six faces, is a
/// full opaque block to the lighting, and nothing is translucent — which is
/// what every synthetic fixture relies on. `emission` / `dampening` may be
/// empty the same way (no emitters; a cube dampens 15, anything else 0).
#[derive(Clone, Copy)]
pub struct MeshInputs<'a> {
    pub render: &'a [RenderKind],
    pub models: &'a [Vec<rewo_data::assets::Quad>],
    pub fluid: &'a [Option<CarriedFluid>],
    pub cull: &'a [CullInfo],
    /// `BlockState.getLightEmission()` per state.
    pub emission: &'a [u8],
    /// `BlockState.getLightDampening()` per state.
    pub dampening: &'a [u8],
    /// Vanilla's "Smooth Lighting" option (`Options.ambientOcclusion`, on by
    /// default): off, every block takes the flat path.
    pub smooth_lighting: bool,
}

impl<'a> MeshInputs<'a> {
    /// The production tables of a bake, smooth lighting on.
    pub fn from_baked(b: &'a rewo_data::assets::BakedAssets) -> Self {
        Self {
            render: &b.render,
            models: &b.models,
            fluid: &b.fluid,
            cull: &b.cull,
            emission: &b.emission,
            dampening: &b.dampening,
            smooth_lighting: true,
        }
    }

    /// Only the geometry tables; no per-state lighting facts (the legacy
    /// fallbacks of [`MeshInputs`] apply), smooth lighting on.
    pub fn geometry(
        render: &'a [RenderKind],
        models: &'a [Vec<rewo_data::assets::Quad>],
        fluid: &'a [Option<CarriedFluid>],
    ) -> Self {
        Self {
            render,
            models,
            fluid,
            cull: &[],
            emission: &[],
            dampening: &[],
            smooth_lighting: true,
        }
    }
}

/// The per-state lookups the emitters share.
#[derive(Clone, Copy)]
struct Tables<'a> {
    render: &'a [RenderKind],
    cull: &'a [CullInfo],
    emission: &'a [u8],
    dampening: &'a [u8],
    smooth_lighting: bool,
}

impl<'a> Tables<'a> {
    fn new(inputs: &MeshInputs<'a>) -> Self {
        Self {
            render: inputs.render,
            cull: inputs.cull,
            emission: inputs.emission,
            dampening: inputs.dampening,
            smooth_lighting: inputs.smooth_lighting,
        }
    }

    fn get(&self, state: usize) -> Option<&'a RenderKind> {
        self.render.get(state)
    }

    fn info(&self, state: u32) -> CullInfo {
        match self.cull.get(state as usize) {
            Some(c) => *c,
            None => {
                let cube = self.is_cube(state);
                CullInfo {
                    occludes: if cube { 0b11_1111 } else { 0 },
                    ao_occluder: cube,
                    view_blocking: cube,
                    shade_dark: cube,
                    ambient_occlusion: true,
                    ..CullInfo::default()
                }
            }
        }
    }

    fn emission(&self, state: u32) -> u8 {
        self.emission.get(state as usize).copied().unwrap_or(0)
    }

    fn dampening(&self, state: u32) -> u8 {
        match self.dampening.get(state as usize) {
            Some(d) => *d,
            None if self.is_cube(state) => 15,
            None => 0,
        }
    }

    /// `LightCoordsUtil.getLightCoords(BrightnessGetter.DEFAULT, level, state,
    /// pos)`: `FULL_BRIGHT` for an emissive-rendering state, else the stored
    /// light at `pos`, with the block channel raised to the state's own
    /// emission (`withBlock`).
    fn light_coords(&self, world: &World, state: u32, x: i32, y: i32, z: i32) -> i32 {
        if self.info(state).emissive_rendering {
            return FULL_BRIGHT;
        }
        let (block, sky) = world.light_at(x, y, z);
        let packed = light_coords(block, sky);
        let emission = self.emission(state);
        if block < emission {
            packed & 0xFF_0000 | (emission as i32) << 4
        } else {
            packed
        }
    }

    /// `ModelBlockRenderer.tesselateBlock`'s choice: smooth lighting on, the
    /// state emits no light, and its (first) model uses ambient occlusion.
    fn uses_ao(&self, state: u32) -> bool {
        self.smooth_lighting && self.emission(state) == 0 && self.info(state).ambient_occlusion
    }

    fn is_cube(&self, state: u32) -> bool {
        matches!(self.render.get(state as usize), Some(RenderKind::Cube { .. }))
    }

    /// `neighbor.getFaceOcclusionShape(face) == Shapes.block()` — `face` is
    /// the neighbour's own face (the one touching us).
    fn occludes(&self, state: u32, face: usize) -> bool {
        self.info(state).occludes & (1 << face) != 0
    }

    /// `Block.shouldRenderFace(state, neighbor, face)` for the two cases Rewo
    /// can decide exactly: a fully occluding neighbour face, and
    /// `skipRendering`. Partial-shape against partial-shape pairs (two slabs
    /// side by side) are drawn — vanilla culls some of those via
    /// `Shapes.joinIsNotEmpty`, which needs per-face shapes the bake does not
    /// carry; the result is hidden overdraw, never a hole.
    fn should_render(&self, state: u32, neighbor: u32, face: usize) -> bool {
        let opp = face ^ 1;
        if self.occludes(neighbor, opp) {
            return false;
        }
        let s = self.info(state);
        if s.skip == 0 {
            return true;
        }
        let n = self.info(neighbor);
        let same = n.block == s.block && self.cull.get(neighbor as usize).is_some();
        let skip = match s.skip {
            // `HalfTransparentBlock` / `PowderSnowBlock`: `neighbor.is(this)`.
            1 => same,
            // `IronBarsBlock`: same block, or both in `#bars` (every pane and
            // bar declares all four connection properties).
            2 => {
                (same || (n.bars && s.bars))
                    && (face < 2 || (s.connect & (1 << face) != 0 && n.connect & (1 << opp) != 0))
            }
            // `MangroveRootsBlock`: same block on the Y axis.
            3 => same && face < 2,
            _ => false,
        };
        !skip
    }
}

#[cfg(any(test, feature = "oracle"))]
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
    mesh_column_reference_with(
        world,
        MeshInputs::geometry(table, models, carried),
        cx,
        cz,
    )
}

#[cfg(any(test, feature = "oracle"))]
/// [`mesh_column_reference`] over full [`MeshInputs`].
pub(crate) fn mesh_column_reference_with(
    world: &World,
    inputs: MeshInputs<'_>,
    cx: i32,
    cz: i32,
) -> Option<ColumnMesh> {
    let table = Tables::new(&inputs);
    let ao = AoView { world, table };
    let (models, carried) = (inputs.models, inputs.fluid);
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
                    if let Some(f) = fluid_at(table.render, carried, state) {
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
                                &ao,
                                &mut tint_cache,
                                &mut vertices,
                                &mut indices,
                                &mut tvertices,
                                &mut tindices,
                                wx,
                                y,
                                wz,
                                state,
                                faces,
                                raw_faces,
                                tint,
                            );
                            bump(y as f32);
                        }
                        Some(RenderKind::Model(idx)) => {
                            emit_model(
                                &ao,
                                models,
                                &mut tint_cache,
                                &mut vertices,
                                &mut indices,
                                &mut tvertices,
                                &mut tindices,
                                wx,
                                y,
                                wz,
                                state,
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
/// blocks can legitimately share an atlas layer on one face, and `light`/`color`
/// are lossy summaries of *appearance*, not of *material*. Keying on the block
/// state as well makes coalescence a deliberate "same block" decision rather
/// than a coincidence of packed bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FaceKey {
    state: u32,
    /// The exact `pack_light_word` output — layer and the (uniform) light.
    light: u32,
    /// The exact `pack_color` output — the (uniform) vertex color.
    color: u32,
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
    color: u32,
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
            color,
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
                vertices, indices, face, base, w as i32, h as i32, key.light, key.color,
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
    mesh_column_with(
        world,
        MeshInputs::geometry(table, models, carried),
        cx,
        cz,
    )
}

/// [`mesh_column`] over full [`MeshInputs`] — the production entry point,
/// which also culls against non-occluding cubes (glass, leaves, ice, …),
/// applies `skipRendering`, and routes translucent materials into
/// [`ColumnMesh::tvertices`].
pub fn mesh_column_with(
    world: &World,
    inputs: MeshInputs<'_>,
    cx: i32,
    cz: i32,
) -> Option<ColumnMesh> {
    let table = Tables::new(&inputs);
    let ao = AoView { world, table };
    let (models, carried) = (inputs.models, inputs.fluid);
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
                    if let Some(f) = fluid_at(table.render, carried, state) {
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
                            let translucent = table.info(state).translucent;
                            for face in 0..6 {
                                let Some(cf) = cube_face(
                                    &ao,
                                    &mut tint_cache,
                                    wx,
                                    y,
                                    wz,
                                    state,
                                    face,
                                    faces,
                                    raw_faces,
                                    tint,
                                ) else {
                                    continue;
                                };
                                visible_cube_faces += 1;
                                // Translucent material: blended pass, unit
                                // quads (the per-column sort needs no merge).
                                if translucent {
                                    push_cube_face(
                                        &mut tvertices,
                                        &mut tindices,
                                        wx,
                                        y,
                                        wz,
                                        face,
                                        &cf,
                                    );
                                    unit_fallback_faces += 1;
                                    continue;
                                }
                                // A face whose four vertices agree has no
                                // gradient to lose, so it may merge. Anything
                                // else must stay a unit quad — and so must every
                                // up (+Y) face, whatever its lighting.
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
                                if face != 0 && cf.lit.uniform() {
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
                                                cf.lit.coords[0],
                                            ),
                                            color: pack_color(cf.lit.color[0]),
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
                                &ao,
                                models,
                                &mut tint_cache,
                                &mut vertices,
                                &mut indices,
                                &mut tvertices,
                                &mut tindices,
                                wx,
                                y,
                                wz,
                                state,
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
    table: Tables<'_>,
    carried: &[Option<CarriedFluid>],
    x: i32,
    y: i32,
    z: i32,
    want_lava: bool,
) -> Option<u8> {
    let f = fluid_at(table.render, carried, world.block_state_at(x, y, z))?;
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
    table: Tables<'_>,
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

    // `FluidRenderer.getLightCoords(level, pos)`: the brighter of the cell and
    // the one above, each through `LightCoordsUtil.getLightCoords` (so lava's
    // own emission lights it). The top and the sides sample the fluid's cell,
    // the bottom the cell below it.
    let lc = |x: i32, yy: i32, z: i32| table.light_coords(world, world.block_state_at(x, yy, z), x, yy, z);
    let fluid_light = |yy: i32| light_max(lc(wx, yy, wz), lc(wx, yy + 1, wz));
    // `ARGB.scaleRGB(tintColor, factor)`: `up()` on top, `down()` below, and
    // `up() * north()` / `up() * west()` on the Z / X sides.
    let cardinal = world.cardinal_light();
    let up = cardinal.by_mesh_face(0);
    let mut quad = |p: [([f32; 3], [f32; 2]); 4], factor: f32, coords: i32| {
        let color = scale_rgb(tint_rgb, factor);
        let base = vertices.len() as u32;
        for (pos, uv) in p {
            vertices.push(MeshVertex::new(pos, uv, fluid_layer as u32, coords, color));
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
            up,
            fluid_light(y),
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
            || table.occludes(world.block_state_at(nx, y, nz), face ^ 1)
        {
            continue;
        }
        // `faceDir.getAxis() == Z ? north() : west()`.
        let side = cardinal.by_mesh_face(if face < 4 { 2 } else { 4 });
        quad(corners, up * side, fluid_light(y));
    }
    // Bottom — against anything that isn't fluid, isn't covered by our own
    // shape, and isn't a full cube. Vanilla's `renderDown` has TWO neighbour
    // gates (`:78-79`): `shouldRenderFace` — whose self half is the new term —
    // and `!isFaceOccludedByNeighbor(DOWN, 0.8888889F, blockStateDown)`, the
    // hard-coded MAX_FLUID_HEIGHT. `is_full_cube` stands in for the second, as
    // on the sides.
    if !same(wx, y - 1, wz)
        && f.self_occludes & (1 << 1) == 0
        && !table.occludes(world.block_state_at(wx, y - 1, wz), 0)
    {
        quad(
            [
                ([x0, yf, z0], [0.0, 0.0]),
                ([x0, yf, z1], [0.0, 1.0]),
                ([x1, yf, z1], [1.0, 1.0]),
                ([x1, yf, z0], [1.0, 0.0]),
            ],
            cardinal.by_mesh_face(1),
            fluid_light(y - 1),
        );
    }
}

/// [`smooth_light::AoWorld`] over the chunk snapshot and the per-state tables:
/// each question reads the block state at the position, then the table.
struct AoView<'a> {
    world: &'a World,
    table: Tables<'a>,
}

impl AoView<'_> {
    fn state(&self, (x, y, z): (i32, i32, i32)) -> u32 {
        self.world.block_state_at(x, y, z)
    }
}

impl smooth_light::AoWorld for AoView<'_> {
    fn light_coords(&self, p: (i32, i32, i32)) -> i32 {
        self.table.light_coords(self.world, self.state(p), p.0, p.1, p.2)
    }

    fn shade_brightness(&self, p: (i32, i32, i32)) -> f32 {
        if self.table.info(self.state(p)).shade_dark {
            0.2
        } else {
            1.0
        }
    }

    fn is_view_blocking(&self, p: (i32, i32, i32)) -> bool {
        self.table.info(self.state(p)).view_blocking
    }

    fn light_dampening(&self, p: (i32, i32, i32)) -> i32 {
        self.table.dampening(self.state(p)) as i32
    }

    /// `isShapeFullBlock(getOcclusionShape())`: every face occludes.
    fn is_solid_render(&self, p: (i32, i32, i32)) -> bool {
        self.table.info(self.state(p)).occludes == 0b11_1111
    }

    fn is_collision_shape_full_block(&self, p: (i32, i32, i32)) -> bool {
        self.table.info(self.state(p)).ao_occluder
    }
}

/// Mesher face order (`[up, down, north, south, west, east]`) →
/// [`smooth_light::Face`].
const SMOOTH_FACE: [smooth_light::Face; 6] = [
    smooth_light::Face::Up,
    smooth_light::Face::Down,
    smooth_light::Face::North,
    smooth_light::Face::South,
    smooth_light::Face::West,
    smooth_light::Face::East,
];

/// One quad's per-vertex light coordinates and colors.
#[derive(Clone, Copy, PartialEq, Debug)]
struct LitQuad {
    coords: [i32; 4],
    color: [[u8; 3]; 4],
}

impl LitQuad {
    /// All four vertices identical — nothing for the rasterizer to
    /// interpolate, so the face may merge with its equals.
    fn uniform(&self) -> bool {
        (1..4).all(|i| self.coords[i] == self.coords[0] && self.color[i] == self.color[0])
    }
}

/// Light one quad of the block `state` at `(wx, y, wz)` the way
/// `ModelBlockRenderer.tesselateBlock` does, then tint it (`putQuadWithTint`).
///
/// `verts` are block-local, in any corner order. `dir` is the quad's
/// direction (mesher order), `cull` its cull face (vanilla's per-direction
/// quad list; `None` for the unculled list), `shade` its `materialInfo.shade`.
#[allow(clippy::too_many_arguments)]
fn light_quad(
    ao: &AoView<'_>,
    state: u32,
    (wx, y, wz): (i32, i32, i32),
    verts: &[[f32; 3]; 4],
    dir: usize,
    cull: Option<usize>,
    shade: bool,
    tint: [u8; 3],
) -> LitQuad {
    let world = ao.world;
    // `quad.materialInfo().shade() ? cardinalLighting.byFace(direction) :
    // cardinalLighting.up()` — both paths.
    let shade_factor = face_shade(world, if shade { dir } else { 0 });
    let pos = (wx, y, wz);
    if ao.table.uses_ao(state) {
        let face = SMOOTH_FACE[dir];
        let order = smooth_light::face_info_order(verts, face);
        let light = smooth_light::quad_ambient_occlusion(ao, pos, face, order.map(|i| verts[i]));
        let mut out = LitQuad {
            coords: [0; 4],
            color: [[0; 3]; 4],
        };
        for (k, &i) in order.iter().enumerate() {
            out.coords[i] = light.light_coords[k];
            out.color[i] = block_vertex_color(light.brightness[k], shade_factor, tint);
        }
        out
    } else {
        // `tesselateFlat`: a culled quad samples its cull face's neighbour; an
        // unculled one `prepareQuadFlat(..., -1)`, i.e. the neighbour only for a
        // `faceCubic` quad. Either way the *own* state decides emission.
        let (ox, oy, oz) = match cull {
            Some(c) => FACE_OFFSETS[c],
            None if smooth_light::face_cubic(ao, pos, SMOOTH_FACE[dir], verts) => FACE_OFFSETS[dir],
            None => (0, 0, 0),
        };
        let coords = ao.table.light_coords(world, state, wx + ox, y + oy, wz + oz);
        LitQuad {
            coords: [coords; 4],
            color: [block_vertex_color(shade_factor, 1.0, tint); 4],
        }
    }
}

/// Everything the cube path resolves for one visible face of one block.
///
/// Both mesher paths go through here, so the greedy path's merge key and the
/// fallback path's vertices are computed by the *same* code — they cannot drift.
struct CubeFace {
    layer: u16,
    /// Per `FACE_CORNERS[face]` vertex.
    lit: LitQuad,
}

/// Resolve one cube face, or `None` if a full-cube neighbour culls it.
#[allow(clippy::too_many_arguments)]
fn cube_face(
    ao: &AoView<'_>,
    cache: &mut TintCache,
    wx: i32,
    y: i32,
    wz: i32,
    state: u32,
    face: usize,
    faces: &[u16; 6],
    raw_faces: &[u16; 6],
    tint: &[TintSource; 6],
) -> Option<CubeFace> {
    let world = ao.world;
    let (dx, dy, dz) = FACE_OFFSETS[face];
    if !ao.table.should_render(state, world.block_state_at(wx + dx, y + dy, wz + dz), face) {
        return None;
    }
    // Dynamic biome tint (or the legacy pre-tinted layer + white). The same
    // (wx,y,wz)+resolver recurs across the 6 faces; the cache serves it once.
    let (layer, tint_rgb) = match biome_tint(world, cache, wx, y, wz, tint[face]) {
        Some(rgb) => (raw_faces[face], rgb),
        None => (faces[face], TINT_WHITE),
    };
    let verts = FACE_CORNERS[face].map(|(corner, _)| corner);
    // A cube face is a shaded quad culled against its own direction.
    let lit = light_quad(ao, state, (wx, y, wz), &verts, face, Some(face), true, tint_rgb);
    Some(CubeFace { layer, lit })
}

/// Whether a model quad is a whole block face: axis-aligned on the block
/// boundary of its direction and spanning the full face.
#[cfg(test)]
fn full_face(verts: &[[f32; 3]; 4], dir: usize) -> bool {
    let (uu, vv, (nx, ny, nz)) = FACE_AXES[dir];
    let n = [nx, ny, nz];
    let an = 3 - uu - vv;
    let plane = if n[an] > 0 { 1.0 } else { 0.0 };
    let (mut umin, mut umax, mut vmin, mut vmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for v in verts {
        if (v[an] - plane).abs() > 1e-5 {
            return false;
        }
        umin = umin.min(v[uu]);
        umax = umax.max(v[uu]);
        vmin = vmin.min(v[vv]);
        vmax = vmax.max(v[vv]);
    }
    umin.abs() < 1e-5 && vmin.abs() < 1e-5 && (umax - 1.0).abs() < 1e-5 && (vmax - 1.0).abs() < 1e-5
}

/// Emit one resolved cube face as a unit quad.
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
            f.lit.coords[i],
            f.lit.color[i],
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
#[cfg(any(test, feature = "oracle"))]
#[allow(clippy::too_many_arguments)]
fn emit_cube(
    ao: &AoView<'_>,
    cache: &mut TintCache,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    tvertices: &mut Vec<MeshVertex>,
    tindices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    state: u32,
    faces: &[u16; 6],
    raw_faces: &[u16; 6],
    tint: &[TintSource; 6],
) -> u32 {
    let (v, i) = if ao.table.info(state).translucent {
        (tvertices, tindices)
    } else {
        (vertices, indices)
    };
    let mut visible = 0;
    for face in 0..6 {
        let Some(f) = cube_face(ao, cache, wx, y, wz, state, face, faces, raw_faces, tint) else {
            continue;
        };
        push_cube_face(v, i, wx, y, wz, face, &f);
        visible += 1;
    }
    visible
}

#[allow(clippy::too_many_arguments)]
fn emit_model(
    ao: &AoView<'_>,
    models: &[Vec<rewo_data::assets::Quad>],
    cache: &mut TintCache,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    tvertices: &mut Vec<MeshVertex>,
    tindices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    state: u32,
    model_idx: u32,
) {
    let world = ao.world;
    let Some(quads) = models.get(model_idx as usize) else {
        return;
    };
    for quad in quads {
        let cull = (quad.cull >= 0).then_some(quad.cull as usize);
        if let Some(face) = cull {
            let (dx, dy, dz) = FACE_OFFSETS[face];
            if !ao.table.should_render(state, world.block_state_at(wx + dx, y + dy, wz + dz), face) {
                continue;
            }
        }
        // Dynamic biome tint (raw layer + tint color) or the legacy pre-tinted
        // layer. A model's quads at one block share a resolver+position, so the
        // cache serves them all from one computation.
        let (layer, tint_rgb) = match biome_tint(world, cache, wx, y, wz, quad.tint) {
            Some(rgb) => (quad.raw_layer, rgb),
            None => (quad.layer, TINT_WHITE),
        };
        let lit = light_quad(
            ao,
            state,
            (wx, y, wz),
            &quad.verts,
            quad.dir as usize,
            cull,
            quad.shade,
            tint_rgb,
        );
        let (vertices, indices) = if quad.translucent {
            (&mut *tvertices, &mut *tindices)
        } else {
            (&mut *vertices, &mut *indices)
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
                lit.coords[i],
                lit.color[i],
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

#[cfg(any(test, feature = "oracle"))]
mod oracle;
#[cfg(any(test, feature = "oracle"))]
pub use oracle::*;

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

    // -- M164: waterlogged blocks ------------------------------------------
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
        // table (every pre-M164 caller passes an empty one).
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
            translucent: false,
        }]];

        // Legacy: no biome context → white color, pre-tinted layer 7.
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(2, 64, 2, 1);
        let m = mesh_column(&w, &table, &models, &[], 0, 0).expect("meshed");
        let v = m.vertices[0];
        assert_eq!(v.layer_index(), 7, "legacy path uses the pre-tinted layer");
        assert_eq!(v.color_rgb(), TINT_WHITE, "legacy path carries a white color");
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
        // Open air, unshaded: gray(1.0) = 255, times the tint (vanilla
        // `ARGB.multiply`, 255 * 100 / 255 = 100) — the tint exactly.
        assert_eq!(v2.color_rgb(), [100, 0, 0], "the tint is carried exactly");
        let c2 = v2.reconstructed_color();
        assert!(
            (c2[0] - 100.0 / 255.0).abs() < 1e-6,
            "red channel = biome water color: {}",
            c2[0]
        );
        assert_eq!(c2[1], 0.0);
        assert_eq!(c2[2], 0.0);
    }

    // -- packed vertex ABI --------------------------------------------------

    #[test]
    fn packed_vertex_is_exactly_28_bytes_with_expected_offsets() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 28, "packed vertex size");
        assert_eq!(std::mem::align_of::<MeshVertex>(), 4, "packed vertex align");
        // Offsets the Vulkan attribute descriptions hard-code.
        let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, TINT_WHITE);
        let base = &v as *const _ as usize;
        assert_eq!(&v.pos as *const _ as usize - base, 0, "pos offset");
        assert_eq!(&v.uv as *const _ as usize - base, 12, "uv offset");
        assert_eq!(&v.light as *const _ as usize - base, 20, "light offset");
        assert_eq!(&v.color as *const _ as usize - base, 24, "color offset");
    }

    /// `block_vertex_color` against values derived by hand from vanilla's
    /// `ARGB` arithmetic (f32 products, `Mth.floor`, `(int)` truncation, and
    /// `a * b / 255` integer division).
    #[test]
    fn block_vertex_color_is_vanillas_integer_arithmetic() {
        // Open-air cube faces: gray(1.0) = floor(255.0) = 255, then
        // (int)(255 * shade): 1.0 -> 255; 0.5 -> 127 (127.5); f32 0.8 =
        // 0.800000011920929 -> 204.0000030 -> 204; f32 0.6 -> 153.0000061 -> 153.
        assert_eq!(block_vertex_color(1.0, 1.0, TINT_WHITE), [255; 3]);
        assert_eq!(block_vertex_color(1.0, 0.5, TINT_WHITE), [127; 3]);
        assert_eq!(block_vertex_color(1.0, 0.8, TINT_WHITE), [204; 3]);
        assert_eq!(block_vertex_color(1.0, 0.6, TINT_WHITE), [153; 3]);
        // Nether up/down: f32 0.9 = 0.899999976 -> 229.4999939 -> 229.
        assert_eq!(block_vertex_color(1.0, 0.9, TINT_WHITE), [229; 3]);
        // An AO corner of 0.8 on a 0.8 face: floor(0.8f * 255) = 204, then
        // (int)(204 * 0.8f) = (int)163.2000024 = 163; tinted (100, 0, 0):
        // 163 * 100 / 255 = 16300 / 255 = 63 (63.92 truncated).
        assert_eq!(block_vertex_color(0.8, 0.8, TINT_WHITE), [163; 3]);
        assert_eq!(block_vertex_color(0.8, 0.8, [100, 0, 0]), [63, 0, 0]);
        // The flat path is gray(shade) with no scale: gray(0.6) = 153.
        assert_eq!(block_vertex_color(0.6, 1.0, TINT_WHITE), [153; 3]);
        // The darkest AO corner: (0.2 + 0.2 + 0.2 + 0.2) * 0.25 = 0.2 in f32
        // (0.200000003); floor(0.2f * 255) = floor(51.0000008) = 51.
        assert_eq!(block_vertex_color(0.2, 1.0, TINT_WHITE), [51; 3]);
        // White is the identity of the tint step for every gray.
        for g in 0..=255u8 {
            let b = g as f32 / 255.0;
            let white = block_vertex_color(b, 1.0, TINT_WHITE);
            assert_eq!(white, [as_8bit_channel(b) as u8; 3], "gray {g}");
        }
    }

    /// The fluid color is `ARGB.scaleRGB(tint, factor)`, never `gray`.
    #[test]
    fn fluid_color_is_scale_rgb_of_the_tint() {
        // Water tint 0x3F76E4 on a north side in the Overworld:
        // factor up() * north() = 1.0 * 0.8f; (int)(63 * 0.8f) = 50,
        // (int)(118 * 0.8f) = 94, (int)(228 * 0.8f) = 182.
        assert_eq!(scale_rgb([63, 118, 228], 1.0 * 0.8), [50, 94, 182]);
        // Lava is untinted (-1): 255 * down() = 127.
        assert_eq!(scale_rgb(TINT_WHITE, 0.5), [127; 3]);
    }

    #[test]
    fn color_bytes_roundtrip_with_opaque_alpha() {
        for c in 0u16..=255 {
            let rgb = [c as u8, (255 - c) as u8, (c as u8).wrapping_mul(7)];
            let v = MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, rgb);
            assert_eq!(v.color_rgb(), rgb, "color byte roundtrip");
            assert_eq!(v.color >> 24, 0xFF, "alpha byte is opaque");
            assert_eq!(v.reconstructed_color(), rgb.map(|b| b as f32 / 255.0));
        }
        assert_eq!(MeshVertex::new([0.0; 3], [0.0; 2], 0, 0, TINT_WHITE).reconstructed_color(), [1.0; 3]);
    }

    /// The light word carries the layer and both smooth channels losslessly,
    /// fractions included; the whole levels are the smooth values' high nibble.
    #[test]
    fn light_word_carries_layer_and_smooth_light() {
        for layer in [0u32, 1, 7, 499, 500, 4095, 0xFFFF] {
            for block in 0u8..16 {
                for sky in [0u8, 1, 7, 15] {
                    let coords = light_coords(block, sky);
                    let v = MeshVertex::new([0.0; 3], [0.0; 2], layer, coords, TINT_WHITE);
                    assert_eq!(v.layer_index(), layer as u16);
                    assert_eq!((v.block_light(), v.sky_light()), (block, sky));
                    assert_eq!((v.block_smooth(), v.sky_smooth()), (block * 16, sky * 16));
                }
            }
        }
        // A smoothBlend result keeps its fraction: 196 = 12.25 levels of sky,
        // 56 = 3.5 levels of block (`smoothPack(56, 196)`).
        let v = MeshVertex::new([0.0; 3], [0.0; 2], 3, 56 | 196 << 16, TINT_WHITE);
        assert_eq!((v.block_smooth(), v.sky_smooth()), (56, 196));
        assert_eq!((v.block_light(), v.sky_light()), (3, 12));
        assert_eq!(v.layer_index(), 3);
        assert_eq!(light_coords(15, 15), FULL_BRIGHT);
    }

    /// UV transport is **exact for every family the mesher emits** — there is no
    /// quantization step at all. This is the regression guard for the rejected
    /// f16 variant: the fluid family below (`1 - k/9`) is precisely what f16
    /// could not represent, and it flipped 6 demo pixels at a texel boundary.
    #[test]
    fn uv_is_stored_exactly_for_every_emitted_family() {
        let store = |x: f32| MeshVertex::new([0.0; 3], [x, x], 0, 0, TINT_WHITE).uv_f32()[0];

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
            let v = MeshVertex::new([0.0; 3], uv, 0, 0, TINT_WHITE);
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
            let coords = light_coords(5, 9);
            let color = [3, 200, 71];
            let cf = CubeFace {
                layer: 12,
                lit: LitQuad {
                    coords: [coords; 4],
                    color: [color; 4],
                },
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
                pack_light_word(12, coords),
                pack_color(color),
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
            !disc.f.uniform(),
            "this face's corners must actually differ: light {:x?}, color {:x?}",
            disc.f.light,
            disc.f.color
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
            translucent: false,
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
        // Smooth lighting blends each light boundary: the unlit/bright half
        // keeps x 8..=10 whole (3 wide, all 4 rows), while the 4-wide lit/dim
        // half borders unlit cells on every edge (x = 11 and 16, z = 10 and
        // 15), so only its 2x2 interior (x 13..=14, z 12..=13) is uniform.
        assert_eq!(rep.block_light_boundary_rects, [(3, 4), (2, 2)]);
        assert_eq!(rep.sky_light_boundary_rects, [(3, 4), (2, 2)]);
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

    /// `face_shade` is `CardinalLighting.byFace` of the world's dimension.
    #[test]
    fn face_shade_is_the_dimension_cardinal_table() {
        use rewo_world::dimension::CardinalLighting;
        let d = World::new(DimensionShape::OVERWORLD);
        let n = nether_world();
        for face in 0..6 {
            assert_eq!(face_shade(&d, face), CardinalLighting::DEFAULT.by_mesh_face(face));
            assert_eq!(face_shade(&n, face), CardinalLighting::NETHER.by_mesh_face(face));
        }
        assert_eq!(face_shade(&n, 0), 0.9, "nether up");
        assert_eq!(face_shade(&n, 1), 0.9, "nether down");
    }

    /// A Nether plate in open air: every corner's AO brightness is 1.0 (all
    /// samples are air, shade 1.0), so each face is `(int)(255 * byFace)` —
    /// 229 top and bottom (f32 0.9 * 255 = 229.49..), 204 north/south, 153
    /// west/east — in both mesher paths. The bottom plane still merges.
    #[test]
    fn nether_plate_faces_take_the_nether_cardinal_shade() {
        let mut w = nether_world();
        plate(&mut w, 4, 5, 10, 4, 5, 1); // a 2×2 plate of STONE
        let table = cube_table();
        let want = [229u8, 229, 204, 204, 153, 153];

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
            let mut seen = [0usize; 6];
            for q in &qs {
                let face = face_of_quad(q);
                seen[face] += 1;
                for v in q {
                    assert_eq!(v.color_rgb(), [want[face]; 3], "{what}: face {face}");
                }
            }
            for face in 0..6 {
                assert!(seen[face] > 0, "{what}: face {face} emitted no quad");
            }
        }

        // The optimized path still merges the bottom plane into one 2×2
        // rectangle; up faces never merge (M15 carve-out).
        let o = mesh_column(&w, &table, &[], &[], 0, 0).expect("optimized");
        let span = |q: &[MeshVertex; 4], axis: usize| {
            let lo = q.iter().map(|v| v.pos[axis]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|v| v.pos[axis]).fold(f32::MIN, f32::max);
            hi - lo
        };
        let of_face = |f: usize| -> Vec<_> {
            quads(&o.vertices, &o.indices).into_iter().filter(|q| face_of_quad(q) == f).collect()
        };
        let downs = of_face(1);
        assert_eq!(downs.len(), 1, "the four bottoms merge into one rectangle");
        assert_eq!((span(&downs[0], 0), span(&downs[0], 2)), (2.0, 2.0));
        let ups = of_face(0);
        assert_eq!(ups.len(), 4, "one unit quad per top");
        for q in &ups {
            assert_eq!((span(q, 0), span(q, 2)), (1.0, 1.0));
        }
    }

    /// Model quads and fluid faces follow the dimension too. A shaded model
    /// quad takes `byFace(direction)`; an **unshaded** one takes `up()` — 1.0
    /// by default but 0.9 in the Nether (`prepareQuadAmbientOcclusion` /
    /// `prepareQuadFlat`: `shade() ? byFace(direction) : up()`). Fluids scale
    /// their tint by `up()` on top, `down()` below and `up() * north()` /
    /// `up() * west()` on the sides.
    #[test]
    fn nether_model_and_fluid_faces_follow_the_dimension() {
        use rewo_data::assets::Quad;

        let quad = |verts: [[f32; 3]; 4], dir: u8, shade: bool| Quad {
            verts,
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 7,
            raw_layer: 7,
            cull: -1,
            dir,
            tint: TintSource::None,
            shade,
            translucent: false,
        };
        // Shaded full top (y = 1), shaded full north face (z = 0), unshaded
        // mid-height horizontal quad. All sample open air: brightness 1.0.
        let models = vec![vec![
            quad([[0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]], 0, true),
            quad([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], 2, true),
            quad([[0.0, 0.5, 0.0], [1.0, 0.5, 0.0], [1.0, 0.5, 1.0], [0.0, 0.5, 1.0]], 0, false),
        ]];
        let table = oracle_model_table();
        let overworld = || {
            let mut w = World::new(DimensionShape::OVERWORLD);
            w.ensure_column(0, 0);
            w
        };

        // (int)(255 * f): 1.0 -> 255, 0.8 -> 204, 0.9 -> 229.
        for (what, mut w, want) in [("default", overworld(), [255u8, 204, 255]), ("nether", nether_world(), [229, 204, 229])] {
            w.set_block(4, 10, 4, 1);
            let m = mesh_column(&w, &table, &models, &[], 0, 0).expect("model meshed");
            let got: Vec<u8> = quads(&m.vertices, &m.indices)
                .iter()
                .map(|q| {
                    assert!(q.iter().all(|v| v.color_rgb() == q[0].color_rgb()), "{what}: uniform quad");
                    q[0].color_rgb()[0]
                })
                .collect();
            assert_eq!(got, want, "{what}: model quad grays");
        }

        // A lone water source (no biome: white tint). Default: top 255, bottom
        // (int)(255 * 0.5) = 127, N/S (int)(255 * 0.8) = 204, W/E 153. Nether:
        // top and bottom 229; N/S (int)(255 * f32(0.9 * 0.8)) = (int)183.6 = 183;
        // W/E (int)(255 * f32(0.9 * 0.6)) = (int)137.7 = 137.
        let ftable = fluid_table();
        for (what, mut w, want) in [
            ("default", overworld(), [255u8, 127, 204, 204, 153, 153]),
            ("nether", nether_world(), [229, 229, 183, 183, 137, 137]),
        ] {
            w.set_block(4, 10, 4, 2);
            let m = mesh_column(&w, &ftable, &[], &[], 0, 0).expect("fluid meshed");
            let qs = quads(&m.tvertices, &m.tindices);
            assert_eq!(qs.len(), 6, "{what}: a lone source emits six faces");
            for q in &qs {
                let face = fluid_face_of_quad(q, [4, 10, 4]);
                for v in q {
                    assert_eq!(v.color_rgb(), [want[face]; 3], "{what}: fluid face {face}");
                }
            }
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

#[cfg(test)]
mod occlusion_tests {
    use super::*;
    use rewo_world::dimension::DimensionShape;

    const AIR: u32 = 0;
    const STONE: u32 = 1;
    const GLASS: u32 = 2;
    const STAINED: u32 = 3;
    const WATER: u32 = 4;
    const PANE_EW: u32 = 5;
    const PANE_N: u32 = 6;
    const BOTTOM_SLAB: u32 = 7;

    fn cube(layer: u16) -> RenderKind {
        RenderKind::Cube {
            faces: [layer; 6],
            raw_faces: [layer; 6],
            tint: [TintSource::None; 6],
        }
    }

    fn render() -> Vec<RenderKind> {
        vec![
            RenderKind::Invisible,
            cube(1),
            cube(2),
            cube(3),
            RenderKind::Fluid {
                layer: 4,
                raw_layer: 4,
                level: 0,
                lava: false,
            },
            RenderKind::Invisible,
            RenderKind::Invisible,
            RenderKind::Invisible,
        ]
    }

    /// Shaped like the bake: stone occludes every face; glass and stained
    /// glass occlude none and skip a same-block neighbour; a bottom slab
    /// covers its own bottom face.
    fn cull() -> Vec<CullInfo> {
        let mut c = vec![CullInfo::default(); 8];
        for (i, info) in c.iter_mut().enumerate() {
            info.block = i as u16;
        }
        c[STONE as usize].occludes = 0b11_1111;
        for s in [STONE, GLASS, STAINED] {
            c[s as usize].ao_occluder = true;
        }
        // Stone is suffocating and dark to AO; glass (`TransparentBlock`,
        // `isViewBlocking(never)`) is neither, though its collision is full.
        c[STONE as usize].view_blocking = true;
        c[STONE as usize].shade_dark = true;
        for info in c.iter_mut() {
            info.ambient_occlusion = true;
        }
        c[GLASS as usize].skip = 1;
        c[STAINED as usize].skip = 1;
        c[STAINED as usize].translucent = true;
        c[PANE_EW as usize] = CullInfo {
            skip: 2,
            block: 5,
            connect: (1 << 4) | (1 << 5),
            ..CullInfo::default()
        };
        // Same block as PANE_EW, connected north only.
        c[PANE_N as usize] = CullInfo {
            skip: 2,
            block: 5,
            connect: 1 << 2,
            ..CullInfo::default()
        };
        c[BOTTOM_SLAB as usize].occludes = 1 << 1;
        c
    }

    fn world(blocks: &[((i32, i32, i32), u32)]) -> World {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        for &((x, y, z), s) in blocks {
            w.set_block(x, y, z, s);
        }
        w
    }

    fn mesh(w: &World, cull: &[CullInfo]) -> ColumnMesh {
        let r = render();
        let inputs = MeshInputs { cull, ..MeshInputs::geometry(&r, &[], &[]) };
        mesh_column_with(w, inputs, 0, 0).expect("meshed")
    }

    fn quads(v: &[MeshVertex]) -> usize {
        v.len() / 4
    }

    /// Glass does not occlude: the stone face against it is drawn (before, it
    /// was culled — a see-through hole), and the glass face against the stone
    /// is culled, because stone does.
    #[test]
    fn a_face_against_glass_is_drawn_and_glass_against_stone_is_not() {
        let w = world(&[((4, 10, 4), STONE), ((5, 10, 4), GLASS)]);
        let m = mesh(&w, &cull());
        assert_eq!(quads(&m.vertices), 6 + 5);
        assert!(m.tvertices.is_empty());
        // Legacy (no cull table): glass counts as an opaque cube, 5 + 5.
        let legacy = mesh(&w, &[]);
        assert_eq!(quads(&legacy.vertices), 10);
    }

    /// `HalfTransparentBlock.skipRendering`: glass against glass draws no
    /// shared face, from either side.
    #[test]
    fn same_block_glass_skips_the_shared_faces() {
        let w = world(&[((4, 10, 4), GLASS), ((5, 10, 4), GLASS)]);
        // (Counted as unit faces: coplanar faces of one state merge.)
        assert_eq!(mesh(&w, &cull()).visible_cube_faces, 10);
        // A different half-transparent block does not skip.
        let w = world(&[((4, 10, 4), GLASS), ((5, 10, 4), STAINED)]);
        let m = mesh(&w, &cull());
        assert_eq!(quads(&m.vertices), 6);
        assert_eq!(quads(&m.tvertices), 6);
    }

    /// A translucent cube goes to the blended set, whole.
    #[test]
    fn translucent_cubes_route_to_the_translucent_set() {
        let w = world(&[((4, 10, 4), STAINED)]);
        let m = mesh(&w, &cull());
        assert!(m.vertices.is_empty());
        assert_eq!(quads(&m.tvertices), 6);
    }

    /// `FluidRenderer.isFaceOccludedByNeighbor`: water's side is hidden by a
    /// neighbour whose face occlusion shape is the full block — stone — and
    /// drawn against glass, which has none.
    #[test]
    fn water_sides_are_drawn_against_glass_but_not_stone() {
        let side_quads = |n: u32| {
            let w = world(&[((4, 10, 4), WATER), ((5, 10, 4), n)]);
            let m = mesh(&w, &cull());
            // East-facing water quads sit on the x = 5 plane.
            m.tvertices
                .chunks_exact(4)
                .filter(|q| q.iter().all(|v| v.pos[0] == 5.0))
                .count()
        };
        assert_eq!(side_quads(STONE), 0);
        assert_eq!(side_quads(GLASS), 1);
    }

    /// A partial shape culls exactly the faces it covers: a cube's top against
    /// a bottom slab's bottom is hidden, its sides are not.
    #[test]
    fn a_slab_bottom_culls_the_face_below_it() {
        let w = world(&[((4, 10, 4), STONE), ((4, 11, 4), BOTTOM_SLAB)]);
        assert_eq!(quads(&mesh(&w, &cull()).vertices), 5);
    }

    /// `IronBarsBlock.skipRendering`: panes stacked vertically always skip;
    /// side by side only when both are connected toward each other.
    #[test]
    fn pane_skip_follows_the_connection_rule() {
        let r = render();
        let c = cull();
        let t = Tables::new(&MeshInputs { cull: &c, ..MeshInputs::geometry(&r, &[], &[]) });
        // Vertical: skip regardless of connections.
        assert!(!t.should_render(PANE_EW, PANE_N, 0));
        assert!(!t.should_render(PANE_EW, PANE_N, 1));
        // East face: EW is connected east, and the neighbour connected west.
        assert!(!t.should_render(PANE_EW, PANE_EW, 5));
        // PANE_N is not connected west, so EW's east face toward it draws.
        assert!(t.should_render(PANE_EW, PANE_N, 5));
        // A non-pane neighbour never skips.
        assert!(t.should_render(PANE_EW, GLASS, 5));
        // Air is not a neighbour to skip against.
        assert!(t.should_render(PANE_EW, AIR, 5));
    }
    /// Smooth lighting reaches model quads: a full top face darkens the two
    /// corners against a suffocating neighbour, as a cube face would. Its
    /// west corners are `(shade3 + shade0 + corner + center) * 0.25` with the
    /// stone's 0.2 in one or two samples; the east corners see only air.
    #[test]
    fn model_quads_get_smooth_lighting() {
        use rewo_data::assets::Quad;
        let top = |y: f32, x1: f32| Quad {
            verts: [[0.0, y, 0.0], [x1, y, 0.0], [x1, y, 1.0], [0.0, y, 1.0]],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 9,
            raw_layer: 9,
            cull: 0,
            dir: 0,
            tint: TintSource::None,
            shade: true,
            translucent: false,
        };
        let mut r = render();
        r.push(RenderKind::Model(0));
        let model = r.len() as u32 - 1;
        let mut c = cull();
        c.push(CullInfo { block: 99, ambient_occlusion: true, ..CullInfo::default() });
        let models = vec![vec![top(1.0, 1.0)]];
        // Stone one up and one west of the model: the top face's west corners
        // sit against it.
        let w = world(&[((4, 10, 4), model), ((3, 11, 4), STONE)]);
        let m = mesh_column_with(&w, MeshInputs { cull: &c, ..MeshInputs::geometry(&r, &models, &[]) }, 0, 0)
            .expect("meshed");
        let corner = |x: f32| -> Vec<[u8; 3]> {
            m.vertices
                .iter()
                .filter(|v| v.pos[1] == 11.0 && v.pos[0] == x && v.layer_index() == 9)
                .map(|v| v.color_rgb())
                .collect()
        };
        // West corners: WEST sample 0.2, the rest air (1.0):
        // (1.0 + 0.2 + 1.0 + 1.0) * 0.25 = 0.8 -> gray 204 -> up() 1.0 -> 204.
        assert_eq!(corner(4.0), vec![[204; 3]; 2], "west corners");
        assert_eq!(corner(5.0), vec![[255; 3]; 2], "east corners");
        // Flat when smooth lighting is off: the whole quad is gray(up()) = 255.
        let flat = mesh_column_with(
            &w,
            MeshInputs { cull: &c, smooth_lighting: false, ..MeshInputs::geometry(&r, &models, &[]) },
            0,
            0,
        )
        .expect("meshed");
        assert!(flat.vertices.iter().filter(|v| v.layer_index() == 9).all(|v| v.color_rgb() == [255; 3]));
    }

    /// The greedy path and the frozen reference agree under a real cull
    /// table too: same opaque unit faces, byte-identical translucent stream.
    #[test]
    fn greedy_matches_the_reference_with_culling() {
        let w = world(&[
            ((4, 10, 4), STONE),
            ((5, 10, 4), GLASS),
            ((6, 10, 4), GLASS),
            ((4, 11, 4), STAINED),
            ((5, 11, 4), STONE),
            ((4, 9, 4), BOTTOM_SLAB),
        ]);
        let r = render();
        let c = cull();
        let inputs = MeshInputs { cull: &c, ..MeshInputs::geometry(&r, &[], &[]) };
        let o = mesh_column_with(&w, inputs, 0, 0).expect("optimized");
        let f = mesh_column_reference_with(&w, inputs, 0, 0).expect("reference");
        let units = |m: &ColumnMesh| {
            let mut v: Vec<UnitFace> = expand_unit_faces(&m.vertices, &m.indices)
                .expect("decodes")
                .into_iter()
                .map(|e| e.f)
                .collect();
            v.sort();
            v
        };
        assert_eq!(units(&o), units(&f));
        assert_eq!(o.tvertices, f.tvertices);
        assert_eq!(o.visible_cube_faces, f.visible_cube_faces);
    }
}
