//! rewo-mesh — M4 mesher: full-cube fast path with ambient occlusion, plus
//! the general model-quad path for everything else (stairs, slabs, fences,
//! glass, plants, torches, …).
//!
//! Per-vertex color = directional face shade × AO × biome tint (white when
//! untinted). Block and sky light are packed *separately* into the layer word
//! and combined in the shader (see `pack_layer`) — they are NOT multiplied into
//! `MeshVertex.color`, so the time of day never forces a remesh.
//!
//! Biome tint is applied at mesh time (M14): a dynamic Grass/Foliage/DryFoliage/
//! Water face selects the *raw* (un-tinted) atlas layer and multiplies its
//! resolved biome color into `MeshVertex.color`; a `Constant` tint
//! (spruce/birch) multiplies a fixed color. Synthetic / no-biome worlds
//! deliberately keep the legacy pre-tinted layers with a white color, so the
//! demo path stays byte-identical. A per-job cache (`TintCache`) memoizes the
//! expensive vanilla radius-2 resolution, so a block's tint is computed once,
//! not once per face.
//!
//! Greedy meshing is deliberately not here (REWO_PLAN.md M4): per-vertex AO
//! makes coplanar faces non-mergeable, so visual parity wins over vertex
//! count for now — the plan's own tension, resolved toward the vanilla look.

pub mod pool;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rewo_data::assets::{RenderKind, TintSource};
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
    /// key = (canonical sampled block x, y, z, resolver code); value = 0..1 RGB.
    map: HashMap<(i32, i32, i32, u8), [f32; 3]>,
}

/// Dynamic biome tint (0..1 RGB multiplier) for a tinted face, or `None` to
/// fall back to the legacy pre-tinted layer (no biome context or an untinted
/// face). Multiplies `MeshVertex.color` alongside shade/AO — the camera sky/fog
/// is a separate uniform, so this never re-runs on a time-of-day change.
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
) -> Option<[f32; 3]> {
    use rewo_world::biome::ColorResolver;
    // No biome context (synthetic / demo world) → legacy path, byte-identical.
    world.biome_context()?;
    // Resolve to (sampled block pos, resolver, cache code); Constant / None
    // short-circuit without touching the cache.
    let (bx, by, bz, resolver, code) = match src {
        TintSource::None => return None,
        TintSource::Constant(c) => {
            return Some([
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            ]);
        }
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
    let v = [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
    ];
    cache.map.insert(key, v);
    Some(v)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub layer: u32,
    pub color: [f32; 3],
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
const FACE_SHADE: [f32; 6] = [1.0, 0.5, 0.8, 0.8, 0.6, 0.6];

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
/// AO level 0..3 → brightness.
const AO_LEVELS: [f32; 4] = [0.45, 0.65, 0.82, 1.0];

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

pub fn mesh_column(
    world: &World,
    table: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
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
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            emit_cube(
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
                        Some(RenderKind::Fluid {
                            layer,
                            raw_layer,
                            level,
                            lava,
                        }) => {
                            // Water blends → translucent set; lava is
                            // opaque (and fullbright) → opaque set.
                            let (fv, fi) = if *lava {
                                (&mut vertices, &mut indices)
                            } else {
                                (&mut tvertices, &mut tindices)
                            };
                            emit_fluid(
                                world,
                                table,
                                &mut tint_cache,
                                fv,
                                fi,
                                wx,
                                y,
                                wz,
                                *layer,
                                *raw_layer,
                                *level,
                                *lava,
                            );
                            bump(y as f32);
                        }
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
fn fluid_level(
    world: &World,
    table: &[RenderKind],
    x: i32,
    y: i32,
    z: i32,
    want_lava: bool,
) -> Option<u8> {
    match table.get(world.block_state_at(x, y, z) as usize) {
        Some(RenderKind::Fluid { level, lava, .. }) if *lava == want_lava => Some(*level),
        _ => None,
    }
}

/// Vanilla-style fluid cell: top face at per-corner heights (max over the
/// four touching same-fluid cells — a simpler take on vanilla's weighted
/// average that still reads as a continuous sloped surface), trapezoid
/// side faces down to the block floor, bottom face against air.
#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    world: &World,
    table: &[RenderKind],
    cache: &mut TintCache,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    layer: u16,
    raw_layer: u16,
    _level: u8,
    lava: bool,
) {
    // Water gets the biome water tint (raw layer + dynamic color); lava never.
    let (fluid_layer, tint_rgb) = if lava {
        (layer, [1.0f32; 3])
    } else {
        match biome_tint(world, cache, wx, y, wz, TintSource::Water) {
            Some(rgb) => (raw_layer, rgb),
            None => (layer, [1.0, 1.0, 1.0]),
        }
    };
    let same = |x: i32, yy: i32, z: i32| fluid_level(world, table, x, yy, z, lava).is_some();
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
            if let Some(lv) = fluid_level(world, table, cx, y, cz, lava) {
                let ch = if same(cx, y + 1, cz) { 1.0 } else { fluid_h(lv) };
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
    let mut quad = |p: [([f32; 3], [f32; 2]); 4], shade: f32, l: (u8, u8)| {
        let c = shade;
        let base = vertices.len() as u32;
        for (pos, uv) in p {
            vertices.push(MeshVertex {
                pos,
                uv,
                layer: pack_layer(fluid_layer as u32, l.0, l.1),
                color: [c * tint_rgb[0], c * tint_rgb[1], c * tint_rgb[2]],
            });
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
            FACE_SHADE[0],
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
        if same(nx, y, nz) || is_full_cube(table, world.block_state_at(nx, y, nz)) {
            continue;
        }
        quad(corners, FACE_SHADE[face], light(nx, y, nz));
    }
    // Bottom — against anything that isn't fluid or a full cube.
    if !same(wx, y - 1, wz) && !is_full_cube(table, world.block_state_at(wx, y - 1, wz)) {
        quad(
            [
                ([x0, yf, z0], [0.0, 0.0]),
                ([x0, yf, z1], [0.0, 1.0]),
                ([x1, yf, z1], [1.0, 1.0]),
                ([x1, yf, z0], [1.0, 0.0]),
            ],
            FACE_SHADE[1],
            light(wx, y - 1, wz),
        );
    }
}

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
) {
    for face in 0..6 {
        let (dx, dy, dz) = FACE_OFFSETS[face];
        let (nx, ny, nz) = (wx + dx, y + dy, wz + dz);
        if is_full_cube(table, world.block_state_at(nx, ny, nz)) {
            continue;
        }
        let (lb, ls) = world.light_at(nx, ny, nz);
        let base = FACE_SHADE[face];
        // Dynamic biome tint (or the legacy pre-tinted layer + white). The same
        // (wx,y,wz)+resolver recurs across the 6 faces; the cache serves it once.
        let (layer, tint_rgb) = match biome_tint(world, cache, wx, y, wz, tint[face]) {
            Some(rgb) => (raw_faces[face], rgb),
            None => (faces[face], [1.0, 1.0, 1.0]),
        };
        let (uu, vv, (fnx, fny, fnz)) = FACE_AXES[face];
        let base_idx = vertices.len() as u32;
        for (corner, uv) in FACE_CORNERS[face] {
            // AO from the three neighbors around this corner, in the layer
            // just outside the face.
            let su = 2 * corner[uu] as i32 - 1;
            let sv = 2 * corner[vv] as i32 - 1;
            let mut off_u = [0i32; 3];
            off_u[uu] = su;
            let mut off_v = [0i32; 3];
            off_v[vv] = sv;
            let np = [wx + fnx, y + fny, wz + fnz];
            let s1 = solid(world, table, [np[0] + off_u[0], np[1] + off_u[1], np[2] + off_u[2]]);
            let s2 = solid(world, table, [np[0] + off_v[0], np[1] + off_v[1], np[2] + off_v[2]]);
            let sc = solid(
                world,
                table,
                [
                    np[0] + off_u[0] + off_v[0],
                    np[1] + off_u[1] + off_v[1],
                    np[2] + off_u[2] + off_v[2],
                ],
            );
            let ao = if s1 && s2 {
                0
            } else {
                3 - (s1 as usize + s2 as usize + sc as usize)
            };
            let c = base * AO_LEVELS[ao];
            vertices.push(MeshVertex {
                pos: [wx as f32 + corner[0], y as f32 + corner[1], wz as f32 + corner[2]],
                uv,
                layer: pack_layer(layer as u32, lb, ls),
                color: [c * tint_rgb[0], c * tint_rgb[1], c * tint_rgb[2]],
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
        let shade = if quad.shade {
            FACE_SHADE[quad.dir as usize]
        } else {
            1.0
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
            None => (quad.layer, [1.0, 1.0, 1.0]),
        };
        let c = shade;
        let base_idx = vertices.len() as u32;
        for i in 0..4 {
            vertices.push(MeshVertex {
                pos: [
                    wx as f32 + quad.verts[i][0],
                    y as f32 + quad.verts[i][1],
                    wz as f32 + quad.verts[i][2],
                ],
                uv: quad.uv[i],
                layer: pack_layer(layer as u32, own_block, own_sky),
                color: [c * tint_rgb[0], c * tint_rgb[1], c * tint_rgb[2]],
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
}

fn solid(world: &World, table: &[RenderKind], p: [i32; 3]) -> bool {
    is_full_cube(table, world.block_state_at(p[0], p[1], p[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_world::dimension::DimensionShape;

    fn fluid_table() -> Vec<RenderKind> {
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

    #[test]
    fn water_source_meshes_translucent_at_vanilla_height() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 9, 4, 1); // floor cube
        w.set_block(4, 10, 4, 2); // water source on it
        let mesh = mesh_column(&w, &fluid_table(), &[], 0, 0).expect("meshed");
        assert!(!mesh.vertices.is_empty(), "floor cube goes to the opaque set");
        assert!(!mesh.tvertices.is_empty(), "water goes to the translucent set");
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

    #[test]
    fn lava_meshes_opaque() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 10, 4, 3);
        let mesh = mesh_column(&w, &fluid_table(), &[], 0, 0).expect("meshed");
        assert!(!mesh.vertices.is_empty(), "lava is opaque geometry");
        assert!(mesh.tvertices.is_empty());
    }

    #[test]
    fn submerged_water_column_is_full_height() {
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(4, 10, 4, 2);
        w.set_block(4, 11, 4, 2); // water above → lower cell is a full column
        let mesh = mesh_column(&w, &fluid_table(), &[], 0, 0).expect("meshed");
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
    /// pre-tinted layer + white color (byte-identical demo path); with a biome
    /// context it takes the RAW layer + the biome water color into
    /// `MeshVertex.color`.
    #[test]
    fn dynamic_biome_tint_vs_legacy_path() {
        use rewo_data::assets::Quad;
        use rewo_world::biome::{BiomeContext, BiomeDef, BiomeRegistry, Colormaps, GrassModifier};
        use std::sync::Arc;

        let table = vec![RenderKind::Invisible, RenderKind::Model(0)];
        let models = vec![vec![Quad {
            verts: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            uv: [[0.0, 0.0]; 4],
            layer: 7,      // legacy pre-tinted layer
            raw_layer: 8,  // raw layer for the biome path
            cull: -1,
            dir: 2,        // north
            tint: TintSource::Water,
            shade: false,  // c = 1.0 → color is exactly the tint
        }]];

        // Legacy: no biome context → white color, pre-tinted layer 7.
        let mut w = World::new(DimensionShape::OVERWORLD);
        w.ensure_column(0, 0);
        w.set_block(2, 64, 2, 1);
        let m = mesh_column(&w, &table, &models, 0, 0).expect("meshed");
        let v = m.vertices[0];
        assert_eq!(v.layer & 0xFFFF, 7, "legacy path uses the pre-tinted layer");
        assert_eq!(v.color, [1.0, 1.0, 1.0], "legacy path is untinted (white)");

        // Biome path: attach a registry whose water_color = (100, 0, 0). The
        // empty column's single-value biome container = index 0.
        let biome = BiomeDef {
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
        };
        let ctx = BiomeContext::new(
            Arc::new(BiomeRegistry::new(vec![biome])),
            Colormaps::neutral(),
            0,
        );
        w.set_biome_context(Arc::new(ctx));
        let m2 = mesh_column(&w, &table, &models, 0, 0).expect("meshed");
        let v2 = m2.vertices[0];
        assert_eq!(v2.layer & 0xFFFF, 8, "biome path uses the RAW layer");
        assert!(
            (v2.color[0] - 100.0 / 255.0).abs() < 1e-6,
            "red channel = biome water color: {}",
            v2.color[0]
        );
        assert_eq!(v2.color[1], 0.0);
        assert_eq!(v2.color[2], 0.0);
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
        assert_eq!(c, [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0]);
    }
}
