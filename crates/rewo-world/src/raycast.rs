//! Voxel ray traversal — what block is the player looking at?
//!
//! Amanatides–Woo DDA from the eye along the look direction, stepping one
//! block boundary at a time, testing each cell against a `solid` predicate
//! (the same `baked.solid` flag collision uses). Returns the first solid
//! block hit plus the face normal we entered through — that face is where a
//! placed block goes (`block + face`), exactly as vanilla's `BlockHitResult`.

/// A block the ray struck: its coordinates + the entered-face normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub block: [i32; 3],
    /// Unit normal of the face the ray entered through (placement side).
    pub face: [i32; 3],
    /// Distance from `origin` to the entered face, in blocks (the direction is
    /// normalized first, so this is a true distance and not a parameter).
    ///
    /// Added for M73's crosshair pick, which has to reconcile this against an
    /// entity hit: `LocalPlayer.pick` truncates the entity ray at the block
    /// hit and then compares the two distances, so "which block" is not enough
    /// — it needs "how far". **This is the distance to the block's entered
    /// face, not to its collision shape**: the traversal treats a cell as
    /// wholly solid, so a ray meeting a slab or a stair reports the face of the
    /// cell rather than the surface inside it. That approximation predates M73
    /// (it is how `target_block` has always chosen a block) and it can only
    /// make a block win a tie it would have lost, never the reverse.
    pub distance: f64,
}

/// Cast a ray from `origin` along `dir` (need not be normalized) up to
/// `max_dist` blocks. `solid(x,y,z)` reports whether a block stops the ray.
pub fn cast<F>(origin: [f64; 3], dir: [f64; 3], max_dist: f64, solid: F) -> Option<RayHit>
where
    F: Fn(i32, i32, i32) -> bool,
{
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-9 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];

    let mut block = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];
    // Origin block first — you can target the block your eye sits in.
    if solid(block[0], block[1], block[2]) {
        return Some(RayHit {
            block,
            face: [0, 0, 0],
            distance: 0.0,
        });
    }

    let mut step = [0i32; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for a in 0..3 {
        if d[a] > 0.0 {
            step[a] = 1;
            t_delta[a] = 1.0 / d[a];
            t_max[a] = ((block[a] as f64 + 1.0) - origin[a]) / d[a];
        } else if d[a] < 0.0 {
            step[a] = -1;
            t_delta[a] = 1.0 / -d[a];
            t_max[a] = (origin[a] - block[a] as f64) / -d[a];
        }
    }

    let mut t = 0.0;
    while t <= max_dist {
        // Advance to the next block across the nearest axis boundary.
        let axis = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
            0
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        t = t_max[axis];
        if t > max_dist {
            break;
        }
        block[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        let mut face = [0, 0, 0];
        face[axis] = -step[axis]; // we entered from the opposite side
        if solid(block[0], block[1], block[2]) {
            return Some(RayHit {
                block,
                face,
                distance: t,
            });
        }
    }
    None
}

/// M230 — `BlockGetter.clip` with `ClipContext.Block.OUTLINE` (and fluids
/// ignored): walk the cells the ray crosses, in order, and stop at the first
/// whose shape (`shapes(x, y, z)`, block-local `[x0,y0,z0,x1,y1,z1]` boxes)
/// the ray actually meets, reporting the true hit — the surface point's
/// distance and the face of the box it entered, so a ray grazing past a
/// button, or over a slab, goes on to what is behind it.
///
/// `VoxelShape.clip` from inside a box (the eye in a block) is a hit at the
/// start with the face opposite the ray's major axis; `inside` reports it,
/// as `BlockHitResult.isInside` does.
pub fn cast_shapes<'s, F>(
    origin: [f64; 3],
    dir: [f64; 3],
    max_dist: f64,
    shapes: F,
) -> Option<ShapeHit>
where
    F: Fn(i32, i32, i32) -> &'s [[f32; 6]],
{
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-9 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let clip = |cell: [i32; 3]| -> Option<ShapeHit> {
        let boxes = shapes(cell[0], cell[1], cell[2]);
        let mut best: Option<ShapeHit> = None;
        for b in boxes {
            let lo = [
                cell[0] as f64 + b[0] as f64,
                cell[1] as f64 + b[1] as f64,
                cell[2] as f64 + b[2] as f64,
            ];
            let hi = [
                cell[0] as f64 + b[3] as f64,
                cell[1] as f64 + b[4] as f64,
                cell[2] as f64 + b[5] as f64,
            ];
            let inside = (0..3).all(|a| origin[a] > lo[a] && origin[a] < hi[a]);
            if inside {
                let major = (0..3)
                    .max_by(|&i, &j| d[i].abs().total_cmp(&d[j].abs()))
                    .unwrap_or(1);
                let mut face = [0; 3];
                face[major] = if d[major] > 0.0 { -1 } else { 1 };
                return Some(ShapeHit {
                    hit: RayHit {
                        block: cell,
                        face,
                        distance: 0.0,
                    },
                    inside: true,
                });
            }
            // Slab method: the latest entry and the earliest exit.
            let (mut t0, mut t1, mut axis) = (0.0f64, max_dist, usize::MAX);
            let mut missed = false;
            for a in 0..3 {
                if d[a].abs() < 1e-12 {
                    if origin[a] < lo[a] || origin[a] > hi[a] {
                        missed = true;
                    }
                    continue;
                }
                let (mut ta, mut tb) = ((lo[a] - origin[a]) / d[a], (hi[a] - origin[a]) / d[a]);
                if ta > tb {
                    std::mem::swap(&mut ta, &mut tb);
                }
                if ta > t0 {
                    t0 = ta;
                    axis = a;
                }
                t1 = t1.min(tb);
            }
            if missed || t0 > t1 || axis == usize::MAX {
                continue;
            }
            if best.is_none_or(|h| t0 < h.hit.distance) {
                let mut face = [0; 3];
                face[axis] = if d[axis] > 0.0 { -1 } else { 1 };
                best = Some(ShapeHit {
                    hit: RayHit {
                        block: cell,
                        face,
                        distance: t0,
                    },
                    inside: false,
                });
            }
        }
        best
    };
    let mut block = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];
    if let Some(h) = clip(block) {
        return Some(h);
    }
    let mut step = [0i32; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for a in 0..3 {
        if d[a] > 0.0 {
            step[a] = 1;
            t_delta[a] = 1.0 / d[a];
            t_max[a] = ((block[a] as f64 + 1.0) - origin[a]) / d[a];
        } else if d[a] < 0.0 {
            step[a] = -1;
            t_delta[a] = 1.0 / -d[a];
            t_max[a] = (origin[a] - block[a] as f64) / -d[a];
        }
    }
    loop {
        let axis = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
            0
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > max_dist {
            return None;
        }
        block[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        if let Some(h) = clip(block) {
            return Some(h);
        }
    }
}

/// A [`cast_shapes`] hit: the [`RayHit`] (now the true surface point's
/// distance) and whether the ray started inside the shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeHit {
    pub hit: RayHit,
    pub inside: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    const SLAB: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
    const BUTTON: &[[f32; 6]] = &[[0.3125, 0.0, 0.375, 0.6875, 0.125, 0.625]];

    /// The true surface: a ray straight down onto a bottom slab hits its top
    /// at 0.5, not the cell's top at 1.0.
    #[test]
    fn a_ray_down_meets_a_slab_at_its_top() {
        let h = cast_shapes([0.5, 3.0, 0.5], [0.0, -1.0, 0.0], 10.0, |_, y, _| {
            if y == 0 {
                SLAB
            } else {
                &[]
            }
        })
        .unwrap();
        assert_eq!(h.hit.block, [0, 0, 0]);
        assert_eq!(h.hit.face, [0, 1, 0]);
        assert!((h.hit.distance - 2.5).abs() < 1e-9, "{}", h.hit.distance);
    }

    /// A small shape: a ray through the button's cell that misses the button
    /// goes on to the floor behind it; one at the button hits the button.
    #[test]
    fn a_ray_past_a_button_reaches_what_is_behind() {
        let world = |x: i32, y: i32, _z: i32| -> &'static [[f32; 6]] {
            match (x, y) {
                (0, 1) => BUTTON,
                (_, 0) => CUBE,
                _ => &[],
            }
        };
        // Down through the cell's corner: misses the button, hits the floor.
        let miss = cast_shapes([0.1, 3.0, 0.1], [0.0, -1.0, 0.0], 10.0, world).unwrap();
        assert_eq!(miss.hit.block, [0, 0, 0]);
        // Down through the middle: the button's top.
        let hit = cast_shapes([0.5, 3.0, 0.5], [0.0, -1.0, 0.0], 10.0, world).unwrap();
        assert_eq!(hit.hit.block, [0, 1, 0]);
        assert!((hit.hit.distance - (3.0 - 1.125)).abs() < 1e-9);
    }

    /// An eye inside a shape hits it at once, flagged inside.
    #[test]
    fn an_eye_inside_a_shape_is_an_inside_hit() {
        let h = cast_shapes([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 5.0, |_, _, _| CUBE).unwrap();
        assert!(h.inside);
        assert_eq!(h.hit.distance, 0.0);
        assert_eq!(h.hit.face, [-1, 0, 0]);
    }

    /// Reach still bounds it, and empty space misses.
    #[test]
    fn the_shape_cast_respects_reach() {
        let wall = |x: i32, _: i32, _: i32| -> &'static [[f32; 6]] {
            if x >= 5 {
                CUBE
            } else {
                &[]
            }
        };
        assert!(cast_shapes([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 3.0, wall).is_none());
        assert!(cast_shapes([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 10.0, wall).is_some());
    }

    #[test]
    fn ray_down_hits_the_floor_from_above() {
        // Solid everything at y <= 0.
        let hit = cast([0.5, 5.0, 0.5], [0.0, -1.0, 0.0], 10.0, |_, y, _| y <= 0);
        let h = hit.expect("should hit the floor");
        assert_eq!(h.block, [0, 0, 0]);
        assert_eq!(h.face, [0, 1, 0], "entered through the top face");
    }

    #[test]
    fn ray_forward_hits_a_wall_and_reports_the_near_face() {
        // Solid wall at x >= 3.
        let hit = cast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 10.0, |x, _, _| x >= 3);
        let h = hit.expect("should hit the wall");
        assert_eq!(h.block, [3, 0, 0]);
        assert_eq!(h.face, [-1, 0, 0], "entered through the −X face");
    }

    #[test]
    fn ray_into_empty_space_misses() {
        assert!(cast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 4.0, |_, _, _| false).is_none());
    }

    #[test]
    fn respects_max_distance() {
        // Wall is 5 away but reach is 3 → miss.
        assert!(cast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 3.0, |x, _, _| x >= 5).is_none());
    }

    #[test]
    fn the_hit_distance_is_measured_along_the_normalized_direction() {
        // A wall three blocks east of an eye at x=0.5: the entered face is the
        // plane x=3, so the distance is 2.5 — and it must not depend on the
        // length of the direction vector the caller passed.
        for scale in [1.0, 7.0] {
            let h = cast([0.5, 0.5, 0.5], [scale, 0.0, 0.0], 10.0, |x, _, _| x >= 3).unwrap();
            assert!(
                (h.distance - 2.5).abs() < 1e-9,
                "scale {scale}: {}",
                h.distance
            );
        }
        // Standing inside a solid cell is distance zero, not the first
        // boundary crossing.
        let inside = cast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 10.0, |_, _, _| true).unwrap();
        assert_eq!(inside.distance, 0.0);
    }

    #[test]
    fn diagonal_ray_hits_the_expected_face() {
        // Solid at y<=0; a ray angled mostly down-and-forward enters via top.
        let hit = cast([0.5, 3.0, 0.5], [0.3, -1.0, 0.0], 10.0, |_, y, _| y <= 0);
        assert_eq!(hit.unwrap().face, [0, 1, 0]);
    }
}
