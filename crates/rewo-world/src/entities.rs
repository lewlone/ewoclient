//! Entity table — id → transform, with vanilla-style interpolation and a
//! UUID → player-name map (Player Info) for nametags.
//!
//! Movement model (decompiled `Entity.lerpTo` semantics): packets set an
//! authoritative **target** — absolute from add/teleport/position-sync,
//! delta-accumulated from the move packets (the client-side mirror of
//! `VecDeltaCodec`: deltas apply to the last transmitted position, so
//! quantization can't drift). Each 20 Hz tick the rendered position steps
//! `(target - cur) / steps_left` toward it (3-step lerp, converging exactly
//! on the third tick); frames blend `prev → cur` by the partial-tick alpha.

use std::collections::HashMap;

use rewo_data::swing_anim::{SwingAnimation, SwingAnimationType};
use rewo_data::use_item::{ItemUseAnimation, UseProfile};

/// Vanilla's interpolation step count for tracked entities.
const LERP_STEPS: u32 = 3;

/// `net.minecraft.world.InteractionHand`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InteractionHand {
    /// The enum's first constant, and what `getUsedItemHand()` returns when the
    /// flags byte's bit 2 is clear.
    #[default]
    MainHand,
    OffHand,
}

/// `net.minecraft.world.entity.HumanoidArm`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HumanoidArm {
    Left,
    #[default]
    Right,
}

impl HumanoidArm {
    /// `HumanoidArm.getOpposite()`.
    pub const fn opposite(self) -> Self {
        match self {
            HumanoidArm::Left => HumanoidArm::Right,
            HumanoidArm::Right => HumanoidArm::Left,
        }
    }
}

/// One hand's item, reduced to what the swing needs: the item id (diagnostics
/// + equality) and the resolved `minecraft:swing_animation`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HeldItem {
    /// Item registry protocol id.
    pub item_id: i32,
    /// `ItemStack.getSwingAnimation()` — the prototype value from the item id,
    /// or a `DataComponentPatch` override when the server sent one.
    pub swing: SwingAnimation,
    /// `getUseDuration()` / `getUseAnimation()`, resolved from the item id
    /// (M23). Drives the client-side use clock and the eight use-driven arm
    /// poses.
    pub use_profile: UseProfile,
    /// `ItemStack.hasFoil()` — whether this stack draws an enchantment glint
    /// (M45). Like `charged`, it exists only in the patch, so it is decoded at
    /// the wire and carried rather than derived from the item id.
    pub glint: bool,
    /// `CrossbowItem.isCharged(stack)` — the patch's
    /// `minecraft:charged_projectiles` list is non-empty. The sole gate on
    /// `ArmPose::CrossbowHold`, and the one held-item property that comes from
    /// the *stack* rather than the item id.
    pub charged: bool,
}

/// What a hand holds, including the case where the wire said something this
/// client cannot resolve exactly.
///
/// The third arm is the point. An item id outside the registry, or a component
/// patch holding a codec this client does not transcribe, leaves
/// `getSwingAnimation()` genuinely unknowable. Substituting the bare default —
/// or the item's prototype — would put a *wrong* animation on screen and call
/// it right. `Unknown` instead suppresses the combat pose (and CEM's
/// `swing_progress`) for that entity until an exact equipment update repairs
/// it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HandItem {
    /// `ItemStack.EMPTY` — also the state of a hand no packet ever mentioned,
    /// which is exact: `getItemBySlot` on a fresh entity is EMPTY, and
    /// `EMPTY.getSwingAnimation()` is `SwingAnimation.DEFAULT` (its components
    /// map is empty, so `getOrDefault` returns the default).
    #[default]
    Empty,
    Held(HeldItem),
    /// The wire carried something unresolvable; see the type docs.
    Unknown,
}

impl HandItem {
    /// `getItemInHand(hand).getSwingAnimation()`, or `None` when unknowable.
    pub fn swing(self) -> Option<SwingAnimation> {
        match self {
            HandItem::Empty => Some(SwingAnimation::DEFAULT),
            HandItem::Held(i) => Some(i.swing),
            HandItem::Unknown => None,
        }
    }

    pub fn held(self) -> Option<HeldItem> {
        match self {
            HandItem::Held(i) => Some(i),
            _ => None,
        }
    }

    /// `getItemInHand(hand)`'s use profile, or `None` when unknowable.
    ///
    /// The empty hand answers [`UseProfile::UNUSABLE`], which is exact:
    /// `ItemStack.EMPTY` carries no components, so `Item.getUseDuration`'s
    /// final `else` returns 0 and `getUseAnimation`'s returns `NONE`.
    pub fn use_profile(self) -> Option<UseProfile> {
        match self {
            HandItem::Empty => Some(UseProfile::UNUSABLE),
            HandItem::Held(i) => Some(i.use_profile),
            HandItem::Unknown => None,
        }
    }

    /// `CrossbowItem.isCharged(getItemInHand(hand))`. An empty hand holds no
    /// crossbow, and an unknowable stack must not be claimed charged.
    pub fn is_charged(self) -> bool {
        matches!(self, HandItem::Held(i) if i.charged)
    }

    /// The item id, for `ItemStack.isSameItem` — which compares only the item,
    /// not the components. `None` covers both the empty stack (`Items.AIR`) and
    /// the unresolvable one, which `updatingUsingItem` must treat as "not the
    /// same as what I started using": that is the direction that stops use
    /// rather than inventing a countdown.
    pub fn same_item_key(self) -> Option<i32> {
        match self {
            HandItem::Held(i) => Some(i.item_id),
            _ => None,
        }
    }

    pub fn is_unknown(self) -> bool {
        matches!(self, HandItem::Unknown)
    }
}

/// The three mob effects `LivingEntity.getCurrentSwingDuration()` consults.
///
/// `MobEffectUtil.hasDigSpeed` is `HASTE || CONDUIT_POWER`;
/// `getDigSpeedAmplification` is `max(hasteAmp, conduitAmp)`; the else-branch
/// reads `MINING_FATIGUE`'s amplifier. Client-side `hasEffect` is a plain
/// `activeEffects.containsKey` — `tickClient` only counts the duration down and
/// never removes, so membership changes **only** on an update/remove packet.
/// That is why these are stored as bare amplifiers with no expiry clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwingEffect {
    Haste,
    ConduitPower,
    MiningFatigue,
}

/// Amplifiers of the effects that change a swing's duration; `None` = the
/// entity does not have that effect (client `hasEffect` == false).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SwingEffects {
    haste: Option<i32>,
    conduit_power: Option<i32>,
    mining_fatigue: Option<i32>,
}

impl SwingEffects {
    fn slot(&mut self, effect: SwingEffect) -> &mut Option<i32> {
        match effect {
            SwingEffect::Haste => &mut self.haste,
            SwingEffect::ConduitPower => &mut self.conduit_power,
            SwingEffect::MiningFatigue => &mut self.mining_fatigue,
        }
    }

    fn is_empty(&self) -> bool {
        *self == SwingEffects::default()
    }

    /// The exact tail of `LivingEntity.getCurrentSwingDuration()`:
    ///
    /// ```text
    /// if (MobEffectUtil.hasDigSpeed(this))
    ///     return d - (1 + MobEffectUtil.getDigSpeedAmplification(this));
    /// return hasEffect(MINING_FATIGUE) ? d + (1 + amp) * 2 : d;
    /// ```
    ///
    /// Dig speed wins outright — a hasted *and* fatigued entity gets only the
    /// shortening. No clamp: vanilla will happily produce a non-positive
    /// duration under enough haste, and `attackAnim = swingTime / duration`
    /// then divides by it.
    ///
    /// Wrapping arithmetic throughout: the amplifier arrives as an unbounded
    /// wire VarInt and Java `int` math wraps rather than trapping, so a debug
    /// build must not abort where vanilla would simply overflow.
    fn adjust(&self, base: i32) -> i32 {
        match (self.haste, self.conduit_power) {
            (None, None) => match self.mining_fatigue {
                Some(amp) => base.wrapping_add(1i32.wrapping_add(amp).wrapping_mul(2)),
                None => base,
            },
            (haste, conduit) => {
                base.wrapping_sub(1i32.wrapping_add(haste.unwrap_or(0).max(conduit.unwrap_or(0))))
            }
        }
    }
}

/// `LivingEntity`'s swing fields — the exact combat-swing state machine
/// (`swing` / `updateSwingTime` / `getAttackAnim`), driven by
/// `ClientboundAnimatePacket` actions 0 and 3.
#[derive(Clone, Copy, Debug)]
struct SwingState {
    /// `LivingEntity.swinging`.
    swinging: bool,
    /// `LivingEntity.swingTime`. `-1` right after an accepted swing: the first
    /// `updateSwingTime` increments it to 0 before dividing.
    swing_time: i32,
    /// `LivingEntity.swingingArm` (`null` before the first swing).
    swinging_arm: Option<InteractionHand>,
    /// `LivingEntity.attackAnim` — `swingTime / currentSwingDuration`.
    attack_anim: f32,
    /// `LivingEntity.oAttackAnim`, snapshotted at the top of `baseTick`.
    o_attack_anim: f32,
    /// Whether this entity's **client** class runs `updateSwingTime`.
    ///
    /// It is not universal: on the client only `Player.aiStep`,
    /// `Monster.aiStep`, `RemotePlayer.tick` and `Mannequin.tick` call it, so a
    /// cow or a hoglin can be sent `swing()` (via `Mob.doHurtTarget`) and its
    /// `attackAnim` still never advances. The caller supplies the answer from
    /// `rewo_data::entity_types::EntityClasses::ticks_swing`, whose name table
    /// is machine-extracted from exactly those call sites plus the decompiled
    /// `extends` graph — so this is vanilla's whole set, not the subset Rewo
    /// happens to pose (OptiFine CEM publishes `swing_progress` for every mob).
    ticks_swing: bool,
}

impl SwingState {
    fn new(ticks_swing: bool) -> Self {
        SwingState {
            swinging: false,
            swing_time: 0,
            swinging_arm: None,
            attack_anim: 0.0,
            o_attack_anim: 0.0,
            ticks_swing,
        }
    }

    /// `LivingEntity.getAttackAnim(partialTicks)` — the wrap makes the
    /// end-of-swing step (5/6 → 0) interpolate forward through 1.0 instead of
    /// snapping backwards.
    fn attack_anim(&self, partial: f32) -> f32 {
        let mut diff = self.attack_anim - self.o_attack_anim;
        if diff < 0.0 {
            diff += 1.0;
        }
        self.o_attack_anim + diff * partial
    }
}

/// A model-visible entity-event animation (`ClientboundEntityEventPacket`).
///
/// Vanilla routes each event byte through the concrete entity class'
/// `handleEntityEvent`, where an id like 4 means "attack" on a warden and
/// something entirely different (or nothing) on any other entity — so the
/// *(kind, id)* pair, not the id alone, names the animation. Only the three
/// model-visible ones are represented here; every other event byte is an
/// unmodelled status (hurt flash, particles, sound) that this client ignores.
///
/// Each is a one-shot `AnimationState`: the wire carries no time, so the
/// client stamps the receipt tick and the renderer measures elapsed from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntityEvent {
    /// Warden id 4 — `attackAnimationState.start(tickCount)` (also stops the
    /// roar; the render layer drops the roar while this plays).
    WardenAttack,
    /// Warden id 62 — `sonicBoomAnimationState.start(tickCount)`.
    WardenSonicBoom,
    /// Armadillo id 64 — restarts the peek from tick 0 even while the
    /// metadata-driven SCARED/balled state (which normally holds the peek at
    /// its end pose) persists.
    ArmadilloPeek,
    /// Warden id 61 — `tendrilAnimation = 10` (M57). Not a keyframe rig: a
    /// 10-tick countdown decremented once per client tick, which drives both
    /// `WardenModel.animateTendrils`' sway and the tendril emissive layer's
    /// alpha (`getTendrilAnimation` = `lerp(a, prev, cur) / 10`).
    WardenTendril,
}

impl EntityEvent {
    /// Dense count for the fixed-size per-entity store.
    pub const COUNT: usize = 4;

    /// Slot index into [`EntityEventStarts`].
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Per-entity receipt ticks of the active model-visible events. `None` = the
/// event has never fired for this entity (so its rig contributes nothing).
type EntityEventStarts = [Option<i64>; EntityEvent::COUNT];

/// Vanilla `Allay` dance loop length (`DANCING_LOOP_DURATION`) — the
/// spin-then-sway period in ticks.
const ALLAY_DANCING_LOOP: f32 = 55.0;
/// Vanilla `Allay` spin sub-window / progress denominator
/// (`SPINNING_ANIMATION_DURATION`).
const ALLAY_SPINNING_DURATION: f32 = 15.0;

/// Client-side Allay dance state — the exact counters `Allay.tick()` runs on
/// the client (`dancingAnimationTicks` / `spinningAnimationTicks` /
/// `spinningAnimationTicks0`) plus the `DATA_DANCING` metadata flag that gates
/// them. These are NOT derivable from a single receipt tick (unlike the M17
/// entity events): `spinning_ticks` ramps *up* while spinning and *down* while
/// not, clamped to `0..=15`, so its per-tick history is load-bearing. The
/// model reads `isSpinning()` (a step function of `dancing_ticks`) and
/// `getSpinningProgress()` (the interpolated `spinning_ticks`).
#[derive(Clone, Copy, Debug, Default)]
struct AllayDance {
    /// `Allay.isDancing()` = the `DATA_DANCING` (index-16 BOOLEAN) metadata.
    dancing: bool,
    /// `dancingAnimationTicks` — increments each dancing tick, 0 otherwise;
    /// `isSpinning()` = `dancing_ticks % 55 < 15`. Held as f32 to mirror
    /// vanilla's float modulo exactly (it only ever holds integers).
    dancing_ticks: f32,
    /// `spinningAnimationTicks` — ramps toward 15 while spinning, toward 0
    /// otherwise, clamped `0..=15`.
    spinning_ticks: f32,
    /// `spinningAnimationTicks0` — the previous tick's `spinning_ticks`, the
    /// lerp base for `getSpinningProgress(partial)`.
    spinning_ticks0: f32,
}

impl AllayDance {
    /// `dancing_ticks % 55 < 15` — vanilla `Allay.isSpinning()`, read at render
    /// from the current (post-tick) counter.
    fn is_spinning(&self) -> bool {
        self.dancing_ticks.rem_euclid(ALLAY_DANCING_LOOP) < ALLAY_SPINNING_DURATION
    }

    /// Advance one client tick — a verbatim port of the client branch of
    /// `Allay.tick()`. `dancing_ticks` increments *before* `is_spinning()`
    /// reads it (vanilla's statement order), and `spinning_ticks0` snapshots
    /// the previous value before the ramp.
    fn tick(&mut self) {
        if self.dancing {
            self.dancing_ticks += 1.0;
            self.spinning_ticks0 = self.spinning_ticks;
            if self.is_spinning() {
                self.spinning_ticks += 1.0;
            } else {
                self.spinning_ticks -= 1.0;
            }
            self.spinning_ticks = self.spinning_ticks.clamp(0.0, ALLAY_SPINNING_DURATION);
        } else {
            self.dancing_ticks = 0.0;
            self.spinning_ticks = 0.0;
            self.spinning_ticks0 = 0.0;
        }
    }

    /// `getSpinningProgress(partial)` = `lerp(partial, ticks0, ticks) / 15`.
    fn spinning_progress(&self, alpha: f32) -> f32 {
        let a = alpha.clamp(0.0, 1.0);
        (self.spinning_ticks0 + (self.spinning_ticks - self.spinning_ticks0) * a)
            / ALLAY_SPINNING_DURATION
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EntityState {
    pub uuid: u128,
    pub type_id: i32,
    /// Authoritative synced target position (see module docs).
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Head yaw (degrees) — the server points it at nearby players, so the
    /// model can turn its head toward the viewer. Defaults to the body yaw.
    pub head_yaw: f32,
    lerp_steps: u32,
    cur: [f64; 3],
    prev: [f64; 3],
    /// Walk-cycle phase (vanilla `animationPosition`) — advances by
    /// `limb_amount` each tick, so a still entity's limbs freeze.
    limb_swing: f32,
    /// Smoothed horizontal speed 0..1 (vanilla `animationSpeed`) — the
    /// swing amplitude. The server never sends limb angles; both are
    /// derived here from the entity's own motion, exactly as vanilla does.
    limb_amount: f32,
    /// The cape's lagging anchor (M60) — vanilla `ClientAvatarState`'s
    /// cloak position. Carried by every entity rather than only players
    /// because [`EntityState`] is one `Copy` struct and a side map keyed by
    /// id would have to be ticked in lockstep with this one anyway; six
    /// doubles is cheaper than that coupling. Only the cape reads it.
    cloak: CloakAnchor,
    /// `LivingEntity.fallFlyTicks` — `if (isFallFlying()) ++ else = 0`, once
    /// per tick, off shared flag 7. The client simulates it (the server
    /// sends only the flag), and the cape's `fallFlyingScale` is its only
    /// consumer.
    fall_fly_ticks: i32,
}

/// Vanilla `ClientAvatarState`'s cloak position — the lagging anchor the
/// cape's angles are measured against.
///
/// It is *not* the entity's position: it chases it at a quarter of the
/// remaining distance per tick, so a player who starts running leaves their
/// cloak behind and the gap is what lifts the cape. `O` is the previous
/// tick's value, for render interpolation.
///
/// All six start at **zero**, exactly as vanilla's fields do. That is not a
/// neutral choice — it means a player spawning within 10 blocks of the world
/// origin on an axis has their cloak converge onto it from 0 over several
/// ticks instead of snapping — but it is what vanilla does, and the snap
/// branch is written against the *position*, not against a first-tick flag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CloakAnchor {
    x: f64,
    y: f64,
    z: f64,
    xo: f64,
    yo: f64,
    zo: f64,
}

impl EntityState {
    pub fn new(uuid: u128, type_id: i32, x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Self {
        Self {
            uuid,
            type_id,
            x,
            y,
            z,
            yaw,
            pitch,
            head_yaw: yaw,
            lerp_steps: 0,
            cur: [x, y, z],
            prev: [x, y, z],
            limb_swing: 0.0,
            limb_amount: 0.0,
            cloak: CloakAnchor::default(),
            fall_fly_ticks: 0,
        }
    }

    pub fn set_head_yaw(&mut self, yaw: f32) {
        self.head_yaw = yaw;
    }

    /// Absolute target (teleport / position sync): start a fresh 3-tick lerp.
    pub fn set_target(&mut self, x: f64, y: f64, z: f64) {
        self.x = x;
        self.y = y;
        self.z = z;
        self.lerp_steps = LERP_STEPS;
    }

    /// Relative move (the short-delta packets): accumulate onto the synced
    /// target, never onto the rendered position.
    pub fn nudge(&mut self, dx: f64, dy: f64, dz: f64) {
        self.set_target(self.x + dx, self.y + dy, self.z + dz);
    }

    pub fn set_rot(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
    }

    fn tick(&mut self, fall_flying: bool) {
        let before = self.cur;
        // `AbstractClientPlayer.tick` runs `clientAvatarState.tick(position(),
        // …)` **before** `super.tick()`, so the cloak chases the position the
        // entity had entering this tick — the same value that becomes `xo`,
        // which is what the render then lerps `xo → getX()` against.
        self.cloak.move_cloak(before);
        // `LivingEntity.aiStep`'s tail.
        if fall_flying {
            self.fall_fly_ticks += 1;
        } else {
            self.fall_fly_ticks = 0;
        }
        self.prev = self.cur;
        if self.lerp_steps > 0 {
            let n = self.lerp_steps as f64;
            self.cur[0] += (self.x - self.cur[0]) / n;
            self.cur[1] += (self.y - self.cur[1]) / n;
            self.cur[2] += (self.z - self.cur[2]) / n;
            self.lerp_steps -= 1;
        }
        // Walk animation from this tick's horizontal displacement (vanilla
        // LivingEntity.aiStep): target = min(1, dist·4), smoothed by 0.4,
        // phase advances by the smoothed amount.
        let dx = self.cur[0] - before[0];
        let dz = self.cur[2] - before[2];
        let target = ((dx * dx + dz * dz).sqrt() as f32 * 4.0).min(1.0);
        self.limb_amount += (target - self.limb_amount) * 0.4;
        self.limb_swing += self.limb_amount;
    }

    /// Overwrite **this tick's** position without touching `prev` or the
    /// synced target — `Entity.setPos` as `positionRider` calls it (M72).
    ///
    /// `prev` is deliberately left alone: `tickPassenger` runs
    /// `setOldPosAndRot()` (Rewo's `prev = cur`, already done by
    /// [`Self::tick`]) *before* `rideTick()` reaches `positionRider`, so the
    /// render lerp blends the last tick's derived position into this one. The
    /// synced target is left alone too, because it stays authoritative for the
    /// moment the rider dismounts.
    fn set_derived_pos(&mut self, p: [f64; 3]) {
        self.cur = p;
    }

    /// Frame position: last tick's `prev` blended toward `cur` by the
    /// partial-tick alpha (0..1).
    pub fn render_pos(&self, alpha: f32) -> [f64; 3] {
        let a = alpha.clamp(0.0, 1.0) as f64;
        [
            self.prev[0] + (self.cur[0] - self.prev[0]) * a,
            self.prev[1] + (self.cur[1] - self.prev[1]) * a,
            self.prev[2] + (self.cur[2] - self.prev[2]) * a,
        ]
    }

    /// `WalkAnimationState.setSpeed` — a direct assignment that bypasses the
    /// smoothing. `handleDamageEvent` calls it with 1.5, which is *above* the
    /// 1.0 the movement target can reach, so a hurt entity's limbs kick past
    /// anything walking produces and then decay back through `update`.
    pub fn set_limb_speed(&mut self, speed: f32) {
        self.limb_amount = speed;
    }

    /// Walk-cycle phase + amplitude for the model's limb swing.
    ///
    /// The amplitude is clamped to 1.0 because vanilla's
    /// `WalkAnimationState.speed(partialTicks)` is
    /// `Math.min(Mth.lerp(...), 1.0F)`. Movement alone can never exceed it
    /// (the target is already `min(1, dist*4)` and the lerp cannot overshoot),
    /// so this is a no-op for walking and only bites after
    /// [`Self::set_limb_speed`] pushes it to 1.5.
    pub fn limb(&self) -> (f32, f32) {
        (self.limb_swing, self.limb_amount.min(1.0))
    }

    /// `ClientAvatarState.getInterpolatedCloak{X,Y,Z}(partialTicks)`.
    pub fn cloak_pos(&self, alpha: f32) -> [f64; 3] {
        self.cloak.interpolated(alpha)
    }

    /// `LivingEntity.getFallFlyingTicks()`.
    pub fn fall_fly_ticks(&self) -> i32 {
        self.fall_fly_ticks
    }
}

impl CloakAnchor {
    /// `ClientAvatarState.moveCloak`, transcribed per-axis.
    ///
    /// Each axis independently either eases a quarter of the way toward the
    /// position, or — past a 10-block gap — teleports, **rewriting `O` too**
    /// so the render interpolation does not draw the cape streaking across
    /// the intervening ground on the frame a player teleports.
    ///
    /// The threshold is exclusive on both sides: vanilla writes
    /// `if (!(d > 10.0) && !(d < -10.0))`, so a gap of exactly ±10 eases.
    fn move_cloak(&mut self, pos: [f64; 3]) {
        self.xo = self.x;
        self.yo = self.y;
        self.zo = self.z;
        let d = [pos[0] - self.x, pos[1] - self.y, pos[2] - self.z];
        let cur = [&mut self.x, &mut self.y, &mut self.z];
        let old = [&mut self.xo, &mut self.yo, &mut self.zo];
        for (i, (c, o)) in cur.into_iter().zip(old).enumerate() {
            if !(d[i] > 10.0) && !(d[i] < -10.0) {
                *c += d[i] * 0.25;
            } else {
                *c = pos[i];
                *o = *c;
            }
        }
    }

    fn interpolated(&self, alpha: f32) -> [f64; 3] {
        let a = alpha.clamp(0.0, 1.0) as f64;
        [
            self.xo + (self.x - self.xo) * a,
            self.yo + (self.y - self.yo) * a,
            self.zo + (self.z - self.zo) * a,
        ]
    }
}

/// One worn armour piece: the item, and the dye its component patch carried.
///
/// The dye rides here rather than being looked up later because it exists only
/// in the stack's `DataComponentPatch` — the equipment packet is the one place
/// it is ever seen, and by the time a frame is drawn the stack is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WornPiece {
    pub item: i32,
    /// `minecraft:dyed_color`'s RGB, absent on an undyed piece.
    pub dye: Option<i32>,
    /// `minecraft:trim`'s `(material, pattern)` registry ids (M48). Here for
    /// the same reason the dye is: the component patch on the equipment packet
    /// is the only place either is ever seen.
    pub trim: Option<(i32, i32)>,
    /// `ItemStack.hasFoil()` — whether this piece wears an enchantment glint
    /// (M50). Third component-patch fact riding the same packet, for the same
    /// reason: the stack is gone by the time a frame is drawn.
    pub foil: bool,
}

#[derive(Default)]
pub struct EntityTable {
    map: HashMap<i32, EntityState>,
    /// Player Info: profile UUID → name. Populated before the player's
    /// `add_entity` arrives; survives entity unload (list membership, not
    /// entity lifetime).
    names: HashMap<u128, String>,
    /// Custom names from entity metadata, keyed by entity id (a named mob
    /// shows this above its model). Cleared on entity removal.
    custom_names: HashMap<i32, String>,
    /// Entity pose ordinals from metadata index 6 (warden roar, frog
    /// croak, breeze shoot…). Cleared on entity removal.
    poses: HashMap<i32, u8>,
    /// Mob-specific gesture states (sniffer/armadillo enum ordinals at
    /// metadata index 17). Cleared on entity removal.
    gesture_states: HashMap<i32, u8>,
    /// Slime / magma-cube size (metadata index 16). Cleared on removal.
    sizes: HashMap<i32, i32>,
    /// Baby flag (index 16 BOOLEAN, ageable / zombie mobs). Cleared on removal.
    babies: std::collections::HashSet<i32>,
    /// Item-use state (index 8 BYTE, `DATA_LIVING_ENTITY_FLAGS`) — M23. An
    /// entry exists only once the flags byte has been seen, which is exact: a
    /// `LivingEntity` that never sent it has the `0` default, i.e. not using.
    /// Cleared on removal, so a reused id cannot inherit a use clock.
    uses: HashMap<i32, UseState>,
    /// Death state (index 9 FLOAT health, plus entity-event 3) — M24. Absent
    /// means `DeathState::ALIVE`, which is exact: `entityData.define` seeds
    /// health at 1.0. Cleared on removal.
    deaths: HashMap<i32, DeathState>,
    /// `ClientboundUpdateAttributesPacket` snapshots — M57. Absent means the
    /// server has synced nothing for this entity, which is **not** the same as
    /// "the defaults apply": that distinction is what
    /// [`crate::attributes::resolve`] returns as
    /// [`crate::attributes::Source`], so a caller can never mistake an
    /// untracked entity for a healthy one. Cleared on removal, so a reused id
    /// cannot inherit the previous occupant's max health.
    attributes: HashMap<i32, crate::attributes::EntityAttributes>,
    /// `Entity.DATA_SHARED_FLAGS_ID` — metadata index **0**, BYTE (M59).
    ///
    /// Absent means the byte has never been sent, which is exact: `Entity`
    /// seeds it at `(byte) 0`, so every flag reads false either way. No kind
    /// gate on the way in — index 0 is `Entity`'s own first slot, so every
    /// entity that exists owns it. Cleared on removal.
    shared_flags: HashMap<i32, u8>,
    /// `Entity.DATA_CUSTOM_NAME_VISIBLE` — metadata index **3**, BOOLEAN
    /// (M70). Absent means the flag has never been sent, which is exact:
    /// `defineSynchedData` seeds it `false`. Membership means `true`; the
    /// router removes the id when the server sends `false`, so a toggle back
    /// off is not a latch. Cleared on removal.
    ///
    /// The index is pinned by counting `Entity`'s own `defineId` calls in
    /// declaration order — 0 shared flags, 1 air supply, 2 custom name,
    /// **3 custom-name-visible**, 4 silent, 5 no-gravity, 6 pose, 7 ticks
    /// frozen — the same argument that pins `DATA_POSE` to 6, which is
    /// independently confirmed by the working pose decode.
    custom_name_visible: std::collections::HashSet<i32>,
    /// `ClientboundSetPassengersPacket` — vehicle id → its passengers, in wire
    /// order (M70). Present-and-empty and absent both mean "nothing is
    /// riding"; the packet is the only writer, so an entity never seen as a
    /// vehicle simply has no entry.
    ///
    /// This exists for `Entity.isVehicle()`, which is `!passengers.isEmpty()`
    /// — **something is riding this entity**, not the reverse. Cleared on
    /// removal from both directions.
    passengers: HashMap<i32, Vec<i32>>,
    /// The inverse index — passenger id → the vehicle it rides.
    ///
    /// Not a convenience: vanilla's `handleSetEntityPassengers` calls
    /// `passenger.startRiding(vehicle)`, which detaches the passenger from
    /// whatever it was riding first. Without this map a passenger that moves
    /// between vehicles would leave the old one reading as still ridden, and
    /// that vehicle's label would stay suppressed forever.
    vehicle_of: HashMap<i32, i32>,
    /// Per-entity-type attachment points, when the caller has supplied them
    /// (M72). `None` keeps [`Self::tick_lerp`] at its pre-M72 behaviour, which
    /// is what every gate that builds a bare `EntityTable` relies on; the live
    /// client sets it once at session start.
    ///
    /// It lives here rather than being passed per tick so that a caller cannot
    /// tick the table and forget to reposition the riders — that would leave
    /// every passenger one tick stale, which is exactly the kind of drift that
    /// only shows up as jitter under motion.
    attachments: Option<std::sync::Arc<rewo_data::entity_attachments::Attachments>>,
    /// `Avatar.DATA_PLAYER_MODE_CUSTOMISATION` (index 16, BYTE) — the
    /// skin-part toggle mask, players only (M60). Absent means the byte has
    /// never been sent; vanilla seeds it at `(byte) 0`, so an absent entry
    /// reads as **every part hidden**, cape included. That is the exact
    /// default and not a fallback: a real client always sends its mask.
    /// Cleared on removal.
    model_customisation: HashMap<i32, u8>,
    /// `ItemEntity.DATA_ITEM` (index 8, ITEM_STACK) → `(item protocol id,
    /// count)`. Absent means the entity has sent no stack, which is
    /// `ItemStack.EMPTY` and renders nothing. Cleared on removal.
    item_stacks: HashMap<i32, (i32, i32, bool)>,
    /// Worn armour item ids per entity, head first (M46).
    armor: HashMap<i32, [Option<WornPiece>; 4]>,
    /// Allay dance state (index 16 BOOLEAN → `DATA_DANCING`, for Allays only).
    /// An entry is created lazily when `set_dancing` is first called (the
    /// server only sends `DATA_DANCING` once it flips off its `false` default);
    /// its counters advance each tick in [`Self::tick_lerp`]. Cleared on removal
    /// AND on (re-)add, so a reused id can't inherit a stale dance clock.
    dances: HashMap<i32, AllayDance>,
    /// Model-visible entity-event receipt ticks (warden attack/sonic boom,
    /// armadillo peek). Cleared on removal AND on (re-)add, so a reused entity
    /// id can never inherit a previous occupant's animation timing — vanilla's
    /// `AnimationState`s die with the entity.
    events: HashMap<i32, EntityEventStarts>,
    /// Combat-swing state (`ClientboundAnimatePacket` actions 0 / 3). Created
    /// lazily on the first swing; cleared on removal AND on (re-)add.
    swings: HashMap<i32, SwingState>,
    /// Held items by hand: `[MAIN_HAND, OFF_HAND]`
    /// (`ClientboundSetEquipmentPacket`). An absent entry is two
    /// [`HandItem::Empty`] hands, exact for an entity that sent no equipment.
    hands: HashMap<i32, [HandItem; 2]>,
    /// `Avatar.DATA_PLAYER_MAIN_HAND` (metadata index 15, HUMANOID_ARM
    /// serializer). Absent = `HumanoidArm::Right`, which is both
    /// `Avatar.DEFAULT_MAIN_HAND` and the non-left-handed `Mob.getMainArm()`.
    main_arms: HashMap<i32, HumanoidArm>,
    /// Amplifiers of the swing-duration effects, per entity. Only entities that
    /// actually have one get a map entry.
    swing_effects: HashMap<i32, SwingEffects>,
    /// `LivingEntity.hurtTime` / `hurtDuration` — the damage response (M21).
    /// Absent = never hurt, which renders identically to `hurtTime == 0`.
    hurts: HashMap<i32, HurtState>,
    /// `Player.hurtDir` — the camera tilt's *direction* (M81), in degrees.
    ///
    /// **A separate map from [`Self::hurts`], and deliberately so.** The clock
    /// above self-evicts at zero; `Player.hurtDir` is a plain field that
    /// nothing ever resets, so it outlives the animation it steered. That is
    /// observable: `dealDefaultKnockback` calls `indicateDamage` only when the
    /// hit was *not* blocked, so a blocked hit re-arms the clock through
    /// `damage_event` with no fresh `hurt_animation` — and vanilla then tilts
    /// along the direction of the *previous* hit. Folding this into
    /// `HurtState` would zero it instead.
    ///
    /// Keyed by entity id but written only for players: `animateHurt`'s
    /// yaw-storing override is on `Player`, and `LivingEntity.getHurtDir()`
    /// returns a flat `0.0F`.
    hurt_dirs: HashMap<i32, f32>,
    /// Per-mob combat state the M20 rigs read: `Mob.DATA_MOB_FLAGS_ID` (index
    /// 15 BYTE), `Raider.IS_CELEBRATING` (16 BOOLEAN),
    /// `SpellcasterIllager.DATA_SPELL_CASTING_ID` (17 BYTE) and
    /// `Pillager.IS_CHARGING_CROSSBOW` (17 BOOLEAN). Absent = every default.
    mob_state: HashMap<i32, MobState>,
    /// Simulated cape spines (M61), one per player currently showing a cape.
    ///
    /// A side map rather than a field on [`EntityState`] because that struct
    /// is `Copy` and is copied whole on every read: seventeen joints in two
    /// Verlet buffers is 816 bytes, which is exactly the coupling cost the
    /// cloak anchor's comment weighed six doubles against and found
    /// acceptable. Entries appear when the entity's cape becomes visible and
    /// are dropped the tick it stops being, so a recycled id can never
    /// inherit a chain.
    wavy_capes: HashMap<i32, crate::wavy_cape::WavyCape>,
    /// Whether the wavy cape is switched on at all (M61) — the feature is
    /// opt-in and vanilla is the default. While this is false nothing above
    /// is allocated, ticked, or read, so the vanilla cape's behaviour is
    /// unreachable from the flag.
    wavy_capes_enabled: bool,
    /// `NewMinecartBehavior`'s client interpolation schedule, one per minecart
    /// the server has sent a `move_minecart_along_track` for (M77).
    ///
    /// Created on the first packet rather than at spawn. Vanilla runs
    /// `lerpClientPositionAndRotation` every tick from the cart's construction,
    /// but with an empty inbox that is a snapshot of the live position into
    /// `oldLerp` and a clear of an already-empty list — no observable effect,
    /// so the lazy entry is exact and costs nothing for a cart nobody moves.
    /// Cleared on removal AND on (re-)add, for the reason every animation
    /// clock here is: a recycled server id must not inherit a schedule.
    minecarts: HashMap<i32, crate::minecart::MinecartLerp>,
    /// `Leashable.LeashData.delayedLeashHolderId` (M77) — the raw `destId` a
    /// `set_entity_link` carried, **including 0**.
    ///
    /// The presence of an entry is `getLeashData() != null`, which
    /// `setDelayedLeashHolderId` establishes unconditionally; the stored `0`
    /// is vanilla's "no holder" and is a different state from no entry at all.
    /// Cleared on removal.
    leash_data: HashMap<i32, i32>,
    /// `AbstractHurtingProjectile.accelerationPower` (M77). Absent means the
    /// server has sent none, which reads as the field's own default rather
    /// than as an error. Cleared on removal.
    projectile_power: HashMap<i32, f64>,
}

/// `LivingEntity`'s damage-response fields, set by
/// `ClientboundDamageEventPacket` and ticked down every tick.
///
/// Vanilla stores `hurtDuration` alongside `hurtTime` even though
/// `handleDamageEvent` always sets both to 10, because other paths (the death
/// tilt) divide by it. Kept as a field rather than folded to a constant so the
/// division is the same expression vanilla writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HurtState {
    /// `LivingEntity.hurtTime` — counts down to 0, one per tick.
    pub hurt_time: i32,
    /// `LivingEntity.hurtDuration` — 10 for every `handleDamageEvent`.
    pub hurt_duration: i32,
}

/// `LivingEntity`'s item-use state (M23) — the pair of fields
/// `onSyncedDataUpdated` reconstructs from the `DATA_LIVING_ENTITY_FLAGS` bit,
/// plus the flag itself.
///
/// **Why this can exist at all.** `useItemRemaining` is never transmitted. The
/// server sends only the flags byte; the client derives the countdown from the
/// item the entity is holding. So a remote entity's eating / drawing / blocking
/// progress is exactly reproducible from data Rewo already receives — the
/// premise that blocked this through M19, M20 and M22 was simply wrong.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UseState {
    /// `isUsingItem()` — bit 1 of the flags byte.
    pub using: bool,
    /// `getUsedItemHand()` — bit 2 selects OFF_HAND.
    pub hand: InteractionHand,
    /// `LivingEntity.useItem`, reduced to what the client reads back:
    /// `None` is `ItemStack.EMPTY`, `Some(id)` is the item it started using.
    /// `updatingUsingItem` compares this against the *current* hand item each
    /// tick and stops using when they differ.
    pub item_id: Option<i32>,
    /// `useItem.getUseDuration(this)` at the moment use started — the value
    /// `getTicksUsingItem()` subtracts `remaining` from.
    pub duration: i32,
    /// `LivingEntity.useItemRemaining`.
    ///
    /// **Deliberately unclamped, and it can go negative.**
    /// `updateUsingItem` runs `--this.useItemRemaining` unconditionally; only
    /// the *completion* branch after it is server-side. A client whose server
    /// has not yet cleared the flag keeps counting down past zero, and
    /// `getTicksUsingItem()` keeps growing with it — which the spear pose's
    /// sway term actually reads. Flooring this at 0 would be a plausible
    /// tidying that changed rendered output.
    pub remaining: i32,
}

impl UseState {
    /// `LivingEntity.getUseItemRemainingTicks()`.
    pub fn remaining_ticks(self) -> i32 {
        self.remaining
    }

    /// `LivingEntity.getTicksUsingItem()`:
    /// `isUsingItem() ? useItem.getUseDuration(this) - getUseItemRemainingTicks() : 0`.
    pub fn ticks_using_item(self) -> i32 {
        if self.using {
            self.duration - self.remaining
        } else {
            0
        }
    }

    /// `LivingEntity.getTicksUsingItem(partialTicks)`:
    /// `!isUsingItem() ? 0.0F : getTicksUsingItem() + partialTicks`.
    pub fn ticks_using_item_partial(self, alpha: f32) -> f32 {
        if self.using {
            self.ticks_using_item() as f32 + alpha
        } else {
            0.0
        }
    }

    /// Whether a use-driven arm pose is reachable at all:
    /// `getUsedItemHand() == hand && getUseItemRemainingTicks() > 0`.
    pub fn poses_hand(self, hand: InteractionHand) -> bool {
        self.using && self.hand == hand && self.remaining > 0
    }
}

impl HurtState {
    /// The *hurt* half of `LivingEntityRenderer`'s
    /// `state.hasRedOverlay = entity.hurtTime > 0 || entity.deathTime > 0`.
    ///
    /// M21 shipped only this term because `deathTime` was unmodelled; M24 adds
    /// the other one. The two live on different state, so the disjunction is
    /// assembled by the caller ([`EntityTable::has_red_overlay`]) rather than
    /// here — this method deliberately answers only what it can see.
    pub fn has_red_overlay(self) -> bool {
        self.hurt_time > 0
    }
}

/// `LivingEntity`'s death state (M24) — the client's own death clock.
///
/// Like the item-use clock, `deathTime` is **not** synchronised: the server
/// sends the entity's health (or a death entity-event), and every client counts
/// the 20 ticks itself. `LivingEntity.tick` runs
/// `if (isDeadOrDying() && level().shouldTickDeath(this)) tickDeath();`, and
/// `tickDeath` is just `this.deathTime++` plus a server-side removal at 20.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeathState {
    /// `LivingEntity.getHealth()` — `DATA_HEALTH_ID`, metadata index 9.
    /// `1.0` is the `entityData.define` default, which is what an entity that
    /// has not sent health yet reads as.
    pub health: f32,
    /// `LivingEntity.dead`, set by `die()`. Entity-event 3 calls it on the
    /// client for every non-player, so a mob can be dying with health that has
    /// not been re-sent.
    pub dead: bool,
    /// `LivingEntity.deathTime`, counted up locally once dying.
    pub death_time: i32,
}

impl DeathState {
    /// The state of an entity that has sent nothing: `entityData.define(
    /// DATA_HEALTH_ID, 1.0F)`, so **not** dying. A `Default` of `0.0` health
    /// would make every freshly-spawned entity read as dead.
    pub const ALIVE: DeathState = DeathState {
        health: 1.0,
        dead: false,
        death_time: 0,
    };

    /// `LivingEntity.isDeadOrDying()` — `getHealth() <= 0.0F || this.dead`.
    pub fn is_dead_or_dying(self) -> bool {
        self.health <= 0.0 || self.dead
    }

    /// `LivingEntityRenderer`:
    /// `state.deathTime = entity.deathTime > 0 ? entity.deathTime + partialTicks : 0`.
    ///
    /// Note the guard is on the **integer** count, so the first rendered frame
    /// after `tickDeath` sees `1 + alpha`, never a fractional value below 1.
    pub fn render_death_time(self, alpha: f32) -> f32 {
        if self.death_time > 0 {
            self.death_time as f32 + alpha
        } else {
            0.0
        }
    }
}

impl Default for DeathState {
    fn default() -> Self {
        DeathState::ALIVE
    }
}

/// The synced mob state the M20 arm rigs consume. Every field defaults to
/// vanilla's `define(...)` default, so an entity that has sent no metadata
/// renders exactly as one whose flags are all clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MobState {
    /// `Mob.DATA_MOB_FLAGS_ID`. Bit 1 no-AI, bit 2 left-handed, bit 4
    /// aggressive.
    pub flags: u8,
    /// `Raider.isCelebrating()`.
    pub celebrating: bool,
    /// `SpellcasterIllager.DATA_SPELL_CASTING_ID`; `isCastingSpell()` is `> 0`.
    pub spell_casting: u8,
    /// `Pillager.isChargingCrossbow()`.
    pub charging_crossbow: bool,
    /// `Sheep.DATA_WOOL_ID` (metadata index **18**, BYTE): the low nibble is
    /// the `DyeColor` id and bit 0x10 is the sheared flag. Vanilla's
    /// `define(DATA_WOOL_ID, (byte)0)` default is white-and-woolly, which is
    /// what 0 means here too.
    ///
    /// Index 18, not 17: `AgeableMob` declares **two** accessors —
    /// `DATA_BABY_ID` (16) then `AGE_LOCKED` (17) — so `Sheep`'s own first
    /// accessor lands one slot further down than a naive count suggests.
    pub wool: u8,
    /// `Creaking.IS_ACTIVE` (index 17, BOOLEAN) — the eyes' emissive layer
    /// alpha. `Creaking` declares `CAN_MOVE` (16) first, so `IS_ACTIVE` is 17.
    pub creaking_active: bool,
    /// The mob's synched texture variant (M64), if it has one.
    ///
    /// **The units differ by mob and only the caller knows which**: for a cat,
    /// wolf or frog this is a raw `minecraft:{cat,wolf,frog}_variant` registry
    /// id (26.x moved them to datapack registries carrying a `Holder`); for a
    /// horse, llama or axolotl it is the enum ordinal their `int` carries.
    /// `None` means the server has not spoken, which is not the same as 0 —
    /// a horse defaults to `DATA_ID_TYPE_VARIANT = 0`, i.e. WHITE, whereas an
    /// unresolved one keeps whatever Rewo baked.
    pub variant: Option<i32>,
    /// `TamableAnimal.DATA_FLAGS_ID` — index **18**, BYTE, bit 0x04 is
    /// `isTame()` and bit 0x01 `isInSittingPose()` (M64).
    ///
    /// The same slot *and the same serializer* as `Sheep.DATA_WOOL_ID`: both
    /// classes extend `Animal`, whose accessor count is zero, so both put
    /// their first byte at 18. Only the entity **kind** separates them — the
    /// M18 rule again, and the reason `apply_set_entity_data` gates this on
    /// the type being a cat or a wolf.
    pub tamable_flags: u8,
}

impl MobState {
    /// `Mob.isAggressive()` — `(flags & 4) != 0`.
    pub fn is_aggressive(self) -> bool {
        self.flags & 4 != 0
    }

    /// `Mob.isLeftHanded()` — `(flags & 2) != 0`. This *is* `Mob.getMainArm()`.
    pub fn is_left_handed(self) -> bool {
        self.flags & 2 != 0
    }

    /// `SpellcasterIllager.isCastingSpell()` — the client branch reads the
    /// synced byte, not the server-only tick counter.
    pub fn is_casting_spell(self) -> bool {
        self.spell_casting > 0
    }

    /// `Sheep.getColor()` — `DyeColor.byId(DATA_WOOL_ID & 15)`.
    pub fn wool_color(self) -> u8 {
        self.wool & 15
    }

    /// `Sheep.isSheared()` — `(DATA_WOOL_ID & 16) != 0`.
    pub fn is_sheared(self) -> bool {
        self.wool & 16 != 0
    }

    /// `TamableAnimal.isTame()` — `(DATA_FLAGS_ID & 4) != 0`.
    pub fn is_tame(self) -> bool {
        self.tamable_flags & 4 != 0
    }
}

/// `LivingEntity.getCurrentSwingDuration()` for one entity, over borrowed
/// fields so the tick loop can compute it while holding `swings` mutably.
///
/// `hand = swingingArm != null ? swingingArm : MAIN_HAND`, then the item in
/// **that** hand supplies the base duration, then the haste / mining-fatigue
/// adjustment. `None` when that hand is [`HandItem::Unknown`] — there is no
/// duration to be had, and inventing one would silently drive both the accept
/// predicate and `attackAnim`.
fn current_swing_duration(
    hands: &HashMap<i32, [HandItem; 2]>,
    swing_effects: &HashMap<i32, SwingEffects>,
    id: i32,
    swinging_arm: Option<InteractionHand>,
) -> Option<i32> {
    let hand = swinging_arm.unwrap_or(InteractionHand::MainHand);
    let base = hand_swing(hands, id, hand)?.duration;
    Some(
        swing_effects
            .get(&id)
            .copied()
            .unwrap_or_default()
            .adjust(base),
    )
}

/// `getItemInHand(hand).getSwingAnimation()`; `None` when unknowable.
fn hand_swing(
    hands: &HashMap<i32, [HandItem; 2]>,
    id: i32,
    hand: InteractionHand,
) -> Option<SwingAnimation> {
    hands
        .get(&id)
        .map(|h| h[hand_slot(hand)])
        .unwrap_or_default()
        .swing()
}

const fn hand_slot(hand: InteractionHand) -> usize {
    match hand {
        InteractionHand::MainHand => 0,
        InteractionHand::OffHand => 1,
    }
}

impl EntityTable {
    pub fn add(&mut self, id: i32, state: EntityState) {
        // A fresh entity at this id inherits no stale event timing — the
        // packet stream sends `remove_entities` before reusing an id, but a
        // dropped removal must not leave one warden's attack clock running on
        // its replacement. The Allay dance clock dies with the entity too.
        self.events.remove(&id);
        self.dances.remove(&id);
        self.wavy_capes.remove(&id);
        // Same reason (M77): a schedule is a clock, and a fresh cart at a
        // recycled id must not be dragged toward the previous one's rail.
        self.minecarts.remove(&id);
        self.clear_swing(id);
        self.map.insert(id, state);
    }

    pub fn remove(&mut self, id: i32) {
        self.map.remove(&id);
        self.custom_names.remove(&id);
        self.poses.remove(&id);
        self.gesture_states.remove(&id);
        self.sizes.remove(&id);
        self.babies.remove(&id);
        self.events.remove(&id);
        self.dances.remove(&id);
        self.mob_state.remove(&id);
        self.hurts.remove(&id);
        // The direction outlives the *clock*, not the *entity*: vanilla's
        // `hurtDir` is a field on the Player object, and the object is gone.
        self.hurt_dirs.remove(&id);
        self.uses.remove(&id);
        self.deaths.remove(&id);
        self.item_stacks.remove(&id);
        self.attributes.remove(&id);
        self.shared_flags.remove(&id);
        self.custom_name_visible.remove(&id);
        self.clear_riding(id);
        self.model_customisation.remove(&id);
        self.wavy_capes.remove(&id);
        self.minecarts.remove(&id);
        self.leash_data.remove(&id);
        self.projectile_power.remove(&id);
        self.clear_swing(id);
    }

    /// Detach an entity from the passenger graph in **both** directions (M70).
    ///
    /// The second direction is the one that matters. A rider that despawns
    /// while mounted is never mentioned by another `set_passengers`, so
    /// without this its vehicle keeps a stale roster, reads as ridden forever,
    /// and silently loses its label for the rest of the session.
    fn clear_riding(&mut self, id: i32) {
        if let Some(riders) = self.passengers.remove(&id) {
            for rider in riders {
                self.vehicle_of.remove(&rider);
            }
        }
        if let Some(vehicle) = self.vehicle_of.remove(&id) {
            if let Some(list) = self.passengers.get_mut(&vehicle) {
                list.retain(|&p| p != id);
            }
        }
    }

    /// Drop every swing-related record for an id — the swing clock, the held
    /// items, the main arm and the duration effects all die with the entity, so
    /// a recycled server id can never inherit a previous occupant's swing,
    /// weapon, handedness or haste.
    fn clear_swing(&mut self, id: i32) {
        self.swings.remove(&id);
        self.hands.remove(&id);
        self.main_arms.remove(&id);
        self.swing_effects.remove(&id);
    }

    /// Stamp a model-visible entity event's receipt tick — an unconditional
    /// restart (vanilla `AnimationState.start(tick)`), so repeated events
    /// re-clock the rig from zero. Only called for a looked-up entity of the
    /// matching kind; the caller enforces that.
    pub fn start_event(&mut self, id: i32, event: EntityEvent, tick: i64) {
        self.events.entry(id).or_insert([None; EntityEvent::COUNT])[event.index()] = Some(tick);
    }

    /// The receipt tick of a model-visible event on an entity, or `None` if it
    /// has never fired. The renderer measures `(now − start) · 0.05 s` from it.
    pub fn event_start(&self, id: i32, event: EntityEvent) -> Option<i64> {
        self.events.get(&id).and_then(|e| e[event.index()])
    }

    /// Set / clear an entity's metadata custom name.
    pub fn set_custom_name(&mut self, id: i32, name: Option<String>) {
        match name {
            Some(n) => {
                self.custom_names.insert(id, n);
            }
            None => {
                self.custom_names.remove(&id);
            }
        }
    }

    pub fn custom_name(&self, id: i32) -> Option<&str> {
        self.custom_names.get(&id).map(|s| s.as_str())
    }

    /// Entity pose ordinal from metadata (index 6) — STANDING=0 default.
    pub fn set_pose(&mut self, id: i32, pose: u8) {
        self.poses.insert(id, pose);
    }

    pub fn pose(&self, id: i32) -> u8 {
        self.poses.get(&id).copied().unwrap_or(0)
    }

    /// Mob gesture state (sniffer/armadillo/… enum ordinal at index 17).
    pub fn set_gesture_state(&mut self, id: i32, state: u8) {
        self.gesture_states.insert(id, state);
    }

    pub fn gesture_state(&self, id: i32) -> u8 {
        self.gesture_states.get(&id).copied().unwrap_or(0)
    }

    /// Slime / magma-cube size (index 16). Vanilla default is 1; callers
    /// choose their own fallback for entities that never sent it.
    pub fn set_size(&mut self, id: i32, size: i32) {
        self.sizes.insert(id, size);
    }

    pub fn size(&self, id: i32) -> Option<i32> {
        self.sizes.get(&id).copied()
    }

    /// Baby flag (index 16 BOOLEAN). `set` toggles membership.
    pub fn set_baby(&mut self, id: i32, baby: bool) {
        if baby {
            self.babies.insert(id);
        } else {
            self.babies.remove(&id);
        }
    }

    pub fn is_baby(&self, id: i32) -> bool {
        self.babies.contains(&id)
    }

    /// Set an Allay's `DATA_DANCING` flag (index-16 BOOLEAN). Creates the dance
    /// entry on first call and only flips its `dancing` field — vanilla's
    /// `setDancing` just writes the metadata; the counter reset on a false flag
    /// happens on the next [`Self::tick_lerp`], exactly like `Allay.tick()`.
    /// Only the kind-aware router calls this (for the Allay type), so the
    /// `dances` map holds Allays only.
    pub fn set_dancing(&mut self, id: i32, dancing: bool) {
        self.dances.entry(id).or_default().dancing = dancing;
    }

    /// Render-frame Allay dance inputs: `Some((is_spinning, spinning_progress))`
    /// iff the entity is currently dancing, else `None`. `is_spinning` is the
    /// step function `dancing_ticks % 55 < 15`; `spinning_progress` is the
    /// partial-tick-interpolated `spinning_ticks / 15`. Mirrors the
    /// `AllayRenderState` fields the model consumes.
    pub fn allay_dance_render(&self, id: i32, alpha: f32) -> Option<(bool, f32)> {
        let d = self.dances.get(&id)?;
        d.dancing
            .then(|| (d.is_spinning(), d.spinning_progress(alpha)))
    }

    // -- death (M24) ---------------------------------------------------------

    /// Apply `LivingEntity.DATA_HEALTH_ID` (metadata index 9, FLOAT).
    ///
    /// Vanilla's setter clamps to `[0, getMaxHealth()]`, but that clamp is on
    /// the *writing* side (`setHealth`); what arrives over the wire is the
    /// already-clamped value, so it is stored as sent. Only the sign matters
    /// here: `isDeadOrDying()` tests `<= 0`.
    /// Apply one `AttributeSnapshot` from `update_attributes` (M57).
    ///
    /// Replaces that attribute's base and modifier list wholesale, per
    /// `handleUpdateAttributes`' `setBaseValue` → `removeModifiers()` → add
    /// sequence, and leaves every other attribute untouched.
    pub fn set_attribute(
        &mut self,
        id: i32,
        attr: i32,
        base: f64,
        modifiers: Vec<crate::attributes::Modifier>,
    ) {
        self.attributes
            .entry(id)
            .or_default()
            .apply(attr, base, modifiers);
    }

    /// Everything the server has synced for this entity, or `None` when it has
    /// synced nothing.
    ///
    /// `None` is deliberately not an empty set: [`crate::attributes::resolve`]
    /// treats "no packet" and "a packet that cleared everything" identically
    /// only because both fall back to the supplier, and a caller that wants to
    /// know whether the server has ever spoken can ask here.
    pub fn attributes(&self, id: i32) -> Option<&crate::attributes::EntityAttributes> {
        self.attributes.get(&id)
    }

    pub fn set_health(&mut self, id: i32, health: f32) {
        self.deaths.entry(id).or_default().health = health;
    }

    // -- shared flags (M59) ---------------------------------------------------

    /// Apply `Entity.DATA_SHARED_FLAGS_ID` (metadata index 0, BYTE).
    pub fn set_shared_flags(&mut self, id: i32, flags: u8) {
        self.shared_flags.insert(id, flags);
    }

    /// `Entity.getSharedFlag(flag)` — `(flags & 1 << flag) != 0`, with the
    /// never-sent case reading as the `(byte) 0` default `defineSynchedData`
    /// seeds.
    pub fn shared_flag(&self, id: i32, flag: u32) -> bool {
        self.shared_flags.get(&id).copied().unwrap_or(0) & (1 << flag) != 0
    }

    /// `Entity.isInvisible()` — `getSharedFlag(FLAG_INVISIBLE)`, and
    /// `FLAG_INVISIBLE` is **5**.
    pub fn is_invisible(&self, id: i32) -> bool {
        self.shared_flag(id, 5)
    }

    /// `Entity.isDiscrete()` — which is `isShiftKeyDown()`, which is
    /// `getSharedFlag(FLAG_SHIFT_KEY_DOWN)`, and that flag is **1** (M70).
    ///
    /// Three `Entity` methods share this one bit verbatim — `isDiscrete`,
    /// `isSuppressingBounce` and `isDescending` — so the flag is not
    /// "sneaking" in any narrower sense than the shift key being down. Note it
    /// is *not* `isCrouching()`, which reads the pose instead: a player
    /// sneaking while flying is discrete but not crouching.
    pub fn is_discrete(&self, id: i32) -> bool {
        self.shared_flag(id, 1)
    }

    // -- custom-name visibility + passengers (M70) -----------------------------

    /// Apply `Entity.DATA_CUSTOM_NAME_VISIBLE` (metadata index 3, BOOLEAN).
    pub fn set_custom_name_visible(&mut self, id: i32, visible: bool) {
        if visible {
            self.custom_name_visible.insert(id);
        } else {
            self.custom_name_visible.remove(&id);
        }
    }

    /// `Entity.isCustomNameVisible()`, which is also
    /// `LivingEntity.shouldShowName()`.
    pub fn is_custom_name_visible(&self, id: i32) -> bool {
        self.custom_name_visible.contains(&id)
    }

    /// Apply `ClientboundSetPassengersPacket` — the vehicle's roster,
    /// replacing whatever it held.
    ///
    /// Mirrors `handleSetEntityPassengers`: every listed passenger
    /// `startRiding`s this vehicle, which detaches it from its previous one
    /// first, and anyone dropped from the roster stops riding.
    pub fn set_passengers(&mut self, vehicle: i32, riders: Vec<i32>) {
        // Anyone the vehicle used to carry and no longer does has dismounted.
        if let Some(previous) = self.passengers.get(&vehicle) {
            for old in previous.clone() {
                if !riders.contains(&old) {
                    self.vehicle_of.remove(&old);
                }
            }
        }
        // Each new rider detaches from whatever it rode before.
        for &rider in &riders {
            if let Some(&prior) = self.vehicle_of.get(&rider) {
                if prior != vehicle {
                    if let Some(list) = self.passengers.get_mut(&prior) {
                        list.retain(|&p| p != rider);
                    }
                }
            }
            self.vehicle_of.insert(rider, vehicle);
        }
        self.passengers.insert(vehicle, riders);
    }

    /// `Entity.isVehicle()` — `!this.passengers.isEmpty()`.
    ///
    /// True when **something is riding this entity**. The mirror question
    /// ("is this entity riding something") is [`Self::vehicle_of`], and the
    /// two are not interchangeable: a ridden horse is a vehicle, its rider is
    /// not.
    pub fn is_vehicle(&self, id: i32) -> bool {
        self.passengers.get(&id).is_some_and(|p| !p.is_empty())
    }

    /// The vehicle this entity is riding, if any — `Entity.getVehicle()`.
    pub fn vehicle_of(&self, id: i32) -> Option<i32> {
        self.vehicle_of.get(&id).copied()
    }

    /// Apply `Avatar.DATA_PLAYER_MODE_CUSTOMISATION` (index 16, BYTE).
    /// Only the kind-aware router calls this, and only for a player.
    pub fn set_model_customisation(&mut self, id: i32, mask: u8) {
        self.model_customisation.insert(id, mask);
    }

    /// `Avatar.isModelPartShown(PlayerModelPart.CAPE)` — the mask's **bit 0**
    /// (`PlayerModelPart.CAPE(0, "cape")`, `mask = 1 << bit`), which becomes
    /// `AvatarRenderState.showCape` and is `CapeLayer`'s first gate.
    pub fn shows_cape(&self, id: i32) -> bool {
        self.model_customisation.get(&id).copied().unwrap_or(0) & 1 != 0
    }

    /// `LivingEntity.handleEntityEvent(3)` — the death event.
    ///
    /// ```text
    /// case 3:
    ///    <play the death sound>
    ///    if (!(this instanceof Player)) { this.setHealth(0.0F); this.die(...); }
    /// ```
    ///
    /// The player exclusion is vanilla's, not a simplification: a dying player
    /// keeps whatever health the server last sent and never gets `dead` set by
    /// this path. The sound is not modelled (Rewo has no audio), so the two
    /// model-visible halves are what this applies.
    pub fn kill(&mut self, id: i32, is_player: bool) {
        if is_player {
            return;
        }
        let st = self.deaths.entry(id).or_default();
        st.health = 0.0;
        // `die()` early-returns when already dead, so a repeated event cannot
        // restart anything — and `deathTime` is deliberately left alone, which
        // is why a second event does not rewind the topple.
        st.dead = true;
    }

    /// `LivingEntity.tick`: `if (isDeadOrDying() && level().shouldTickDeath(this))
    /// tickDeath();`, and `tickDeath` is `this.deathTime++`.
    ///
    /// **`shouldTickDeath` is not modelled and is treated as true.** On a client
    /// it is a simulation-distance test — `entity.chunkPosition()
    /// .getChessboardDistance(player.chunkPosition()) <= serverSimulationDistance`
    /// — and Rewo does not retain the login packet's simulation distance. The
    /// consequence is bounded and worth stating: an entity dying *outside* the
    /// server's simulation distance would topple here and stand still in
    /// vanilla. Inside it, which is every entity close enough to look at, the
    /// two agree.
    fn tick_deaths(&mut self) {
        for st in self.deaths.values_mut() {
            if st.is_dead_or_dying() {
                st.death_time += 1;
            }
        }
    }

    /// The entity's death state. Absent is [`DeathState::ALIVE`].
    pub fn death_state(&self, id: i32) -> DeathState {
        self.deaths.get(&id).copied().unwrap_or_default()
    }

    /// `LivingEntityRenderer`:
    /// `state.hasRedOverlay = entity.hurtTime > 0 || entity.deathTime > 0`.
    ///
    /// The whole disjunction, assembled from the two clocks that own its
    /// terms. M21 shipped the first; this is the milestone that closes the
    /// exclusion it stated.
    pub fn has_red_overlay(&self, id: i32) -> bool {
        self.hurt_state(id).has_red_overlay() || self.death_state(id).death_time > 0
    }

    // -- item entities (M24b) ------------------------------------------------

    /// Set an `ItemEntity`'s stack from `DATA_ITEM` (metadata index 8,
    /// ITEM_STACK). `None` is an explicitly empty stack, which clears it —
    /// vanilla's `ItemEntity` with an empty stack renders nothing.
    pub fn set_item_stack(&mut self, id: i32, stack: Option<(i32, i32, bool)>) {
        match stack {
            Some(s) => {
                self.item_stacks.insert(id, s);
            }
            None => {
                self.item_stacks.remove(&id);
            }
        }
    }

    /// The `(item id, count)` a dropped stack shows, or `None` when the entity
    /// has sent no stack (or an empty one).
    pub fn item_stack(&self, id: i32) -> Option<(i32, i32, bool)> {
        self.item_stacks.get(&id).copied()
    }

    // -- item use (M23) ------------------------------------------------------

    /// Apply `LivingEntity.DATA_LIVING_ENTITY_FLAGS` (metadata index 8, BYTE),
    /// reproducing `onSyncedDataUpdated`'s client branch verbatim:
    ///
    /// ```text
    /// if (isUsingItem() && useItem.isEmpty()) {
    ///    useItem = getItemInHand(getUsedItemHand());
    ///    if (!useItem.isEmpty()) useItemRemaining = useItem.getUseDuration(this);
    /// } else if (!isUsingItem() && !useItem.isEmpty()) {
    ///    useItem = ItemStack.EMPTY;
    ///    useItemRemaining = 0;
    /// }
    /// ```
    ///
    /// Three consequences the shape encodes, each of which a naive
    /// "start a timer when the bit is set" would get wrong:
    ///
    /// 1. A **repeated** `using = true` does not restart the clock, because
    ///    `useItem` is no longer empty and the first branch does not run.
    /// 2. Starting with an **empty hand** leaves `useItem` empty, so a *later*
    ///    flags update can still latch an item — the branch is guarded on
    ///    `useItem.isEmpty()`, not on the flag having changed.
    /// 3. The hand is read at latch time and again every tick, so a hand swap
    ///    mid-use is caught by [`Self::tick_uses`], not here.
    ///
    /// The caller gates this on the entity actually being a `LivingEntity`:
    /// index 8 is the first slot a direct `Entity` subclass may claim too (an
    /// `AbstractArrow` puts its own BYTE there), so the serializer alone does
    /// not disambiguate it.
    pub fn set_living_flags(&mut self, id: i32, flags: u8) {
        let using = flags & 1 != 0;
        let hand = if flags & 2 != 0 {
            InteractionHand::OffHand
        } else {
            InteractionHand::MainHand
        };
        let held = self.hand_item(id, hand);
        let st = self.uses.entry(id).or_default();
        st.using = using;
        st.hand = hand;
        if using && st.item_id.is_none() {
            // `useItem = getItemInHand(usedHand)`. An unresolvable stack has no
            // known duration, so it latches nothing and the pose stays
            // suppressed — the same fail-closed answer M19 gives a swing.
            if let (Some(held_item), Some(profile)) = (held.held(), held.use_profile()) {
                st.item_id = Some(held_item.item_id);
                st.duration = profile.duration;
                st.remaining = profile.duration;
            }
        } else if !using && st.item_id.is_some() {
            st.item_id = None;
            st.duration = 0;
            st.remaining = 0;
        }
    }

    /// `LivingEntity.updatingUsingItem`, once per tick:
    ///
    /// ```text
    /// if (isUsingItem()) {
    ///    if (ItemStack.isSameItem(getItemInHand(getUsedItemHand()), useItem)) {
    ///       useItem = getItemInHand(getUsedItemHand());
    ///       updateUsingItem(useItem);          // --useItemRemaining
    ///    } else {
    ///       stopUsingItem();                   // useItem = EMPTY; remaining = 0
    ///    }
    /// }
    /// ```
    ///
    /// Note `stopUsingItem` does **not** clear the using flag on a client —
    /// that half sits inside a `!level().isClientSide()` guard — so an entity
    /// whose held item changed mid-use stays flagged as using with an empty
    /// `useItem`, and can latch again on a later flags update.
    fn tick_uses(&mut self) {
        for (id, st) in self.uses.iter_mut() {
            if !st.using {
                continue;
            }
            let held = self
                .hands
                .get(id)
                .map(|h| h[hand_slot(st.hand)])
                .unwrap_or_default();
            // `ItemStack.isSameItem` compares only the item. An empty
            // `useItem` matches an empty hand (both are `Items.AIR`) but never
            // an unresolvable one, which has no known item.
            let same = match st.item_id {
                Some(item_id) => held.same_item_key() == Some(item_id),
                None => held == HandItem::Empty,
            };
            if same {
                st.remaining -= 1;
            } else {
                st.item_id = None;
                st.duration = 0;
                st.remaining = 0;
            }
        }
    }

    /// The entity's item-use state. Absent means the flags byte was never
    /// received, which is exactly the `0` default: not using.
    pub fn use_state(&self, id: i32) -> UseState {
        self.uses.get(&id).copied().unwrap_or_default()
    }

    /// `AvatarRenderer.getArmPose`'s use-animation branch input: the animation
    /// of the item in `hand`, but only while that hand is the one actually in
    /// use and the countdown has not run out.
    ///
    /// `None` when no use pose applies — either the gate is closed or the held
    /// stack is unresolvable.
    pub fn use_animation(&self, id: i32, hand: InteractionHand) -> Option<ItemUseAnimation> {
        let st = self.use_state(id);
        if !st.poses_hand(hand) {
            return None;
        }
        self.hand_item(id, hand).use_profile().map(|p| p.animation)
    }

    // -- combat swings (M19) ------------------------------------------------

    /// Set one hand's item — `ClientboundSetEquipmentPacket`'s MAINHAND /
    /// OFFHAND slots. Armour slots never reach here: they change no swing
    /// input. [`HandItem::Unknown`] is a first-class value, not an absence.
    /// What is worn in one armour slot (M46), with the dye it carries (M47).
    ///
    /// `EquipmentSlot`'s wire ids are `0 mainhand, 1 offhand, 2 feet, 3 legs,
    /// 4 chest, 5 head` — so the armour occupies **2..=5 and runs bottom-up**,
    /// which is why this indexes by `5 - id` rather than by the id.
    pub fn set_armor(&mut self, id: i32, slot: usize, item: Option<WornPiece>) {
        if slot >= 4 {
            return;
        }
        let e = self.armor.entry(id).or_insert([None; 4]);
        e[slot] = item;
    }

    /// What this entity wears, head first. All `None` for anything the server
    /// has not equipped.
    pub fn armor(&self, id: i32) -> [Option<WornPiece>; 4] {
        self.armor.get(&id).copied().unwrap_or([None; 4])
    }

    pub fn set_hand_item(&mut self, id: i32, hand: InteractionHand, item: HandItem) {
        let slot = self.hands.entry(id).or_default();
        slot[hand_slot(hand)] = item;
        if slot.iter().all(|s| *s == HandItem::Empty) {
            self.hands.remove(&id);
        }
    }

    /// `getItemInHand(hand)`.
    pub fn hand_item(&self, id: i32, hand: InteractionHand) -> HandItem {
        self.hands
            .get(&id)
            .map(|h| h[hand_slot(hand)])
            .unwrap_or_default()
    }

    /// `LivingEntity.getItemHeldByArm(arm)`:
    /// `getMainArm() == arm ? getMainHandItem() : getOffhandItem()`.
    pub fn item_by_arm(&self, id: i32, arm: HumanoidArm) -> HandItem {
        let hand = if self.main_arm(id) == arm {
            InteractionHand::MainHand
        } else {
            InteractionHand::OffHand
        };
        self.hand_item(id, hand)
    }

    /// Whether every swing input for this entity is exactly known.
    ///
    /// `false` means some equipment update could not be resolved (an
    /// unregistered item id, or a component patch this client cannot walk), so
    /// the combat pose and CEM's `swing_progress` are **suppressed** rather
    /// than guessed. A later exact equipment update repairs it.
    pub fn swing_inputs_known(&self, id: i32) -> bool {
        self.hands
            .get(&id)
            .map(|h| h.iter().all(|s| !s.is_unknown()))
            .unwrap_or(true)
    }

    /// `Avatar.setMainArm` from metadata index 15 (HUMANOID_ARM serializer).
    pub fn set_main_arm(&mut self, id: i32, arm: HumanoidArm) {
        self.main_arms.insert(id, arm);
    }

    /// `LivingEntity.getMainArm()`. Right unless the entity told us otherwise.
    ///
    /// Two classes answer this differently and both are honoured:
    /// `Avatar.getMainArm()` reads `DATA_PLAYER_MAIN_HAND` (index 15,
    /// HUMANOID_ARM), while `Mob.getMainArm()` is
    /// `isLeftHanded() ? LEFT : RIGHT` from `DATA_MOB_FLAGS_ID` bit 2 (index
    /// 15, BYTE). They are different serializers on the same slot, so at most
    /// one is ever recorded for a given entity and no precedence rule is
    /// needed — `set_mob_flags` writes through to the same map.
    pub fn main_arm(&self, id: i32) -> HumanoidArm {
        self.main_arms.get(&id).copied().unwrap_or_default()
    }

    /// `Mob.DATA_MOB_FLAGS_ID` (index 15, BYTE). The caller must already have
    /// established that this entity is a `Mob` — an `ArmorStand` puts its own
    /// unrelated client flags at the same index with the same serializer.
    ///
    /// Writes handedness through to the main-arm map because
    /// `Mob.getMainArm()` *is* `isLeftHanded()`; there is no separate mob arm
    /// field in vanilla either.
    pub fn set_mob_flags(&mut self, id: i32, flags: u8) {
        self.mob_state.entry(id).or_default().flags = flags;
        self.main_arms.insert(
            id,
            if flags & 2 != 0 {
                HumanoidArm::Left
            } else {
                HumanoidArm::Right
            },
        );
    }

    /// `Raider.setCelebrating` (index 16, BOOLEAN). Kind-gated by the caller:
    /// the same slot is `DATA_BABY_ID` on an ageable mob and `DATA_DANCING` on
    /// an Allay.
    pub fn set_celebrating(&mut self, id: i32, celebrating: bool) {
        self.mob_state.entry(id).or_default().celebrating = celebrating;
    }

    /// `SpellcasterIllager.DATA_SPELL_CASTING_ID` (index 17, BYTE).
    pub fn set_spell_casting(&mut self, id: i32, spell: u8) {
        self.mob_state.entry(id).or_default().spell_casting = spell;
    }

    /// `Pillager.setChargingCrossbow` (index 17, BOOLEAN).
    /// `Sheep.DATA_WOOL_ID` (index 18, BYTE). Kind-gated by the caller.
    pub fn set_wool(&mut self, id: i32, wool: u8) {
        self.mob_state.entry(id).or_default().wool = wool;
    }

    /// The sheep's dye colour `0..15`, or `None` for an entity that has sent no
    /// wool byte. `None` is *not* "untinted": `SheepWoolLayer` tints
    /// unconditionally and `DyeColor.WHITE` is the default, so the renderer
    /// treats `None` and `Some(0)` identically — the distinction only says
    /// whether the server has spoken.
    pub fn wool_color(&self, id: i32) -> Option<u8> {
        self.mob_state.get(&id).map(|s| s.wool_color())
    }

    /// `Sheep.isSheared()` — bit 0x10 of the same wool byte (M64).
    ///
    /// Defaults to false for an entity that has sent none, which is vanilla's
    /// `define(DATA_WOOL_ID, (byte)0)`: white and woolly.
    pub fn is_sheared(&self, id: i32) -> bool {
        self.mob_state.get(&id).is_some_and(|s| s.is_sheared())
    }

    /// The mob's synched texture variant (M64) — see [`MobState::variant`]
    /// for why the units depend on the kind. Kind-gated by the caller.
    pub fn set_variant(&mut self, id: i32, variant: i32) {
        self.mob_state.entry(id).or_default().variant = Some(variant);
    }

    /// `None` when the server has not sent one, which leaves the mob on the
    /// texture Rewo baked rather than on some default it invented.
    pub fn variant(&self, id: i32) -> Option<i32> {
        self.mob_state.get(&id).and_then(|s| s.variant)
    }

    /// `TamableAnimal.DATA_FLAGS_ID` (index 18, BYTE). Kind-gated by the
    /// caller — the sheep's wool byte is the same slot *and* serializer.
    pub fn set_tamable_flags(&mut self, id: i32, flags: u8) {
        self.mob_state.entry(id).or_default().tamable_flags = flags;
    }

    /// `TamableAnimal.isTame()`, which is what `Wolf.getTexture` branches on.
    pub fn is_tame(&self, id: i32) -> bool {
        self.mob_state.get(&id).is_some_and(|s| s.is_tame())
    }

    /// `Creaking.IS_ACTIVE` (index 17, BOOLEAN). Kind-gated by the caller.
    pub fn set_creaking_active(&mut self, id: i32, active: bool) {
        self.mob_state.entry(id).or_default().creaking_active = active;
    }

    /// `Creaking.isActive()` — vanilla's synched default is `false`, so an
    /// untracked entity reads as a creaking that has not woken.
    pub fn creaking_active(&self, id: i32) -> bool {
        self.mob_state.get(&id).is_some_and(|s| s.creaking_active)
    }

    pub fn set_charging_crossbow(&mut self, id: i32, charging: bool) {
        self.mob_state.entry(id).or_default().charging_crossbow = charging;
    }

    /// The synced mob state driving the M20 arm rigs. Every default matches
    /// vanilla's `define(...)`, so an entity that sent nothing is not a special
    /// case.
    pub fn mob_state(&self, id: i32) -> MobState {
        self.mob_state.get(&id).copied().unwrap_or_default()
    }

    /// Record (`Some(amplifier)`) or drop (`None`) one swing-duration effect —
    /// the client half of `ClientboundUpdateMobEffectPacket` /
    /// `ClientboundRemoveMobEffectPacket` for the three effects
    /// `getCurrentSwingDuration` reads. Client `hasEffect` is pure map
    /// membership, so no expiry clock belongs here (see [`SwingEffect`]).
    pub fn set_swing_effect(&mut self, id: i32, effect: SwingEffect, amplifier: Option<i32>) {
        let e = self.swing_effects.entry(id).or_default();
        *e.slot(effect) = amplifier;
        if e.is_empty() {
            self.swing_effects.remove(&id);
        }
    }

    /// `LivingEntity.getCurrentSwingDuration()` — the ticks the *current* swing
    /// runs for. `None` when the swinging hand's item is unknowable.
    pub fn current_swing_duration(&self, id: i32) -> Option<i32> {
        current_swing_duration(
            &self.hands,
            &self.swing_effects,
            id,
            self.swings.get(&id).and_then(|s| s.swinging_arm),
        )
    }

    /// `LivingEntity.swing(hand)` — the accept/restart rule, verbatim:
    ///
    /// ```text
    /// if (!swinging || swingTime >= getCurrentSwingDuration() / 2 || swingTime < 0) {
    ///     swingTime = -1; swinging = true; swingingArm = hand;
    /// }
    /// ```
    ///
    /// So a repeat inside the first half of a running swing is **ignored**
    /// (integer `duration / 2`), and one at or past the halfway point restarts
    /// it. `ticks_swing` records whether this entity's client class advances
    /// `updateSwingTime` (see [`SwingState::ticks_swing`]). Returns whether the
    /// swing was accepted.
    ///
    /// With an unknown duration the accept predicate cannot be evaluated at
    /// all. The swing is then recorded unconditionally so `swingingArm` stays
    /// current for the render state — harmless, because a suppressed entity
    /// produces no pose either way — and the clock does not advance until the
    /// inputs are repaired.
    pub fn swing(&mut self, id: i32, hand: InteractionHand, ticks_swing: bool) -> bool {
        let duration = self.current_swing_duration(id);
        let s = self
            .swings
            .entry(id)
            .or_insert_with(|| SwingState::new(ticks_swing));
        s.ticks_swing = ticks_swing;
        let accept = match duration {
            Some(d) => !s.swinging || s.swing_time >= d / 2 || s.swing_time < 0,
            None => true,
        };
        if accept {
            s.swing_time = -1;
            s.swinging = true;
            s.swinging_arm = Some(hand);
        }
        accept
    }

    /// `ArmedEntityRenderState`: `attackArm = swingingArm != OFF_HAND ? mainArm
    /// : mainArm.getOpposite()`. Note the test is against `OFF_HAND`, so a
    /// `null` swinging arm also yields the main arm.
    pub fn attack_arm(&self, id: i32) -> HumanoidArm {
        let main = self.main_arm(id);
        match self.swings.get(&id).and_then(|s| s.swinging_arm) {
            Some(InteractionHand::OffHand) => main.opposite(),
            _ => main,
        }
    }

    /// `ArmedEntityRenderState.swingAnimationType` —
    /// `getItemHeldByArm(attackArm).getSwingAnimation().type()`, or `None` when
    /// that arm's item is unknowable. Note this reads the *arm*, while the
    /// duration reads the *hand*: with an off-hand swing both name the same
    /// physical hand, so they agree.
    pub fn swing_animation_type(&self, id: i32) -> Option<SwingAnimationType> {
        self.item_by_arm(id, self.attack_arm(id))
            .swing()
            .map(|s| s.kind)
    }

    /// `LivingEntity.swinging` — whether a swing is currently in flight.
    ///
    /// This is the *boolean* the renderer's arm-pose choice reads
    /// (`attack != null && attack.type() == STAB && avatar.swinging` in
    /// `AvatarRenderer.getArmPose`), which is not the same question as
    /// `getAttackAnim() > 0`: the flag is set the instant a swing is accepted,
    /// while `attackAnim` is still 0 until the first `updateSwingTime`.
    pub fn is_swinging(&self, id: i32) -> bool {
        self.swings.get(&id).is_some_and(|s| s.swinging)
    }

    /// `LivingEntity.getAttackAnim(partialTicks)` — 0 for an entity that has
    /// never swung.
    pub fn attack_anim(&self, id: i32, partial: f32) -> f32 {
        self.swings.get(&id).map_or(0.0, |s| s.attack_anim(partial))
    }

    /// The raw swing fields — `(swinging, swingTime, attackAnim, oAttackAnim,
    /// swingingArm)`. For the `swingshot` oracle and tests; the renderer only
    /// needs [`Self::attack_anim`].
    pub fn swing_debug(&self, id: i32) -> Option<(bool, i32, f32, f32, Option<InteractionHand>)> {
        self.swings.get(&id).map(|s| {
            (
                s.swinging,
                s.swing_time,
                s.attack_anim,
                s.o_attack_anim,
                s.swinging_arm,
            )
        })
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, id: i32) -> Option<&EntityState> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: i32) -> Option<&mut EntityState> {
        self.map.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (i32, &EntityState)> {
        self.map.iter().map(|(id, e)| (*id, e))
    }

    /// Advance every entity's interpolation one 20 Hz tick, plus the Allay
    /// dance counters (vanilla `Allay.tick()` runs on the client every tick)
    /// and the combat-swing clock.
    /// The `dances` map holds only Allays, so advancing all of them is exact.
    pub fn tick_lerp(&mut self) {
        for (id, e) in self.map.iter_mut() {
            // `LivingEntity.isFallFlying()` is `getSharedFlag(7)` — the cape's
            // `fallFlyingScale` needs the *count*, which only the client keeps.
            let fall_flying = self.shared_flags.get(id).copied().unwrap_or(0) & (1 << 7) != 0;
            e.tick(fall_flying);
        }
        for d in self.dances.values_mut() {
            d.tick();
        }
        self.tick_wavy_capes();
        self.tick_uses();
        self.tick_deaths();
        self.tick_swings();
        // `LivingEntity.baseTick`: `if (this.hurtTime > 0) this.hurtTime--;`
        // The entry is dropped once it reaches 0 so an entity that has healed
        // costs nothing — an absent entry and `hurt_time == 0` render alike.
        self.hurts.retain(|_, h| {
            if h.hurt_time > 0 {
                h.hurt_time -= 1;
            }
            h.hurt_time > 0
        });
        // After every entity's own lerp step (so `prev` already holds last
        // tick's derived position) and **before** the riders are placed: a
        // minecart is positioned by `AbstractMinecart.tick` inside
        // `ClientLevel.tickNonPassenger`, and its passengers only afterwards,
        // in the `tickPassenger` loop that follows. See [`crate::minecart`].
        self.tick_minecarts();
        // Last, and only after every entity's own lerp step has moved
        // `prev = cur`: vanilla's `tickPassenger` → `rideTick` →
        // `positionRider` overwrites a rider's position at the end of the
        // tick it was ticked in. See [`crate::riding`] for why a passenger
        // does not interpolate.
        self.position_riders();
    }

    /// `AbstractMinecart.tick` → `NewMinecartBehavior.tick` →
    /// `lerpClientPositionAndRotation`, for every cart carrying a schedule
    /// (M77).
    ///
    /// The four assignments are vanilla's: `setPos`, `setDeltaMovement`,
    /// `setXRot`, `setYRot`. `setPos` lands on
    /// [`EntityState::set_derived_pos`] — the same writer `positionRider`
    /// uses — because it must overwrite **this tick's** position without
    /// touching `prev` (already advanced above) or the synced target. That is
    /// what leaves the generic `render_pos` chord tracking the schedule one
    /// tick behind, which is exactly the baseline vanilla measures
    /// `passengerOffset` against.
    ///
    /// The rotations are written straight onto the entity: `MinecartBehavior`'s
    /// `setXRot`/`setYRot` forward to `Entity` with no `% 360`, unlike
    /// `moveOrInterpolateTo`'s branch.
    fn tick_minecarts(&mut self) {
        if self.minecarts.is_empty() {
            return;
        }
        let Self { map, minecarts, .. } = self;
        for (id, lerp) in minecarts.iter_mut() {
            // A schedule whose entity has gone is left alone rather than
            // ticked against a position that no longer exists. `remove` drops
            // the entry, so this is only reachable if a schedule were pushed
            // for an untracked id — which `push_minecart_steps` refuses.
            let Some(e) = map.get(id) else { continue };
            let (pos, yaw, pitch) = (e.cur, e.yaw, e.pitch);
            let Some(sample) = lerp.tick(pos, yaw, pitch) else {
                continue;
            };
            if let Some(e) = map.get_mut(id) {
                e.set_derived_pos(sample.position);
                e.yaw = sample.y_rot;
                e.pitch = sample.x_rot;
            }
        }
    }

    /// `handleMinecartAlongTrack`'s `lerpSteps.addAll(packet.lerpSteps())`
    /// (M77) — an append onto this cart's inbox, creating it on first use.
    ///
    /// A no-op for an untracked id. That mirrors the packet handler's
    /// `packet.getEntity(level) instanceof AbstractMinecart` guard, and is a
    /// belt as well: `rewo_net`'s router already refuses an id the table does
    /// not hold, so reaching this with one would mean the two disagreed.
    pub fn push_minecart_steps(&mut self, id: i32, steps: &[crate::minecart::MinecartStep]) {
        if !self.map.contains_key(&id) {
            return;
        }
        self.minecarts.entry(id).or_default().push_steps(steps);
    }

    /// `AbstractMinecartRenderer.newExtractState` — the schedule sampled at
    /// the true partial tick, or `None` when `cartHasPosRotLerp()` is false
    /// and the generic [`EntityState::render_pos`] stands unchallenged.
    ///
    /// This does **not** replace `render_pos`: both are live, and vanilla
    /// carries their difference to a passenger as `state.passengerOffset`.
    /// They coincide exactly at `alpha == 1.0`, which is the sample
    /// [`Self::tick_minecarts`] wrote into the entity.
    pub fn minecart_render(&self, id: i32, alpha: f32) -> Option<crate::minecart::MinecartSample> {
        self.minecarts.get(&id)?.sample(alpha)
    }

    /// The raw schedule, for witnesses that need to see the countdown or the
    /// segment rather than a sampled position.
    pub fn minecart_lerp(&self, id: i32) -> Option<&crate::minecart::MinecartLerp> {
        self.minecarts.get(&id)
    }

    /// `Leashable.setDelayedLeashHolderId(entityId)` (M77).
    ///
    /// Vanilla's body is `setLeashData(new LeashData(entityId))` then
    /// `dropLeash(this, false, false)`. The drop is a **no-op here by
    /// construction**: it is guarded on `leashData.leashHolder != null`, and
    /// the leash data it reads is the one just installed, whose resolved
    /// holder is null. So the whole handler is this one assignment — including
    /// for `destId == 0`, which installs a leash record holding nothing rather
    /// than clearing the record.
    pub fn set_leash_holder(&mut self, id: i32, dest: i32) {
        self.leash_data.insert(id, dest);
    }

    /// `getLeashData() != null` and, if so, its `delayedLeashHolderId` — the
    /// raw wire value, `0` included.
    pub fn leash_data(&self, id: i32) -> Option<i32> {
        self.leash_data.get(&id).copied()
    }

    /// `Leashable.getLeashHolder()` — the resolved holder, or `None`.
    ///
    /// Vanilla: `delayedLeashHolderId != 0 && isClientSide && level.getEntity(
    /// delayedLeashHolderId) != null` promotes the id to a cached `Entity`
    /// reference and zeroes the delayed id; the method then returns that
    /// reference. Rewo has no entity references, so it resolves on demand
    /// instead of caching.
    ///
    /// **The one place that diverges**: after vanilla has promoted, the cached
    /// reference survives the holder leaving the tracking range, so a leash
    /// keeps pointing at a now-unloaded entity until the server re-sends.
    /// Resolving on demand reports `None` there. Nothing in this client reads
    /// it yet — the rope is not drawn — and the divergence is recorded rather
    /// than modelled because reproducing it needs a `&mut` accessor whose only
    /// consumer would be the cache itself.
    pub fn leash_holder(&self, id: i32) -> Option<i32> {
        let dest = self.leash_data.get(&id).copied()?;
        if dest == 0 || !self.map.contains_key(&dest) {
            return None;
        }
        Some(dest)
    }

    /// `AbstractHurtingProjectile.accelerationPower = packet.
    /// getAccelerationPower()` (M77).
    pub fn set_projectile_power(&mut self, id: i32, power: f64) {
        self.projectile_power.insert(id, power);
    }

    /// The last `projectile_power` this entity was sent, if any.
    pub fn projectile_power(&self, id: i32) -> Option<f64> {
        self.projectile_power.get(&id).copied()
    }

    /// Supply the attachment table, enabling per-tick passenger positioning
    /// (M72). Without it [`Self::tick_lerp`] leaves riders at their own synced
    /// positions, which is the pre-M72 behaviour.
    pub fn set_attachments(
        &mut self,
        attachments: std::sync::Arc<rewo_data::entity_attachments::Attachments>,
    ) {
        self.attachments = Some(attachments);
    }

    /// `ClientLevel.tickNonPassenger` → `tickPassenger` → `Entity.rideTick`.
    ///
    /// Walks vehicle-first from every **root** — an entity that carries
    /// passengers and is not itself a passenger — so a rider of a rider is
    /// positioned after the vehicle it hangs off has already been positioned.
    /// Vanilla gets that ordering from the recursion in `tickPassenger`; here
    /// it is explicit, with a visited set because a malformed roster could
    /// otherwise describe a cycle and the recursion would not terminate.
    fn position_riders(&mut self) {
        let Some(att) = self.attachments.clone() else {
            return;
        };
        if self.passengers.is_empty() {
            return;
        }
        let roots: Vec<i32> = self
            .passengers
            .iter()
            .filter(|(id, riders)| !riders.is_empty() && !self.vehicle_of.contains_key(id))
            .map(|(id, _)| *id)
            .collect();
        let mut visited: std::collections::HashSet<i32> = std::collections::HashSet::new();
        let mut stack = roots;
        while let Some(vehicle_id) = stack.pop() {
            if !visited.insert(vehicle_id) {
                continue;
            }
            let Some(v) = self.vehicle_inputs(vehicle_id) else {
                continue;
            };
            let riders = match self.passengers.get(&vehicle_id) {
                Some(r) if !r.is_empty() => r.clone(),
                _ => continue,
            };
            for (index, rider_id) in riders.iter().copied().enumerate() {
                // `tickPassenger` starts with `entity.getVehicle() != vehicle
                // -> stopRiding()`. Kept for the same reason vanilla keeps it,
                // and honestly labelled: with `set_passengers` maintaining both
                // maps together (it detaches a rider from its previous vehicle
                // before adding it here) an inconsistent pair is unreachable,
                // so this is a belt and not a load-bearing guard. `rideshot`'s
                // `r4.a_rider_that_moved_on` says so explicitly — removing this
                // line leaves the gate green.
                if self.vehicle_of.get(&rider_id) != Some(&vehicle_id) {
                    continue;
                }
                let Some(state) = self.map.get(&rider_id) else {
                    continue;
                };
                let r = crate::riding::RiderInputs {
                    type_id: state.type_id,
                    yaw: state.yaw,
                    scale: Self::age_scale(&att, state.type_id, self.babies.contains(&rider_id)),
                    index,
                };
                if let Some(pos) = crate::riding::rider_position(&att, &v, &r) {
                    if let Some(state) = self.map.get_mut(&rider_id) {
                        state.set_derived_pos(pos);
                    }
                }
                // `AbstractHorse`/`Chicken.positionRider`'s trailing
                // `livingEntity.yBodyRot = this.yBodyRot`. The head is a
                // separate field and is deliberately untouched.
                if let Some(yaw) = crate::riding::forced_body_yaw(&att, &v, r.type_id) {
                    if let Some(state) = self.map.get_mut(&rider_id) {
                        state.yaw = yaw;
                    }
                }
                stack.push(rider_id);
            }
        }
    }

    /// `LivingEntity.getAgeScale()` — `isBaby() ? 0.5F : 1.0F`, and 1.0 for
    /// anything that is not a `LivingEntity` at all (an `Entity` has no age
    /// scale, and its `getPassengerRidingPosition` passes a literal 1.0).
    ///
    /// The `minecraft:scale` attribute is the other half of vanilla's factor
    /// and is **not** applied: Rewo's renderer does not scale a model by it
    /// either, so honouring it here alone would place a rider off the mount it
    /// is drawn on.
    fn age_scale(
        att: &rewo_data::entity_attachments::Attachments,
        type_id: i32,
        baby: bool,
    ) -> f32 {
        if baby && att.is_living(type_id) {
            0.5
        } else {
            1.0
        }
    }

    fn vehicle_inputs(&self, id: i32) -> Option<crate::riding::VehicleInputs> {
        let att = self.attachments.as_ref()?;
        let e = self.map.get(&id)?;
        let baby = self.babies.contains(&id);
        Some(crate::riding::VehicleInputs {
            type_id: e.type_id,
            pos: e.cur,
            yaw: e.yaw,
            scale: Self::age_scale(att, e.type_id, baby),
            passenger_count: self.passengers.get(&id).map_or(0, |p| p.len()),
            // Only `AbstractCubeMob` reads it; vanilla's own default is 1.
            cube_size: self.sizes.get(&id).copied().unwrap_or(1),
            limb: e.limb(),
            baby,
        })
    }

    /// Switch the wavy cape (M61) on or off. Off is the default and is what
    /// the vanilla cape milestone's 38 witnesses grade; turning it off also
    /// drops every simulated chain, so a toggle mid-session cannot leave one
    /// player waving and another not.
    pub fn set_wavy_capes(&mut self, on: bool) {
        self.wavy_capes_enabled = on;
        if !on {
            self.wavy_capes.clear();
        }
    }

    pub fn wavy_capes_enabled(&self) -> bool {
        self.wavy_capes_enabled
    }

    /// This entity's simulated cape spine, if it has one this tick.
    pub fn wavy_cape(&self, id: i32) -> Option<&crate::wavy_cape::WavyCape> {
        self.wavy_capes.get(&id)
    }

    /// Advance every visible cape's cloth simulation one tick (M61).
    ///
    /// Runs from [`Self::tick_lerp`] **after** every entity's own tick, so
    /// the anchor and the cloak gap are both read at their end-of-tick
    /// values — the same pair the renderer would resolve at `alpha == 1`.
    ///
    /// Membership is `shows_cape`, the metadata bit alone. That is a
    /// deliberate superset of "a cape is actually drawn": the remaining
    /// gates (an uploaded cape sheet, an elytra in the chest slot, the
    /// invisibility flag) need the skin cache and the equipment table, and
    /// neither belongs in a world tick. Simulating a chain nobody renders
    /// costs one entry and a few hundred flops; asking the renderer for the
    /// answer would make the simulation frame-driven, which rule 4 forbids.
    fn tick_wavy_capes(&mut self) {
        if !self.wavy_capes_enabled {
            return;
        }
        // Disjoint field borrows: the entity map and the customisation mask
        // are read while the chain map is written.
        let map = &self.map;
        let masks = &self.model_customisation;
        let capes = &mut self.wavy_capes;
        let shows = |id: &i32| masks.get(id).copied().unwrap_or(0) & 1 != 0;
        capes.retain(|id, _| map.contains_key(id) && shows(id));
        for (id, e) in map.iter() {
            if !shows(id) {
                continue;
            }
            let cloak = e.cloak_pos(1.0);
            let pos = e.render_pos(1.0);
            let a = crate::cape::cape_angles(
                cloak,
                pos,
                e.yaw,
                e.fall_fly_ticks() as f32 + 1.0,
                0.0,
                0.0,
            );
            let anchor = crate::wavy_cape::anchor_in_cape_space(a.flap, a.lean, a.lean2, e.yaw);
            // The forcing is vanilla's own lagging-cloak gap, in blocks;
            // `wavy_cape::ANCHOR_ACCEL` is what turns it into an
            // acceleration, and its comment carries the derivation.
            let delta = [
                cloak[0] - pos[0],
                cloak[1] - pos[1],
                cloak[2] - pos[2],
            ];
            capes
                .entry(*id)
                .or_insert_with(|| {
                    crate::wavy_cape::WavyCape::new(crate::wavy_cape::SEGMENTS, anchor)
                })
                .tick(anchor, delta);
        }
    }

    /// `LivingEntity.handleDamageEvent` — the client half of
    /// `ClientboundDamageEventPacket`.
    ///
    /// Vanilla also sets `invulnerableTime = 20`, plays the hurt sound and
    /// records the damage source; none of those is model-visible, so the two
    /// that are — the hurt clock and the walk-speed kick — are what this
    /// applies. A repeat re-arms the clock from 10 rather than extending it.
    pub fn hurt(&mut self, id: i32) {
        self.hurts.insert(
            id,
            HurtState {
                hurt_duration: 10,
                hurt_time: 10,
            },
        );
        // `this.walkAnimation.setSpeed(1.5F)` is the first line of
        // `handleDamageEvent`, before the clock — the limbs kick on the hit.
        if let Some(e) = self.map.get_mut(&id) {
            e.set_limb_speed(1.5);
        }
    }

    /// The damage-response state driving the red overlay. Defaults for an
    /// entity that has never been hurt.
    pub fn hurt_state(&self, id: i32) -> HurtState {
        self.hurts.get(&id).copied().unwrap_or_default()
    }

    /// `Entity.animateHurt(yaw)` — the client half of
    /// `ClientboundHurtAnimationPacket` (M81).
    ///
    /// Three overrides collapse into `is_player`:
    ///
    /// * `Entity.animateHurt` is **empty**, so a non-living entity does
    ///   nothing at all. The caller applies that gate, because only it knows
    ///   the class table.
    /// * `LivingEntity.animateHurt` sets `hurtDuration = 10; hurtTime =
    ///   hurtDuration` and **ignores the yaw entirely** — the parameter is
    ///   dead in the base class, and `getHurtDir()` returns a flat `0.0F`.
    /// * `Player.animateHurt` calls `super` and *then* stores
    ///   `this.hurtDir = yaw`.
    ///
    /// So the yaw survives on a player and nowhere else. Note the clock half
    /// is byte-identical to [`Self::hurt`]'s, minus the walk kick: vanilla's
    /// `animateHurt` does not touch `walkAnimation`, which `handleDamageEvent`
    /// does. Sharing the clock rather than duplicating it is the point — the
    /// two packets arm one machine, and only this one steers it.
    pub fn animate_hurt(&mut self, id: i32, yaw: f32, is_player: bool) {
        self.hurts.insert(
            id,
            HurtState {
                hurt_duration: 10,
                hurt_time: 10,
            },
        );
        if is_player {
            self.hurt_dirs.insert(id, yaw);
        }
    }

    /// `LivingEntity.getHurtDir()` — the direction the damage tilt leans away
    /// from, in degrees, relative to the victim's own body yaw at the moment
    /// of the hit (the server subtracts `getYRot()` before sending).
    ///
    /// `0.0` for an entity that is not a player and for a player that has
    /// never taken a directed hit — which is exactly the base-class return,
    /// not a placeholder.
    pub fn hurt_dir(&self, id: i32) -> f32 {
        self.hurt_dirs.get(&id).copied().unwrap_or(0.0)
    }

    /// `LivingEntity.baseTick` (`oAttackAnim = attackAnim`) followed by
    /// `updateSwingTime`, for the entities whose client class runs it:
    ///
    /// ```text
    /// int d = getCurrentSwingDuration();
    /// if (swinging) { swingTime++; if (swingTime >= d) { swingTime = 0; swinging = false; } }
    /// else swingTime = 0;
    /// attackAnim = (float)swingTime / d;
    /// ```
    ///
    /// The division is unguarded on purpose — vanilla divides by whatever
    /// `getCurrentSwingDuration` returned, including 0 under extreme haste. An
    /// *unknown* duration is different: the clock freezes rather than dividing
    /// by an invented number, and the pose is suppressed for that entity.
    fn tick_swings(&mut self) {
        let Self {
            swings,
            hands,
            swing_effects,
            ..
        } = self;
        for (id, s) in swings.iter_mut() {
            if !s.ticks_swing {
                continue;
            }
            let Some(duration) = current_swing_duration(hands, swing_effects, *id, s.swinging_arm)
            else {
                continue;
            };
            s.o_attack_anim = s.attack_anim;
            if s.swinging {
                // Java `int` increment wraps; a debug build must not abort.
                s.swing_time = s.swing_time.wrapping_add(1);
                if s.swing_time >= duration {
                    s.swing_time = 0;
                    s.swinging = false;
                }
            } else {
                s.swing_time = 0;
            }
            s.attack_anim = s.swing_time as f32 / duration as f32;
        }
    }

    pub fn set_name(&mut self, uuid: u128, name: String) {
        self.names.insert(uuid, name);
    }

    pub fn remove_name(&mut self, uuid: u128) {
        self.names.remove(&uuid);
    }

    pub fn name_of(&self, uuid: u128) -> Option<&str> {
        self.names.get(&uuid).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_step_lerp_converges_exactly() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.set_target(3.0, 0.0, 0.0);
        e.tick(false); // (3-0)/3 → 1
        assert_eq!(e.render_pos(1.0)[0], 1.0);
        assert_eq!(e.render_pos(0.5)[0], 0.5, "partial tick blends prev→cur");
        e.tick(false); // (3-1)/2 → 2
        e.tick(false); // (3-2)/1 → 3 exact
        assert_eq!(e.render_pos(1.0)[0], 3.0);
        e.tick(false); // no steps left — stays put
        assert_eq!(e.render_pos(0.0)[0], 3.0);
    }

    /// `moveCloak`'s easing branch: each tick closes a quarter of the gap,
    /// so the remaining distance is `0.75ⁿ` of the original — a geometric
    /// series that never quite arrives.
    #[test]
    fn the_cloak_anchor_closes_a_quarter_of_the_gap_per_tick() {
        let mut a = CloakAnchor::default();
        for n in 1..=8u32 {
            a.move_cloak([8.0, 0.0, 0.0]);
            let remaining = 8.0 - a.x;
            let want = 8.0 * 0.75f64.powi(n as i32);
            assert!(
                (remaining - want).abs() < 1e-12,
                "tick {n}: remaining {remaining} != {want}"
            );
        }
        // And `O` trails by exactly one tick, which is what the render lerps.
        assert!(a.xo < a.x);
    }

    /// Past ±10 blocks the axis teleports **and rewrites `O`**, so the
    /// interpolated anchor does not sweep across the gap on the next frame.
    /// A gap of exactly 10 still eases — vanilla's test is strict.
    #[test]
    fn a_gap_over_ten_blocks_snaps_and_rewrites_the_previous_slot() {
        let mut a = CloakAnchor::default();
        a.move_cloak([11.0, 10.0, -11.0]);
        assert_eq!(a.x, 11.0, "snapped");
        assert_eq!(a.xo, 11.0, "and O with it");
        assert_eq!(a.z, -11.0, "negative side snaps too");
        assert_eq!(a.zo, -11.0);
        assert_eq!(a.y, 2.5, "exactly 10 is not over — it eases");
        assert_eq!(a.yo, 0.0);
        // The whole point: with O rewritten, every partial tick reads the
        // destination rather than a streak from the old position.
        assert_eq!(a.interpolated(0.0)[0], 11.0);
        assert_eq!(a.interpolated(0.5)[0], 11.0);
    }

    /// Shared flag 7 is `isFallFlying()`; the counter is the client's.
    #[test]
    fn fall_fly_ticks_count_up_while_flying_and_reset_at_once() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..3 {
            e.tick(true);
        }
        assert_eq!(e.fall_fly_ticks(), 3);
        e.tick(false);
        assert_eq!(e.fall_fly_ticks(), 0, "reset, not decayed");
    }

    #[test]
    fn deltas_accumulate_on_the_target_not_the_render_pos() {
        let mut e = EntityState::new(0, 0, 10.0, 0.0, 0.0, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0); // mid-lerp — target must not lose the first
        assert_eq!(e.x, 11.0);
        for _ in 0..3 {
            e.tick(false);
        }
        assert_eq!(e.render_pos(1.0)[0], 11.0);
    }

    #[test]
    fn still_entity_has_no_limb_swing_but_walking_builds_it_up() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..10 {
            e.tick(false); // no target set → cur never moves
        }
        let (_, amt_still) = e.limb();
        assert!(amt_still < 1e-6, "still entity: amount {amt_still}");
        // Now walk ~0.2 blk/tick and let the smoother ramp.
        for _ in 0..20 {
            e.nudge(0.2, 0.0, 0.0);
            e.tick(false);
        }
        let (swing, amt) = e.limb();
        assert!(amt > 0.5, "sustained walk drives amount up: {amt}");
        assert!(swing > 0.0, "phase advanced: {swing}");
    }

    #[test]
    fn events_restart_and_clear_on_lifecycle() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
        // A stamp records the receipt tick.
        t.start_event(1, EntityEvent::WardenAttack, 100);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(100));
        // Independent slots: sonic boom does not disturb attack.
        t.start_event(1, EntityEvent::WardenSonicBoom, 105);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(100));
        assert_eq!(t.event_start(1, EntityEvent::WardenSonicBoom), Some(105));
        // A repeated event is an unconditional restart (new tick).
        t.start_event(1, EntityEvent::WardenAttack, 140);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), Some(140));
        // Removal clears every slot.
        t.remove(1);
        assert_eq!(t.event_start(1, EntityEvent::WardenAttack), None);
        assert_eq!(t.event_start(1, EntityEvent::WardenSonicBoom), None);
    }

    #[test]
    fn readding_an_id_drops_stale_event_timing() {
        let mut t = EntityTable::default();
        t.add(7, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.start_event(7, EntityEvent::ArmadilloPeek, 50);
        // Re-adding the same id (a dropped despawn, then a new occupant) must
        // not inherit the previous entity's peek clock.
        t.add(7, EntityState::new(1, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.event_start(7, EntityEvent::ArmadilloPeek), None);
    }

    #[test]
    fn allay_dance_counters_track_vanilla_tick() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        // No dance flag yet → the render helper is None (not dancing).
        assert_eq!(t.allay_dance_render(1, 1.0), None);
        // Server sends DATA_DANCING=true; counters have not advanced yet, so
        // the render helper reports "dancing, at-start" (isSpinning() reads the
        // pre-increment dancing_ticks=0, so 0 % 55 < 15 → true).
        t.set_dancing(1, true);
        assert_eq!(t.allay_dance_render(1, 1.0), Some((true, 0.0)));
        // Five dancing ticks: dancing_ticks=5, spinning_ticks ramps 0→5,
        // spinning_ticks0=4 (the previous tick's value).
        for _ in 0..5 {
            t.tick_lerp();
        }
        let (spinning, prog) = t.allay_dance_render(1, 1.0).unwrap();
        assert!(spinning, "5 % 55 < 15 → spinning");
        assert!((prog - 5.0 / 15.0).abs() < 1e-6, "progress @alpha=1 = 5/15: {prog}");
        // Partial-tick interpolation blends spinning_ticks0=4 → spinning_ticks=5.
        let half = t.allay_dance_render(1, 0.5).unwrap().1;
        assert!((half - 4.5 / 15.0).abs() < 1e-6, "progress @alpha=0.5 = 4.5/15: {half}");
        // Tick 15 crosses out of the spin window (15 % 55 = 15, not < 15), so
        // spinning flips off and spinning_ticks starts ramping back down.
        for _ in 5..15 {
            t.tick_lerp();
        }
        let (spinning15, _) = t.allay_dance_render(1, 1.0).unwrap();
        assert!(!spinning15, "15 % 55 = 15 is NOT < 15 → not spinning");
        // A full loop later (tick 55) re-enters the spin window.
        for _ in 15..55 {
            t.tick_lerp();
        }
        assert!(
            t.allay_dance_render(1, 1.0).unwrap().0,
            "55 % 55 = 0 < 15 → spinning again"
        );
    }

    #[test]
    fn dance_flag_false_resets_counters_next_tick() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        for _ in 0..7 {
            t.tick_lerp();
        }
        assert!(t.allay_dance_render(1, 1.0).is_some());
        // Flag off: the render helper reports None immediately (isDancing() is
        // the metadata flag), and the counters zero on the next tick.
        t.set_dancing(1, false);
        assert_eq!(t.allay_dance_render(1, 1.0), None, "not dancing → None");
        t.tick_lerp();
        // Dancing again from a clean slate — counters restart at 0.
        t.set_dancing(1, true);
        assert_eq!(t.allay_dance_render(1, 1.0), Some((true, 0.0)));
        t.tick_lerp();
        let (_, prog) = t.allay_dance_render(1, 1.0).unwrap();
        assert!((prog - 1.0 / 15.0).abs() < 1e-6, "one tick after restart: 1/15, got {prog}");
    }

    #[test]
    fn dance_state_clears_on_removal_and_readd() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        t.tick_lerp();
        assert!(t.allay_dance_render(1, 1.0).is_some());
        t.remove(1);
        assert_eq!(t.allay_dance_render(1, 1.0), None, "removal clears the dance");
        // A reused id (dropped despawn, new occupant) must not inherit the clock.
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        t.tick_lerp();
        let (_, prog) = t.allay_dance_render(1, 1.0).unwrap();
        assert!((prog - 1.0 / 15.0).abs() < 1e-6, "fresh occupant restarts at 1/15, got {prog}");
    }

    // -- combat swings (M19) ------------------------------------------------

    fn swinger() -> EntityTable {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    fn held(item_id: i32, kind: SwingAnimationType, duration: i32) -> HandItem {
        HandItem::Held(HeldItem {
            item_id,
            swing: SwingAnimation::new(kind, duration),
            use_profile: UseProfile::UNUSABLE,
            charged: false,
            glint: false,
        })
    }

    #[test]
    fn swing_lifecycle_matches_vanilla_with_the_default_duration() {
        let mut t = swinger();
        assert_eq!(
            t.current_swing_duration(1),
            Some(6),
            "bare hand = SwingAnimation.DEFAULT"
        );
        assert!(t.swing(1, InteractionHand::MainHand, true));
        // Accepted: swingTime = -1, swinging, attackAnim untouched (still 0).
        assert_eq!(
            t.swing_debug(1),
            Some((true, -1, 0.0, 0.0, Some(InteractionHand::MainHand)))
        );
        // Ticks 1..6 walk swingTime 0..5 → attackAnim 0/6 .. 5/6.
        for step in 0..6 {
            t.tick_lerp();
            let (swinging, time, anim, _, _) = t.swing_debug(1).unwrap();
            assert!(swinging, "still swinging at step {step}");
            assert_eq!(time, step, "swingTime at step {step}");
            assert!(
                (anim - step as f32 / 6.0).abs() < 1e-6,
                "attackAnim at step {step}: {anim}"
            );
        }
        // Tick 7 hits swingTime == duration → reset, swing over.
        t.tick_lerp();
        let (swinging, time, anim, o, _) = t.swing_debug(1).unwrap();
        assert!(!swinging);
        assert_eq!(time, 0);
        assert_eq!(anim, 0.0);
        assert!(
            (o - 5.0 / 6.0).abs() < 1e-6,
            "oAttackAnim keeps the last frame: {o}"
        );
    }

    #[test]
    fn first_half_repeats_are_ignored_and_the_half_boundary_restarts() {
        let mut t = swinger();
        assert!(t.swing(1, InteractionHand::MainHand, true));
        // swingTime: -1 → 0 (tick 1) → 1 (tick 2) → 2 (tick 3). duration/2 = 3.
        for _ in 0..3 {
            t.tick_lerp();
        }
        assert_eq!(t.swing_debug(1).unwrap().1, 2);
        assert!(
            !t.swing(1, InteractionHand::OffHand, true),
            "2 < 6/2 and already swinging → rejected"
        );
        assert_eq!(
            t.swing_debug(1).unwrap().4,
            Some(InteractionHand::MainHand),
            "a rejected swing must not change the arm"
        );
        assert_eq!(t.swing_debug(1).unwrap().1, 2, "…nor the clock");
        // One more tick puts swingTime at exactly duration/2 = 3 → accepted.
        t.tick_lerp();
        assert_eq!(t.swing_debug(1).unwrap().1, 3);
        assert!(t.swing(1, InteractionHand::OffHand, true));
        assert_eq!(
            t.swing_debug(1),
            Some((true, -1, 3.0 / 6.0, 2.0 / 6.0, Some(InteractionHand::OffHand)))
        );
    }

    #[test]
    fn attack_anim_wraps_forward_when_the_swing_ends() {
        let mut t = swinger();
        t.swing(1, InteractionHand::MainHand, true);
        for _ in 0..7 {
            t.tick_lerp();
        }
        // oAttackAnim 5/6, attackAnim 0 → diff −5/6 wraps to +1/6.
        assert_eq!(t.attack_anim(1, 0.0), 5.0 / 6.0);
        let half = t.attack_anim(1, 0.5);
        assert!(
            (half - (5.0 / 6.0 + 0.5 / 6.0)).abs() < 1e-6,
            "wrapped half: {half}"
        );
        let full = t.attack_anim(1, 1.0);
        assert!((full - 1.0).abs() < 1e-6, "wrapped full: {full}");
    }

    #[test]
    fn attack_arm_follows_the_swinging_hand_and_the_main_arm() {
        let mut t = swinger();
        // Default main arm is RIGHT.
        assert_eq!(t.attack_arm(1), HumanoidArm::Right);
        t.swing(1, InteractionHand::MainHand, true);
        assert_eq!(t.attack_arm(1), HumanoidArm::Right);
        t.swing(1, InteractionHand::OffHand, true);
        assert_eq!(t.attack_arm(1), HumanoidArm::Left, "off hand → opposite");
        // A left-handed entity mirrors both.
        t.set_main_arm(1, HumanoidArm::Left);
        assert_eq!(t.attack_arm(1), HumanoidArm::Right);
        t.swing(1, InteractionHand::MainHand, true);
        assert_eq!(t.attack_arm(1), HumanoidArm::Left);
    }

    #[test]
    fn duration_and_type_come_from_the_item_in_the_swinging_hand() {
        let mut t = swinger();
        // Iron spear (STAB, 19) main hand; bare off hand.
        t.set_hand_item(
            1,
            InteractionHand::MainHand,
            held(1329, SwingAnimationType::Stab, 19),
        );
        assert_eq!(t.current_swing_duration(1), Some(19));
        assert_eq!(t.swing_animation_type(1), Some(SwingAnimationType::Stab));
        // Swinging the (empty) off hand falls back to the default on both.
        t.swing(1, InteractionHand::OffHand, true);
        assert_eq!(t.current_swing_duration(1), Some(6));
        assert_eq!(
            t.swing_animation_type(1),
            Some(SwingAnimationType::Whack),
            "attackArm is now LEFT, which holds nothing → DEFAULT"
        );
        // Put the spear in the off hand instead: an off-hand swing is a STAB.
        t.set_hand_item(1, InteractionHand::MainHand, HandItem::Empty);
        t.set_hand_item(
            1,
            InteractionHand::OffHand,
            held(1329, SwingAnimationType::Stab, 19),
        );
        assert_eq!(t.current_swing_duration(1), Some(19));
        assert_eq!(t.swing_animation_type(1), Some(SwingAnimationType::Stab));
    }

    #[test]
    fn an_unknown_hand_suppresses_the_swing_instead_of_guessing() {
        let mut t = swinger();
        t.set_hand_item(1, InteractionHand::MainHand, HandItem::Unknown);
        assert!(!t.swing_inputs_known(1));
        assert_eq!(
            t.current_swing_duration(1),
            None,
            "no item → no duration, and no invented default"
        );
        assert_eq!(t.swing_animation_type(1), None);
        // The swing is still recorded (the arm stays current) but the clock
        // never advances, so nothing divides by a guessed duration.
        assert!(t.swing(1, InteractionHand::MainHand, true));
        for _ in 0..12 {
            t.tick_lerp();
        }
        assert_eq!(t.swing_debug(1).unwrap().1, -1, "frozen at the accept value");
        assert_eq!(t.attack_anim(1, 1.0), 0.0);
        // An exact update repairs it and the clock resumes.
        t.set_hand_item(
            1,
            InteractionHand::MainHand,
            held(1329, SwingAnimationType::Stab, 19),
        );
        assert!(t.swing_inputs_known(1));
        assert_eq!(t.current_swing_duration(1), Some(19));
        t.tick_lerp();
        assert_eq!(t.swing_debug(1).unwrap().1, 0, "clock resumed");
        // An unknown OFF hand also suppresses, even with a known main hand.
        t.set_hand_item(1, InteractionHand::OffHand, HandItem::Unknown);
        assert!(!t.swing_inputs_known(1));
    }

    #[test]
    fn haste_and_mining_fatigue_adjust_the_swing() {
        let mut t = swinger();
        assert_eq!(t.current_swing_duration(1), Some(6));
        // Haste I (amplifier 0) → 6 − (1 + 0) = 5.
        t.set_swing_effect(1, SwingEffect::Haste, Some(0));
        assert_eq!(t.current_swing_duration(1), Some(5));
        // Conduit power II (amplifier 1) alongside → max(0, 1) → 6 − 2 = 4.
        t.set_swing_effect(1, SwingEffect::ConduitPower, Some(1));
        assert_eq!(t.current_swing_duration(1), Some(4));
        // Mining fatigue is ignored while dig speed is present.
        t.set_swing_effect(1, SwingEffect::MiningFatigue, Some(0));
        assert_eq!(t.current_swing_duration(1), Some(4));
        // Drop both dig-speed effects → fatigue I gives 6 + (1 + 0)·2 = 8.
        t.set_swing_effect(1, SwingEffect::Haste, None);
        t.set_swing_effect(1, SwingEffect::ConduitPower, None);
        assert_eq!(t.current_swing_duration(1), Some(8));
        t.set_swing_effect(1, SwingEffect::MiningFatigue, None);
        assert_eq!(
            t.current_swing_duration(1),
            Some(6),
            "no effects → the item value"
        );
    }

    #[test]
    fn an_absurd_amplifier_wraps_like_java_int_math_instead_of_panicking() {
        // The amplifier is an unbounded wire VarInt and Java `int` arithmetic
        // wraps, so a debug build must not abort where vanilla would overflow.
        let mut t = swinger();
        t.set_swing_effect(1, SwingEffect::MiningFatigue, Some(i32::MAX));
        assert_eq!(
            t.current_swing_duration(1),
            Some(6i32.wrapping_add(1i32.wrapping_add(i32::MAX).wrapping_mul(2)))
        );
        t.set_swing_effect(1, SwingEffect::MiningFatigue, None);
        t.set_swing_effect(1, SwingEffect::Haste, Some(i32::MAX));
        assert_eq!(
            t.current_swing_duration(1),
            Some(6i32.wrapping_sub(1i32.wrapping_add(i32::MAX)))
        );
        // A *negative* amplifier is clamped by vanilla's own `max(a, b)` over
        // two zero-initialised locals, not by us: `getDigSpeedAmplification`
        // starts `a = b = 0` and only overwrites a present effect, so a
        // conduit-less haste of i32::MIN still yields max(i32::MIN, 0) = 0.
        t.set_swing_effect(1, SwingEffect::Haste, Some(i32::MIN));
        assert_eq!(t.current_swing_duration(1), Some(5));
    }

    #[test]
    fn an_unclocked_entity_stores_the_swing_but_never_advances_it() {
        let mut t = swinger();
        assert!(t.swing(1, InteractionHand::MainHand, false));
        for _ in 0..20 {
            t.tick_lerp();
        }
        assert_eq!(
            t.swing_debug(1),
            Some((true, -1, 0.0, 0.0, Some(InteractionHand::MainHand))),
            "vanilla's non-Monster mobs never call updateSwingTime"
        );
        assert_eq!(t.attack_anim(1, 1.0), 0.0);
    }

    #[test]
    fn swing_state_dies_with_the_entity() {
        let mut t = swinger();
        t.set_hand_item(
            1,
            InteractionHand::MainHand,
            held(1329, SwingAnimationType::Stab, 19),
        );
        t.set_main_arm(1, HumanoidArm::Left);
        t.set_swing_effect(1, SwingEffect::Haste, Some(3));
        t.swing(1, InteractionHand::OffHand, true);
        t.tick_lerp();
        assert!(t.swing_debug(1).is_some());
        t.remove(1);
        assert_eq!(t.swing_debug(1), None);
        assert_eq!(t.hand_item(1, InteractionHand::MainHand), HandItem::Empty);
        assert_eq!(t.main_arm(1), HumanoidArm::Right);
        assert_eq!(t.current_swing_duration(1), Some(6));
        // A reused id starts clean too (a dropped despawn must not leak).
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_hand_item(
            1,
            InteractionHand::MainHand,
            held(1329, SwingAnimationType::Stab, 19),
        );
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.current_swing_duration(1), Some(6));
        assert_eq!(t.attack_anim(1, 1.0), 0.0);
        assert!(t.swing_inputs_known(1));
    }

    #[test]
    fn names_are_keyed_by_uuid_independent_of_entities() {
        let mut t = EntityTable::default();
        t.set_name(7, "Vwyla".into());
        t.add(1, EntityState::new(7, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.name_of(t.get(1).unwrap().uuid), Some("Vwyla"));
        t.remove(1);
        assert_eq!(t.name_of(7), Some("Vwyla"), "name outlives the entity");
        t.remove_name(7);
        assert_eq!(t.name_of(7), None);
    }
}

#[cfg(test)]
mod m20_mob_state_tests {
    use super::*;

    #[test]
    fn the_flag_bits_are_vanillas() {
        // `Mob`: bit 1 no-AI, bit 2 left-handed, bit 4 aggressive.
        let s = MobState {
            flags: 0b0000_0110,
            ..Default::default()
        };
        assert!(s.is_aggressive());
        assert!(s.is_left_handed());
        let s = MobState {
            flags: 0b0000_0001,
            ..Default::default()
        };
        assert!(!s.is_aggressive(), "no-AI must not read as aggressive");
        assert!(!s.is_left_handed());
        // `isCastingSpell()` is `> 0`, not `!= 0` on a signed byte — the wire
        // value is unsigned here so any non-zero casts.
        assert!(!MobState::default().is_casting_spell());
        assert!(MobState {
            spell_casting: 3,
            ..Default::default()
        }
        .is_casting_spell());
    }

    #[test]
    fn the_flags_byte_writes_handedness_through_to_the_main_arm() {
        // `Mob.getMainArm()` *is* `isLeftHanded()`; there is no separate field.
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.main_arm(1), HumanoidArm::Right);
        t.set_mob_flags(1, 0b0000_0010);
        assert_eq!(t.main_arm(1), HumanoidArm::Left);
        t.set_mob_flags(1, 0b0000_0100);
        assert_eq!(t.main_arm(1), HumanoidArm::Right, "clearing bit 2 restores it");
        assert!(t.mob_state(1).is_aggressive());
    }

    #[test]
    fn mob_state_dies_with_the_entity_and_is_not_inherited() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_mob_flags(1, 0b0000_0110);
        t.set_celebrating(1, true);
        t.set_spell_casting(1, 3);
        t.set_charging_crossbow(1, true);
        assert_ne!(t.mob_state(1), MobState::default());
        t.remove(1);
        assert_eq!(t.mob_state(1), MobState::default());
        // A recycled server id must not inherit the previous occupant's state.
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.mob_state(1), MobState::default());
    }
}

#[cfg(test)]
mod m21_hurt_tests {
    use super::*;

    fn t() -> EntityTable {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    #[test]
    fn handle_damage_event_arms_both_fields_and_kicks_the_walk() {
        let mut t = t();
        assert_eq!(t.hurt_state(1), HurtState::default());
        t.hurt(1);
        // `hurtDuration = 10; hurtTime = this.hurtDuration;`
        assert_eq!(t.hurt_state(1).hurt_time, 10);
        assert_eq!(t.hurt_state(1).hurt_duration, 10);
        assert!(t.hurt_state(1).has_red_overlay());
        // `walkAnimation.setSpeed(1.5F)` — stored at 1.5, rendered clamped.
        assert_eq!(t.get(1).unwrap().limb().1, 1.0);
    }

    #[test]
    fn the_clock_counts_down_one_per_tick_and_stops_at_zero() {
        let mut t = t();
        t.hurt(1);
        for want in (0..10).rev() {
            t.tick_lerp();
            assert_eq!(t.hurt_state(1).hurt_time, want);
        }
        // Already 0 — `if (this.hurtTime > 0)` guards the decrement.
        t.tick_lerp();
        assert_eq!(t.hurt_state(1).hurt_time, 0);
        assert!(!t.hurt_state(1).has_red_overlay());
    }

    #[test]
    fn a_repeat_re_arms_rather_than_extending() {
        let mut t = t();
        t.hurt(1);
        t.tick_lerp();
        t.tick_lerp();
        assert_eq!(t.hurt_state(1).hurt_time, 8);
        t.hurt(1);
        assert_eq!(t.hurt_state(1).hurt_time, 10);
    }

    #[test]
    fn the_hurt_clock_dies_with_the_entity() {
        let mut t = t();
        t.hurt(1);
        t.remove(1);
        assert_eq!(t.hurt_state(1), HurtState::default());
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(t.hurt_state(1), HurtState::default(), "a reused id must not inherit it");
    }

    /// A usable item, for the M23 clock tests.
    fn usable(item_id: i32, duration: i32) -> HandItem {
        HandItem::Held(HeldItem {
            item_id,
            swing: SwingAnimation::DEFAULT,
            use_profile: UseProfile {
                duration,
                animation: ItemUseAnimation::Block,
            },
            charged: false,
            glint: false,
        })
    }

    #[test]
    fn the_use_clock_runs_down_and_can_go_negative() {
        // `updateUsingItem` runs `--useItemRemaining` unconditionally; only the
        // completion branch after it is server-side. So a client whose server
        // has not yet cleared the flag counts past zero, and
        // `getTicksUsingItem()` keeps growing — which the spear sway reads.
        let mut t = t();
        t.set_hand_item(1, InteractionHand::MainHand, usable(7, 3));
        t.set_living_flags(1, 1);
        assert_eq!(t.use_state(1).remaining, 3);
        for _ in 0..5 {
            t.tick_lerp();
        }
        assert_eq!(t.use_state(1).remaining, -2, "must not floor at zero");
        assert_eq!(t.use_state(1).ticks_using_item(), 5);
        assert!(
            !t.use_state(1).poses_hand(InteractionHand::MainHand),
            "a run-out clock closes the pose gate"
        );
    }

    #[test]
    fn re_sending_the_using_flag_does_not_restart_the_clock() {
        let mut t = t();
        t.set_hand_item(1, InteractionHand::MainHand, usable(7, 40));
        t.set_living_flags(1, 1);
        t.tick_lerp();
        t.set_living_flags(1, 1);
        assert_eq!(
            t.use_state(1).remaining,
            39,
            "onSyncedDataUpdated's latch is guarded on useItem.isEmpty()"
        );
        // Clearing and re-setting *does* restart it, because the clear empties
        // `useItem` and re-opens that guard.
        t.set_living_flags(1, 0);
        t.set_living_flags(1, 1);
        assert_eq!(t.use_state(1).remaining, 40);
    }

    #[test]
    fn starting_with_an_empty_hand_latches_nothing_but_stays_armed() {
        // `useItem = getItemInHand(...)` assigns EMPTY, and the duration write
        // is guarded on `!useItem.isEmpty()` — so the guard stays open and a
        // later flags update can still latch.
        let mut t = t();
        t.set_living_flags(1, 1);
        assert!(t.use_state(1).using);
        assert_eq!(t.use_state(1).remaining, 0);
        t.set_hand_item(1, InteractionHand::MainHand, usable(7, 40));
        t.set_living_flags(1, 1);
        assert_eq!(t.use_state(1).remaining, 40);
    }

    #[test]
    fn an_unresolvable_hand_latches_no_use_clock() {
        // Same fail-closed rule the swing applies: no known duration means no
        // derived countdown, so the pose stays suppressed rather than guessed.
        let mut t = t();
        t.set_hand_item(1, InteractionHand::MainHand, HandItem::Unknown);
        t.set_living_flags(1, 1);
        assert!(t.use_state(1).using);
        assert_eq!(t.use_state(1).item_id, None);
        assert_eq!(t.use_state(1).remaining, 0);
    }

    #[test]
    fn the_off_hand_bit_selects_the_hand_that_poses() {
        let mut t = t();
        t.set_hand_item(1, InteractionHand::OffHand, usable(7, 40));
        t.set_living_flags(1, 0b11);
        let st = t.use_state(1);
        assert_eq!(st.hand, InteractionHand::OffHand);
        assert_eq!(st.remaining, 40);
        assert!(st.poses_hand(InteractionHand::OffHand));
        assert!(!st.poses_hand(InteractionHand::MainHand));
    }

    #[test]
    fn removing_an_entity_drops_its_use_clock() {
        let mut t = t();
        t.set_hand_item(1, InteractionHand::MainHand, usable(7, 40));
        t.set_living_flags(1, 1);
        t.remove(1);
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            t.use_state(1),
            UseState::default(),
            "a reused id must not inherit a use clock"
        );
    }

    #[test]
    fn health_alone_starts_the_death_clock() {
        // `isDeadOrDying()` is `getHealth() <= 0 || dead`, so a health update
        // is enough — the client does not need the death entity-event.
        let mut t = t();
        t.set_health(1, 0.0);
        assert!(t.death_state(1).is_dead_or_dying());
        assert!(!t.death_state(1).dead, "health alone must not set `dead`");
        t.tick_lerp();
        t.tick_lerp();
        assert_eq!(t.death_state(1).death_time, 2);
    }

    #[test]
    fn healing_back_above_zero_stops_the_clock_where_it_stood() {
        // Vanilla never rewinds `deathTime`; it simply stops incrementing when
        // the entity is no longer dying. Worth pinning because "reset on heal"
        // is the intuitive-but-wrong behaviour.
        let mut t = t();
        t.set_health(1, 0.0);
        t.tick_lerp();
        t.tick_lerp();
        t.set_health(1, 5.0);
        t.tick_lerp();
        assert_eq!(t.death_state(1).death_time, 2);
        assert!(!t.death_state(1).is_dead_or_dying());
    }

    #[test]
    fn the_death_event_survives_a_health_update() {
        // `die()` sets `dead`, which `isDeadOrDying()` ORs in — so a server
        // that re-sends full health after the death event does not resurrect
        // the corpse.
        let mut t = t();
        t.kill(1, false);
        t.set_health(1, 20.0);
        assert!(t.death_state(1).is_dead_or_dying(), "`dead` is sticky");
    }

    #[test]
    fn walking_alone_never_reaches_the_clamp() {
        // The clamp added for the hurt kick must be a no-op for movement: the
        // target is already `min(1, dist*4)` and the lerp cannot overshoot.
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for i in 0..200 {
            e.set_target(i as f64 * 0.5, 0.0, 0.0);
            e.tick(false);
            assert!(e.limb().1 <= 1.0);
        }
    }
}
