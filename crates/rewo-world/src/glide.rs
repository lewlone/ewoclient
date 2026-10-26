//! A gliding (elytra) entity's render inputs (M402).
//!
//! Arithmetic over entity state, kept beside [`crate::cape`] for the same
//! reason: no GPU and no model, so the renderer turns these numbers into
//! rotations and the gates grade them directly.
//!
//! * [`speed_value`] — `HumanoidMobRenderer.extractHumanoidRenderState`'s
//!   `speedValue`, which divides `HumanoidModel`'s walk swing;
//! * [`flying_y_rot`] — `AvatarRenderer.extractFlightData`'s `flyingYRot`,
//!   the turn between where a glider looks and where it moves.
//!
//! * [`ElytraAnimation`] — `ElytraAnimationState` (M403), the wings'
//!   angles, eased 0.3 a tick toward folded, crouched or spread.
//!
//! The body tilt's scale is [`crate::cape::fall_flying_scale`], which the
//! cape already reads.

/// `HumanoidMobRenderer.extractHumanoidRenderState`'s `speedValue` while
/// fall flying: `(|v|² / 0.2)³`, never below 1. Outside a glide it is 1.
///
/// `v` is `entity.getDeltaMovement()` (this tick's, not interpolated).
pub fn speed_value(v: [f64; 3]) -> f32 {
    let s = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as f32 / 0.2;
    (s * (s * s)).max(1.0)
}

/// `Entity.calculateViewVector(xRot, yRot)` (degrees).
pub fn view_vector(pitch_deg: f32, yaw_deg: f32) -> [f64; 3] {
    let f = pitch_deg * (std::f32::consts::PI / 180.0);
    let g = -yaw_deg * (std::f32::consts::PI / 180.0);
    let (cf, sf) = (f.cos(), f.sin());
    let (cg, sg) = (g.cos(), g.sin());
    [(sg * cf) as f64, -sf as f64, (cg * cf) as f64]
}

/// `AvatarRenderer.extractFlightData`'s `flyingYRot` (radians), or `None`
/// where vanilla leaves `shouldApplyFlyingYRot` false: the movement or the
/// look has no horizontal part (`horizontalDistanceSqr() <= 1e-5`).
///
/// `movement` is `deltaMovementOnPreviousTick().lerp(getDeltaMovement(), a)`
/// and `look` is `getViewVector(a)`. The angle is `signum(cross) ·
/// acos(min(1, |dot|))` of the two horizontal directions, so it never
/// exceeds π/2: a glider moving backwards turns no further than sideways.
pub fn flying_y_rot(movement: [f64; 3], look: [f64; 3]) -> Option<f32> {
    let hm = movement[0] * movement[0] + movement[2] * movement[2];
    let hl = look[0] * look[0] + look[2] * look[2];
    if hm <= 1.0e-5 || hl <= 1.0e-5 {
        return None;
    }
    let (m, l) = (hm.sqrt(), hl.sqrt());
    let dot = movement[0] / m * (look[0] / l) + movement[2] / m * (look[2] / l);
    let sign = movement[0] * look[2] - movement[2] * look[0];
    let signum = if sign > 0.0 {
        1.0
    } else if sign < 0.0 {
        -1.0
    } else {
        0.0
    };
    Some((signum * dot.abs().min(1.0).acos()) as f32)
}

/// M403 — `ElytraAnimationState`: the left wing's `xRot`, `yRot`, `zRot`
/// (radians; the right wing mirrors `yRot` and `zRot`), and the previous
/// tick's for the frame's lerp. Every `LivingEntity` keeps one and ticks it
/// at the end of `aiStep`, wearing an elytra or not, so the wings open from
/// wherever they were when one is put on. A new entity's angles are 0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElytraAnimation {
    rot: [f32; 3],
    rot_old: [f32; 3],
}

impl ElytraAnimation {
    /// `ElytraAnimationState.tick`: the target is spread while fall flying
    /// (folding back toward the rest pose as the dive steepens, by
    /// `1 − (−dir.y)^1.5`), raised and swept while crouching, else at rest;
    /// each angle closes 0.3 of its gap.
    pub fn tick(&mut self, fall_flying: bool, crouching: bool, movement: [f64; 3]) {
        use std::f32::consts::PI;
        self.rot_old = self.rot;
        let target = if fall_flying {
            let mut ratio = 1.0f32;
            if movement[1] < 0.0 {
                let len = (movement[0] * movement[0]
                    + movement[1] * movement[1]
                    + movement[2] * movement[2])
                    .sqrt();
                // `Vec3.normalize()` returns ZERO below 1e-5.
                let y = if len < 1.0e-5 { 0.0 } else { movement[1] / len };
                ratio = 1.0 - (-y).powf(1.5) as f32;
            }
            let lerp = |a: f32, b: f32| a + ratio * (b - a);
            [lerp(PI / 12.0, PI / 9.0), 0.0, lerp(-PI / 12.0, -PI / 2.0)]
        } else if crouching {
            [PI * 2.0 / 9.0, 0.087_266_46, -PI / 4.0]
        } else {
            [PI / 12.0, 0.0, -PI / 12.0]
        };
        for (r, t) in self.rot.iter_mut().zip(target) {
            *r += (t - *r) * 0.3;
        }
    }

    /// `getRotX/Y/Z(partialTicks)`: `Mth.lerp(a, old, current)`.
    pub fn at(&self, alpha: f32) -> [f32; 3] {
        std::array::from_fn(|i| self.rot_old[i] + alpha * (self.rot[i] - self.rot_old[i]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    /// Slow or still, the swing is untouched; at the glide's usual 1.5
    /// blocks a tick it is divided by `(2.25 / 0.2)³` ≈ 1424.
    #[test]
    fn the_walk_swing_divides_only_when_fast() {
        assert_eq!(speed_value([0.0; 3]), 1.0);
        assert_eq!(speed_value([0.3, 0.0, 0.0]), 1.0);
        let s = speed_value([1.5, 0.0, 0.0]);
        assert!((s - (2.25f32 / 0.2).powi(3)).abs() < 1e-2, "{s}");
    }

    /// Yaw 0 looks south (+z), 90 west (−x); pitch 90 looks straight down.
    #[test]
    fn the_view_vector_is_vanillas() {
        let s = view_vector(0.0, 0.0);
        assert!(s[0].abs() < 1e-6 && (s[2] - 1.0).abs() < 1e-6);
        let w = view_vector(0.0, 90.0);
        assert!((w[0] + 1.0).abs() < 1e-6 && w[2].abs() < 1e-6);
        let d = view_vector(90.0, 0.0);
        assert!((d[1] + 1.0).abs() < 1e-6);
    }

    /// Moving where it looks, no turn; moving east while looking south, a
    /// quarter turn whose sign is the cross product's; backwards folds onto
    /// 0 (the `|dot|`); no horizontal movement, no turn at all.
    #[test]
    fn the_flying_turn_is_vanillas() {
        let south = view_vector(0.0, 0.0);
        assert_eq!(flying_y_rot([0.0, -0.1, 1.0], south), Some(0.0));
        let east = flying_y_rot([1.0, 0.0, 0.0], south).unwrap();
        assert!((east - FRAC_PI_2).abs() < 1e-6, "{east}");
        let west = flying_y_rot([-1.0, 0.0, 0.0], south).unwrap();
        assert!((west + FRAC_PI_2).abs() < 1e-6, "{west}");
        let back = flying_y_rot([0.0, 0.0, -1.0], south).unwrap();
        assert!(back.abs() < 1e-6, "{back}");
        assert_eq!(flying_y_rot([0.0, -1.0, 0.001], south), None);
        assert_eq!(flying_y_rot([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]), None);
    }

    /// M403 — at rest the wings ease to `(π/12, 0, −π/12)`; gliding level
    /// they spread to `(π/9, 0, −π/2)`; diving straight down they stay
    /// folded; crouching they sweep up; and the frame lerps from the last
    /// tick's angles.
    #[test]
    fn the_wings_ease_as_elytra_animation_state() {
        use std::f32::consts::PI;
        let settle = |ff: bool, cr: bool, v: [f64; 3]| {
            let mut a = ElytraAnimation::default();
            for _ in 0..200 {
                a.tick(ff, cr, v);
            }
            a.at(1.0)
        };
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(near(
            settle(false, false, [0.0; 3]),
            [PI / 12.0, 0.0, -PI / 12.0]
        ));
        assert!(near(
            settle(true, false, [1.0, 0.0, 0.0]),
            [PI / 9.0, 0.0, -PI / 2.0]
        ));
        assert!(near(
            settle(true, false, [0.0, -1.0, 0.0]),
            [PI / 12.0, 0.0, -PI / 12.0]
        ));
        assert!(near(
            settle(false, true, [0.0; 3]),
            [PI * 2.0 / 9.0, 0.087_266_46, -PI / 4.0]
        ));
        let mut a = ElytraAnimation::default();
        a.tick(false, false, [0.0; 3]);
        let first = a.at(1.0)[0];
        assert!((first - 0.3 * PI / 12.0).abs() < 1e-6);
        assert!((a.at(0.5)[0] - first / 2.0).abs() < 1e-6);
    }
}
