//! Vanilla "Smooth Lighting" — a pure transcription of Minecraft 26.2's
//! per-vertex light blend + ambient occlusion. **Not wired into the mesher
//! yet** (that is a separate step); this module only implements and tests the
//! algorithm.
//!
//! Sources (the local decompile under `%APPDATA%/EwoClient/rewo/26.2/`):
//!
//! * `net/minecraft/client/renderer/block/BlockModelLighter.java` —
//!   `prepareQuadAmbientOcclusion`, `prepareQuadShape`, and the tables
//!   `AdjacencyInfo`, `AmbientVertexRemap`, `SizeInfo`.
//! * `net/minecraft/util/LightCoordsUtil.java` — `smoothBlend`,
//!   `smoothWeightedBlend`, `smoothPack` / `smoothBlock` / `smoothSky`.
//!
//! Operation order, integer vs float ops and the two table sets are kept as in
//! Java; the only changes are mechanical (snake_case, `&impl AoWorld` instead of
//! `BlockAndTintGetter`, `QuadLight` instead of `QuadInstance`).
//!
//! # Light packing
//!
//! One light coordinate is a single `i32` holding two channels in `LightCoordsUtil`'s
//! "smooth" units — the light level × 16, so 0..=240: the block channel in bits
//! 0..7 and the sky channel in bits 16..23. `LightCoordsUtil.pack(block, sky)`
//! builds one from integer light levels (`block << 4 | sky << 20`), so `pack(15, 15)`
//! = 240 | 15728640 = 15728880 = vanilla's `FULL_BRIGHT`, and `pack(0, 15)` =
//! 15728640 = `FULL_SKY`. `smoothPack` / `smoothBlock` / `smoothSky` address the
//! same two bytes directly, which is why blending can keep the sub-level fraction
//! (bits 0..3 and 16..19) that `pack` leaves at zero.
//!
//! # Vertex order
//!
//! `vertices` are the quad's corners in block-local coordinates (each component
//! 0..=1) in `BakedQuad` vertex order — the order `FaceBakery` fills
//! `position0..3` from `net/minecraft/client/renderer/FaceInfo` (min/max are the
//! quad's own per-axis extents):
//!
//! ```text
//! Down  [0] (min_x, min_y, max_z)  [1] (min_x, min_y, min_z)  [2] (max_x, min_y, min_z)  [3] (max_x, min_y, max_z)
//! Up    [0] (min_x, max_y, min_z)  [1] (min_x, max_y, max_z)  [2] (max_x, max_y, max_z)  [3] (max_x, max_y, min_z)
//! North [0] (max_x, max_y, min_z)  [1] (max_x, min_y, min_z)  [2] (min_x, min_y, min_z)  [3] (min_x, max_y, min_z)
//! South [0] (min_x, max_y, max_z)  [1] (min_x, min_y, max_z)  [2] (max_x, min_y, max_z)  [3] (max_x, max_y, max_z)
//! West  [0] (min_x, max_y, min_z)  [1] (min_x, min_y, min_z)  [2] (min_x, min_y, max_z)  [3] (min_x, max_y, max_z)
//! East  [0] (max_x, max_y, max_z)  [1] (max_x, min_y, max_z)  [2] (max_x, min_y, min_z)  [3] (max_x, max_y, min_z)
//! ```
//!
//! The four computed samples are scattered onto those slots by
//! `AmbientVertexRemap`, so `QuadLight[i]` belongs to `vertices[i]`.
//!
//! # What is deliberately not here
//!
//! * The `BlockModelLighter.Cache` (a per-thread LRU of the two lookups) — the
//!   `AoWorld` implementation is free to cache.
//! * `QuadInstance.scaleColor(...)`: vanilla multiplies the AO brightness by a
//!   flat per-face cardinal shade (`CardinalLighting.byFace`, 0.5/1.0/0.8/0.6)
//!   after this algorithm. It is constant across the four vertices and is the
//!   mesher's existing face-shade step, so `QuadLight::brightness` stops at the
//!   value vanilla hands to `ARGB.gray`.
//! * `prepareQuadFlat` (flat lighting, no AO) — only `prepareQuadShape(..., ambientOcclusion = true)`
//!   is ported.
//! * `AdjacencyInfo`'s `shadeWeight` constructor argument (0.5/1.0/0.8/0.8/0.6/0.6):
//!   the Java field is never read.

/// A block face. `Face::index()` is `Direction.get3DDataValue()`, the order every
/// per-face table in vanilla is keyed by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Face {
    /// `Direction.get3DDataValue()` — index into the per-face tables below.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// `Direction.getStepX() / getStepY() / getStepZ()` — the offset of the
    /// neighbouring block.
    pub const fn step(self) -> (i32, i32, i32) {
        match self {
            Face::Down => (0, -1, 0),
            Face::Up => (0, 1, 0),
            Face::North => (0, 0, -1),
            Face::South => (0, 0, 1),
            Face::West => (-1, 0, 0),
            Face::East => (1, 0, 0),
        }
    }
}

/// Per-vertex output of [`quad_ambient_occlusion`] — what vanilla writes into
/// `QuadInstance` via `setLightCoords(remap.vertN, ...)` and
/// `setColor(remap.vertN, ARGB.gray(...))`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadLight {
    /// `LightCoordsUtil` packed light per vertex (see the module docs for the
    /// packing). Vanilla's `setLightCoords`.
    pub light_coords: [i32; 4],
    /// Ambient-occlusion brightness per vertex, 0.0..=1.0. This is what vanilla
    /// passes to `ARGB.gray` (before the flat cardinal shade).
    pub brightness: [f32; 4],
}

/// The per-position block facts vanilla reads, one method per Java method it
/// stands for. Positions are world `(x, y, z)`; every question is about the
/// block *at* that position (`level.getBlockState(pos)` first, then the state
/// answers), including the quad's own block at `pos`.
pub trait AoWorld {
    /// `LightCoordsUtil.getLightCoords(BrightnessGetter.DEFAULT, level, state, pos)`.
    ///
    /// That is `pack(level.getBrightness(LightLayer.BLOCK, pos), level.getBrightness(LightLayer.SKY, pos))`
    /// unless `state.emissiveRendering()`, which short-circuits to `FULL_BRIGHT`
    /// = 15728880, and with `state.getLightEmission()` raising the block channel
    /// through `withBlock` when it is larger. Packed as described in the module
    /// docs.
    fn light_coords(&self, pos: (i32, i32, i32)) -> i32;

    /// `BlockState.getShadeBrightness(level, pos)`. The default
    /// (`BlockBehaviour.getShadeBrightness`) is 0.2 for
    /// `isCollisionShapeFullBlock` and 1.0 otherwise.
    fn shade_brightness(&self, pos: (i32, i32, i32)) -> f32;

    /// `BlockState.isViewBlocking(level, pos)`. Default predicate
    /// `isSuffocating` = `blocksMotion && isCollisionShapeFullBlock`.
    fn is_view_blocking(&self, pos: (i32, i32, i32)) -> bool;

    /// `BlockState.getLightDampening()`: 15 when `isSolidRender()`, otherwise
    /// `propagatesSkylightDown() ? 0 : 1`.
    fn light_dampening(&self, pos: (i32, i32, i32)) -> i32;

    /// `BlockState.isSolidRender()` — `Block.isShapeFullBlock(getOcclusionShape())`.
    fn is_solid_render(&self, pos: (i32, i32, i32)) -> bool;

    /// `BlockState.isCollisionShapeFullBlock(level, pos)` —
    /// `Block.isShapeFullBlock(getCollisionShape(level, pos))`.
    fn is_collision_shape_full_block(&self, pos: (i32, i32, i32)) -> bool;
}

/// `LightCoordsUtil.block(packed)` — integer block level 0..=15.
#[inline]
fn block(packed: i32) -> i32 {
    packed >> 4 & 15
}

/// `LightCoordsUtil.sky(packed)` — integer sky level 0..=15.
#[inline]
fn sky(packed: i32) -> i32 {
    packed >> 20 & 15
}

/// `LightCoordsUtil.smoothPack(block, sky)` — the two smooth channels.
#[inline]
fn smooth_pack(block: i32, sky: i32) -> i32 {
    block & 0xFF | (sky & 0xFF) << 16
}

/// `LightCoordsUtil.smoothBlock(packed)` — block channel in smooth units.
#[inline]
fn smooth_block(packed: i32) -> i32 {
    packed & 0xFF
}

/// `LightCoordsUtil.smoothSky(packed)` — sky channel in smooth units.
#[inline]
fn smooth_sky(packed: i32) -> i32 {
    packed >> 16 & 0xFF
}

/// `LightCoordsUtil.smoothBlend(neighbor1, neighbor2, neighbor3, center)`:
/// the truncated average of the four samples, with vanilla's substitution of
/// "no information" samples. `|=` on the sky channel and the shifts are
/// Java's, and so is the 16711935 (`0x00FF00FF`) mask that drops the carries.
#[inline]
fn smooth_blend(neighbor1: i32, neighbor2: i32, neighbor3: i32, center: i32) -> i32 {
    let mut n1 = neighbor1;
    let mut n2 = neighbor2;
    let mut n3 = neighbor3;
    if sky(center) > 2 || block(center) > 2 {
        if n1 == 0 {
            n1 = center;
        } else if sky(n1) == 0 {
            n1 |= center & 0xFF0000;
        }

        if n2 == 0 {
            n2 = center;
        } else if sky(n2) == 0 {
            n2 |= center & 0xFF0000;
        }

        if n3 == 0 {
            n3 = center;
        } else if sky(n3) == 0 {
            n3 |= center & 0xFF0000;
        }
    }

    // Java `int` addition wraps; four packed values can reach 62915520 so the
    // sum cannot overflow, but wrapping keeps the transcription literal.
    let sum = n1.wrapping_add(n2).wrapping_add(n3).wrapping_add(center);
    (sum >> 2) & 0x00FF_00FF
}

/// `LightCoordsUtil.smoothWeightedBlend(coords1..4, weight1..4)` — the four
/// blended samples re-weighted by one vertex's shape weights. Java's `(int)`
/// cast truncates toward zero, as does `as i32`.
#[inline]
fn smooth_weighted_blend(coords: [i32; 4], weights: [f32; 4]) -> i32 {
    let sky = (smooth_sky(coords[0]) as f32 * weights[0]
        + smooth_sky(coords[1]) as f32 * weights[1]
        + smooth_sky(coords[2]) as f32 * weights[2]
        + smooth_sky(coords[3]) as f32 * weights[3]) as i32;
    let block = (smooth_block(coords[0]) as f32 * weights[0]
        + smooth_block(coords[1]) as f32 * weights[1]
        + smooth_block(coords[2]) as f32 * weights[2]
        + smooth_block(coords[3]) as f32 * weights[3]) as i32;
    smooth_pack(block, sky)
}

/// Vanilla `BlockModelLighter.SizeInfo`, in declaration order — `index()` is the
/// variant's hardcoded `index`, i.e. the slot in [`QuadShape::face_shape`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizeInfo {
    Down,
    Up,
    North,
    South,
    West,
    East,
    FlipDown,
    FlipUp,
    FlipNorth,
    FlipSouth,
    FlipWest,
    FlipEast,
}

impl SizeInfo {
    const COUNT: usize = 12;

    /// `SizeInfo.index`.
    const fn index(self) -> usize {
        match self {
            SizeInfo::Down => 0,
            SizeInfo::Up => 1,
            SizeInfo::North => 2,
            SizeInfo::South => 3,
            SizeInfo::West => 4,
            SizeInfo::East => 5,
            SizeInfo::FlipDown => 6,
            SizeInfo::FlipUp => 7,
            SizeInfo::FlipNorth => 8,
            SizeInfo::FlipSouth => 9,
            SizeInfo::FlipWest => 10,
            SizeInfo::FlipEast => 11,
        }
    }
}

/// Vanilla `BlockModelLighter.AdjacencyInfo` for one face. `vert_weights[k]` is
/// the Java `vert{k}Weights`: eight `SizeInfo`s, used as four products
/// (`vertNweight01`, `_23`, `_45`, `_67`).
struct AdjacencyInfo {
    /// `AdjacencyInfo.corners` — the four side-sample directions, in Java order.
    corners: [Face; 4],
    /// `AdjacencyInfo.doNonCubicWeight`.
    do_non_cubic_weight: bool,
    vert_weights: [[SizeInfo; 8]; 4],
}

/// `AdjacencyInfo.BY_FACING` — indexed by [`Face::index`].
const ADJACENCY: [AdjacencyInfo; 6] = [
    // DOWN (Direction.DOWN), shadeWeight 0.5F
    AdjacencyInfo {
        corners: [Face::West, Face::East, Face::North, Face::South],
        do_non_cubic_weight: true,
        vert_weights: [
            // vert0Weights
            [
                SizeInfo::FlipWest, SizeInfo::South, SizeInfo::FlipWest, SizeInfo::FlipSouth,
                SizeInfo::West, SizeInfo::FlipSouth, SizeInfo::West, SizeInfo::South,
            ],
            // vert1Weights
            [
                SizeInfo::FlipWest, SizeInfo::North, SizeInfo::FlipWest, SizeInfo::FlipNorth,
                SizeInfo::West, SizeInfo::FlipNorth, SizeInfo::West, SizeInfo::North,
            ],
            // vert2Weights
            [
                SizeInfo::FlipEast, SizeInfo::North, SizeInfo::FlipEast, SizeInfo::FlipNorth,
                SizeInfo::East, SizeInfo::FlipNorth, SizeInfo::East, SizeInfo::North,
            ],
            // vert3Weights
            [
                SizeInfo::FlipEast, SizeInfo::South, SizeInfo::FlipEast, SizeInfo::FlipSouth,
                SizeInfo::East, SizeInfo::FlipSouth, SizeInfo::East, SizeInfo::South,
            ],
        ],
    },
    // UP (Direction.UP), shadeWeight 1.0F
    AdjacencyInfo {
        corners: [Face::East, Face::West, Face::North, Face::South],
        do_non_cubic_weight: true,
        vert_weights: [
            // vert0Weights
            [
                SizeInfo::East, SizeInfo::South, SizeInfo::East, SizeInfo::FlipSouth,
                SizeInfo::FlipEast, SizeInfo::FlipSouth, SizeInfo::FlipEast, SizeInfo::South,
            ],
            // vert1Weights
            [
                SizeInfo::East, SizeInfo::North, SizeInfo::East, SizeInfo::FlipNorth,
                SizeInfo::FlipEast, SizeInfo::FlipNorth, SizeInfo::FlipEast, SizeInfo::North,
            ],
            // vert2Weights
            [
                SizeInfo::West, SizeInfo::North, SizeInfo::West, SizeInfo::FlipNorth,
                SizeInfo::FlipWest, SizeInfo::FlipNorth, SizeInfo::FlipWest, SizeInfo::North,
            ],
            // vert3Weights
            [
                SizeInfo::West, SizeInfo::South, SizeInfo::West, SizeInfo::FlipSouth,
                SizeInfo::FlipWest, SizeInfo::FlipSouth, SizeInfo::FlipWest, SizeInfo::South,
            ],
        ],
    },
    // NORTH (Direction.NORTH), shadeWeight 0.8F
    AdjacencyInfo {
        corners: [Face::Up, Face::Down, Face::East, Face::West],
        do_non_cubic_weight: true,
        vert_weights: [
            // vert0Weights
            [
                SizeInfo::Up, SizeInfo::FlipWest, SizeInfo::Up, SizeInfo::West,
                SizeInfo::FlipUp, SizeInfo::West, SizeInfo::FlipUp, SizeInfo::FlipWest,
            ],
            // vert1Weights
            [
                SizeInfo::Up, SizeInfo::FlipEast, SizeInfo::Up, SizeInfo::East,
                SizeInfo::FlipUp, SizeInfo::East, SizeInfo::FlipUp, SizeInfo::FlipEast,
            ],
            // vert2Weights
            [
                SizeInfo::Down, SizeInfo::FlipEast, SizeInfo::Down, SizeInfo::East,
                SizeInfo::FlipDown, SizeInfo::East, SizeInfo::FlipDown, SizeInfo::FlipEast,
            ],
            // vert3Weights
            [
                SizeInfo::Down, SizeInfo::FlipWest, SizeInfo::Down, SizeInfo::West,
                SizeInfo::FlipDown, SizeInfo::West, SizeInfo::FlipDown, SizeInfo::FlipWest,
            ],
        ],
    },
    // SOUTH (Direction.SOUTH), shadeWeight 0.8F
    AdjacencyInfo {
        corners: [Face::West, Face::East, Face::Down, Face::Up],
        do_non_cubic_weight: true,
        vert_weights: [
            // vert0Weights
            [
                SizeInfo::Up, SizeInfo::FlipWest, SizeInfo::FlipUp, SizeInfo::FlipWest,
                SizeInfo::FlipUp, SizeInfo::West, SizeInfo::Up, SizeInfo::West,
            ],
            // vert1Weights
            [
                SizeInfo::Down, SizeInfo::FlipWest, SizeInfo::FlipDown, SizeInfo::FlipWest,
                SizeInfo::FlipDown, SizeInfo::West, SizeInfo::Down, SizeInfo::West,
            ],
            // vert2Weights
            [
                SizeInfo::Down, SizeInfo::FlipEast, SizeInfo::FlipDown, SizeInfo::FlipEast,
                SizeInfo::FlipDown, SizeInfo::East, SizeInfo::Down, SizeInfo::East,
            ],
            // vert3Weights
            [
                SizeInfo::Up, SizeInfo::FlipEast, SizeInfo::FlipUp, SizeInfo::FlipEast,
                SizeInfo::FlipUp, SizeInfo::East, SizeInfo::Up, SizeInfo::East,
            ],
        ],
    },
    // WEST (Direction.WEST), shadeWeight 0.6F
    AdjacencyInfo {
        corners: [Face::Up, Face::Down, Face::North, Face::South],
        do_non_cubic_weight: true,
        vert_weights: [
            // vert0Weights
            [
                SizeInfo::Up, SizeInfo::South, SizeInfo::Up, SizeInfo::FlipSouth,
                SizeInfo::FlipUp, SizeInfo::FlipSouth, SizeInfo::FlipUp, SizeInfo::South,
            ],
            // vert1Weights
            [
                SizeInfo::Up, SizeInfo::North, SizeInfo::Up, SizeInfo::FlipNorth,
                SizeInfo::FlipUp, SizeInfo::FlipNorth, SizeInfo::FlipUp, SizeInfo::North,
            ],
            // vert2Weights
            [
                SizeInfo::Down, SizeInfo::North, SizeInfo::Down, SizeInfo::FlipNorth,
                SizeInfo::FlipDown, SizeInfo::FlipNorth, SizeInfo::FlipDown, SizeInfo::North,
            ],
            // vert3Weights
            [
                SizeInfo::Down, SizeInfo::South, SizeInfo::Down, SizeInfo::FlipSouth,
                SizeInfo::FlipDown, SizeInfo::FlipSouth, SizeInfo::FlipDown, SizeInfo::South,
            ],
        ],
    },
    // EAST (Direction.EAST), shadeWeight 0.6F
    AdjacencyInfo {
        corners: [Face::Down, Face::Up, Face::North, Face::South],
        do_non_cubic_weight: true,
        vert_weights: [
            // vert0Weights
            [
                SizeInfo::FlipDown, SizeInfo::South, SizeInfo::FlipDown, SizeInfo::FlipSouth,
                SizeInfo::Down, SizeInfo::FlipSouth, SizeInfo::Down, SizeInfo::South,
            ],
            // vert1Weights
            [
                SizeInfo::FlipDown, SizeInfo::North, SizeInfo::FlipDown, SizeInfo::FlipNorth,
                SizeInfo::Down, SizeInfo::FlipNorth, SizeInfo::Down, SizeInfo::North,
            ],
            // vert2Weights
            [
                SizeInfo::FlipUp, SizeInfo::North, SizeInfo::FlipUp, SizeInfo::FlipNorth,
                SizeInfo::Up, SizeInfo::FlipNorth, SizeInfo::Up, SizeInfo::North,
            ],
            // vert3Weights
            [
                SizeInfo::FlipUp, SizeInfo::South, SizeInfo::FlipUp, SizeInfo::FlipSouth,
                SizeInfo::Up, SizeInfo::FlipSouth, SizeInfo::Up, SizeInfo::South,
            ],
        ],
    },
];

/// `BlockModelLighter.AmbientVertexRemap.BY_FACING` — `REMAP[face][slot]` is the
/// output vertex index vanilla writes the slot's sample to. Java order:
/// DOWN(0, 1, 2, 3), UP(2, 3, 0, 1), NORTH(3, 0, 1, 2), SOUTH(0, 1, 2, 3),
/// WEST(3, 0, 1, 2), EAST(1, 2, 3, 0).
const AMBIENT_VERTEX_REMAP: [[usize; 4]; 6] = [
    [0, 1, 2, 3], // DOWN
    [2, 3, 0, 1], // UP
    [3, 0, 1, 2], // NORTH
    [0, 1, 2, 3], // SOUTH
    [3, 0, 1, 2], // WEST
    [1, 2, 3, 0], // EAST
];

/// `FaceInfo`'s four corners per face, as `(x, y, z)` extent selectors: `true`
/// picks the quad's max on that axis, `false` its min. Indexed by
/// [`Face::index`]; the rows are the module docs' vertex-order table.
const FACE_INFO: [[[bool; 3]; 4]; 6] = [
    [[false, false, true], [false, false, false], [true, false, false], [true, false, true]], // DOWN
    [[false, true, false], [false, true, true], [true, true, true], [true, true, false]],     // UP
    [[true, true, false], [true, false, false], [false, false, false], [false, true, false]], // NORTH
    [[false, true, true], [false, false, true], [true, false, true], [true, true, true]],     // SOUTH
    [[false, true, false], [false, false, false], [false, false, true], [false, true, true]], // WEST
    [[true, true, true], [true, false, true], [true, false, false], [true, true, false]],     // EAST
];

/// Where each `FaceInfo` corner sits in an arbitrarily ordered quad:
/// `order[k]` is the index in `vertices` of `BakedQuad` vertex `k`.
///
/// This is `FaceBakery.recalculateWinding`, which vanilla runs on every quad
/// without an element rotation: it takes the quad's extents and moves the
/// vertex equal to each `FaceInfo` corner into that slot. Rewo's bake emits
/// its own corner order, so the lighter needs the same permutation to put each
/// result on the right vertex. A vertex that equals no extent corner (an
/// element-rotated quad, which vanilla leaves in its pre-rotation order) takes
/// the nearest unclaimed corner — an approximation for those quads only.
pub fn face_info_order(vertices: &[[f32; 3]; 4], direction: Face) -> [usize; 4] {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for v in vertices {
        for axis in 0..3 {
            min[axis] = min[axis].min(v[axis]);
            max[axis] = max[axis].max(v[axis]);
        }
    }
    let mut order = [usize::MAX; 4];
    let mut used = [false; 4];
    for (k, corner) in FACE_INFO[direction.index()].iter().enumerate() {
        let target: [f32; 3] = std::array::from_fn(|a| if corner[a] { max[a] } else { min[a] });
        let exact = (0..4).find(|&i| !used[i] && vertices[i] == target);
        let i = exact.unwrap_or_else(|| {
            (0..4)
                .filter(|&i| !used[i])
                .min_by(|&a, &b| {
                    let d = |i: usize| (0..3).map(|x| (vertices[i][x] - target[x]).powi(2)).sum::<f32>();
                    d(a).total_cmp(&d(b))
                })
                .expect("four vertices, four corners")
        });
        used[i] = true;
        order[k] = i;
    }
    order
}

/// `BlockModelLighter.faceCubic` for a quad — what `prepareQuadFlat` asks
/// (`faceCubic ? pos.relative(direction) : pos` is where it samples light).
pub fn face_cubic(
    world: &impl AoWorld,
    pos: (i32, i32, i32),
    direction: Face,
    vertices: &[[f32; 3]; 4],
) -> bool {
    prepare_quad_shape(world, pos, direction, vertices).face_cubic
}

/// The face shape `BlockModelLighter` keeps between `prepareQuadShape` and the
/// ambient-occlusion pass.
struct QuadShape {
    /// `BlockModelLighter.faceCubic`.
    face_cubic: bool,
    /// `BlockModelLighter.facePartial`.
    face_partial: bool,
    /// `BlockModelLighter.faceShape`, indexed by [`SizeInfo::index`].
    face_shape: [f32; SizeInfo::COUNT],
}

/// `BlockModelLighter.prepareQuadShape(level, state, pos, quad, /* ambientOcclusion */ true)`
/// — the bounding box of the four corners decides `facePartial`, `faceCubic`
/// and the `faceShape` weights.
fn prepare_quad_shape(
    world: &impl AoWorld,
    pos: (i32, i32, i32),
    direction: Face,
    vertices: &[[f32; 3]; 4],
) -> QuadShape {
    let mut min = [32.0f32; 3];
    let mut max = [-32.0f32; 3];
    for vertex in vertices {
        for axis in 0..3 {
            min[axis] = min[axis].min(vertex[axis]);
            max[axis] = max[axis].max(vertex[axis]);
        }
    }

    let (min_x, min_y, min_z) = (min[0], min[1], min[2]);
    let (max_x, max_y, max_z) = (max[0], max[1], max[2]);

    let mut face_shape = [0.0f32; SizeInfo::COUNT];
    face_shape[SizeInfo::West.index()] = min_x;
    face_shape[SizeInfo::East.index()] = max_x;
    face_shape[SizeInfo::Down.index()] = min_y;
    face_shape[SizeInfo::Up.index()] = max_y;
    face_shape[SizeInfo::North.index()] = min_z;
    face_shape[SizeInfo::South.index()] = max_z;
    face_shape[SizeInfo::FlipWest.index()] = 1.0 - min_x;
    face_shape[SizeInfo::FlipEast.index()] = 1.0 - max_x;
    face_shape[SizeInfo::FlipDown.index()] = 1.0 - min_y;
    face_shape[SizeInfo::FlipUp.index()] = 1.0 - max_y;
    face_shape[SizeInfo::FlipNorth.index()] = 1.0 - min_z;
    face_shape[SizeInfo::FlipSouth.index()] = 1.0 - max_z;

    const MIN_EPSILON: f32 = 1.0E-4; // Java `minEpsilon`
    const MAX_EPSILON: f32 = 0.9999; // Java `maxEpsilon`

    // `facePartial`: is the quad smaller than a full face in its own plane?
    let face_partial = match direction {
        Face::Down | Face::Up => {
            min_x >= MIN_EPSILON
                || min_z >= MIN_EPSILON
                || max_x <= MAX_EPSILON
                || max_z <= MAX_EPSILON
        }
        Face::North | Face::South => {
            min_x >= MIN_EPSILON
                || min_y >= MIN_EPSILON
                || max_x <= MAX_EPSILON
                || max_y <= MAX_EPSILON
        }
        Face::West | Face::East => {
            min_y >= MIN_EPSILON
                || min_z >= MIN_EPSILON
                || max_y <= MAX_EPSILON
                || max_z <= MAX_EPSILON
        }
    };

    // `faceCubic`: does the quad lie on the outer face of a full block?
    let face_cubic = match direction {
        Face::Down => min_y == max_y && (min_y < MIN_EPSILON || world.is_collision_shape_full_block(pos)),
        Face::Up => min_y == max_y && (max_y > MAX_EPSILON || world.is_collision_shape_full_block(pos)),
        Face::North => min_z == max_z && (min_z < MIN_EPSILON || world.is_collision_shape_full_block(pos)),
        Face::South => min_z == max_z && (max_z > MAX_EPSILON || world.is_collision_shape_full_block(pos)),
        Face::West => min_x == max_x && (min_x < MIN_EPSILON || world.is_collision_shape_full_block(pos)),
        Face::East => min_x == max_x && (max_x > MAX_EPSILON || world.is_collision_shape_full_block(pos)),
    };

    QuadShape {
        face_cubic,
        face_partial,
        face_shape,
    }
}

/// Vanilla's `vertNweight01`, `vertNweight23`, `vertNweight45`, `vertNweight67`:
/// the four products of the eight `SizeInfo` entries of one `vertNWeights` row.
fn weight_products(weights: &[SizeInfo; 8], face_shape: &[f32; SizeInfo::COUNT]) -> [f32; 4] {
    [
        face_shape[weights[0].index()] * face_shape[weights[1].index()],
        face_shape[weights[2].index()] * face_shape[weights[3].index()],
        face_shape[weights[4].index()] * face_shape[weights[5].index()],
        face_shape[weights[6].index()] * face_shape[weights[7].index()],
    ]
}

/// `BlockModelLighter.prepareQuadAmbientOcclusion(level, state, centerPosition,
/// quad, outputInstance)` — one quad's per-vertex light and AO brightness.
///
/// `pos` is vanilla's `centerPosition` (the block being meshed) and `vertices`
/// the quad's four corners in `BakedQuad` order (see the module docs). The
/// returned [`QuadLight`] is vanilla's `QuadInstance` state after
/// `AmbientVertexRemap`, i.e. before `scaleColor`.
pub fn quad_ambient_occlusion(
    world: &impl AoWorld,
    pos: (i32, i32, i32),
    direction: Face,
    vertices: [[f32; 3]; 4],
) -> QuadLight {
    let shape = prepare_quad_shape(world, pos, direction, &vertices);
    let normal = direction.step();
    let base = if shape.face_cubic {
        offset(pos, normal)
    } else {
        pos
    };
    let info = &ADJACENCY[direction.index()];

    // The four side samples of the quad's corner: `info.corners[i]` from `base`.
    let p0 = offset(base, info.corners[0].step());
    let light0 = world.light_coords(p0);
    let shade0 = world.shade_brightness(p0);
    let p1 = offset(base, info.corners[1].step());
    let light1 = world.light_coords(p1);
    let shade1 = world.shade_brightness(p1);
    let p2 = offset(base, info.corners[2].step());
    let light2 = world.light_coords(p2);
    let shade2 = world.shade_brightness(p2);
    let p3 = offset(base, info.corners[3].step());
    let light3 = world.light_coords(p3);
    let shade3 = world.shade_brightness(p3);

    // Vanilla probes translucency one step *past* each side sample, along the
    // face normal: `level.getBlockState(pos.setWithOffset(basePosition,
    // info.corners[i]).move(direction))`, then
    // `!corner.isViewBlocking(level, pos) || corner.getLightDampening() == 0`.
    let q0 = offset(p0, normal);
    let translucent0 = !world.is_view_blocking(q0) || world.light_dampening(q0) == 0;
    let q1 = offset(p1, normal);
    let translucent1 = !world.is_view_blocking(q1) || world.light_dampening(q1) == 0;
    let q2 = offset(p2, normal);
    let translucent2 = !world.is_view_blocking(q2) || world.light_dampening(q2) == 0;
    let q3 = offset(p3, normal);
    let translucent3 = !world.is_view_blocking(q3) || world.light_dampening(q3) == 0;

    // The four diagonal samples, one per vertex pair. When both of a pair's
    // side samples are opaque (`!translucentN && !translucentM`) vanilla drops
    // the diagonal and reuses `shade0`/`light0` — the corners[0] sample, even
    // for the 12 and 13 pairs. That is what the Java does, bug or not.
    let (shade_corner02, light_corner02) = if !translucent2 && !translucent0 {
        (shade0, light0)
    } else {
        let p = offset(base, offset(info.corners[0].step(), info.corners[2].step()));
        (world.shade_brightness(p), world.light_coords(p))
    };
    let (shade_corner03, light_corner03) = if !translucent3 && !translucent0 {
        (shade0, light0)
    } else {
        let p = offset(base, offset(info.corners[0].step(), info.corners[3].step()));
        (world.shade_brightness(p), world.light_coords(p))
    };
    let (shade_corner12, light_corner12) = if !translucent2 && !translucent1 {
        (shade0, light0)
    } else {
        let p = offset(base, offset(info.corners[1].step(), info.corners[2].step()));
        (world.shade_brightness(p), world.light_coords(p))
    };
    let (shade_corner13, light_corner13) = if !translucent3 && !translucent1 {
        (shade0, light0)
    } else {
        let p = offset(base, offset(info.corners[1].step(), info.corners[3].step()));
        (world.shade_brightness(p), world.light_coords(p))
    };

    // The centre sample: the block itself, replaced by the block the face looks
    // into unless that one renders solid.
    let mut light_center = world.light_coords(pos);
    let next = offset(pos, normal);
    if shape.face_cubic || !world.is_solid_render(next) {
        light_center = world.light_coords(next);
    }
    let shade_center = world.shade_brightness(if shape.face_cubic { base } else { pos });

    // Vanilla computes the same four averages in both branches below
    // (`tempShade1..4` / `lightLevel1..4`) and the same four `smoothBlend`
    // results (`_tc1.._tc4`, or inlined per vertex in the plain branch); the
    // helpers are pure, so both are computed once here:
    //
    //   tempShade1 / _tc1 <- shade3, shade0, shadeCorner03, shadeCenter / light3, light0, lightCorner03, lightCenter
    //   tempShade2 / _tc2 <- shade2, shade0, shadeCorner02, shadeCenter / light2, light0, lightCorner02, lightCenter
    //   tempShade3 / _tc3 <- shade2, shade1, shadeCorner12, shadeCenter / light2, light1, lightCorner12, lightCenter
    //   tempShade4 / _tc4 <- shade3, shade1, shadeCorner13, shadeCenter / light3, light1, lightCorner13, lightCenter
    let temp_shade = [
        (shade3 + shade0 + shade_corner03 + shade_center) * 0.25,
        (shade2 + shade0 + shade_corner02 + shade_center) * 0.25,
        (shade2 + shade1 + shade_corner12 + shade_center) * 0.25,
        (shade3 + shade1 + shade_corner13 + shade_center) * 0.25,
    ];
    let blended = [
        smooth_blend(light3, light0, light_corner03, light_center),
        smooth_blend(light2, light0, light_corner02, light_center),
        smooth_blend(light2, light1, light_corner12, light_center),
        smooth_blend(light3, light1, light_corner13, light_center),
    ];

    let remap = &AMBIENT_VERTEX_REMAP[direction.index()];
    let mut out = QuadLight {
        light_coords: [0; 4],
        brightness: [0.0; 4],
    };

    if shape.face_partial && info.do_non_cubic_weight {
        for (&out_vertex, vert_weights) in remap.iter().zip(info.vert_weights.iter()) {
            let weights = weight_products(vert_weights, &shape.face_shape);
            out.brightness[out_vertex] = (temp_shade[0] * weights[0]
                + temp_shade[1] * weights[1]
                + temp_shade[2] * weights[2]
                + temp_shade[3] * weights[3])
                .clamp(0.0, 1.0);
            out.light_coords[out_vertex] = smooth_weighted_blend(blended, weights);
        }
    } else {
        for (&out_vertex, &light) in remap.iter().zip(blended.iter()) {
            out.light_coords[out_vertex] = light;
        }
        for (&out_vertex, &brightness) in remap.iter().zip(temp_shade.iter()) {
            out.brightness[out_vertex] = brightness;
        }
    }

    out
}

/// `pos` + a direction step — vanilla's `BlockPos.setWithOffset` / `move`.
#[inline]
fn offset(pos: (i32, i32, i32), step: (i32, i32, i32)) -> (i32, i32, i32) {
    (pos.0 + step.0, pos.1 + step.1, pos.2 + step.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `LightCoordsUtil.pack(15, 15)` = 15<<4 | 15<<20 = 240 + 15728640 = 15728880.
    const FULL_BRIGHT: i32 = 15728880;
    /// `LightCoordsUtil.pack(0, 15)` = 15<<20 = 15728640.
    const FULL_SKY: i32 = 15728640;

    /// The six facts `AoWorld` reports at one position.
    #[derive(Clone, Copy)]
    struct Cell {
        light: i32,
        shade: f32,
        view_blocking: bool,
        dampening: i32,
        solid_render: bool,
        full_block: bool,
    }

    /// Air under an open sky: `getShadeBrightness` 1.0 (not a full collision
    /// shape), `getLightDampening` 0 (`propagatesSkylightDown`), nothing
    /// occluding, `getLightCoords` = `pack(0, 15)`.
    const SKY_AIR: Cell = Cell {
        light: FULL_SKY,
        shade: 1.0,
        view_blocking: false,
        dampening: 0,
        solid_render: false,
        full_block: false,
    };

    /// Air with no light at all — `pack(0, 0)`.
    const DARK_AIR: Cell = Cell {
        light: 0,
        ..SKY_AIR
    };

    /// Air lit at 10 sky levels only — `pack(0, 10)` = 10<<20 = 0x00A0_0000.
    const LIT_AIR: Cell = Cell {
        light: 0x00A0_0000,
        ..SKY_AIR
    };

    /// A full opaque cube standing on the ground under an open sky:
    /// `getShadeBrightness` 0.2 (`isCollisionShapeFullBlock`), `isSolidRender`
    /// so `getLightDampening` = 15, `isViewBlocking` (suffocating), and
    /// `getLightCoords` = `pack(0, 0)` — the light engine stores 0 at a node
    /// below its column's lowest sky source and the block emits nothing.
    const SOLID: Cell = Cell {
        light: 0,
        shade: 0.2,
        view_blocking: true,
        dampening: 15,
        solid_render: true,
        full_block: true,
    };

    /// A stub `AoWorld`: one default cell plus per-position overrides.
    struct TestWorld {
        default: Cell,
        cells: Vec<((i32, i32, i32), Cell)>,
    }

    impl TestWorld {
        fn new(default: Cell) -> Self {
            Self {
                default,
                cells: Vec::new(),
            }
        }

        fn at(mut self, pos: (i32, i32, i32), cell: Cell) -> Self {
            self.cells.push((pos, cell));
            self
        }

        fn cell(&self, pos: (i32, i32, i32)) -> Cell {
            self.cells
                .iter()
                .find(|(p, _)| *p == pos)
                .map(|(_, c)| *c)
                .unwrap_or(self.default)
        }
    }

    impl AoWorld for TestWorld {
        fn light_coords(&self, pos: (i32, i32, i32)) -> i32 {
            self.cell(pos).light
        }
        fn shade_brightness(&self, pos: (i32, i32, i32)) -> f32 {
            self.cell(pos).shade
        }
        fn is_view_blocking(&self, pos: (i32, i32, i32)) -> bool {
            self.cell(pos).view_blocking
        }
        fn light_dampening(&self, pos: (i32, i32, i32)) -> i32 {
            self.cell(pos).dampening
        }
        fn is_solid_render(&self, pos: (i32, i32, i32)) -> bool {
            self.cell(pos).solid_render
        }
        fn is_collision_shape_full_block(&self, pos: (i32, i32, i32)) -> bool {
            self.cell(pos).full_block
        }
    }

    /// `face_info_order` is `recalculateWinding`'s permutation: the identity on
    /// a quad already in `FaceInfo` order, and it undoes any reordering.
    #[test]
    fn face_info_order_recovers_the_baked_quad_order() {
        let faces = [Face::Down, Face::Up, Face::North, Face::South, Face::West, Face::East];
        for face in faces {
            // A partial quad, so min and max differ on both in-plane axes.
            let (lo, hi) = (0.25f32, 0.75f32);
            let canonical: [[f32; 3]; 4] = FACE_INFO[face.index()].map(|c| {
                std::array::from_fn(|a| {
                    let plane_axis = match face {
                        Face::Down | Face::Up => 1,
                        Face::North | Face::South => 2,
                        Face::West | Face::East => 0,
                    };
                    if a == plane_axis {
                        if c[a] { 1.0 } else { 0.0 }
                    } else if c[a] {
                        hi
                    } else {
                        lo
                    }
                })
            });
            assert_eq!(face_info_order(&canonical, face), [0, 1, 2, 3], "{face:?} identity");
            for shift in 1..4 {
                let rotated: [[f32; 3]; 4] = std::array::from_fn(|i| canonical[(i + shift) % 4]);
                let order = face_info_order(&rotated, face);
                for k in 0..4 {
                    assert_eq!(rotated[order[k]], canonical[k], "{face:?} shift {shift} vertex {k}");
                }
            }
        }
    }

    /// The up face of the full cube at (0,0,0): `FaceInfo.UP`'s four corners of
    /// the unit square at y = 1 — (min_x, max_y, min_z) … clockwise.
    const UP_FACE: [[f32; 3]; 4] = [
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
        [1.0, 1.0, 0.0],
    ];

    /// Open sky, no neighbours, sky light 15: every vertex is fully lit and
    /// unshaded.
    #[test]
    fn open_sky_is_fully_lit_and_unshaded() {
        let quad = quad_ambient_occlusion(&TestWorld::new(SKY_AIR), (0, 0, 0), Face::Up, UP_FACE);

        // `faceCubic` (min_y == max_y == 1 > 0.9999) puts the samples at
        // base (0,1,0) + the four corners and four diagonals; every one is
        // sky-lit air: `pack(0, 15)` = 0<<4 | 15<<20 = 15728640 = 0x00F0_0000.
        // `smoothBlend` of four identical samples is that sample:
        // (4 * 0x00F0_0000) >> 2 & 0x00FF_00FF = 0x00F0_0000.
        assert_eq!(quad.light_coords, [0x00F0_0000; 4]);
        // (1.0 + 1.0 + 1.0 + 1.0) * 0.25 = 1.0 at every vertex.
        assert_eq!(quad.brightness, [1.0; 4]);

        // With block light 15 as well the packed value is vanilla's FULL_BRIGHT:
        // `pack(15, 15)` = 15<<4 | 15<<20 = 240 + 15728640 = 15728880 = 0x00F0_00F0.
        let lit = TestWorld::new(Cell {
            light: FULL_BRIGHT,
            ..SKY_AIR
        });
        let quad = quad_ambient_occlusion(&lit, (0, 0, 0), Face::Up, UP_FACE);
        assert_eq!(quad.light_coords, [0x00F0_00F0; 4]);
        assert_eq!(quad.brightness, [1.0; 4]);
    }

    /// One opaque block diagonally adjacent to a corner of the up face darkens
    /// that corner through `shadeCorner03`.
    #[test]
    fn opaque_diagonal_sample_darkens_one_corner() {
        // The opaque block sits at (1,1,1) = base (0,1,0) + EAST + SOUTH, the
        // diagonal sample of the corner shared by the EAST (corners[0]) and
        // SOUTH (corners[3]) sides. `AmbientVertexRemap.UP = (2, 3, 0, 1)` sends
        // that slot to vertex 2, and `FaceInfo.UP`'s vertex 2 is
        // (max_x, max_y, max_z) — the corner facing the block. Translucency is
        // probed at (1,2,0) and (0,2,1), which stay air.
        let world = TestWorld::new(SKY_AIR).at((1, 1, 1), SOLID);
        let quad = quad_ambient_occlusion(&world, (0, 0, 0), Face::Up, UP_FACE);

        // Vertex 2: (shade3 + shade0 + shadeCorner03 + shadeCenter) * 0.25
        //   = (1.0 + 1.0 + 0.2 + 1.0) * 0.25 = 3.2 * 0.25 = 0.8
        // (f32: 2.0 + 0.2 rounds to f32(2.2); + 1.0 is f32(3.2) exactly;
        // * 0.25 scales the exponent, so the literal 0.8 is exact).
        // The other three vertices see only air: (1 + 1 + 1 + 1) * 0.25 = 1.0.
        assert_eq!(quad.brightness, [1.0, 1.0, 0.8, 1.0]);

        // Vertex 2's light is `smoothBlend(light3, light0, lightCorner03, lightCenter)`
        // = `smoothBlend(FULL_SKY, FULL_SKY, 0, FULL_SKY)`. `sky(center) = 15 > 2`
        // and the dark sample is exactly 0, so vanilla's
        // `if (neighbor == 0) neighbor = center` substitution replaces it with
        // the centre: (4 * 0x00F0_0000) >> 2 & 0x00FF_00FF = 0x00F0_0000.
        assert_eq!(quad.light_coords, [0x00F0_0000; 4]);
    }

    /// Both side samples around a corner opaque: vanilla drops the diagonal
    /// sample and reuses the `corners[0]` sample.
    #[test]
    fn both_sides_occluded_replaces_the_diagonal_sample() {
        // Up face of the cube at (0,0,0), base = (0,1,0). The corner under test
        // is again vertex 2 (slot 0). The two side samples of that corner are
        // (1,1,0) = corners[0] (shade 0.5, sky-lit) and (0,1,1) = corners[3]
        // (shade 1.0); the diagonal sample (1,1,1) carries shade 0.25 and
        // `pack(0, 4)` = 4<<20 = 0x0040_0000 so it is visible when used.
        // Vanilla reads the sides' translucency at base + corner + normal, i.e.
        // (1,2,0) and (0,2,1) — opaque there means `translucent0`/`translucent3`
        // are false and `if (!translucent3 && !translucent0)` fires.
        let base_world = || {
            TestWorld::new(SKY_AIR)
                .at((1, 1, 0), Cell { shade: 0.5, ..SKY_AIR })
                .at((1, 1, 1), Cell { light: 0x0040_0000, shade: 0.25, ..SKY_AIR })
        };

        // Both side probes opaque: the diagonal is replaced by `shade0`/`light0`.
        // Vertex 2: (1.0 + 0.5 + 0.5 + 1.0) * 0.25 = 3.0 * 0.25 = 0.75 — note
        // the replacement is `shade0` = 0.5, not `shade3` = 1.0 (which would give
        // (1 + 0.5 + 1 + 1) * 0.25 = 0.875).
        let quad = quad_ambient_occlusion(
            &base_world().at((1, 2, 0), SOLID).at((0, 2, 1), SOLID),
            (0, 0, 0),
            Face::Up,
            UP_FACE,
        );
        assert_eq!(quad.brightness, [1.0, 1.0, 0.75, 0.875]);
        // The light of vertex 2 is `smoothBlend(FULL_SKY, FULL_SKY, FULL_SKY, FULL_SKY)`
        // = 0x00F0_0000 once the diagonal has been replaced.
        assert_eq!(quad.light_coords, [0x00F0_0000; 4]);

        // Only one side probe opaque: the rule needs both, so the diagonal is
        // used: (1.0 + 0.5 + 0.25 + 1.0) * 0.25 = 2.75 * 0.25 = 0.6875, and
        // `smoothBlend(FULL_SKY, FULL_SKY, 0x0040_0000, FULL_SKY)`:
        // sky(center) = 15 > 2 but no sample is 0, so all four stand —
        // (240 + 240 + 64 + 240) / 4 = 784 / 4 = 196 = 0xC4 in the sky channel
        // (12.25 levels), i.e. 0x00C4_0000.
        for probe in [(1, 2, 0), (0, 2, 1)] {
            let world = base_world().at(probe, SOLID);
            let quad = quad_ambient_occlusion(&world, (0, 0, 0), Face::Up, UP_FACE);
            assert_eq!(quad.brightness, [1.0, 1.0, 0.6875, 0.875], "probe {probe:?}");
            assert_eq!(quad.light_coords, [0x00F0_0000, 0x00F0_0000, 0x00C4_0000, 0x00F0_0000], "probe {probe:?}");
        }

        // Neither probe opaque: same as above, the diagonal is used.
        let quad = quad_ambient_occlusion(&base_world(), (0, 0, 0), Face::Up, UP_FACE);
        assert_eq!(quad.brightness, [1.0, 1.0, 0.6875, 0.875]);
        assert_eq!(quad.light_coords, [0x00F0_0000, 0x00F0_0000, 0x00C4_0000, 0x00F0_0000]);
    }

    /// A partial (non-cubic) face runs the weighted branch
    /// (`facePartial && doNonCubicWeight`): each vertex is a weighted sum of the
    /// four corner blends instead of one quarter each.
    #[test]
    fn partial_face_uses_the_weighted_blend() {
        // A half-height top face (y = 0.5) covering only x in 0..0.5: the top of
        // a slab-like block that fills half the block in x. `facePartial` for UP
        // looks at the in-plane extents only (x and z), so the face must be
        // inset there to be partial; a full-width half-height top face would be
        // `facePartial = false`. Vertices in `FaceInfo.UP` order.
        let vertices = [
            [0.0, 0.5, 0.0],
            [0.0, 0.5, 1.0],
            [0.5, 0.5, 1.0],
            [0.5, 0.5, 0.0],
        ];
        // Samples around the block's own position (0,0,0): `faceCubic` is false
        // (min_y == max_y == 0.5 is not > 0.9999 and the block is not a full
        // collision shape), so `base` is the centre block itself and the samples
        // sit in its own layer; translucency is probed one step up, at y = 1.
        // Shades are picked as exact binary fractions so the expectations are
        // exact; the diagonal lights are `pack(0, 10)` = 10<<20 = 0x00A0_0000.
        let world = TestWorld::new(LIT_AIR)
            .at((1, 0, 0), Cell { light: 0x0040_0000, shade: 1.0, ..SKY_AIR })
            .at((-1, 0, 0), Cell { light: 0x0080_0000, shade: 0.5, ..SKY_AIR })
            .at((0, 0, -1), Cell { light: 0x0020_0000, shade: 1.0, ..SKY_AIR })
            .at((0, 0, 1), Cell { light: 0x0060_0000, shade: 0.25, ..SKY_AIR })
            .at((1, 0, -1), Cell { shade: 0.75, ..LIT_AIR })
            .at((1, 0, 1), Cell { shade: 0.5, ..LIT_AIR })
            .at((-1, 0, -1), Cell { shade: 1.0, ..LIT_AIR })
            .at((-1, 0, 1), Cell { shade: 0.25, ..LIT_AIR });

        let shape = prepare_quad_shape(&world, (0, 0, 0), Face::Up, &vertices);
        assert!(shape.face_partial && !shape.face_cubic, "must take the weighted branch");

        let quad = quad_ambient_occlusion(&world, (0, 0, 0), Face::Up, vertices);

        // The four corner blends (tempShade1..4):
        //   t1 = (shade3 + shade0 + shadeCorner03 + shadeCenter) * 0.25 = (0.25 + 1.0 + 0.5 + 1.0) * 0.25 = 2.75 * 0.25 = 0.6875
        //   t2 = (shade2 + shade0 + shadeCorner02 + shadeCenter) * 0.25 = (1.0 + 1.0 + 0.75 + 1.0) * 0.25 = 3.75 * 0.25 = 0.9375
        //   t3 = (shade2 + shade1 + shadeCorner12 + shadeCenter) * 0.25 = (1.0 + 0.5 + 1.0 + 1.0) * 0.25 = 3.5 * 0.25 = 0.875
        //   t4 = (shade3 + shade1 + shadeCorner13 + shadeCenter) * 0.25 = (0.25 + 0.5 + 0.25 + 1.0) * 0.25 = 2.0 * 0.25 = 0.5
        //
        // `faceShape` from x in 0..0.5, y = 0.5, z in 0..1: WEST 0.0, EAST 0.5,
        // DOWN 0.5, UP 0.5, NORTH 0.0, SOUTH 1.0, FLIP_WEST 1.0, FLIP_EAST 0.5,
        // FLIP_NORTH 1.0, FLIP_SOUTH 0.0. `AdjacencyInfo.UP`'s weight rows give
        // (vertNweight01, _23, _45, _67):
        //   vert0 (EAST,SOUTH / EAST,FLIP_SOUTH / FLIP_EAST,FLIP_SOUTH / FLIP_EAST,SOUTH)
        //     = (0.5*1.0, 0.5*0.0, 0.5*0.0, 0.5*1.0) = (0.5, 0, 0, 0.5)
        //     -> 0.5*0.6875 + 0.5*0.5 = 0.59375, and vertex 2 via the remap
        //   vert1 (EAST,NORTH / EAST,FLIP_NORTH / FLIP_EAST,FLIP_NORTH / FLIP_EAST,NORTH)
        //     = (0.5*0.0, 0.5*1.0, 0.5*1.0, 0.5*0.0) = (0, 0.5, 0.5, 0)
        //     -> 0.5*0.9375 + 0.5*0.875 = 0.90625, vertex 3
        //   vert2 (WEST,NORTH / …) = (0*0, 0*1, 1*1, 1*0) = (0, 0, 1, 0) -> t3 = 0.875, vertex 0
        //   vert3 (WEST,SOUTH / …) = (0*1, 0*0, 1*0, 1*1) = (0, 0, 0, 1) -> t4 = 0.5, vertex 1
        assert_eq!(quad.brightness, [0.875, 0.5, 0.59375, 0.90625]);

        // The four `smoothBlend` results (block channel is 0 everywhere, so the
        // packed value is sky_smooth << 16; sky(center) = 10 > 2 and no sample
        // is 0, so nothing is substituted):
        //   _tc1 = (96 + 64 + 160 + 160) / 4 = 480 / 4 = 120 -> 0x0078_0000
        //   _tc2 = (32 + 64 + 160 + 160) / 4 = 416 / 4 = 104 -> 0x0068_0000
        //   _tc3 = (32 + 128 + 160 + 160) / 4 = 480 / 4 = 120 -> 0x0078_0000
        //   _tc4 = (96 + 128 + 160 + 160) / 4 = 544 / 4 = 136 -> 0x0088_0000
        // (64 = pack(0,4), 128 = pack(0,8), 32 = pack(0,2), 96 = pack(0,6),
        // 160 = pack(0,10) in smooth units.) Re-weighted per vertex the same way
        // as the brightness: vert0 -> (120*0.5 + 136*0.5) = 128 = 0x0080_0000,
        // vert1 -> (104*0.5 + 120*0.5) = 112 = 0x0070_0000, vert2 -> 120,
        // vert3 -> 136, then `smoothPack` (block 0, sky in bits 16..23).
        assert_eq!(
            quad.light_coords,
            [0x0078_0000, 0x0088_0000, 0x0080_0000, 0x0070_0000]
        );
    }

    /// Block light only: one neighbour holds `pack(14, 0)` and it blends into
    /// the two corners of its edge.
    #[test]
    fn block_light_blends_into_the_corners() {
        // Up face of the cube at (0,0,0), base = (0,1,0), every position dark
        // except the side sample (1,1,0) = corners[0], which holds the light of
        // a level-14 source: `pack(14, 0)` = 14<<4 = 224 = 0x0000_00E0.
        let world = TestWorld::new(DARK_AIR).at((1, 1, 0), Cell { light: 0x0000_00E0, ..DARK_AIR });
        let quad = quad_ambient_occlusion(&world, (0, 0, 0), Face::Up, UP_FACE);

        // `lightCenter` is the centre block's own `pack(0, 0)`, and so
        // `sky(center) = block(center) = 0`: the `neighbor == 0 -> center`
        // substitution of `smoothBlend` does not fire and the raw average is
        // taken. Both slots that sample corners[0] see
        // (0 + 224 + 0 + 0) >> 2 & 0x00FF_00FF = 56 & 0x00FF_00FF = 56 =
        // 0x0000_0038 — 224 = 14 levels * 16 in smooth units, 56 = 3.5 levels * 16.
        // Slot 0 -> vertex 2 (EAST+SOUTH corner), slot 1 -> vertex 3 (EAST+NORTH
        // corner); slots 2 and 3 see no source at all and stay 0.
        assert_eq!(quad.light_coords, [0, 0, 0x0000_0038, 0x0000_0038]);

        // No occluders anywhere, so every brightness is (1 + 1 + 1 + 1) * 0.25.
        assert_eq!(quad.brightness, [1.0; 4]);
    }
}
