//! rewo-mesh — M4 mesher: full-cube fast path with ambient occlusion, plus
//! the general model-quad path for everything else (stairs, slabs, fences,
//! glass, plants, torches, …).
//!
//! Per-vertex color = directional face shade × baked block/sky light × AO.
//! Biome tint is **baked into the texture layers** at asset time (the
//! grayscale grass/foliage textures get the colormap multiply), so the
//! mesher doesn't re-tint.
//!
//! Greedy meshing is deliberately not here (REWO_PLAN.md M4): per-vertex AO
//! makes coplanar faces non-mergeable, so visual parity wins over vertex
//! count for now — the plan's own tension, resolved toward the vanilla look.

pub mod pool;

use bytemuck::{Pod, Zeroable};
use rewo_data::assets::RenderKind;
use rewo_world::World;

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
                        Some(RenderKind::Cube { faces, .. }) => {
                            emit_cube(
                                world, table, &mut vertices, &mut indices, wx, y, wz, faces,
                            );
                            bump(y as f32);
                        }
                        Some(RenderKind::Model(idx)) => {
                            emit_model(
                                world,
                                table,
                                models,
                                &mut vertices,
                                &mut indices,
                                wx,
                                y,
                                wz,
                                *idx,
                            );
                            bump(y as f32);
                        }
                        Some(RenderKind::Fluid { layer, level, lava }) => {
                            // Water blends → translucent set; lava is
                            // opaque (and fullbright) → opaque set.
                            let (fv, fi) = if *lava {
                                (&mut vertices, &mut indices)
                            } else {
                                (&mut tvertices, &mut tindices)
                            };
                            emit_fluid(world, table, fv, fi, wx, y, wz, *layer, *level, *lava);
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
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    layer: u16,
    _level: u8,
    lava: bool,
) {
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

    // Face brightness mirrors the cube path (the cell the face looks into);
    // lava is fullbright.
    let light = |x: i32, yy: i32, z: i32| -> f32 {
        if lava {
            1.0
        } else {
            world.brightness_at(x, yy, z) as f32 / 15.0
        }
    };
    let mut quad = |p: [([f32; 3], [f32; 2]); 4], shade: f32, l: f32| {
        let c = shade * (0.25 + 0.75 * l);
        let base = vertices.len() as u32;
        for (pos, uv) in p {
            vertices.push(MeshVertex {
                pos,
                uv,
                layer: layer as u32,
                color: [c, c, c],
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
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    wx: i32,
    y: i32,
    wz: i32,
    faces: &[u16; 6],
) {
    for face in 0..6 {
        let (dx, dy, dz) = FACE_OFFSETS[face];
        let (nx, ny, nz) = (wx + dx, y + dy, wz + dz);
        if is_full_cube(table, world.block_state_at(nx, ny, nz)) {
            continue;
        }
        let light = world.brightness_at(nx, ny, nz) as f32 / 15.0;
        let base = FACE_SHADE[face] * (0.25 + 0.75 * light);
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
                layer: faces[face] as u32,
                color: [c, c, c],
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
    // Model quads get flat (per-face) shading + the block's own cell light;
    // AO on arbitrary quads is an M4-followon.
    let own_light = world.brightness_at(wx, y, wz).max(1) as f32 / 15.0;
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
        let c = shade * (0.25 + 0.75 * own_light);
        let base_idx = vertices.len() as u32;
        for i in 0..4 {
            vertices.push(MeshVertex {
                pos: [
                    wx as f32 + quad.verts[i][0],
                    y as f32 + quad.verts[i][1],
                    wz as f32 + quad.verts[i][2],
                ],
                uv: quad.uv[i],
                layer: quad.layer as u32,
                color: [c, c, c],
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
                tint: [false; 6],
            },
            RenderKind::Fluid {
                layer: 1,
                level: 0,
                lava: false,
            },
            RenderKind::Fluid {
                layer: 2,
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
}
