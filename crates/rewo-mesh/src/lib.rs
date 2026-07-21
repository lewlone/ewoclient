//! rewo-mesh — M2 face-culled mesher for full-cube blocks.
//!
//! One mesh per column; a face is emitted when its neighbor is not a full
//! cube. Brightness = classic per-face shade × the neighbor cell's light
//! (that's the cell the face radiates into). Binary greedy meshing, packed
//! vertices, AO, and the model-quad path all land in M4 — this is the
//! correctness baseline the plan wants to diff against.

use bytemuck::{Pod, Zeroable};
use rewo_data::assets::RenderKind;
use rewo_world::World;

/// 36 bytes; the packed 8–12 byte format is an M4 concern.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub layer: u32,
    pub shade: f32,
}

pub struct ColumnMesh {
    pub cx: i32,
    pub cz: i32,
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    /// World-space Y bounds of emitted geometry (for the cull AABB).
    pub y_min: f32,
    pub y_max: f32,
}

/// Face order matches `rewo_data::assets::FACE_NAMES`:
/// [up, down, north(-Z), south(+Z), west(-X), east(+X)].
const FACE_OFFSETS: [(i32, i32, i32); 6] = [
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

/// Classic Minecraft face shading.
const FACE_SHADE: [f32; 6] = [1.0, 0.5, 0.8, 0.8, 0.6, 0.6];

/// Corner positions (unit cube) + UVs per face. v=0 is the texture top,
/// which must sit at the block's top on side faces.
const FACE_CORNERS: [[([f32; 3], [f32; 2]); 4]; 6] = [
    // up (+Y)
    [
        ([0.0, 1.0, 0.0], [0.0, 0.0]),
        ([1.0, 1.0, 0.0], [1.0, 0.0]),
        ([1.0, 1.0, 1.0], [1.0, 1.0]),
        ([0.0, 1.0, 1.0], [0.0, 1.0]),
    ],
    // down (-Y)
    [
        ([0.0, 0.0, 0.0], [0.0, 0.0]),
        ([0.0, 0.0, 1.0], [0.0, 1.0]),
        ([1.0, 0.0, 1.0], [1.0, 1.0]),
        ([1.0, 0.0, 0.0], [1.0, 0.0]),
    ],
    // north (-Z)
    [
        ([1.0, 1.0, 0.0], [0.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0]),
        ([0.0, 0.0, 0.0], [1.0, 1.0]),
        ([1.0, 0.0, 0.0], [0.0, 1.0]),
    ],
    // south (+Z)
    [
        ([0.0, 1.0, 1.0], [0.0, 0.0]),
        ([1.0, 1.0, 1.0], [1.0, 0.0]),
        ([1.0, 0.0, 1.0], [1.0, 1.0]),
        ([0.0, 0.0, 1.0], [0.0, 1.0]),
    ],
    // west (-X)
    [
        ([0.0, 1.0, 0.0], [0.0, 0.0]),
        ([0.0, 1.0, 1.0], [1.0, 0.0]),
        ([0.0, 0.0, 1.0], [1.0, 1.0]),
        ([0.0, 0.0, 0.0], [0.0, 1.0]),
    ],
    // east (+X)
    [
        ([1.0, 1.0, 1.0], [0.0, 0.0]),
        ([1.0, 1.0, 0.0], [1.0, 0.0]),
        ([1.0, 0.0, 0.0], [1.0, 1.0]),
        ([1.0, 0.0, 1.0], [0.0, 1.0]),
    ],
];

fn is_cube(table: &[RenderKind], state: u32) -> bool {
    matches!(
        table.get(state as usize),
        Some(RenderKind::Cube { .. })
    )
}

/// Mesh one loaded column. Returns None if it produced no geometry.
pub fn mesh_column(
    world: &World,
    table: &[RenderKind],
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

    for si in 0..shape.section_count() {
        if col.section_is_trivial(si) {
            continue;
        }
        let sy = shape.min_y + (si as i32) * 16;
        for y in sy..sy + 16 {
            for lz in 0..16 {
                for lx in 0..16 {
                    let state = col.block_state_at(&shape, lx, y, lz);
                    let Some(RenderKind::Cube { faces }) = table.get(state as usize) else {
                        continue;
                    };
                    for (face, &(dx, dy, dz)) in FACE_OFFSETS.iter().enumerate() {
                        let (nx, ny, nz) = (base_x + lx + dx, y + dy, base_z + lz + dz);
                        // Neighbor lookup: fast path inside this column.
                        let neighbor = if (0..16).contains(&(lx + dx))
                            && (0..16).contains(&(lz + dz))
                        {
                            col.block_state_at(&shape, lx + dx, ny, lz + dz)
                        } else {
                            world.block_state_at(nx, ny, nz)
                        };
                        if is_cube(table, neighbor) {
                            continue;
                        }
                        let light = world.brightness_at(nx, ny, nz) as f32 / 15.0;
                        let shade = FACE_SHADE[face] * (0.25 + 0.75 * light);
                        emit_face(
                            &mut vertices,
                            &mut indices,
                            [lx as f32, y as f32, lz as f32],
                            face,
                            faces[face] as u32,
                            shade,
                        );
                        y_min = y_min.min(y as f32);
                        y_max = y_max.max(y as f32 + 1.0);
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

fn emit_face(
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
    origin: [f32; 3],
    face: usize,
    layer: u32,
    shade: f32,
) {
    let base = vertices.len() as u32;
    for (offset, uv) in FACE_CORNERS[face] {
        vertices.push(MeshVertex {
            pos: [
                origin[0] + offset[0],
                origin[1] + offset[1],
                origin[2] + offset[2],
            ],
            uv,
            layer,
            shade,
        });
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_tables_are_consistent() {
        // Every face has 4 corners on the correct axis plane.
        for (face, corners) in FACE_CORNERS.iter().enumerate() {
            let (dx, dy, dz) = FACE_OFFSETS[face];
            for (pos, _) in corners {
                if dy == 1 {
                    assert_eq!(pos[1], 1.0);
                } else if dy == -1 {
                    assert_eq!(pos[1], 0.0);
                }
                if dx == 1 {
                    assert_eq!(pos[0], 1.0);
                } else if dx == -1 {
                    assert_eq!(pos[0], 0.0);
                }
                if dz == 1 {
                    assert_eq!(pos[2], 1.0);
                } else if dz == -1 {
                    assert_eq!(pos[2], 0.0);
                }
            }
        }
    }

    #[test]
    fn side_faces_have_texture_top_at_block_top() {
        // v=0 (texture top) must be at y=1 for the four side faces.
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
