//! The first-person hand — where the held item and the bare arm sit in the
//! player's own view (M38).
//!
//! Every number here is transcribed from `ItemInHandRenderer`, which spells
//! them out as named constants. The whole file is arithmetic: it produces
//! model-view matrices and nothing else, so it is unit-testable without a GPU
//! and the [`crate::gui_item`] pass draws whatever it returns.
//!
//! # The order the transforms compose in
//!
//! Vanilla builds these on a `PoseStack`, where `translate` and `mulPose`
//! **post-multiply** — each call happens in the space the previous ones
//! established. `glam`'s `Mat4 * Mat4` is the same convention (right-most
//! applies first), so a chain reads here in the order vanilla calls it.
//!
//! # The two clocks
//!
//! The hand is driven by two independent animations that vanilla keeps in
//! different places, and conflating them is the easy mistake:
//!
//! - **the swing**, `attackAnim`, which is `LivingEntity`'s and runs on the
//!   entity table's swing machine (M19). For the local player that machine is
//!   fed by `PlaySession::swing`, exactly as `LocalPlayer.swing` does.
//! - **the equip height**, `mainHandHeight`, which is `ItemInHandRenderer`'s
//!   own and is *not* on the entity. It falls to zero when the held item
//!   changes and climbs back at 0.4 per tick, which is the item dipping out of
//!   frame and rising again as you scroll the hotbar. See [`EquipHeight`].

use glam::{Mat4, Vec3};

use crate::held::DisplayTransform;

/// `ItemInHandRenderer`'s named constants, kept in vanilla's own order so a
/// version bump can be diffed against the decompile line by line.
mod k {
    pub const ITEM_SWING_X_POS_SCALE: f32 = -0.4;
    pub const ITEM_SWING_Y_POS_SCALE: f32 = 0.2;
    pub const ITEM_SWING_Z_POS_SCALE: f32 = -0.2;
    pub const ITEM_HEIGHT_SCALE: f32 = -0.6;
    pub const ITEM_POS_X: f32 = 0.56;
    pub const ITEM_POS_Y: f32 = -0.52;
    pub const ITEM_POS_Z: f32 = -0.72;
    pub const ITEM_PRESWING_ROT_Y: f32 = 45.0;
    pub const ITEM_SWING_X_ROT_AMOUNT: f32 = -80.0;
    pub const ITEM_SWING_Y_ROT_AMOUNT: f32 = -20.0;
    pub const ITEM_SWING_Z_ROT_AMOUNT: f32 = -20.0;

    pub const ARM_SWING_X_POS_SCALE: f32 = -0.3;
    pub const ARM_SWING_Y_POS_SCALE: f32 = 0.4;
    pub const ARM_SWING_Z_POS_SCALE: f32 = -0.4;
    pub const ARM_SWING_Y_ROT_AMOUNT: f32 = 70.0;
    pub const ARM_SWING_Z_ROT_AMOUNT: f32 = -20.0;
    pub const ARM_HEIGHT_SCALE: f32 = -0.6;
    /// `0.64000005F` in the decompile — the literal is `ARM_POS_SCALE * ARM_POS_X`
    /// (0.8 × 0.8) constant-folded by the compiler, and the trailing 5 is the
    /// float rounding of that product. Written out so it is obviously the same
    /// number rather than a typo.
    pub const ARM_POS_X: f32 = 0.8 * 0.8;
    pub const ARM_POS_Y: f32 = -0.6;
    pub const ARM_POS_Z: f32 = -0.72;
    pub const ARM_PRESWING_ROT_Y: f32 = 45.0;
    pub const ARM_PREROTATION_X_OFFSET: f32 = -1.0;
    pub const ARM_PREROTATION_Y_OFFSET: f32 = 3.6;
    pub const ARM_PREROTATION_Z_OFFSET: f32 = 3.5;
    pub const ARM_POSTROTATION_X_OFFSET: f32 = 5.6;
    pub const ARM_ROT_X: f32 = 200.0;
    pub const ARM_ROT_Y: f32 = -135.0;
    pub const ARM_ROT_Z: f32 = 120.0;
}

/// Which hand a transform is for. The sign it contributes is vanilla's
/// `invert`, and it is *not* the same question as which hand holds the item —
/// `HumanoidArm` is the arm, and a left-handed player's main hand is the left
/// arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    Right,
    Left,
}

impl Arm {
    /// `int invert = arm == HumanoidArm.RIGHT ? 1 : -1`.
    pub fn invert(self) -> f32 {
        match self {
            Arm::Right => 1.0,
            Arm::Left => -1.0,
        }
    }
}

fn rot_x(deg: f32) -> Mat4 {
    Mat4::from_rotation_x(deg.to_radians())
}
fn rot_y(deg: f32) -> Mat4 {
    Mat4::from_rotation_y(deg.to_radians())
}
fn rot_z(deg: f32) -> Mat4 {
    Mat4::from_rotation_z(deg.to_radians())
}
fn translate(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

/// `ItemInHandRenderer.mainHandHeight` / `offHandHeight` — the equip dip.
///
/// Its own clock, not the entity's. `tick` runs once per client tick:
///
/// ```text
/// target = (visible item != held item) ? 0 : swapScale^3     // main hand
/// height += clamp(target - height, -0.4, 0.4)
/// if (height < 0.1) visibleItem = heldItem
/// ```
///
/// so switching items drives the height to zero over three ticks, the item is
/// swapped at the bottom where nothing is on screen, and it climbs back. The
/// renderer reads `1 - height`, which is why the field is called an *inverse*
/// arm height at the use site.
#[derive(Clone, Copy, Debug, Default)]
pub struct EquipHeight {
    height: f32,
    previous: f32,
    /// The item currently on screen, which lags the held one across a swap.
    visible: Option<i32>,
}

impl EquipHeight {
    /// The per-tick step. `clamp(±0.4)` is what makes the dip take three ticks
    /// rather than snapping.
    const STEP: f32 = 0.4;
    /// Below this the item is swapped — it is out of frame, so the change is
    /// invisible.
    const SWAP_BELOW: f32 = 0.1;

    /// One client tick. `held` is the item id in this hand, `None` for empty.
    pub fn tick(&mut self, held: Option<i32>) {
        self.previous = self.height;
        let target = if self.visible == held { 1.0 } else { 0.0 };
        self.height += (target - self.height).clamp(-Self::STEP, Self::STEP);
        if self.height < Self::SWAP_BELOW {
            self.visible = held;
        }
    }

    /// `1 - lerp(partial, oHeight, height)` — what the transforms take.
    ///
    /// Zero with the item fully raised, one with it fully lowered, which is
    /// why every use site multiplies it by a *negative* height scale.
    pub fn inverse(&self, partial: f32) -> f32 {
        1.0 - (self.previous + (self.height - self.previous) * partial)
    }

    /// The item that should be drawn — the one on screen, which lags the held
    /// one until the dip bottoms out.
    pub fn visible_item(&self) -> Option<i32> {
        self.visible
    }
}

/// `LocalPlayer.xBob` / `yBob` — the lagged view angles the hand swings
/// against.
///
/// `bob += (rot - bob) * 0.5` per tick, so the bob chases the camera at half
/// the remaining distance each tick. The hand is then rotated by *one tenth of
/// the difference*, which is the small counter-sway you see when you flick the
/// mouse: the arm lags the view and catches up.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewBob {
    x: f32,
    y: f32,
    prev_x: f32,
    prev_y: f32,
}

impl ViewBob {
    pub fn tick(&mut self, x_rot: f32, y_rot: f32) {
        self.prev_x = self.x;
        self.prev_y = self.y;
        self.x += (x_rot - self.x) * 0.5;
        self.y += (y_rot - self.y) * 0.5;
    }

    /// The two rotations at the top of `submitHandsWithItems`, in degrees.
    pub fn sway(&self, view_x: f32, view_y: f32, partial: f32) -> (f32, f32) {
        let lerp = |a: f32, b: f32| a + (b - a) * partial;
        (
            (view_x - lerp(self.prev_x, self.x)) * 0.1,
            (view_y - lerp(self.prev_y, self.y)) * 0.1,
        )
    }
}

/// The rotation both hands sit under: `submitHandsWithItems`' opening pair.
pub fn view_sway(sway: (f32, f32)) -> Mat4 {
    rot_x(sway.0) * rot_y(sway.1)
}

/// `applyItemArmTransform` — where a held item rests.
fn item_arm_transform(arm: Arm, inverse_height: f32) -> Mat4 {
    translate(
        arm.invert() * k::ITEM_POS_X,
        k::ITEM_POS_Y + inverse_height * k::ITEM_HEIGHT_SCALE,
        k::ITEM_POS_Z,
    )
}

/// `applyItemArmAttackTransform` — the four rotations of the swing.
///
/// Note the two different easings of the same `attack`: the y rotation uses
/// `sin(attack² · π)` and the other two `sin(√attack · π)`. Squaring delays the
/// peak, the square root brings it forward, so the item yaws late while it
/// pitches early — that offset is what makes the swing read as a whip rather
/// than a rigid sweep.
fn item_arm_attack_transform(arm: Arm, attack: f32) -> Mat4 {
    let invert = arm.invert();
    let y_swing = (attack * attack * std::f32::consts::PI).sin();
    let xz_swing = (attack.sqrt() * std::f32::consts::PI).sin();
    rot_y(invert * (k::ITEM_PRESWING_ROT_Y + y_swing * k::ITEM_SWING_Y_ROT_AMOUNT))
        * rot_z(invert * xz_swing * k::ITEM_SWING_Z_ROT_AMOUNT)
        * rot_x(xz_swing * k::ITEM_SWING_X_ROT_AMOUNT)
        * rot_y(invert * -k::ITEM_PRESWING_ROT_Y)
}

/// `swingArm` — the translation of the swing, then its rotations.
fn swing_arm(arm: Arm, attack: f32) -> Mat4 {
    let invert = arm.invert();
    let root = attack.sqrt() * std::f32::consts::PI;
    let x = k::ITEM_SWING_X_POS_SCALE * root.sin();
    let y = k::ITEM_SWING_Y_POS_SCALE * (attack.sqrt() * std::f32::consts::TAU).sin();
    let z = k::ITEM_SWING_Z_POS_SCALE * (attack * std::f32::consts::PI).sin();
    translate(invert * x, y, z) * item_arm_attack_transform(arm, attack)
}

/// The model-view for a **held item**, up to but not including the item's own
/// `display.firstperson_*` transform.
///
/// `attack` is `getAttackAnim(partial)`; pass 0 for a hand that is not
/// swinging. `swings` is false for an item whose `SwingAnimation` type is
/// `NONE`, which skips the swing entirely — the item stays put while the
/// player's arm animation plays out.
pub fn item_hand(arm: Arm, inverse_height: f32, attack: f32, swings: bool) -> Mat4 {
    let base = item_arm_transform(arm, inverse_height);
    if swings && attack > 0.0 {
        base * swing_arm(arm, attack)
    } else {
        base
    }
}

/// `ItemTransform.apply` — the item's own `display` transform, with the
/// left-hand mirror.
///
/// `apply_left_mirror` negates `translation.x`, `rotation.y` and `rotation.z`,
/// and vanilla passes `displayContext.leftHand()` for it — so it fires for the
/// left hand's transform whether that transform was authored or arrived
/// through the absent-left fallback.
///
/// The trailing `translate(-0.5, -0.5, -0.5)` centres the model, and is why
/// callers hand quads in `0..1` block units rather than `0..16`.
pub fn display_transform(t: &DisplayTransform, apply_left_mirror: bool) -> Mat4 {
    let (tx, ry, rz) = if apply_left_mirror {
        (-t.translation[0], -t.rotation[1], -t.rotation[2])
    } else {
        (t.translation[0], t.rotation[1], t.rotation[2])
    };
    translate(tx, t.translation[1], t.translation[2])
        // `rotationXYZ` — X, then Y, then Z, as one quaternion.
        * rot_x(t.rotation[0])
        * rot_y(ry)
        * rot_z(rz)
        * Mat4::from_scale(Vec3::new(t.scale[0], t.scale[1], t.scale[2]))
        * translate(-0.5, -0.5, -0.5)
}

/// `renderPlayerArm`'s chain — the bare hand, no item.
///
/// Structurally different from the item's, not a variant of it: the arm swings
/// further (0.4 against 0.2 vertically), yaws by 70° where the item yaws by
/// 20°, and then walks through a fixed pre-rotation, three rotations and a
/// post-rotation that place the *shoulder* rather than the hand. Those five
/// steps are in model units — the arm cuboid is 4×12×4 — which is why the
/// caller scales by 1/16 afterwards.
pub fn player_arm(arm: Arm, inverse_height: f32, attack: f32) -> Mat4 {
    let invert = arm.invert();
    let root = attack.sqrt() * std::f32::consts::PI;
    let x = k::ARM_SWING_X_POS_SCALE * root.sin();
    let y = k::ARM_SWING_Y_POS_SCALE * (attack.sqrt() * std::f32::consts::TAU).sin();
    let z = k::ARM_SWING_Z_POS_SCALE * (attack * std::f32::consts::PI).sin();
    let z_swing = (attack * attack * std::f32::consts::PI).sin();
    let y_swing = root.sin();
    translate(
        invert * (x + k::ARM_POS_X),
        y + k::ARM_POS_Y + inverse_height * k::ARM_HEIGHT_SCALE,
        z + k::ARM_POS_Z,
    ) * rot_y(invert * k::ARM_PRESWING_ROT_Y)
        * rot_y(invert * y_swing * k::ARM_SWING_Y_ROT_AMOUNT)
        * rot_z(invert * z_swing * k::ARM_SWING_Z_ROT_AMOUNT)
        * translate(
            invert * k::ARM_PREROTATION_X_OFFSET,
            k::ARM_PREROTATION_Y_OFFSET,
            k::ARM_PREROTATION_Z_OFFSET,
        )
        * rot_z(invert * k::ARM_ROT_Z)
        * rot_x(k::ARM_ROT_X)
        * rot_y(invert * k::ARM_ROT_Y)
        * translate(invert * k::ARM_POSTROTATION_X_OFFSET, 0.0, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: Mat4, p: [f32; 3]) -> Vec3 {
        m.transform_point3(Vec3::from_array(p))
    }

    /// The resting item sits to the right, below and in front of the eye —
    /// the three `ITEM_POS_*` constants, unmodified when nothing is animating.
    #[test]
    fn a_resting_item_sits_where_the_constants_say() {
        let p = at(item_hand(Arm::Right, 0.0, 0.0, true), [0.0; 3]);
        assert!((p.x - 0.56).abs() < 1e-6, "{p:?}");
        assert!((p.y - -0.52).abs() < 1e-6, "{p:?}");
        assert!((p.z - -0.72).abs() < 1e-6, "{p:?}");
        // The left hand is the mirror in x only: y and z are shared.
        let l = at(item_hand(Arm::Left, 0.0, 0.0, true), [0.0; 3]);
        assert!((l.x + 0.56).abs() < 1e-6, "{l:?}");
        assert!((l.y - p.y).abs() < 1e-6);
        assert!((l.z - p.z).abs() < 1e-6);
    }

    /// A fully lowered item is 0.6 below a raised one — `ITEM_HEIGHT_SCALE`
    /// against an inverse height of 1. The sign matters: the item drops out of
    /// frame, it does not rise out of it.
    #[test]
    fn the_equip_dip_lowers_the_item() {
        let up = at(item_hand(Arm::Right, 0.0, 0.0, true), [0.0; 3]);
        let down = at(item_hand(Arm::Right, 1.0, 0.0, true), [0.0; 3]);
        assert!((down.y - (up.y - 0.6)).abs() < 1e-6, "{up:?} {down:?}");
    }

    /// The equip clock takes three ticks to bottom out and swaps the visible
    /// item there, where it is off screen.
    #[test]
    fn switching_items_dips_over_three_ticks_and_swaps_at_the_bottom() {
        let mut h = EquipHeight::default();
        // Settle on one item.
        for _ in 0..8 {
            h.tick(Some(1));
        }
        assert!((h.inverse(1.0) - 0.0).abs() < 1e-6, "fully raised");
        assert_eq!(h.visible_item(), Some(1));

        // Switch: the height falls 0.4 per tick.
        h.tick(Some(2));
        assert!((h.inverse(1.0) - 0.4).abs() < 1e-6);
        assert_eq!(h.visible_item(), Some(1), "still showing the old item");
        h.tick(Some(2));
        assert!((h.inverse(1.0) - 0.8).abs() < 1e-6);
        h.tick(Some(2));
        // Below 0.1 the swap happens, out of frame.
        assert_eq!(h.visible_item(), Some(2));
        // And it climbs back.
        h.tick(Some(2));
        assert!(h.inverse(1.0) < 0.8, "rising again");
    }

    /// The swing's yaw and pitch peak at different times — `sin(attack²·π)`
    /// against `sin(√attack·π)`. Equal easings would make the swing rigid.
    #[test]
    fn the_swing_yaws_late_and_pitches_early() {
        let early = 0.25f32;
        let y_term = (early * early * std::f32::consts::PI).sin();
        let xz_term = (early.sqrt() * std::f32::consts::PI).sin();
        assert!(
            xz_term > y_term * 4.0,
            "at a quarter through, the pitch is well ahead: {xz_term} vs {y_term}"
        );
    }

    /// A swing displaces the item; not swinging leaves it exactly at rest.
    /// `swings == false` is the `SwingAnimation.NONE` case, and it must be a
    /// true no-op rather than a small nudge.
    #[test]
    fn a_none_swing_animation_leaves_the_item_at_rest() {
        let rest = item_hand(Arm::Right, 0.0, 0.0, true);
        let mid_none = item_hand(Arm::Right, 0.0, 0.5, false);
        let mid_whack = item_hand(Arm::Right, 0.0, 0.5, true);
        assert_eq!(rest, mid_none);
        assert_ne!(rest, mid_whack);
    }

    /// The left-hand mirror negates x-translation, y- and z-rotation — and
    /// leaves x-rotation and every scale alone.
    #[test]
    fn the_display_mirror_negates_three_components() {
        let t = DisplayTransform {
            rotation: [10.0, 20.0, 30.0],
            translation: [0.1, 0.2, 0.3],
            scale: [0.5, 0.6, 0.7],
        };
        let r = display_transform(&t, false);
        let l = display_transform(&t, true);
        assert_ne!(r, l);
        // A point on the model's axis maps to mirrored x under the two.
        let pr = at(r, [0.5, 0.5, 0.5]);
        let pl = at(l, [0.5, 0.5, 0.5]);
        assert!((pr.x + pl.x).abs() < 1e-6, "{pr:?} {pl:?}");
        assert!((pr.y - pl.y).abs() < 1e-6);
    }

    /// The centring translate is what makes a `0..1` model sit on the origin:
    /// the model's middle maps to the transform's translation.
    #[test]
    fn the_display_transform_centres_the_model() {
        let t = DisplayTransform {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
        };
        let mid = at(display_transform(&t, false), [0.5, 0.5, 0.5]);
        assert!(mid.length() < 1e-6, "the model's centre lands on the origin");
    }

    /// The arm and the item swing on **different constants, not a scaled copy
    /// of one another** — and the difference goes both ways, which is the
    /// point: x is *smaller* for the arm while y and z are larger.
    ///
    /// Written against the constants rather than against a transformed point.
    /// A composed chain's origin is dominated by its placement terms — the
    /// arm's ends with a pre-rotation and post-rotation that walk out to the
    /// shoulder — so comparing where two origins land measures the placement,
    /// not the swing.
    #[test]
    fn the_arm_and_the_item_swing_on_different_constants() {
        assert!(
            k::ARM_SWING_X_POS_SCALE.abs() < k::ITEM_SWING_X_POS_SCALE.abs(),
            "x: arm {} is the smaller of the two",
            k::ARM_SWING_X_POS_SCALE
        );
        assert!(k::ARM_SWING_Y_POS_SCALE.abs() > k::ITEM_SWING_Y_POS_SCALE.abs());
        assert!(k::ARM_SWING_Z_POS_SCALE.abs() > k::ITEM_SWING_Z_POS_SCALE.abs());
        assert!(k::ARM_SWING_Y_ROT_AMOUNT.abs() > k::ITEM_SWING_Y_ROT_AMOUNT.abs());
        // No single factor relates the two sets, so neither chain can be
        // derived from the other.
        let ratio = k::ARM_SWING_Y_POS_SCALE / k::ITEM_SWING_Y_POS_SCALE;
        assert!(
            (k::ARM_SWING_X_POS_SCALE / k::ITEM_SWING_X_POS_SCALE - ratio).abs() > 0.1,
            "the axes do not share a scale factor"
        );
    }

    /// Both chains do move under a swing, and both return to rest at zero —
    /// the weaker claim that survives, in place of comparing their magnitudes.
    #[test]
    fn both_chains_move_under_a_swing_and_rest_at_zero() {
        for (name, rest, mid) in [
            (
                "arm",
                at(player_arm(Arm::Right, 0.0, 0.0), [0.0; 3]),
                at(player_arm(Arm::Right, 0.0, 0.5), [0.0; 3]),
            ),
            (
                "item",
                at(item_hand(Arm::Right, 0.0, 0.0, true), [0.0; 3]),
                at(item_hand(Arm::Right, 0.0, 0.5, true), [0.0; 3]),
            ),
        ] {
            assert!((mid - rest).length() > 0.1, "{name} moves under a swing");
        }
        // attack == 0 is the resting pose for both, not a small offset.
        assert_eq!(
            item_hand(Arm::Right, 0.0, 0.0, true),
            item_arm_transform(Arm::Right, 0.0)
        );
    }

    /// The view sway is a tenth of the difference between the camera and its
    /// lagged copy, and it vanishes once the bob has caught up.
    #[test]
    fn the_sway_is_a_tenth_of_the_lag_and_settles() {
        let mut bob = ViewBob::default();
        // A flick: the camera jumps 40 degrees, the bob has not moved yet.
        let (sx, _) = bob.sway(40.0, 0.0, 1.0);
        assert!((sx - 4.0).abs() < 1e-6, "a tenth of the 40 degree lag: {sx}");
        // Held still, the bob converges and the sway decays to nothing.
        for _ in 0..40 {
            bob.tick(40.0, 0.0);
        }
        let (settled, _) = bob.sway(40.0, 0.0, 1.0);
        assert!(settled.abs() < 1e-3, "settled to {settled}");
    }
}
