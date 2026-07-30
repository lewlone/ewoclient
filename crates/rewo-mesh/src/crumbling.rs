//! The block-break decal's geometry (M81) — `submitBlockDestroyAnimation`
//! plus `SheetedDecalTextureGenerator`.
//!
//! Vanilla re-collects the block's **own model parts** and draws them a second
//! time with a `destroy_stage_N` texture bound. Two properties of that make it
//! cheap to reproduce here and expensive to get wrong:
//!
//! * **The geometry is the block's, to the bit.** A slab's crack covers the
//!   slab, a fence's covers the posts. `LevelRenderer` guards on
//!   `getRenderShape() == RenderShape.MODEL`, so a block with no model gets no
//!   crack at all.
//! * **The model's own UVs are discarded.** `SheetedDecalTextureGenerator`
//!   implements `setUv` as a **no-op** and regenerates the coordinate from the
//!   vertex *position*, projected into the plane of its own face. That is why
//!   a `destroy_stage` texture — a standalone 16×16, not an atlas sprite —
//!   tiles one-per-block-face rather than landing wherever the block's atlas
//!   coordinates happen to point.
//!
//! The projection is `rotateY(π)` then `rotateX(-π/2)` then the face's own
//! `Direction.getRotation()` quaternion, and finally `uv = (-p.x, -p.y)` at
//! `textureScale = 1.0`. Worked through for all six faces it reduces to a
//! full `0..1` tile per face with per-face sign flips, which `TileMode::Repeat`
//! sampling resolves — but the chain is transcribed rather than replaced by
//! that summary, because the summary is only true for axis-aligned faces and
//! the generator runs on every quad a model has.
//!
//! No face culling. Vanilla walks `part.getQuads(d)` for **every** direction
//! plus the null-direction list, with no `cullface` test, and relies on the
//! depth test to hide the faces a neighbour covers. Reproducing the cull here
//! would be a plausible optimisation that changed what is drawn at an exposed
//! corner.

use rewo_data::assets::RenderKind;

/// One decal quad: four world-space corners and their regenerated UVs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalQuad {
    pub verts: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
}

/// Unit-cube corners per face, in Rewo's face order
/// (0 up, 1 down, 2 north, 3 south, 4 west, 5 east).
///
/// The same winding the mesher's `FACE_CORNERS` uses, minus its atlas UVs —
/// those are exactly what the decal generator throws away.
const CUBE_CORNERS: [[[f32; 3]; 4]; 6] = [
    [
        [0.0, 1.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ], // up
    [
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
    ], // down
    [
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
    ], // north
    [
        [0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
        [1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    ], // south
    [
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
    ], // west
    [
        [1.0, 1.0, 1.0],
        [1.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 0.0, 1.0],
    ], // east
];

/// `Direction.getRotation()` as `(angleX, angleY, angleZ)` in radians, in
/// Rewo's face order.
///
/// JOML's `rotationXYZ` composes to `Rx · Ry · Rz`, so the triple is applied
/// to a vector innermost-last: Z, then Y, then X. Written as a matrix product
/// below rather than as a quaternion, because the quaternion's argument order
/// is the one thing here a reader cannot check by inspection.
const FACE_ROTATION: [[f32; 3]; 6] = [
    [0.0, 0.0, 0.0],                                    // up   — identity
    [std::f32::consts::PI, 0.0, 0.0],                   // down — rotationX(π)
    [
        std::f32::consts::FRAC_PI_2,
        0.0,
        std::f32::consts::PI,
    ], // north
    [std::f32::consts::FRAC_PI_2, 0.0, 0.0],            // south
    [
        std::f32::consts::FRAC_PI_2,
        0.0,
        std::f32::consts::FRAC_PI_2,
    ], // west
    [
        std::f32::consts::FRAC_PI_2,
        0.0,
        -std::f32::consts::FRAC_PI_2,
    ], // east
];

/// `SheetedDecalTextureGenerator.setNormal`'s coordinate, for one vertex.
///
/// `p` is block-local (the pose inverse gives back the model-space position);
/// `dir` is the quad's snapped facing.
pub fn decal_uv(p: [f32; 3], dir: usize) -> [f32; 2] {
    // `worldPos.rotateY((float) Math.PI)` — (x, y, z) → (−x, y, −z).
    let a = [-p[0], p[1], -p[2]];
    // `worldPos.rotateX((float) (-Math.PI / 2))` — (x, y, z) → (x, z, −y).
    let b = [a[0], a[2], -a[1]];
    let [ax, ay, az] = FACE_ROTATION[dir.min(5)];
    let c = rot_x(ax, rot_y(ay, rot_z(az, b)));
    // `setUv(-worldPos.x() * textureScale, -worldPos.y() * textureScale)`,
    // with `textureScale = 1.0F`.
    [-c[0], -c[1]]
}

fn rot_x(a: f32, v: [f32; 3]) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
}

fn rot_y(a: f32, v: [f32; 3]) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

fn rot_z(a: f32, v: [f32; 3]) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
}

/// The decal quads for one block, in **world** space.
///
/// Empty for a state with no model geometry — air, a fluid, or an invisible
/// block — which is `RenderShape.MODEL`'s guard by another name.
pub fn block_decal_quads(
    render: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
    state: u32,
    pos: [i32; 3],
) -> Vec<DecalQuad> {
    let origin = [pos[0] as f32, pos[1] as f32, pos[2] as f32];
    let mut out = Vec::new();
    match render.get(state as usize) {
        Some(RenderKind::Cube { .. }) => {
            for (face, corners) in CUBE_CORNERS.iter().enumerate() {
                let mut q = DecalQuad {
                    verts: [[0.0; 3]; 4],
                    uv: [[0.0; 2]; 4],
                };
                for (i, corner) in corners.iter().enumerate() {
                    q.uv[i] = decal_uv(*corner, face);
                    for k in 0..3 {
                        q.verts[i][k] = origin[k] + corner[k];
                    }
                }
                out.push(q);
            }
        }
        Some(RenderKind::Model(idx)) => {
            let Some(quads) = models.get(*idx as usize) else {
                return out;
            };
            for quad in quads {
                let mut q = DecalQuad {
                    verts: [[0.0; 3]; 4],
                    uv: [[0.0; 2]; 4],
                };
                for i in 0..4 {
                    q.uv[i] = decal_uv(quad.verts[i], quad.dir as usize);
                    for k in 0..3 {
                        q.verts[i][k] = origin[k] + quad.verts[i][k];
                    }
                }
                out.push(q);
            }
        }
        // `RenderShape.INVISIBLE` and the fluid path: nothing to crack.
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six axis-aligned faces each map to a full unit tile. Checked by
    /// sampling the face's own corners: |u| and |v| must together sweep 0→1.
    #[test]
    fn every_cube_face_maps_to_one_whole_tile() {
        for (face, corners) in CUBE_CORNERS.iter().enumerate() {
            let uvs: Vec<[f32; 2]> = corners.iter().map(|c| decal_uv(*c, face)).collect();
            let us: Vec<f32> = uvs.iter().map(|q| q[0]).collect();
            let vs: Vec<f32> = uvs.iter().map(|q| q[1]).collect();
            let span = |a: &[f32]| {
                a.iter().cloned().fold(f32::MIN, f32::max) - a.iter().cloned().fold(f32::MAX, f32::min)
            };
            assert!(
                (span(&us) - 1.0).abs() < 1e-5,
                "face {face}: u spans {} (want 1.0) — {us:?}",
                span(&us)
            );
            assert!(
                (span(&vs) - 1.0).abs() < 1e-5,
                "face {face}: v spans {} (want 1.0) — {vs:?}",
                span(&vs)
            );
        }
    }

    /// The projection is planar: a face's UV must not vary along its own
    /// normal. Two points differing only in the normal axis map to the same
    /// coordinate.
    ///
    /// **To a tolerance, not to the bit**, and the reason is worth keeping:
    /// `sin(π)` is not zero in f32 (nor in Java's `Math.sin`, which is what
    /// JOML feeds `rotateY`), so the half-turn leaks a ~1e-7 cross-term from
    /// the normal axis into the tangent plane. Vanilla carries the identical
    /// leak. A bit-exact assertion here would be asserting more than the
    /// original does.
    #[test]
    fn the_projection_is_planar_per_face() {
        let close = |a: [f32; 2], b: [f32; 2]| {
            (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
        };
        // Up (+Y): moving in y must not move the uv.
        let (a, b) = (decal_uv([0.3, 0.0, 0.7], 0), decal_uv([0.3, 1.0, 0.7], 0));
        assert!(close(a, b), "{a:?} vs {b:?}");
        // North (−Z): moving in z must not move the uv.
        let (a, b) = (decal_uv([0.3, 0.4, 0.0], 2), decal_uv([0.3, 0.4, 0.9], 2));
        assert!(close(a, b), "{a:?} vs {b:?}");
        // …and the leak really is only that small: a *tangent* move of the
        // same size must move the coordinate by the same size.
        let far = decal_uv([0.3, 0.4, 0.9], 0);
        assert!(!close(a, far), "a tangent move must not be a no-op");
    }

    /// The top face's coordinate is exactly `(x, z)` — the one case the
    /// rotation chain reduces to the identity, so it pins the two fixed
    /// rotations without the per-face quaternion in the way.
    #[test]
    fn the_top_face_is_the_bare_xz_projection() {
        let uv = decal_uv([0.25, 1.0, 0.75], 0);
        assert!((uv[0] - 0.25).abs() < 1e-6 && (uv[1] - 0.75).abs() < 1e-6, "{uv:?}");
    }

    #[test]
    fn a_cube_state_cracks_on_all_six_faces_in_world_space() {
        let render = vec![RenderKind::Cube {
            faces: [0; 6],
            raw_faces: [0; 6],
            tint: [rewo_data::assets::TintSource::None; 6],
        }];
        let q = block_decal_quads(&render, &[], 0, [10, -3, 7]);
        assert_eq!(q.len(), 6, "no cullface test — every face is emitted");
        for quad in &q {
            for v in quad.verts {
                assert!((10.0..=11.0).contains(&v[0]), "x {v:?}");
                assert!((-3.0..=-2.0).contains(&v[1]), "y {v:?}");
                assert!((7.0..=8.0).contains(&v[2]), "z {v:?}");
            }
        }
    }

    #[test]
    fn an_invisible_state_cracks_nothing() {
        let render = vec![RenderKind::Invisible];
        assert!(block_decal_quads(&render, &[], 0, [0, 0, 0]).is_empty());
    }

    #[test]
    fn a_fluid_state_cracks_nothing() {
        let render = vec![RenderKind::Fluid {
            layer: 0,
            raw_layer: 0,
            level: 0,
            lava: false,
        }];
        assert!(block_decal_quads(&render, &[], 0, [0, 0, 0]).is_empty());
    }

    #[test]
    fn an_out_of_range_state_cracks_nothing() {
        assert!(block_decal_quads(&[], &[], 999, [0, 0, 0]).is_empty());
    }
}
