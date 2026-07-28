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

use crate::gui_item::GuiItemVertex;
use crate::held::{DisplayTransform, HeldItemModel};

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

/// `SpearAnimations.firstPersonAttack` — the `STAB` swing.
///
/// A different shape from `WHACK`, not a retuning of it: three easings run over
/// three overlapping windows of the same `attack`, and the pose is built from
/// their *differences*. `startingAmount - middleAmount` drives the wind-up and
/// `startingAmount - endingAmount` the thrust, so the spear draws back before
/// it goes forward. `outBack` overshoots past one, which is what makes the
/// middle read as a lunge rather than a slide.
fn spear_attack(arm: Arm, attack: f32) -> Mat4 {
    use crate::entities::{ease_in_out_expo, ease_in_out_sine, ease_out_back, spear_progress};
    let invert = arm.invert();
    let starting = ease_in_out_sine(spear_progress(attack, 0.0, 0.05));
    let middle = ease_out_back(spear_progress(attack, 0.05, 0.2));
    let ending = ease_in_out_expo(spear_progress(attack, 0.4, 1.0));
    translate(
        invert * 0.1 * (starting - middle),
        -0.075 * (starting - ending),
        0.65 * (starting - middle),
    ) * rot_x(-70.0 * (starting - ending))
        * translate(0.0, 0.0, -0.25 * (ending - middle))
}

/// Which swing rig an item plays — `ItemStack.getSwingAnimation().type()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwingKind {
    /// The item does not move with the arm.
    None,
    /// The ordinary sweep — every item but the spears.
    Whack,
    /// The seven spears' thrust.
    Stab,
}

/// The model-view for a **held item**, up to but not including the item's own
/// `display.firstperson_*` transform.
///
/// `attack` is `getAttackAnim(partial)`; pass 0 for a hand that is not
/// swinging. `swings` is false for an item whose `SwingAnimation` type is
/// `NONE`, which skips the swing entirely — the item stays put while the
/// player's arm animation plays out.
pub fn item_hand(arm: Arm, inverse_height: f32, attack: f32, swings: bool) -> Mat4 {
    item_hand_kind(
        arm,
        inverse_height,
        attack,
        if swings { SwingKind::Whack } else { SwingKind::None },
    )
}

/// [`item_hand`] with the rig named rather than inferred.
///
/// The three arms are vanilla's own `switch` on the swing type, and `NONE` is
/// a true no-op: the item holds still while the *player's* arm animation plays
/// out elsewhere.
pub fn item_hand_kind(arm: Arm, inverse_height: f32, attack: f32, kind: SwingKind) -> Mat4 {
    let base = item_arm_transform(arm, inverse_height);
    match kind {
        SwingKind::None => base,
        _ if attack <= 0.0 => base,
        SwingKind::Whack => base * swing_arm(arm, attack),
        SwingKind::Stab => base * spear_attack(arm, attack),
    }
}


// ---------------------------------------------------------------------------
// The use-driven poses (M38).
//
// `submitArmWithItem`'s middle branch: while the player is using an item in
// this hand, the resting pose is replaced by one keyed on the item's
// `ItemUseAnimation`. Every one of them reads `useItemRemainingTicks`, which
// M23 established the client *derives* rather than receives.
// ---------------------------------------------------------------------------

/// `ItemUseAnimation` — the rig an item plays while it is being used.
///
/// The wire ids are the enum's declared ints, and the `custom` flag below is
/// its third constructor argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseAnim {
    None,
    Eat,
    Drink,
    Block,
    Bow,
    Trident,
    Crossbow,
    Spyglass,
    TootHorn,
    Brush,
    Bundle,
    Spear,
}

impl UseAnim {
    /// `hasCustomArmTransform()` — **true only for `EAT`, `DRINK` and
    /// `SPEAR`**, and it inverts the order of two calls.
    ///
    /// For everything else the resting arm transform is applied *first* and
    /// the pose refines it. For these three it is skipped there and applied
    /// *after* the pose instead, so the pose operates in an un-offset frame.
    /// Getting the order wrong puts an eaten apple most of a block from where
    /// vanilla holds it, which looks like a bad constant rather than a
    /// misordering.
    pub fn has_custom_arm_transform(self) -> bool {
        matches!(self, UseAnim::Eat | UseAnim::Drink | UseAnim::Spear)
    }

    /// From `ItemUseAnimation`'s declared int, which is also its wire id.
    pub fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => UseAnim::None,
            1 => UseAnim::Eat,
            2 => UseAnim::Drink,
            3 => UseAnim::Block,
            4 => UseAnim::Bow,
            5 => UseAnim::Trident,
            6 => UseAnim::Crossbow,
            7 => UseAnim::Spyglass,
            8 => UseAnim::TootHorn,
            9 => UseAnim::Brush,
            10 => UseAnim::Bundle,
            11 => UseAnim::Spear,
            _ => return None,
        })
    }
}

/// What the hand needs to know about an in-progress use.
#[derive(Clone, Copy, Debug)]
pub struct UsePose {
    pub anim: UseAnim,
    /// `getUseItemRemainingTicks()`, which counts **down** and is deliberately
    /// unclamped — see `rewo_world::entities::UseState::remaining`.
    pub remaining: i32,
    /// `getUseDuration()` for the stack in hand.
    pub duration: i32,
}

impl UsePose {
    /// `timeHeld` — how long the item has been held, the rising counterpart of
    /// `remaining`.
    fn time_held(&self, partial: f32) -> f32 {
        self.duration as f32 - (self.remaining as f32 - partial + 1.0)
    }
}

/// `applyEatTransform` — the food jiggle.
///
/// Two parts. A small bob, `|cos(t/4 · π)| · 0.1`, which runs only for the
/// first four fifths of the use — `scaledUsageTime < 0.8` — and is what makes
/// eating look like chewing rather than holding. And the jiggle proper,
/// `1 - scaledUsageTime²⁷`, whose absurd exponent keeps it near **one** for
/// almost the whole use and collapses only at the very end: the item is swung
/// aside for the duration and snaps back as the last bite lands.
fn eat_transform(arm: Arm, use_pose: &UsePose, partial: f32) -> Mat4 {
    let invert = arm.invert();
    let curr = use_pose.remaining as f32 - partial + 1.0;
    let scaled = curr / use_pose.duration.max(1) as f32;
    let bob = if scaled < 0.8 {
        let h = ((curr / 4.0 * std::f32::consts::PI).cos() * 0.1).abs();
        translate(0.0, h, 0.0)
    } else {
        Mat4::IDENTITY
    };
    // `Math.pow(x, 27)` in double, which matters at the tail where the value
    // is collapsing fast.
    let jiggle = 1.0 - (scaled as f64).powf(27.0) as f32;
    bob * translate(jiggle * 0.6 * invert, jiggle * -0.5, 0.0)
        * rot_y(invert * jiggle * 90.0)
        * rot_x(jiggle * 10.0)
        * rot_z(invert * jiggle * 30.0)
}

/// `applyBrushTransform` — the sweep, which is on its **own ten-tick loop**
/// rather than the use's duration.
///
/// `remaining % 10` is the whole trick: brushing runs for as long as you hold
/// it, so the animation cannot key on progress through a fixed duration the
/// way eating does. It cycles instead.
fn brush_transform(arm: Arm, use_pose: &UsePose, partial: f32) -> Mat4 {
    let cycle = (use_pose.remaining % 10) as f32;
    let scaled = 1.0 - (cycle - partial + 1.0) / 10.0;
    let angle = -15.0 + 75.0 * (scaled * 2.0 * std::f32::consts::PI).cos();
    if arm == Arm::Right {
        translate(-0.25, 0.22, 0.35) * rot_x(-80.0) * rot_y(90.0) * rot_x(angle)
    } else {
        translate(0.1, 0.83, 0.35)
            * rot_x(-80.0)
            * rot_y(-90.0)
            * rot_x(angle)
            * translate(-0.3, 0.22, 0.35)
    }
}

/// The bow's draw — and the shake that appears once it is nearly full.
///
/// `power = (p² + 2p) / 3` over twenty ticks, clamped to one. Past 0.1 the
/// item jitters by `sin((timeHeld - 0.1) · 1.3) · (power - 0.1) · 0.004`,
/// which is four thousandths of a block: the strain, not a wobble you could
/// mistake for a bug. The z scale stretches the bow as it draws.
fn bow_transform(arm: Arm, use_pose: &UsePose, partial: f32) -> Mat4 {
    let invert = arm.invert();
    let time_held = use_pose.time_held(partial);
    let mut power = time_held / 20.0;
    power = (power * power + power * 2.0) / 3.0;
    power = power.min(1.0);
    let mut m = translate(invert * -0.2785682, 0.18344387, 0.15731531)
        * rot_x(-13.935)
        * rot_y(invert * 35.3)
        * rot_z(invert * -9.785);
    if power > 0.1 {
        let shake = ((time_held - 0.1) * 1.3).sin() * (power - 0.1);
        m *= translate(0.0, shake * 0.004, 0.0);
    }
    m * translate(0.0, 0.0, power * 0.04)
        * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.0 + power * 0.2))
        // `Axis.YN` — the *negative* Y axis, so this is a rotation the other
        // way round from every `YP` in this file.
        * rot_y(-invert * 45.0)
}

/// The shield-block pose, and the exception inside it.
///
/// `case BLOCK` applies its offsets **only when the item is not a shield** —
/// a real shield carries its own `display` transform for the context and
/// would be posed twice. Anything else with a `BLOCK` use animation gets the
/// hand-authored pose instead.
fn block_transform(arm: Arm, is_shield: bool) -> Mat4 {
    if is_shield {
        return Mat4::IDENTITY;
    }
    let invert = arm.invert();
    translate(invert * -0.14142136, 0.08, 0.14142136)
        * rot_x(-102.25)
        * rot_y(invert * 13.365)
        * rot_z(invert * 78.05)
}

/// The trident's wind-up.
fn trident_transform(arm: Arm) -> Mat4 {
    let invert = arm.invert();
    translate(invert * -0.5, 0.7, 0.1) * rot_x(-55.0) * rot_y(invert * 35.3) * rot_z(invert * -9.785)
}

/// The pose for an item being used, composed with the resting transform in
/// the order `hasCustomArmTransform` dictates.
///
/// `Spyglass` returns `None`: vanilla suppresses the whole hand while
/// scoping (`if (!player.isScoping())` guards the entire body of
/// `submitArmWithItem`), so the caller draws nothing rather than drawing
/// something in the wrong place.
pub fn use_hand(
    arm: Arm,
    inverse_height: f32,
    use_pose: &UsePose,
    partial: f32,
    is_shield: bool,
) -> Option<Mat4> {
    let base = item_arm_transform(arm, inverse_height);
    let pose = match use_pose.anim {
        UseAnim::Spyglass => return None,
        UseAnim::None | UseAnim::TootHorn | UseAnim::Bundle | UseAnim::Crossbow => Mat4::IDENTITY,
        UseAnim::Eat | UseAnim::Drink => eat_transform(arm, use_pose, partial),
        UseAnim::Block => block_transform(arm, is_shield),
        UseAnim::Bow => bow_transform(arm, use_pose, partial),
        UseAnim::Trident => trident_transform(arm),
        UseAnim::Brush => brush_transform(arm, use_pose, partial),
        // `SPEAR`'s use rig is `SpearAnimations.firstPersonUse`, which reads a
        // kinetic-hit-feedback counter the wire does not carry. Left at the
        // resting pose rather than approximated.
        UseAnim::Spear => Mat4::IDENTITY,
    };
    Some(if use_pose.anim.has_custom_arm_transform() {
        // The pose first, the resting offset after — see
        // [`UseAnim::has_custom_arm_transform`].
        pose * base
    } else {
        base * pose
    })
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

// ---------------------------------------------------------------------------
// The geometry, and the pass that draws it (M38).
// ---------------------------------------------------------------------------

/// `ModelPart.Cube`'s own 1/16 — vertices are authored in model units and
/// divided here, which is what puts the arm chain's `3.6`/`5.6` translates on
/// the same scale as the world's blocks. Measured: with this divide the arm
/// lands about 1.1 blocks below the eye and 0.7 in front; without it, ten
/// blocks away.
pub const MODEL_UNIT: f32 = 1.0 / 16.0;

/// A player skin's edge in texels — 64x64 since 1.8, which the entity pass's
/// skin pool assumes as well.
const SKIN_PX: f32 = 64.0;

/// `AvatarRenderer.renderHand`'s fixed tilt: `rightArm.zRot = 0.1`,
/// `leftArm.zRot = -0.1`, in **radians**.
///
/// Set unconditionally, on top of `arm.resetPose()` — so the first-person arm
/// is not the model's animated arm, it is the rest pose plus this one nudge.
const HAND_ARM_Z_ROT: f32 = 0.1;

/// One hand's worth of geometry request.
pub struct HandDraw<'a> {
    pub arm: Arm,
    /// The item to draw, or `None` for a bare hand.
    pub item: Option<&'a HeldItemModel>,
    /// `getAttackAnim(partial)` for this hand — 0 when it is not swinging.
    pub attack: f32,
    /// `1 - lerp(oHeight, height)`; see [`EquipHeight::inverse`].
    pub inverse_height: f32,
    /// Which rig this item's swing plays — `getSwingAnimation().type()`.
    /// `None` holds it still; the seven spears `Stab`; everything else
    /// `Whack`.
    pub swings: SwingKind,
    /// Whether this is the main hand. Vanilla draws the bare arm **only** for
    /// the main hand: an empty off-hand shows nothing.
    pub main_hand: bool,
    /// An in-progress use in *this* hand, which replaces the swing entirely —
    /// `submitArmWithItem` takes the use branch or the swing branch, never
    /// both. `None` when the player is not using this hand's item.
    pub using: Option<UsePose>,
    /// Whether the held item is a shield, which the `BLOCK` pose excepts.
    pub is_shield: bool,
}

/// The skin's placement in the hand atlas, and the arm quads to draw.
pub struct ArmGeometry {
    /// `(u0, v0, du, dv)` of the 64×64 skin within the atlas.
    pub skin_uv: [f32; 4],
    /// Whether the player model is the slim variant — the arm box is 3 px
    /// wide rather than 4, and its UVs differ.
    pub slim: bool,
}

fn quad_verts(
    out: &mut Vec<GuiItemVertex>,
    m: &Mat4,
    pos: &[[f32; 3]; 4],
    uv: &[[f32; 2]; 4],
    shade: f32,
) {
    let p: Vec<[f32; 3]> = pos
        .iter()
        .map(|v| m.transform_point3(Vec3::from_array(*v)).to_array())
        .collect();
    let v = |i: usize| GuiItemVertex {
        pos: p[i],
        uv: uv[i],
        shade,
    };
    out.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
}

/// Build one frame's hand vertices.
///
/// `view` is the rotation both hands sit under — `submitHandsWithItems`'
/// opening bob pair. `atlas` resolves an item texture index to its rect in the
/// hand atlas; `arm_geometry` is `None` when no skin is resident, which draws
/// the item but no bare arm rather than an untextured one.
pub fn build_vertices(
    view: Mat4,
    hands: &[HandDraw<'_>],
    atlas: &dyn Fn(u16) -> Option<[f32; 4]>,
    arm_geometry: Option<&ArmGeometry>,
) -> Vec<GuiItemVertex> {
    let mut out = Vec::new();
    for h in hands {
        match h.item {
            Some(model) => {
                let left = h.arm == Arm::Left;
                let display = if left { &model.first_left } else { &model.first_right };
                // A use in progress replaces the swing: vanilla's branch is
                // `if (isUsingItem() && remaining > 0 && usedHand == hand)`,
                // and the swing `switch` lives in its `else`.
                let pose = match &h.using {
                    Some(u) => match use_hand(h.arm, h.inverse_height, u, 1.0, h.is_shield) {
                        Some(m) => m,
                        // Scoping hides the hand outright.
                        None => continue,
                    },
                    None => item_hand_kind(h.arm, h.inverse_height, h.attack, h.swings),
                };
                let m = view * pose * display_transform(display, left);
                for q in &model.quads {
                    let Some(rect) = atlas(q.tex) else { continue };
                    // The item's quads are in 0..16 model units; the display
                    // transform's trailing centring expects 0..1.
                    let pos: [[f32; 3]; 4] = std::array::from_fn(|i| {
                        [
                            q.verts[i][0] * MODEL_UNIT,
                            q.verts[i][1] * MODEL_UNIT,
                            q.verts[i][2] * MODEL_UNIT,
                        ]
                    });
                    let uv: [[f32; 2]; 4] = std::array::from_fn(|i| {
                        [
                            rect[0] + q.uv[i][0] * rect[2],
                            rect[1] + q.uv[i][1] * rect[3],
                        ]
                    });
                    let shade = crate::gui_item::direction_normal(q.dir).y.mul_add(0.15, 0.85);
                    quad_verts(&mut out, &m, &pos, &uv, shade);
                }
            }
            None => {
                // `submitArmWithItem`: the bare arm is drawn for the main hand
                // only, and only when the player is not invisible.
                let Some(geo) = arm_geometry.filter(|_| h.main_hand) else {
                    continue;
                };
                let m = view * player_arm(h.arm, h.inverse_height, h.attack);
                append_arm(&mut out, &m, h.arm, geo);
            }
        }
    }
    out
}

/// The arm cuboid and its sleeve, from the player model's own mesh.
///
/// Only the one named part, which is exactly what `renderRightHand` submits —
/// not the whole model with the rest hidden.
fn append_arm(out: &mut Vec<GuiItemVertex>, m: &Mat4, arm: Arm, geo: &ArmGeometry) {
    let model = crate::mobs::player_model_for_hand(geo.slim);
    let want = match arm {
        Arm::Right => "right_arm",
        Arm::Left => "left_arm",
    };
    let Some(part_index) = model.parts.iter().position(|p| p.name == want) else {
        return;
    };
    let part = &model.parts[part_index];
    // `arm.resetPose()` then the fixed tilt — the rest pivot, no animation.
    let local = Mat4::from_translation(Vec3::new(
        part.pivot[0] * MODEL_UNIT,
        part.pivot[1] * MODEL_UNIT,
        part.pivot[2] * MODEL_UNIT,
    )) * Mat4::from_rotation_z(match arm {
        Arm::Right => HAND_ARM_Z_ROT,
        Arm::Left => -HAND_ARM_Z_ROT,
    });
    let m = *m * local;
    for q in model.quads.iter().filter(|q| q.part == part_index) {
        let pos: [[f32; 3]; 4] = std::array::from_fn(|i| {
            [
                q.pos[i][0] * MODEL_UNIT,
                q.pos[i][1] * MODEL_UNIT,
                q.pos[i][2] * MODEL_UNIT,
            ]
        });
        // **The model's UVs are in texels, not fractions.** A player arm's
        // span 16..56 of the 64-px skin, so they must be normalised before
        // being remapped into the skin's rect in the hand atlas. Treating them
        // as fractions pushes them far outside it, where the sampler clamps to
        // a transparent edge and the arm renders as nothing at all — which is
        // exactly what it did.
        let uv: [[f32; 2]; 4] = std::array::from_fn(|i| {
            [
                geo.skin_uv[0] + q.uv[i][0] / SKIN_PX * geo.skin_uv[2],
                geo.skin_uv[1] + q.uv[i][1] / SKIN_PX * geo.skin_uv[3],
            ]
        });
        quad_verts(out, &m, &pos, &uv, q.shade);
    }
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


    /// A cube of 0..16 model units, as the block bake produces.
    fn unit_cube_item(first_right: DisplayTransform) -> HeldItemModel {
        // Only the +Z face is needed to bound the shape in x and y; the eight
        // corners are what the projection is compared on, and every face draws
        // from the same eight.
        let mut quads = Vec::new();
        for (dir, verts) in [
            (5u8, [[16.0, 0.0, 0.0], [16.0, 16.0, 0.0], [16.0, 16.0, 16.0], [16.0, 0.0, 16.0]]),
            (4u8, [[0.0, 0.0, 0.0], [0.0, 16.0, 0.0], [0.0, 16.0, 16.0], [0.0, 0.0, 16.0]]),
        ] {
            quads.push(crate::held::HeldQuad {
                verts,
                uv: [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
                tex: 0,
                part: 0,
                dir,
            });
        }
        HeldItemModel {
            quads,
            right: DisplayTransform::default(),
            left: DisplayTransform::default(),
            ground: DisplayTransform::default(),
            gui: DisplayTransform::default(),
            first_right,
            first_left: DisplayTransform::default(),
            from_block: true,
            gui_quads: None,
        }
    }

    /// Where a held block lands on a 1280×720 frame, all the way through the
    /// production geometry builder and the world's own projection.
    ///
    /// The expected numbers are an independent CPU derivation: the pose from
    /// `ItemInHandRenderer`'s constants, the transform from `block/block.json`,
    /// and the projection from `calculateHudFov`'s hard-coded 70 vertical at
    /// near 0.05 — the three things M38 measured rather than assumed.
    #[test]
    fn a_held_block_lands_where_the_decompile_puts_it() {
        // `block/block.json`'s firstperson_righthand.
        let t = DisplayTransform {
            rotation: [0.0, 45.0, 0.0],
            translation: [0.0; 3],
            scale: [0.4, 0.4, 0.4],
        };
        let model = unit_cube_item(t);
        let verts = build_vertices(
            Mat4::IDENTITY,
            &[HandDraw {
                arm: Arm::Right,
                item: Some(&model),
                attack: 0.0,
                inverse_height: 0.0,
                swings: SwingKind::Whack,
                main_hand: true,
                using: None,
                is_shield: false,
            }],
            &|_| Some([0.0, 0.0, 1.0, 1.0]),
            None,
        );
        assert!(!verts.is_empty(), "the builder produced geometry");

        let (w, h) = (1280.0f32, 720.0f32);
        let proj = Mat4::from_cols_array_2d(&crate::world::perspective_reverse_z(
            70f32.to_radians(),
            w / h,
            0.05,
        ));
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for v in &verts {
            let clip = proj * glam::Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
            if clip.w <= 0.0 {
                continue;
            }
            let sx = (clip.x / clip.w + 1.0) / 2.0 * w;
            // The pass draws through a flipped viewport, so clip +y is up and
            // screen y counts down from the top.
            let sy = (1.0 - clip.y / clip.w) / 2.0 * h;
            x0 = x0.min(sx);
            x1 = x1.max(sx);
            y0 = y0.min(sy);
            y1 = y1.max(sy);
        }
        println!("built bbox: x {x0:.1}..{x1:.1}  y {y0:.1}..{y1:.1}");
        // Derived by hand from the decompile, not from this code.
        assert!((x0 - 837.9).abs() < 1.5, "left edge {x0}");
        assert!((x1 - 1298.6).abs() < 1.5, "right edge {x1}");
        assert!((y0 - 524.1).abs() < 1.5, "top edge {y0}");
        assert!((y1 - 1206.8).abs() < 1.5, "bottom edge {y1}");
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
