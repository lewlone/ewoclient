//! Block-entity `Transformation`s — vanilla's `poseStack.mulPose(...)` per
//! renderer, as row-major 3×4 affine matrices in **block units** (M25d).
//!
//! Each `BlockEntityRenderer` pushes one transform before its model. They do
//! not agree on a shape — a chest rotates about the block centre, a shulker box
//! runs a translate-scale-rotate-flip chain that ends up y-down — so they are
//! built here rather than reduced to a facing angle the renderer could not
//! express.
//!
//! JOML's `Matrix4f` methods **post-multiply** (`M = M · X`), so a chain reads
//! left to right in construction order and a *point* is transformed by the
//! rightmost factor first. Getting that backwards is the classic way to place a
//! model somewhere plausible but wrong.

/// A row-major 3×4 affine transform: `[[m00,m01,m02,tx], …]`.
pub type Affine = [[f32; 4]; 3];

/// The identity.
pub const IDENTITY: Affine = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// `a · b` — apply `b` to a point first, then `a`.
pub fn mul(a: &Affine, b: &Affine) -> Affine {
    let mut out = [[0f32; 4]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate().take(3) {
            *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
        row[3] = a[r][0] * b[0][3] + a[r][1] * b[1][3] + a[r][2] * b[2][3] + a[r][3];
    }
    out
}

pub fn translation(x: f32, y: f32, z: f32) -> Affine {
    [
        [1.0, 0.0, 0.0, x],
        [0.0, 1.0, 0.0, y],
        [0.0, 0.0, 1.0, z],
    ]
}

pub fn scale(x: f32, y: f32, z: f32) -> Affine {
    [
        [x, 0.0, 0.0, 0.0],
        [0.0, y, 0.0, 0.0],
        [0.0, 0.0, z, 0.0],
    ]
}

/// A rotation about +Y by `deg` degrees — `Axis.YP.rotationDegrees`.
pub fn rot_y(deg: f32) -> Affine {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [c, 0.0, s, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [-s, 0.0, c, 0.0],
    ]
}

/// `Matrix4f.rotationAround(quaternion, ox, oy, oz)` for a Y rotation —
/// `T(o) · R · T(-o)`.
pub fn rot_y_around(deg: f32, ox: f32, oy: f32, oz: f32) -> Affine {
    mul(
        &translation(ox, oy, oz),
        &mul(&rot_y(deg), &translation(-ox, -oy, -oz)),
    )
}

/// `ChestRenderer.createModelTransformation` —
/// `rotationAround(YP.rotationDegrees(-facing.toYRot()), 0.5, 0, 0.5)`.
pub fn chest(facing_y_rot: f32) -> Affine {
    rot_y_around(-facing_y_rot, 0.5, 0.0, 0.5)
}

/// The six `Direction`s, for the block entities whose facing is not just
/// horizontal (a shulker box can point at the ceiling).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing6 {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Facing6 {
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "down" => Facing6::Down,
            "up" => Facing6::Up,
            "north" => Facing6::North,
            "south" => Facing6::South,
            "west" => Facing6::West,
            "east" => Facing6::East,
            _ => return None,
        })
    }

    /// `Direction.getRotation()` — the quaternion that takes the **up** axis to
    /// this direction, as a rotation matrix.
    ///
    /// Vanilla builds these from axis-angle pairs:
    ///
    /// ```text
    /// DOWN  = new Quaternionf().rotationX(PI)
    /// UP    = identity
    /// NORTH = rotationXYZ(PI/2, 0, PI)
    /// SOUTH = rotationX(PI/2)
    /// WEST  = rotationXYZ(PI/2, 0, PI/2)
    /// EAST  = rotationXYZ(PI/2, 0, -PI/2)
    /// ```
    pub fn rotation(self) -> Affine {
        use std::f32::consts::{FRAC_PI_2, PI};
        match self {
            Facing6::Down => rot_xyz(PI, 0.0, 0.0),
            Facing6::Up => IDENTITY,
            Facing6::North => rot_xyz(FRAC_PI_2, 0.0, PI),
            Facing6::South => rot_xyz(FRAC_PI_2, 0.0, 0.0),
            Facing6::West => rot_xyz(FRAC_PI_2, 0.0, FRAC_PI_2),
            Facing6::East => rot_xyz(FRAC_PI_2, 0.0, -FRAC_PI_2),
        }
    }
}

/// JOML `Quaternionf.rotationXYZ(x, y, z)` as a matrix — X first, then Y, then
/// Z (`M = Rz · Ry · Rx`), the same convention the model parts use.
pub fn rot_xyz(x: f32, y: f32, z: f32) -> Affine {
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    let rx: Affine = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cx, -sx, 0.0],
        [0.0, sx, cx, 0.0],
    ];
    let ry: Affine = [
        [cy, 0.0, sy, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [-sy, 0.0, cy, 0.0],
    ];
    let rz: Affine = [
        [cz, -sz, 0.0, 0.0],
        [sz, cz, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    mul(&rz, &mul(&ry, &rx))
}

/// `ShulkerBoxRenderer.createModelTransform`:
///
/// ```text
/// new Matrix4f()
///    .translation(0.5F, 0.5F, 0.5F)
///    .scale(0.9995F, 0.9995F, 0.9995F)
///    .rotate(direction.getRotation())
///    .scale(1.0F, -1.0F, -1.0F)
///    .translate(0.0F, -1.0F, 0.0F)
/// ```
///
/// The trailing `scale(1, -1, -1)` is why a shulker box's model is written
/// upside down (its lid box sits at negative y): the flip puts it the right way
/// up. The `0.9995` is a hair of shrink so a box against a wall does not
/// z-fight with it.
pub fn shulker_box(facing: Facing6) -> Affine {
    let m = translation(0.5, 0.5, 0.5);
    let m = mul(&m, &scale(0.9995, 0.9995, 0.9995));
    let m = mul(&m, &facing.rotation());
    let m = mul(&m, &scale(1.0, -1.0, -1.0));
    mul(&m, &translation(0.0, -1.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Affine, p: [f32; 3]) -> [f32; 3] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn multiplication_applies_the_right_factor_first() {
        // `mul(T, S)` must scale then translate — the JOML chaining order.
        let m = mul(&translation(1.0, 0.0, 0.0), &scale(2.0, 2.0, 2.0));
        assert!(near(apply(&m, [1.0, 0.0, 0.0]), [3.0, 0.0, 0.0]));
    }

    #[test]
    fn the_chest_rotation_pivots_on_the_block_centre() {
        // A corner of the block must land on another corner, never outside.
        for deg in [0.0f32, 90.0, 180.0, 270.0] {
            let m = chest(deg);
            for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [1.0, 1.0, 0.0]] {
                let q = apply(&m, p);
                assert!(
                    (-1e-5..=1.0 + 1e-5).contains(&q[0])
                        && (-1e-5..=1.0 + 1e-5).contains(&q[2]),
                    "{deg} deg sent {p:?} to {q:?}"
                );
            }
        }
    }

    #[test]
    fn an_upward_shulker_box_fills_its_block() {
        // The model spans y 8..24 px in part space, i.e. 0.5..1.5 blocks; the
        // transform's translate(0,-1,0) + y-flip + translate(0.5) is what maps
        // that onto 0..1.
        let m = shulker_box(Facing6::Up);
        let lo = apply(&m, [-0.5, 0.5, -0.5]);
        let hi = apply(&m, [0.5, 1.5, 0.5]);
        let (ylo, yhi) = (lo[1].min(hi[1]), lo[1].max(hi[1]));
        assert!(ylo > -0.01 && yhi < 1.01, "y {ylo}..{yhi} left the block");
    }

    #[test]
    fn every_facing_keeps_the_box_inside_the_block() {
        for f in [
            Facing6::Down,
            Facing6::Up,
            Facing6::North,
            Facing6::South,
            Facing6::West,
            Facing6::East,
        ] {
            let m = shulker_box(f);
            for &x in &[-0.5f32, 0.5] {
                for &y in &[0.5f32, 1.5] {
                    for &z in &[-0.5f32, 0.5] {
                        let q = apply(&m, [x, y, z]);
                        for k in 0..3 {
                            assert!(
                                (-0.01..=1.01).contains(&q[k]),
                                "{f:?} sent ({x},{y},{z}) to {q:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
