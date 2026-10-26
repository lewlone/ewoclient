//! `EntityRenderer.extractShadow`: which blocks under an entity get a shadow
//! piece, and how dark.
//!
//! Vanilla walks every cell `(x, y, z)` from the entity's radius box
//! (`floor(x - r) ..= floor(x + r)`, likewise z) down from the entity's own
//! cell by `depth` (`min(pow / 0.5 - 1, r)`), where
//!
//! ```text
//! pow = (1 - distanceToCameraSq / 256) * strength     // dies off at 16 blocks
//! ```
//!
//! and for each cell looks at the block BELOW it (`pos.below()`):
//!
//! ```text
//! render shape != INVISIBLE
//! getMaxLocalRawBrightness(pos) > 3
//! isCollisionShapeFullBlock          (a slab, a path, soul sand: no shadow)
//! getShape(...) not empty            -> a piece with that shape's bounds
//! alpha = clamp((pow - (entity.y - y) * 0.5) * 0.5 * Lightmap.getBrightness(dim, brightness), 0, 1)
//! ```
//!
//! The piece sits at the bottom of cell `pos` (the top of the block below),
//! over the shape's x/z extent. `ShadowFeatureRenderer` then textures it with
//! `misc/shadow.png`, mapped so the sheet spans `[-radius, radius]` around the
//! entity (see `rewo_gpu::entities::ShadowQuad`).
//!
//! The block queries are behind [`ShadowGround`] so this is a pure function.

use crate::vignette::level_brightness;

/// One shadow piece, in coordinates relative to the entity (as vanilla's
/// `ShadowPiece` keeps them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowPiece {
    /// `(float)(pos - entity position)` of the cell (the piece's own cell, the
    /// one ABOVE the block that carries the shadow).
    pub rel: [f32; 3],
    /// The block below's `getShape` bounds `[minX, minY, minZ, maxX, maxY,
    /// maxZ]` in block-local coordinates.
    pub bounds: [f32; 6],
    pub alpha: f32,
}

/// The two world facts a shadow reads.
pub trait ShadowGround {
    /// `level.getMaxLocalRawBrightness(pos)` of the cell `(x, y, z)`.
    fn raw_brightness(&self, x: i32, y: i32, z: i32) -> i32;
    /// The block at `(x, y - 1, z)`, when it draws (`getRenderShape() !=
    /// INVISIBLE`) and its collision shape is a full block: its `getShape`
    /// bounds, `None` if the shape is empty or either test fails.
    fn ground(&self, x: i32, y: i32, z: i32) -> Option<[f32; 6]>;
}

/// The shadow-carrying shape of a block from its tables: `Some(bounds)` when
/// its collision shape is a full block (`isCollisionShapeFullBlock`: some box
/// is the whole unit cube) and its `getShape` outline is not empty. The bounds
/// are the outline's, falling back to the unit cube for a state the outline
/// table leaves empty (a full-collision block has a shape in vanilla; only the
/// table is short).
pub fn ground_shape(collision: &[[f32; 6]], outline: &[[f32; 6]]) -> Option<[f32; 6]> {
    const UNIT: [f32; 6] = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    if !collision.contains(&UNIT) {
        return None;
    }
    if outline.is_empty() {
        return Some(UNIT);
    }
    Some(outline.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |acc, b| {
            [
                acc[0].min(b[0]),
                acc[1].min(b[1]),
                acc[2].min(b[2]),
                acc[3].max(b[3]),
                acc[4].max(b[4]),
                acc[5].max(b[5]),
            ]
        },
    ))
}

/// `extractShadow` for one entity at `pos` (its interpolated feet position),
/// with the radius and strength the renderer reports, `distance_sq` from the
/// camera to the entity, and the dimension's `ambient_light`.
pub fn extract_shadow_pieces(
    pos: [f64; 3],
    radius: f32,
    strength: f32,
    distance_sq: f64,
    ambient_light: f32,
    ground: &impl ShadowGround,
) -> Vec<ShadowPiece> {
    let mut out = Vec::new();
    // `Math.min(getShadowRadius(state), 32.0F)` is the caller's; a radius of 0
    // draws nothing.
    if !(radius > 0.0) {
        return out;
    }
    // `(float)((1.0 - distSq / 256.0) * strength)`.
    let pow = ((1.0 - distance_sq / 256.0) * f64::from(strength)) as f32;
    if !(pow > 0.0) {
        return out;
    }
    let r = f64::from(radius);
    let (x0, x1) = ((pos[0] - r).floor() as i32, (pos[0] + r).floor() as i32);
    let (z0, z1) = ((pos[2] - r).floor() as i32, (pos[2] + r).floor() as i32);
    let depth = (pow / 0.5 - 1.0).min(radius);
    let (y0, y1) = (
        (pos[1] - f64::from(depth)).floor() as i32,
        pos[1].floor() as i32,
    );
    for z in z0..=z1 {
        for x in x0..=x1 {
            for y in y0..=y1 {
                let power_at_depth = pow - (pos[1] - f64::from(y)) as f32 * 0.5;
                let Some(bounds) = ground.ground(x, y, z) else {
                    continue;
                };
                let brightness = ground.raw_brightness(x, y, z);
                if brightness <= 3 {
                    continue;
                }
                let alpha = (power_at_depth * 0.5 * level_brightness(ambient_light, brightness))
                    .clamp(0.0, 1.0);
                out.push(ShadowPiece {
                    rel: [
                        (f64::from(x) - pos[0]) as f32,
                        (f64::from(y) - pos[1]) as f32,
                        (f64::from(z) - pos[2]) as f32,
                    ],
                    bounds,
                    alpha,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A world of full blocks at given cells, lit by a per-cell level
    /// (default 15 above the ground, 0 inside a block).
    struct Blocks {
        solid: Vec<(i32, i32, i32)>,
        light: HashMap<(i32, i32, i32), i32>,
    }

    impl Blocks {
        fn floor_at(y: i32, from: i32, to: i32) -> Self {
            let mut solid = Vec::new();
            for x in from..=to {
                for z in from..=to {
                    for yy in (y - 3)..=y {
                        solid.push((x, yy, z));
                    }
                }
            }
            Self {
                solid,
                light: HashMap::new(),
            }
        }
        fn is_solid(&self, p: (i32, i32, i32)) -> bool {
            self.solid.contains(&p)
        }
    }

    impl ShadowGround for Blocks {
        fn raw_brightness(&self, x: i32, y: i32, z: i32) -> i32 {
            self.light
                .get(&(x, y, z))
                .copied()
                .unwrap_or(if self.is_solid((x, y, z)) { 0 } else { 15 })
        }
        fn ground(&self, x: i32, y: i32, z: i32) -> Option<[f32; 6]> {
            self.is_solid((x, y - 1, z))
                .then_some([0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
        }
    }

    /// A player-sized entity standing on a floor whose top is y = 64: the
    /// radius box (0.5 around (0.5, 64, 0.5)) touches cells x, z in 0..=1 (the
    /// upper bound `floor(1.0)` is 1: a box edge exactly on a boundary still
    /// takes the next cell, as vanilla's loop does). With pow 1 the depth is
    /// `min(1 / 0.5 - 1, 0.5)` = 0.5, so cells y 63..=64: the cell at 64 sits
    /// on the floor (alpha 0.5: `1 * 0.5 * brightness(15) = 0.5`), the one at
    /// 63 is inside it, in the dark.
    #[test]
    fn a_player_on_a_floor_shadows_the_four_blocks_it_overlaps() {
        let w = Blocks::floor_at(63, -2, 3);
        let pieces = extract_shadow_pieces([0.5, 64.0, 0.5], 0.5, 1.0, 0.0, 0.0, &w);
        assert_eq!(pieces.len(), 4, "{pieces:?}");
        let mut cells: Vec<_> = pieces
            .iter()
            .map(|p| ((p.rel[0] + 0.5) as i32, (p.rel[2] + 0.5) as i32))
            .collect();
        cells.sort();
        // rel = cell - entity position: -0.5 for cell 0, +0.5 for cell 1.
        assert_eq!(cells, vec![(0, 0), (0, 1), (1, 0), (1, 1)]);
        for p in &pieces {
            assert_eq!(p.rel[1], 0.0, "the bottom of the cell above the floor");
            assert_eq!(p.alpha, 0.5);
            assert_eq!(p.bounds, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        }
    }

    /// The strength and the camera distance scale `pow` linearly and cut the
    /// shadow off at 16 blocks (`distSq / 256` reaching 1).
    #[test]
    fn a_shadow_fades_with_the_camera_distance_and_dies_at_sixteen_blocks() {
        let w = Blocks::floor_at(63, -2, 3);
        let at = |d2: f64, strength: f32| {
            extract_shadow_pieces([0.5, 64.0, 0.5], 0.5, strength, d2, 0.0, &w)
        };
        // distSq 64 -> pow 0.75 -> alpha 0.75 * 0.5 = 0.375.
        assert_eq!(at(64.0, 1.0)[0].alpha, 0.375);
        // An item's 0.75 strength: pow 0.75 too.
        assert_eq!(at(0.0, 0.75)[0].alpha, 0.375);
        assert!(at(256.0, 1.0).is_empty(), "pow reaches 0 at 16 blocks");
        assert!(at(300.0, 1.0).is_empty());
        assert!(extract_shadow_pieces([0.5, 64.0, 0.5], 0.0, 1.0, 0.0, 0.0, &w).is_empty());
    }

    /// A cell the light barely reaches (raw brightness 3 or less) draws
    /// nothing, and the alpha follows the dimension-curved brightness
    /// `Lightmap.getBrightness`: at level 8 with ambient 0, `v = 8/15`,
    /// `v / (4 - 3v)` = 0.2424…
    #[test]
    fn a_dim_cell_is_dark_enough_to_draw_nothing_or_less() {
        let mut w = Blocks::floor_at(63, -2, 3);
        for x in 0..=1 {
            for z in 0..=1 {
                w.light.insert((x, 64, z), 3);
            }
        }
        assert!(extract_shadow_pieces([0.5, 64.0, 0.5], 0.5, 1.0, 0.0, 0.0, &w).is_empty());
        for x in 0..=1 {
            for z in 0..=1 {
                w.light.insert((x, 64, z), 8);
            }
        }
        let p = extract_shadow_pieces([0.5, 64.0, 0.5], 0.5, 1.0, 0.0, 0.0, &w);
        let v = 8.0f32 / 15.0;
        let want = 1.0 * 0.5 * (v / (4.0 - 3.0 * v));
        assert!((p[0].alpha - want).abs() < 1e-6, "{} vs {want}", p[0].alpha);
        // The dimension's ambient light lifts the curve (the Nether's 0.1).
        let p = extract_shadow_pieces([0.5, 64.0, 0.5], 0.5, 1.0, 0.0, 0.1, &w);
        assert!(p[0].alpha > want);
    }

    /// An entity above the ground shadows the block under it only while that
    /// block's top is within `depth` (pow 1 gives 0.5): half a block up still
    /// shadows, with the alpha down by half the height (`1 - 0.5 * 0.5`), and
    /// a whole block up does not.
    #[test]
    fn height_above_the_block_weakens_then_ends_the_shadow() {
        let w = Blocks::floor_at(63, -2, 3);
        // Feet at 64.5: the cell at 64 is 0.5 below; depth 0.5 reaches cell 64
        // (floor(64.0)) and not 63.
        let p = extract_shadow_pieces([0.5, 64.5, 0.5], 0.5, 1.0, 0.0, 0.0, &w);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].rel[1], -0.5);
        // (1 - 0.5 * 0.5) * 0.5 = 0.375.
        assert_eq!(p[0].alpha, 0.375);
        // Feet at 65.5: depth 0.5 reaches only floor(65.0) = 65 and 66: the
        // blocks under cells 65 (air) hold no ground.
        assert!(extract_shadow_pieces([0.5, 65.5, 0.5], 0.5, 1.0, 0.0, 0.0, &w).is_empty());
    }

    /// Only a collision shape that IS the whole cube carries a shadow (a slab,
    /// a path, soul sand and air do not); the bounds are the outline's, and a
    /// full block whose outline the table lacks falls back to the unit cube.
    #[test]
    fn only_a_full_collision_block_carries_a_shadow() {
        const UNIT: [f32; 6] = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        assert_eq!(ground_shape(&[UNIT], &[UNIT]), Some(UNIT));
        assert_eq!(ground_shape(&[UNIT], &[]), Some(UNIT));
        assert_eq!(ground_shape(&[], &[UNIT]), None, "no collision: air, water");
        let slab = [0.0, 0.0, 0.0, 1.0, 0.5, 1.0];
        assert_eq!(ground_shape(&[slab], &[slab]), None, "a bottom slab");
        let path = [0.0, 0.0, 0.0, 1.0, 0.9375, 1.0];
        assert_eq!(ground_shape(&[path], &[UNIT]), None, "a path block");
        // The outline's own bounds, over several boxes.
        let a = [0.0, 0.0, 0.0, 0.5, 1.0, 1.0];
        let b = [0.25, 0.0, 0.0, 1.0, 1.0, 0.75];
        assert_eq!(
            ground_shape(&[UNIT], &[a, b]),
            Some([0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
        );
        let inner = [0.125, 0.0, 0.25, 0.875, 1.0, 0.75];
        assert_eq!(ground_shape(&[UNIT], &[inner]), Some(inner));
    }

    /// A column that is not a full block (`ground` answers `None`) is skipped
    /// and the neighbouring full blocks still get theirs.
    #[test]
    fn a_missing_ground_is_skipped() {
        struct Half(Blocks);
        impl ShadowGround for Half {
            fn raw_brightness(&self, x: i32, y: i32, z: i32) -> i32 {
                self.0.raw_brightness(x, y, z)
            }
            fn ground(&self, x: i32, y: i32, z: i32) -> Option<[f32; 6]> {
                if (x, z) == (0, 0) {
                    None
                } else {
                    self.0.ground(x, y, z)
                }
            }
        }
        let w = Half(Blocks::floor_at(63, -2, 3));
        let p = extract_shadow_pieces([0.5, 64.0, 0.5], 0.5, 1.0, 0.0, 0.0, &w);
        assert_eq!(p.len(), 3);
    }
}
