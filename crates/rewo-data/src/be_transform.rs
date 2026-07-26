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

    /// `Direction.getStepX()` / `getStepZ()` — the horizontal unit step.
    pub fn step_xz(self) -> (f32, f32) {
        match self {
            Facing6::North => (0.0, -1.0),
            Facing6::South => (0.0, 1.0),
            Facing6::West => (-1.0, 0.0),
            Facing6::East => (1.0, 0.0),
            Facing6::Up | Facing6::Down => (0.0, 0.0),
        }
    }

    /// `Direction.getOpposite()`.
    pub fn opposite(self) -> Self {
        match self {
            Facing6::Down => Facing6::Up,
            Facing6::Up => Facing6::Down,
            Facing6::North => Facing6::South,
            Facing6::South => Facing6::North,
            Facing6::West => Facing6::East,
            Facing6::East => Facing6::West,
        }
    }

    /// `Direction.toYRot()`, for the four horizontals. Up and down have no
    /// yaw; vanilla throws there, and a skull is never wall-mounted to one.
    pub fn to_y_rot(self) -> f32 {
        match self {
            Facing6::South => 0.0,
            Facing6::West => 90.0,
            Facing6::North => 180.0,
            Facing6::East => 270.0,
            Facing6::Up | Facing6::Down => 0.0,
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

/// `BannerRenderer.modelTransformation(angle)`:
///
/// ```text
/// MODEL_SCALE       = (0.6666667, -0.6666667, -0.6666667)
/// MODEL_TRANSLATION = (0.5, 0.0, 0.5)
/// new Transformation(MODEL_TRANSLATION, YP.rotationDegrees(-angle), MODEL_SCALE, null)
/// ```
///
/// A `Transformation(t, left, s, right)` is `T · Rleft · S · Rright`, so the
/// scale runs **inside** the rotation.
///
/// Two thirds, not one: a banner's model is 2/3 scale, which is why its 44-px
/// pole fits a two-block-tall banner rather than overshooting it. And the y
/// and z scales are negative — a banner is another entity-authored model, like
/// a skull and unlike a chest.
///
/// A standing banner passes `RotationSegment.convertToDegrees(rotation)` (16
/// steps) and a wall banner passes `direction.toYRot()` — note the wall case
/// is the facing's OWN yaw, not its opposite, which is the reverse of a
/// skull's.
pub fn banner(angle_deg: f32) -> Affine {
    let m = translation(0.5, 0.0, 0.5);
    let m = mul(&m, &rot_y(-angle_deg));
    mul(&m, &scale(0.666_666_7, -0.666_666_7, -0.666_666_7))
}

/// `DecoratedPotRenderer.createModelTransformation`:
///
/// ```text
/// new Matrix4f().rotateAround(YP.rotationDegrees(180 - dir.toYRot()), 0.5, 0.5, 0.5)
/// ```
///
/// About the block **centre** in all three axes, not the floor — unlike a
/// chest, which turns about `(0.5, 0, 0.5)`.
pub fn decorated_pot(facing_y_rot: f32) -> Affine {
    rot_y_around(180.0 - facing_y_rot, 0.5, 0.5, 0.5)
}

/// The four side poses of `createSidesLayer`, in the order the `sherds` list
/// stores them: **back, left, right, front**.
///
/// ```text
/// back  offsetAndRotation(15, 16,  1, 0,     0,     PI)
/// left  offsetAndRotation( 1, 16,  1, 0, -PI/2,     PI)
/// right offsetAndRotation(15, 16, 15, 0,  PI/2,     PI)
/// front offsetAndRotation( 1, 16, 15, PI,    0,      0)
/// ```
///
/// Each is a pose in **model px**; the emitter's `part_transform` applies it,
/// which is why a pot needs no new draw machinery beyond emitting five draws
/// instead of one. Note the front's rotation is about X while the other three
/// are about Z — the plane is built with only its north face, so each side is
/// turned to point outward by a different route.
pub const POT_SIDE_ORDER: [&str; 4] = ["back", "left", "right", "front"];

/// The pose of pot side `i`, indexed by [`POT_SIDE_ORDER`].
pub fn pot_side(i: usize) -> Affine {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (off, rot) = match i {
        0 => ([15.0, 16.0, 1.0], [0.0, 0.0, PI]),
        1 => ([1.0, 16.0, 1.0], [0.0, -FRAC_PI_2, PI]),
        2 => ([15.0, 16.0, 15.0], [0.0, FRAC_PI_2, PI]),
        _ => ([1.0, 16.0, 15.0], [PI, 0.0, 0.0]),
    };
    mul(
        &translation(off[0], off[1], off[2]),
        &rot_xyz(rot[0], rot[1], rot[2]),
    )
}

/// `ConduitRenderer.submit`'s inactive branch — `translate(0.5, 0.5, 0.5)`
/// then a Y rotation.
///
/// The conduit hangs in the middle of its block rather than standing on the
/// floor, which is why this is a plain centre translate with no flip: the
/// shell model is already symmetric about its own origin.
pub fn conduit(rotation_deg: f32) -> Affine {
    mul(&translation(0.5, 0.5, 0.5), &rot_y(rotation_deg))
}

/// `SkullBlockRenderer.createGroundTransformation(segment)`:
///
/// ```text
/// new Matrix4f().translation(0.5F, 0.0F, 0.5F)
///               .rotate(YP.rotationDegrees(-RotationSegment.convertToDegrees(segment)))
///               .scale(-1.0F, -1.0F, 1.0F)
/// ```
///
/// `convertToDegrees` is `segment * 360 / 16` — the same 16-step rotation a
/// standing sign uses.
///
/// The trailing `scale(-1, -1, 1)` is why the skull models are authored the
/// *entity* way up: `SkullModelBase` is a mob model, y-down, and this is what
/// rights it. A chest has no such flip, which is the one thing not to carry
/// across between the two families.
pub fn skull_ground(segment: i32) -> Affine {
    let m = translation(0.5, 0.0, 0.5);
    let m = mul(&m, &rot_y(-(segment as f32) * 360.0 / 16.0));
    mul(&m, &scale(-1.0, -1.0, 1.0))
}

/// `SkullBlockRenderer.createWallTransformation(direction)`:
///
/// ```text
/// new Transformation(
///    new Vector3f(0.5F - dir.getStepX() * 0.25F, 0.25F, 0.5F - dir.getStepZ() * 0.25F),
///    Axis.YP.rotationDegrees(-dir.getOpposite().toYRot()),
///    new Vector3f(-1.0F, -1.0F, 1.0F),
///    null)
/// ```
///
/// A `Transformation(t, left, s, right)` composes as `T · Rleft · S · Rright`.
/// The quarter-block step pushes the skull off the wall it hangs on, and the
/// **opposite** direction's yaw is what turns its face outward — using the
/// facing itself puts the back of the head to the room.
pub fn skull_wall(facing: Facing6) -> Affine {
    let (sx, sz) = facing.step_xz();
    let m = translation(0.5 - sx * 0.25, 0.25, 0.5 - sz * 0.25);
    let m = mul(&m, &rot_y(-facing.opposite().to_y_rot()));
    mul(&m, &scale(-1.0, -1.0, 1.0))
}

// ------------------------------------------------- animated part transforms
//
// A block-entity model's *animated group* gets its own transform, in model px
// and **relative to the group's pose pivot** — the emitter applies it to
// `vertex - pivot`. That is what `ModelPart.render` does: translate by the
// pose offset, rotate, then draw coordinates that are already relative to it.
//
// M25d made the block-level transform a matrix because a chest and a shulker
// box do not agree on a shape. The part level turns out to have the same
// problem one layer down, for the same reason: a chest's lid **rotates about a
// fixed hinge** and a shulker box's lid **slides while it spins**, and a scalar
// "openness" can only express the first. So this is a matrix too, and the
// emitter has no per-type branch at either level.

/// `ChestModel.setupAnim`'s lid angle, in radians.
///
/// ```text
/// open = 1.0F - open;
/// open = 1.0F - open * open * open;
/// lid.xRot = -(open * (float)(Math.PI / 2));
/// ```
///
/// A cubic ease-out, not a ramp: the chest is already 87.5% open half way
/// through its ten ticks, then settles.
pub fn chest_lid_angle(openness: f32) -> f32 {
    let inv = 1.0 - openness;
    let eased = 1.0 - inv * inv * inv;
    -(eased * std::f32::consts::FRAC_PI_2)
}

/// The chest lid group's transform — a pure `xRot` about the hinge.
///
/// The lid and its lock share this: `PartPose.offset(0, 9, 1)` is both their
/// pose offset and their pivot, which is why they are one group.
pub fn chest_lid(openness: f32) -> Affine {
    let (s, c) = chest_lid_angle(openness).sin_cos();
    // xRot: y and z turn, x is the axis. No translation — a hinge does not
    // move, which is exactly what distinguishes it from the shulker lid.
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, c, -s, 0.0],
        [0.0, s, c, 0.0],
    ]
}

/// `ShulkerBoxRenderer.ShulkerBoxModel.setupAnim(progress)`:
///
/// ```text
/// lid.setPos(0.0F, 24.0F - progress * 0.5F * 16.0F, 0.0F);
/// lid.yRot = 270.0F * progress * (float)(Math.PI / 180.0);
/// ```
///
/// Two channels at once, which is the whole reason this level needed a matrix.
/// The rest pose is `(0, 24, 0)` — so at `progress = 0` this is the identity,
/// and a shut box is the baked geometry untouched, exactly rather than
/// approximately.
///
/// The lid travels **8 model px** (`0.5 * 16`), half a block, and spins
/// three-quarters of a turn on the way. In the box's own space that is `-y`;
/// the renderer's trailing `scale(1, -1, -1)` is what turns it into the lid
/// lifting off the base in the world.
pub fn shulker_lid(progress: f32) -> Affine {
    const REST_Y: f32 = 24.0;
    const MAX_LID_HEIGHT: f32 = 0.5;
    const MAX_LID_ROTATION_DEG: f32 = 270.0;
    let y = REST_Y - progress * MAX_LID_HEIGHT * 16.0;
    // `T(setPos) · Ry(yRot)`, applied to pivot-relative coordinates. The pivot
    // the emitter subtracts is the rest pose, so the translation this puts back
    // is the *moved* position — the difference between the two is the slide.
    mul(&translation(0.0, y, 0.0), &rot_y(MAX_LID_ROTATION_DEG * progress))
}

/// A group that does not animate — the identity, put back at its pivot.
///
/// Every static box uses this, and so does an animated group at rest.
pub fn part_at_rest(pivot: [f32; 3]) -> Affine {
    translation(pivot[0], pivot[1], pivot[2])
}

/// The chest lid group's full transform, ready for the emitter: the hinge
/// rotation, then back to the pivot.
pub fn chest_lid_part(openness: f32, pivot: [f32; 3]) -> Affine {
    mul(&part_at_rest(pivot), &chest_lid(openness))
}

/// The shulker lid group's full transform. Unlike the chest's, the translation
/// is already inside [`shulker_lid`] — `setPos` *replaces* the pose offset
/// rather than adding to it, so composing with the pivot again would move the
/// lid twice.
pub fn shulker_lid_part(progress: f32) -> Affine {
    shulker_lid(progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shut_shulker_lid_is_the_pose_offset_itself() {
        // Not "close to" — exactly. `setPos(0, 24 - 0*8, 0)` is the (0,24,0)
        // the bake already applied, so a closed box must be the baked geometry
        // untouched, or every shut box in the world moves a hair on load.
        assert_eq!(apply(&shulker_lid(0.0), [0.0, 0.0, 0.0]), [0.0, 24.0, 0.0]);
        assert_eq!(apply(&shulker_lid(0.0), [3.0, -5.0, 7.0]), [3.0, 19.0, 7.0]);
    }

    #[test]
    fn the_shulker_lid_slides_eight_px_and_spins_three_quarters() {
        let o = apply(&shulker_lid(1.0), [0.0, 0.0, 0.0]);
        assert!((o[1] - 16.0).abs() < 1e-5, "{o:?}");
        // 270 degrees about +Y takes +x to +z.
        let x = apply(&shulker_lid(1.0), [1.0, 0.0, 0.0]);
        assert!((x[0] - o[0]).abs() < 1e-5 && (x[2] - o[2] - 1.0).abs() < 1e-5, "{x:?}");
    }

    #[test]
    fn a_chest_hinge_does_not_translate() {
        // The whole difference from the shulker lid: a hinge turns in place.
        // Its pivot is the only fixed point, and it stays fixed at any angle.
        for openness in [0.0, 0.25, 0.5, 1.0] {
            let m = chest_lid_part(openness, [0.0, 9.0, 1.0]);
            let at_pivot = apply(&m, [0.0, 0.0, 0.0]);
            assert!(
                (at_pivot[0]).abs() < 1e-6
                    && (at_pivot[1] - 9.0).abs() < 1e-6
                    && (at_pivot[2] - 1.0).abs() < 1e-6,
                "openness {openness}: pivot moved to {at_pivot:?}"
            );
        }
    }

    #[test]
    fn the_chest_lid_angle_is_a_cubic_ease_not_a_ramp() {
        assert_eq!(chest_lid_angle(0.0), 0.0);
        assert!((chest_lid_angle(1.0) + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        // 87.5% of the way open at the halfway point: 1-(1-0.5)^3 = 0.875.
        let half = chest_lid_angle(0.5) / -std::f32::consts::FRAC_PI_2;
        assert!((half - 0.875).abs() < 1e-6, "{half}");
    }

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
