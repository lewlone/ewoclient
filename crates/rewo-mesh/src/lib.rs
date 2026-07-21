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
                        _ => {}
                    }
                }
            }
        }
    }

    if indices.is_empty() {
        return None;
    }
    Some(ColumnMesh {
        cx,
        cz,
        vertices,
        indices,
        y_min,
        y_max,
    })
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
