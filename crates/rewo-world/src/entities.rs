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
    /// `minecraft:charged_projectiles` list is non-empty — as its first
    /// projectile's item id (M234: the item model's `Charge` select asks what
    /// it is). The sole gate on `ArmPose::CrossbowHold`, and the one
    /// held-item property that comes from the *stack* rather than the item id.
    pub charged: Option<i32>,
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
        matches!(self, HandItem::Held(i) if i.charged.is_some())
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
    /// Evoker fangs id 4 — `clientSideAttackStarted = true` (M259): the bite
    /// runs from here, `getAnimationProgress` reaching 1 twenty ticks on.
    EvokerFangsBite,
    /// M358 — sheep id 10 — `eatAnimationTick = 40`, one less every client
    /// tick: the head lowers and nibbles (`getHeadEatPositionScale` /
    /// `getHeadEatAngleScale`).
    SheepEat,
    /// M367 — hoglin / zoglin id 4 — `attackAnimationRemainingTicks = 10`,
    /// one less each client tick: the headbutt (`HoglinModel
    /// .animateHeadbutt`).
    HoglinAttack,
}

impl EntityEvent {
    /// Dense count for the fixed-size per-entity store.
    pub const COUNT: usize = 7;

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

/// What an item frame holds (M194): the item's protocol id, its `MAP_ID` if
/// it is a filled map, and whether it has foil.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameItem {
    pub item_id: i32,
    pub map_id: Option<i32>,
    pub foil: bool,
}

/// An item frame's synced fields (M194). `facing` is `None` until slot 8
/// arrives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemFrameState {
    pub facing: Option<u8>,
    pub rotation: u8,
    pub item: Option<FrameItem>,
}

/// M362 — a projectile's client-side flight. Vanilla gives projectiles no
/// `InterpolationHandler` (a server position packet snaps them) and ticks
/// their physics on the client between the server's updates, which for an
/// arrow come once a second (`EntityType.ARROW`'s update interval 20).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    /// `getDefaultGravity()`, taken off the velocity's y after each move.
    pub gravity: f64,
    /// `getAirDrag()` (`applyInertia`), the velocity's factor after each move.
    pub drag: f64,
    /// An `AbstractArrow` (its synced `IN_GROUND`, index 10 BOOLEAN, ends
    /// the flight); a thrown item has none.
    pub arrow: bool,
}

/// One tick's crit-particle trail of a critical arrow
/// (`AbstractArrow.tick`'s `if (isCritArrow())` loop): where the arrow was
/// and the movement it moved by, before the tick's drag and gravity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CritTrail {
    pub origin: [f64; 3],
    pub movement: [f64; 3],
}

impl CritTrail {
    /// The four `addParticle(CRIT, ...)` calls: `origin + movement * i / 4`
    /// with the velocity `(-movement.x, -movement.y + 0.2, -movement.z)`.
    pub fn particles(&self) -> [([f64; 3], [f64; 3]); 4] {
        let (o, m) = (self.origin, self.movement);
        let vel = [-m[0], -m[1] + 0.2, -m[2]];
        std::array::from_fn(|i| {
            let i = i as f64;
            (
                [
                    o[0] + m[0] * i / 4.0,
                    o[1] + m[1] * i / 4.0,
                    o[2] + m[2] * i / 4.0,
                ],
                vel,
            )
        })
    }
}

/// M362 — `AbstractArrow` (arrow, spectral arrow, trident): gravity 0.05,
/// air drag 0.99.
pub const ARROW_FLIGHT: Flight = Flight {
    gravity: 0.05,
    drag: 0.99,
    arrow: true,
};
/// M362 — `ThrowableProjectile` (snowball, egg, ender pearl, potions,
/// experience bottle): gravity 0.03, drag 0.99.
pub const THROWN_FLIGHT: Flight = Flight {
    gravity: 0.03,
    drag: 0.99,
    arrow: false,
};

/// M362 — the flight an entity type flies with, if it is a projectile the
/// client ticks.
pub fn flight_of(name: &str) -> Option<Flight> {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "arrow" | "spectral_arrow" | "trident" => Some(ARROW_FLIGHT),
        "snowball" | "egg" | "ender_pearl" | "splash_potion" | "lingering_potion"
        | "experience_bottle" => Some(THROWN_FLIGHT),
        _ => None,
    }
}

/// M362 — `Projectile.lerpRotation(rotO, rot)`: the previous rotation
/// unwound to within ±180° of the target, then a fifth of the way there.
pub fn lerp_rotation(mut from: f32, to: f32) -> f32 {
    while to - from < -180.0 {
        from -= 360.0;
    }
    while to - from >= 180.0 {
        from += 360.0;
    }
    from + 0.2 * (to - from)
}

/// M361 — `Mth.rotLerp(a, from, to)`: `from + a · wrapDegrees(to − from)`,
/// so a turn across ±180° goes the short way round.
pub fn rot_lerp(alpha: f32, from: f32, to: f32) -> f32 {
    let a = alpha.clamp(0.0, 1.0);
    let mut d = (to - from) % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    from + a * d
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
    /// M361 — `yRotO` / `xRotO` / `yHeadRotO`: the rotations at the start
    /// of the current tick, which the render blends toward `yaw` / `pitch` /
    /// `head_yaw` by the partial tick (`Mth.rotLerp(partialTick, yRotO,
    /// yRot)`). Until M361 every entity was drawn at its tick rotation, so a
    /// turning mob (and a mount steered by the mouse) turned twenty times a
    /// second.
    prev_yaw: f32,
    prev_pitch: f32,
    prev_head_yaw: f32,
    /// M362 — the flight the client ticks this entity with (a projectile).
    flight: Option<Flight>,
    /// `AbstractArrow.isCritArrow()` (`ID_FLAGS` bit 1): a critical arrow
    /// trails crit particles while it flies.
    arrow_crit: bool,
    /// `Camel.updateWalkAnimation`: the camel's walk target is
    /// `min(distance * 6, 1)` eased by 0.2 (the default is `* 4`, `0.4`),
    /// and 0 while it is not standing.
    camel_walk: bool,
    camel_still: bool,
    /// M362 — `AbstractArrow.IN_GROUND` (or a block the client's own flight
    /// hit): the flight has ended.
    in_ground: bool,
    lerp_steps: u32,
    /// M266 — the `InterpolationHandler`'s length: vanilla's default 3, a
    /// display's 0 (it snaps) or its `teleport_duration`.
    lerp_len: u32,
    cur: [f64; 3],
    prev: [f64; 3],
    /// Walk-cycle phase (vanilla `animationPosition`) — advances by
    /// `limb_amount` each tick, so a still entity's limbs freeze.
    limb_swing: f32,
    /// Smoothed horizontal speed 0..1 (vanilla `animationSpeed`) — the
    /// swing amplitude. The server never sends limb angles; both are
    /// derived here from the entity's own motion, exactly as vanilla does.
    limb_amount: f32,
    /// `Entity.deltaMovement` (M141d), for the sound ramps that read
    /// `getDeltaMovement()`.
    ///
    /// **It is not a velocity derived from where the entity is going.** For a
    /// remote entity a client never integrates it into a position — the
    /// position comes from the server and is interpolated — so this is a
    /// *decaying echo of the last `set_entity_motion` packet* and nothing
    /// else. A bee gliding steadily across your view with no motion packets
    /// has a `deltaMovement` falling to zero while it is visibly moving, and
    /// its buzz fades with it. A finite difference over `cur` would be more
    /// truthful about the entity and less faithful to vanilla, which is why
    /// this is stored rather than computed.
    delta_movement: [f64; 3],
    /// `entity instanceof LivingEntity`, supplied by the caller from
    /// `rewo_data::entity_types::EntityClasses::is_living` — the `ticks_swing`
    /// precedent, because the class fact lives in a crate this one does not
    /// want a registry handle from.
    ///
    /// It decides whether the decay runs **at all**: the 0.98 scale and the
    /// deadband both live in `LivingEntity.aiStep`, so a minecart's velocity
    /// is frozen between packets. Both minecart behaviours' client branches
    /// (`OldMinecartBehavior.tick`'s else, `NewMinecartBehavior.tick`'s else)
    /// touch position and rotation only.
    ///
    /// Defaulting to `false` before the first motion packet is exact rather
    /// than convenient: the velocity is zero until then, and every rule below
    /// maps zero to zero.
    motion_living: bool,
    /// `entity.is(EntityTypes.PLAYER)` — the deadband has **two forms** and
    /// this picks between them. See [`Self::tick`].
    motion_player: bool,
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
    /// M169 — `Mob.isSaddled()`, read off the SADDLE equipment slot (7).
    /// Vanilla's is `hasItemInSlot(slot) && isEquippableInSlot(item, slot)`
    /// (`Mob.java:860-862`); the client takes the slot being non-empty, since
    /// the server only ever equips a valid item there.
    saddled: bool,
    /// M169 — `Camel.dashCooldown` / `AbstractNautilus.dashCooldown`, the
    /// client-side counter `getJumpCooldown()` returns. Armed by the DASH
    /// metadata (see [`EntityTable::arm_dash_cooldown`]), decremented per
    /// tick.
    dash_cooldown: i32,
    /// M169 — `Camel.LAST_POSE_CHANGE_TICK` (index 20, LONG), the input
    /// to `isCamelSitting()` (`< 0`) and `isInPoseTransition()`.
    last_pose_change_tick: i64,
    /// M169 — `!Entity.firstTick`: set once this entity has been ticked.
    /// `onSyncedDataUpdated`'s dash arming is gated on `!this.firstTick`,
    /// so the spawn-time metadata never arms a cooldown.
    ticked: bool,
    /// M394 — `Entity.tickCount`: 0 when the client creates the entity
    /// (it is not synced), +1 each tick before the entity's tick.
    pub tick_count: i32,
    /// M395 — `Entity.onGround()`, which the client sets from every move,
    /// position-sync and teleport packet (`handleMoveEntity` and kin).
    pub on_ground: bool,
    /// M395 — `AbstractCubeMob`'s client-side squish: `squish`, `oSquish`,
    /// `targetSquish` and `wasOnGround` ([`Self::tick_squish`]).
    pub squish: f32,
    pub o_squish: f32,
    pub target_squish: f32,
    pub was_on_ground: bool,
    /// M400 — `LivingEntity.swimAmount` / `swimAmountO` (`updateSwimAmount`:
    /// ±0.09 a tick toward 1 while visually swimming, else 0).
    pub swim_amount: f32,
    pub swim_amount_o: f32,
    /// `AbstractHorse`'s rearing, eating and mouth clocks and its flags byte
    /// ([`crate::equine`]); all zero for anything else.
    equine: crate::equine::EquineAnim,
    /// `ClientboundAddEntityPacket.data` (M194) — per-type meaning; an item
    /// frame's facing (`Direction.from3DDataValue`). 0 when absent.
    pub spawn_data: i32,
}

/// `Sniffer.State.SEARCHING` — ordinal 4.
pub const SNIFFER_SEARCHING: u8 = 4;
/// `Sniffer.State.DIGGING` — ordinal 5.
pub const SNIFFER_DIGGING: u8 = 5;

/// `Guardian.getAttackDuration()` for the base species — the larger of the two,
/// and the cap this table applies. See [`EntityTable::tick_guardian_attacks`].
pub const GUARDIAN_MAX_ATTACK_DURATION: i32 = 80;

/// `Guardian`'s attack state — one synced field and one client-side counter.
///
/// The counter is **not** synced and is not derivable from anything that is:
/// vanilla runs it in `Guardian.aiStep`'s client branch, incrementing while
/// `hasActiveAttackTarget()` and capping at the species' attack duration. It
/// is the only input among the ten ramps that is not a decode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GuardianAttack {
    /// `DATA_ID_ATTACK_TARGET` — an entity id, with **0 meaning none**.
    target: i32,
    /// `clientSideAttackTime`.
    client_side_attack_time: i32,
}

impl GuardianAttack {
    /// `hasActiveAttackTarget()`. One definition, two callers.
    fn has_target(&self) -> bool {
        self.target != 0
    }
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
            prev_yaw: yaw,
            prev_pitch: pitch,
            prev_head_yaw: yaw,
            flight: None,
            arrow_crit: false,
            camel_walk: false,
            camel_still: false,
            in_ground: false,
            lerp_steps: 0,
            lerp_len: LERP_STEPS,
            cur: [x, y, z],
            prev: [x, y, z],
            limb_swing: 0.0,
            limb_amount: 0.0,
            delta_movement: [0.0; 3],
            motion_living: false,
            motion_player: false,
            cloak: CloakAnchor::default(),
            fall_fly_ticks: 0,
            saddled: false,
            dash_cooldown: 0,
            last_pose_change_tick: 0,
            ticked: false,
            tick_count: 0,
            on_ground: false,
            squish: 0.0,
            o_squish: 0.0,
            target_squish: 0.0,
            was_on_ground: false,
            swim_amount: 0.0,
            swim_amount_o: 0.0,
            equine: crate::equine::EquineAnim::default(),
            spawn_data: 0,
        }
    }

    pub fn set_head_yaw(&mut self, yaw: f32) {
        self.head_yaw = yaw;
    }

    /// Absolute target (teleport / position sync): start a fresh lerp of the
    /// entity's length (3 unless set), or with a length of 0 snap there,
    /// last tick's position included (`InterpolationHandler.interpolateTo`'s
    /// `snapTo`, M266).
    pub fn set_target(&mut self, x: f64, y: f64, z: f64) {
        self.x = x;
        self.y = y;
        self.z = z;
        if self.lerp_len == 0 {
            self.cur = [x, y, z];
            self.prev = [x, y, z];
            self.lerp_steps = 0;
        } else {
            self.lerp_steps = self.lerp_len;
        }
    }

    /// M266 — `InterpolationHandler.setInterpolationLength`.
    pub fn set_lerp_len(&mut self, steps: u32) {
        self.lerp_len = steps;
    }

    /// Relative move (the short-delta packets): accumulate onto the synced
    /// target, never onto the rendered position.
    pub fn nudge(&mut self, dx: f64, dy: f64, dz: f64) {
        self.set_target(self.x + dx, self.y + dy, self.z + dz);
    }

    /// M395 — `AbstractCubeMob.tick`'s squish, in its order: `oSquish =
    /// squish; squish += (targetSquish − squish)·0.5`; landing (`onGround`
    /// and not `wasOnGround`) aims at −0.5, leaving the ground at 1.0;
    /// `wasOnGround = onGround`; `targetSquish *= 0.6`. Kept for every
    /// entity (only cube mobs draw it).
    pub fn tick_squish(&mut self) {
        self.o_squish = self.squish;
        self.squish += (self.target_squish - self.squish) * 0.5;
        if self.on_ground && !self.was_on_ground {
            self.target_squish = -0.5;
        } else if !self.on_ground && self.was_on_ground {
            self.target_squish = 1.0;
        }
        self.was_on_ground = self.on_ground;
        self.target_squish *= 0.6;
    }

    /// M400 — `LivingEntity.updateSwimAmount`.
    pub fn tick_swim(&mut self, visually_swimming: bool) {
        self.swim_amount_o = self.swim_amount;
        self.swim_amount = if visually_swimming {
            (self.swim_amount + 0.09).min(1.0)
        } else {
            (self.swim_amount - 0.09).max(0.0)
        };
    }

    /// M400 — `getSwimAmount(a)`: `Mth.lerp(a, swimAmountO, swimAmount)`.
    pub fn swim_at(&self, alpha: f32) -> f32 {
        self.swim_amount_o + alpha * (self.swim_amount - self.swim_amount_o)
    }

    /// M395 — `Mth.lerp(a, oSquish, squish)`.
    pub fn squish_at(&self, alpha: f32) -> f32 {
        self.o_squish + alpha * (self.squish - self.o_squish)
    }

    pub fn set_rot(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// `LivingEntity.aiStep`'s velocity half — the 0.98 decay and the deadband
    /// (`LivingEntity.java:3038-3073`), M141d.
    ///
    /// ```java
    /// if (this.isInterpolating()) { this.getInterpolation().interpolate(); }
    /// else if (!this.canSimulateMovement()) {
    ///    this.setDeltaMovement(this.getDeltaMovement().scale(0.98));
    /// }
    /// ```
    ///
    /// **The decay is the `else` of the interpolation branch**, so a remote
    /// entity that is still catching up to a synced position does not decay at
    /// all — its velocity is frozen for those ticks and only starts falling
    /// once it arrives. That is why this shares [`Self::tick`]'s branch rather
    /// than running beside it.
    ///
    /// `authoritative` is `canSimulateMovement()`, which is
    /// `isLocalInstanceAuthoritative()`, which on a client is
    /// `isLocalClientAuthoritative()` — and `Player.isLocalClientAuthoritative`
    /// is `isLocalPlayer()`, so the only source of authority is the local
    /// player riding this entity. Rewo's local player is not in the table at
    /// all, so for everything else it is false.
    ///
    /// **The deadband has two forms and they genuinely differ.** A player's is
    /// joint (`horizontalDistanceSqr() < 9.0E-6`, so x and z fall together);
    /// everything else's is per-axis (`|x| < 0.003`). At `(0.0025, 0.0025)`
    /// the per-axis rule zeroes both and the joint rule keeps them, because
    /// the magnitude is 0.00354. `physics.rs` already records this for the
    /// local player from the other side.
    fn tick_motion(&mut self, interpolating: bool, authoritative: bool) {
        if !self.motion_living {
            // `aiStep` is `LivingEntity`'s. A minecart holds its last packet.
            return;
        }
        if !interpolating && !authoritative {
            for c in &mut self.delta_movement {
                *c *= 0.98;
            }
        }
        // Runs every tick regardless of the branch above, and reads ONE
        // snapshot: vanilla tests `movement.x` after having copied it, so an
        // axis zeroed here cannot change another axis's verdict.
        let [x, y, z] = self.delta_movement;
        let (mut dx, mut dy, mut dz) = (x, y, z);
        if self.motion_player {
            if x * x + z * z < 9.0e-6 {
                dx = 0.0;
                dz = 0.0;
            }
        } else {
            if x.abs() < 0.003 {
                dx = 0.0;
            }
            if z.abs() < 0.003 {
                dz = 0.0;
            }
        }
        if y.abs() < 0.003 {
            dy = 0.0;
        }
        self.delta_movement = [dx, dy, dz];
    }

    fn tick(&mut self, fall_flying: bool, authoritative: bool) {
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
        // M361 — and `setOldPosAndRot()`'s rotation half.
        self.prev_yaw = self.yaw;
        self.prev_pitch = self.pitch;
        self.prev_head_yaw = self.head_yaw;
        // `aiStep`'s interpolate-or-decay branch: the velocity step shares
        // this `if`, because in vanilla they ARE one if/else.
        self.tick_motion(self.lerp_steps > 0, authoritative);
        if self.lerp_steps > 0 {
            let n = self.lerp_steps as f64;
            self.cur[0] += (self.x - self.cur[0]) / n;
            self.cur[1] += (self.y - self.cur[1]) / n;
            self.cur[2] += (self.z - self.cur[2]) / n;
            self.lerp_steps -= 1;
        }
        // Walk animation from this tick's horizontal displacement (vanilla
        // LivingEntity.aiStep). A vehicle the local player steers moves AFTER
        // this (the session's own simulation), so its walk is updated by
        // [`Self::update_walk_after_move`] once it has.
        if !authoritative {
            self.update_walk(before);
        }
    }

    /// `LivingEntity.calculateEntityAnimation` → `updateWalkAnimation`:
    /// `target = min(distance * 4, 1)`, `speed += (target - speed) * 0.4`,
    /// the phase advances by the new speed. A camel's is `* 6` and `0.2`,
    /// and 0 while it is not standing (`Camel.updateWalkAnimation`).
    fn update_walk(&mut self, from: [f64; 3]) {
        let dx = self.cur[0] - from[0];
        let dz = self.cur[2] - from[2];
        let distance = (dx * dx + dz * dz).sqrt() as f32;
        let (scale, factor) = if self.camel_walk {
            (6.0, 0.2)
        } else {
            (4.0, 0.4)
        };
        let target = if self.camel_walk && self.camel_still {
            0.0
        } else {
            (distance * scale).min(1.0)
        };
        self.limb_amount += (target - self.limb_amount) * factor;
        self.limb_swing += self.limb_amount;
    }

    /// The walk animation of a mount the local player steers, once the
    /// session has moved it this tick: the displacement since the tick's
    /// start (`prev`, which [`Self::tick`] set). Until M-playtest a steered
    /// horse or camel stood on frozen limbs, because its position changes
    /// after the entity's own tick and the displacement that tick saw was 0.
    pub fn update_walk_after_move(&mut self) {
        self.update_walk(self.prev);
    }

    /// `AbstractHorse.tickRidden`'s `setRot(rider yaw, rider pitch * 0.5)`
    /// and `yHeadRot = yRot`: the mount looks where its rider does.
    pub fn set_ridden_look(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.head_yaw = yaw;
        self.pitch = pitch;
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

    /// M221 — the vehicle the local player steers is placed by the client's
    /// own simulation (`isLocalInstanceAuthoritative`): this tick's position
    /// and target both become `p`, with no interpolation left to run, so the
    /// render blends last tick's position into this one exactly as it does
    /// for the player.
    pub fn set_simulated(&mut self, p: [f64; 3], yaw: f32) {
        self.x = p[0];
        self.y = p[1];
        self.z = p[2];
        self.cur = p;
        self.lerp_steps = 0;
        self.yaw = yaw;
    }

    /// M362 — `handleAddEntity`'s `setDeltaMovement(packet.getMovement())`:
    /// the spawn packet's velocity (until M362 it was skipped, so an arrow
    /// was born standing still). A non-finite one is ignored.
    pub fn set_spawn_motion(&mut self, v: [f64; 3]) {
        if v.iter().all(|c| c.is_finite()) {
            self.delta_movement = v;
        }
    }

    /// M362 — make this entity a projectile the client flies; its server
    /// positions snap rather than lerp (no `InterpolationHandler`).
    pub fn set_flight(&mut self, flight: Flight) {
        self.flight = Some(flight);
        self.lerp_len = 0;
    }

    /// M362 — one tick of the client's flight (`AbstractArrow.tick` /
    /// `ThrowableProjectile.tick`'s movement): the rotation turns a fifth of
    /// the way toward the movement (`lerpRotation`, `atan2(x, z)` and
    /// `atan2(y, horizontal)`), the move is clipped against `solid` (a point
    /// test on the blocks' collision boxes) and a hit ends the flight there,
    /// else the entity moves by the whole movement; then the drag and the
    /// gravity. Not modelled: water's inertia (and its bubbles), entity hits
    /// (the server removes the projectile). A critical arrow in flight
    /// returns its crit trail.
    pub fn step_flight(&mut self, solid: &dyn Fn([f64; 3]) -> bool) -> Option<CritTrail> {
        let flight = self.flight?;
        if self.in_ground {
            return None;
        }
        let trail = (flight.arrow && self.arrow_crit).then_some(CritTrail {
            origin: self.cur,
            movement: self.delta_movement,
        });
        let m = self.delta_movement;
        let horizontal = (m[0] * m[0] + m[2] * m[2]).sqrt();
        if m.iter().any(|c| *c != 0.0) {
            let yaw = m[0].atan2(m[2]).to_degrees() as f32;
            let pitch = m[1].atan2(horizontal).to_degrees() as f32;
            self.yaw = lerp_rotation(self.yaw, yaw);
            self.pitch = lerp_rotation(self.pitch, pitch);
        }
        let len = (horizontal * horizontal + m[1] * m[1]).sqrt();
        let steps = ((len / 0.05).ceil() as usize).clamp(1, 400);
        let start = self.cur;
        let mut hit = None;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let p = [0, 1, 2].map(|k| start[k] + m[k] * t);
            if solid(p) {
                let t0 = (i - 1) as f64 / steps as f64;
                hit = Some([0, 1, 2].map(|k| start[k] + m[k] * t0));
                break;
            }
        }
        match hit {
            Some(p) => {
                self.cur = p;
                self.delta_movement = [0.0; 3];
                self.in_ground = true;
            }
            None => {
                self.cur = [0, 1, 2].map(|k| start[k] + m[k]);
                for c in &mut self.delta_movement {
                    *c *= flight.drag;
                }
                self.delta_movement[1] -= flight.gravity;
            }
        }
        [self.x, self.y, self.z] = self.cur;
        trail
    }

    /// M377 — the camel's / nautilus's dash cooldown in ticks (0 when none).
    pub fn dash_cooldown(&self) -> i32 {
        self.dash_cooldown
    }

    /// M361 — the frame's body yaw: last tick's blended toward this tick's
    /// along the short way round (`Mth.rotLerp`).
    pub fn render_yaw(&self, alpha: f32) -> f32 {
        rot_lerp(alpha, self.prev_yaw, self.yaw)
    }

    /// M361 — the frame's pitch (`Mth.lerp(partialTick, xRotO, xRot)`).
    pub fn render_pitch(&self, alpha: f32) -> f32 {
        let a = alpha.clamp(0.0, 1.0);
        self.prev_pitch + (self.pitch - self.prev_pitch) * a
    }

    /// M361 — the frame's head yaw (`Mth.rotLerp(partialTick, yHeadRotO,
    /// yHeadRot)`).
    pub fn render_head_yaw(&self, alpha: f32) -> f32 {
        rot_lerp(alpha, self.prev_head_yaw, self.head_yaw)
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
/// M309 — a mannequin's `DATA_PROFILE` (`ResolvableProfile`): the id and
/// name of its game profile or partial profile, the `textures` property if
/// it carries one, and its `PlayerSkin.Patch`'s body texture and model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AvatarProfile {
    pub id: Option<u128>,
    pub name: Option<String>,
    pub textures: Option<String>,
    /// The patch's body texture id (`minecraft:entity/player/wide/steve`).
    pub body_texture: Option<String>,
    /// The patch's model: `Some(true)` slim, `Some(false)` wide.
    pub model_slim: Option<bool>,
}

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
    /// M311 — `isDamageableItem()`'s inputs off the same patch: `None` for a
    /// piece that cannot wear (`unbreakable`, or `damage` / `max_damage`
    /// removed); otherwise `(damage, patched max_damage)`, the damage 0 when
    /// unpatched and the maximum left to the item's prototype when `None`.
    pub wear: Option<(i32, Option<i32>)>,
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
    /// `Entity.DATA_SILENT` — metadata index **4**, BOOLEAN (M138a).
    ///
    /// Same shape and same reasoning as [`Self::custom_name_visible`] one slot
    /// up: membership means `true`, the router removes the id on `false` so a
    /// toggle back on is not a latch, and absent is exactly `false` because
    /// `Entity.java:322` seeds it. Cleared on removal.
    ///
    /// This is what `Entity.isSilent()` reads, and it gates `playSound`, so a
    /// wrong answer here is a sound that plays when a server said it should
    /// not. Until M138a nothing stored it and the sound engine's world answered
    /// a hardcoded `false`.
    silent: std::collections::HashSet<i32>,
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
    /// `Bee.DATA_ANGER_END_TIME` (M141f), by entity id. Cleared on removal and
    /// on (re-)add.
    anger_end_time: HashMap<i32, i64>,
    /// `Guardian`'s synced attack target and the client counter it drives
    /// (M141g). Cleared on removal and on (re-)add.
    guardian_attack: HashMap<i32, GuardianAttack>,
    /// The local player's entity id, if the session has told us (M141d).
    ///
    /// The local player is not a row in this table; this exists only so
    /// `Entity.canSimulateMovement()` can be answered for a vehicle it rides.
    local_player: Option<i32>,
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
    /// M194 — an item frame's synced state (`HangingEntity.DATA_DIRECTION`,
    /// slot 8; `ItemFrame.DATA_ITEM`, 9; `DATA_ROTATION`, 10), for the two
    /// frame types only. Absent fields fall back to the spawn data / vanilla
    /// defaults (see [`EntityTable::item_frame`]). Cleared on removal.
    item_frames: HashMap<i32, ItemFrameState>,
    /// Worn armour item ids per entity, head first (M46). Cleared on removal
    /// and on (re-)add, so a reused id cannot wear the last occupant's
    /// chestplate.
    armor: HashMap<i32, [Option<WornPiece>; 4]>,
    /// M273 — the `textures` property of a worn player head's profile, the
    /// skin `CustomHeadLayer` draws it in. Cleared with the armour.
    head_profiles: HashMap<i32, String>,
    /// M289 — the `textures` property of a dropped or framed player head's
    /// profile (`ItemEntity.DATA_ITEM` / `ItemFrame.DATA_ITEM`). Cleared on
    /// removal.
    stack_profiles: HashMap<i32, String>,
    /// M290 — the `textures` property of a player head in either hand
    /// (`ClientboundSetEquipmentPacket`, slots 0 and 1), by
    /// `InteractionHand`. Cleared on removal.
    hand_profiles: HashMap<i32, [Option<String>; 2]>,
    /// M309 — a mannequin's synched profile (`Mannequin.DATA_PROFILE`).
    /// Cleared on removal.
    avatar_profiles: HashMap<i32, AvatarProfile>,
    /// M294 — the body slot (`EquipmentSlot.BODY`, wire 6): a horse's armour,
    /// a wolf's, a llama's carpet. Cleared on removal.
    body_armor: HashMap<i32, WornPiece>,
    /// M295 — the saddle slot's piece (`EquipmentSlot.SADDLE`, wire 7), for
    /// its equipment layer; `saddled` stays the gameplay flag. Cleared on
    /// removal.
    saddles: HashMap<i32, WornPiece>,
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
    /// M255 — a display entity's synced render state ([`DisplayState`]).
    /// Cleared on removal.
    displays: HashMap<i32, DisplayState>,
    /// M261 — a minecart's synced display ([`MinecartDisplay`]). Cleared on
    /// removal.
    minecart_displays: HashMap<i32, MinecartDisplay>,
    /// M269 — primed TNT's fuse and block ([`PrimedState`]). Cleared on
    /// removal.
    primed: HashMap<i32, PrimedState>,
}

/// M277 — `Creeper.maxSwell`'s default (the `Fuse` tag that changes it is
/// not synced, so a client always counts to 30).
pub const CREEPER_MAX_SWELL: i32 = 30;

/// M269 — primed TNT as synced: `PrimedTnt.DATA_FUSE_ID` (index 8, INT),
/// counted down each client tick as `PrimedTnt.tick` does on the client
/// too, and `DATA_BLOCK_STATE_ID` (9, BLOCK_STATE; `None` is vanilla's
/// default, TNT).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrimedState {
    pub fuse: Option<i32>,
    pub block: Option<u32>,
}

/// M261 — what a minecart carries, as synced: `AbstractMinecart`'s
/// `DATA_ID_CUSTOM_DISPLAY_BLOCK` (index 11, OPTIONAL_BLOCK_STATE) and
/// `DATA_ID_DISPLAY_OFFSET` (12, INT), and `MinecartFurnace.DATA_ID_FUEL`
/// (13, BOOLEAN). `None` is "not sent", which is the type's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MinecartDisplay {
    pub custom_block: Option<u32>,
    pub offset: Option<i32>,
    pub fuel: bool,
}

/// M255 — a display entity's synced render state: `Display`'s
/// `Transformation` and billboard mode, and `Display.TextDisplay`'s text and
/// style. Every field defaults to vanilla's `define(...)` value.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayState {
    /// `DATA_TRANSLATION_ID` (index 11).
    pub translation: [f32; 3],
    /// `DATA_SCALE_ID` (12).
    pub scale: [f32; 3],
    /// `DATA_LEFT_ROTATION_ID` / `DATA_RIGHT_ROTATION_ID` (13, 14),
    /// quaternions `[x, y, z, w]`.
    pub left_rotation: [f32; 4],
    pub right_rotation: [f32; 4],
    /// `DATA_BILLBOARD_RENDER_CONSTRAINTS_ID` (15): 0 fixed, 1 vertical, 2
    /// horizontal, 3 centre.
    pub billboard: u8,
    /// `TextDisplay.DATA_TEXT_ID` (23), as styled runs `(text, sRGB colour)`.
    pub text: Vec<(String, [f32; 3])>,
    /// `DATA_LINE_WIDTH_ID` (24), font px.
    pub line_width: i32,
    /// `DATA_BACKGROUND_COLOR_ID` (25), ARGB.
    pub background: i32,
    /// `DATA_TEXT_OPACITY_ID` (26).
    pub text_opacity: u8,
    /// `DATA_STYLE_FLAGS_ID` (27): 1 shadow, 2 see-through, 4 default
    /// background, 8 / 16 left / right alignment.
    pub style: u8,
    /// M256 — `ItemDisplay.DATA_ITEM_STACK_ID` (23): `(item id, foil)`,
    /// `None` empty.
    pub item: Option<(i32, bool)>,
    /// M256 — `ItemDisplay.DATA_ITEM_DISPLAY_ID` (24): the
    /// `ItemDisplayContext` id (0 none, 1/2 third-person left/right, 3/4
    /// first-person left/right, 5 head, 6 gui, 7 ground, 8 fixed, 9 shelf).
    pub item_context: u8,
    /// M260 — `BlockDisplay.DATA_BLOCK_STATE_ID` (23): the state id; `None`
    /// is air, vanilla's default.
    pub block: Option<u32>,
    /// M265 — the transformation's interpolation (`Display.tick`'s render
    /// state). The four fields above are the synced target.
    pub interp: DisplayInterp,
    /// M267 — `DATA_BRIGHTNESS_OVERRIDE_ID` (16): packed block and sky
    /// light (`LightCoordsUtil.pack`), -1 for none.
    pub brightness: i32,
}

/// M265 — a display's `Transformation`: translation, left rotation, scale,
/// right rotation (quaternions `[x, y, z, w]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayTransformation {
    pub translation: [f32; 3],
    pub left_rotation: [f32; 4],
    pub scale: [f32; 3],
    pub right_rotation: [f32; 4],
}

impl Default for DisplayTransformation {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            left_rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            right_rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

impl DisplayTransformation {
    /// `Transformation.slerp`: translation and scale linear, both rotations
    /// JOML's `Quaternionf.slerp` (the shorter way round).
    pub fn slerp(&self, to: &Self, t: f32) -> Self {
        let lerp3 = |a: [f32; 3], b: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
        };
        Self {
            translation: lerp3(self.translation, to.translation),
            left_rotation: quat_slerp(self.left_rotation, to.left_rotation, t),
            scale: lerp3(self.scale, to.scale),
            right_rotation: quat_slerp(self.right_rotation, to.right_rotation, t),
        }
    }
}

/// JOML's `Quaternionf.slerp(target, alpha)`: the sign flipped toward the
/// shorter arc, a linear blend when the two are nearly equal.
fn quat_slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let cosom = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let abs = cosom.abs();
    let (s0, mut s1) = if 1.0 - abs > 1e-6 {
        let sinsqr = 1.0 - abs * abs;
        let sinom = 1.0 / sinsqr.sqrt();
        let omega = sinsqr.sqrt().atan2(abs);
        (((1.0 - t) * omega).sin() * sinom, (t * omega).sin() * sinom)
    } else {
        (1.0 - t, t)
    };
    if cosom < 0.0 {
        s1 = -s1;
    }
    std::array::from_fn(|i| s0 * a[i] + s1 * b[i])
}

/// M265 — `Display`'s client-side interpolation state: the synced start
/// delay (index 8) and duration (index 9), the flags `onSyncedDataUpdated`
/// raises, and the render state `tick` rebuilds from them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayInterp {
    /// `DATA_TRANSFORMATION_INTERPOLATION_START_DELTA_TICKS_ID` (8).
    pub delay: i32,
    /// `DATA_TRANSFORMATION_INTERPOLATION_DURATION_ID` (9).
    pub duration_synced: i32,
    /// `updateStartTick`, `updateInterpolationDuration`, `updateRenderState`.
    pub pending_start: bool,
    pub pending_duration: bool,
    pub pending_render: bool,
    /// `tickCount`, advanced once per client tick.
    ticks: i64,
    /// `interpolationStartClientTick` and `interpolationDuration`.
    start: i64,
    duration: i32,
    /// The render state's interpolator `(previous, current)`; `None` before
    /// the first tick (vanilla draws nothing then).
    render: Option<(DisplayTransformation, DisplayTransformation)>,
}

impl DisplayInterp {
    /// `calculateInterpolationProgress(partialTicks)`.
    fn progress(&self, partial: f32) -> f32 {
        if self.duration <= 0 {
            return 1.0;
        }
        (((self.ticks - self.start) as f32 + partial) / self.duration as f32).clamp(0.0, 1.0)
    }

    /// `Display.tick`'s client branch (the level has already advanced
    /// `tickCount`): the start tick, then the duration, then, if a render
    /// field changed, a fresh render state (no interpolation, or none yet) or
    /// an interpolated one from where the old one had got to. That point is
    /// the progress the last frame drew, `lastProgress`, so it is measured
    /// before the new start and duration apply.
    pub fn tick(&mut self, target: DisplayTransformation) {
        self.ticks += 1;
        let last = self.progress(0.0);
        if self.pending_start {
            self.pending_start = false;
            self.start = self.ticks + i64::from(self.delay);
        }
        if self.pending_duration {
            self.pending_duration = false;
            self.duration = self.duration_synced;
        }
        if self.pending_render {
            self.pending_render = false;
            self.render = Some(match self.render {
                Some((from, to)) if self.duration != 0 => {
                    let now = if last >= 1.0 {
                        to
                    } else {
                        from.slerp(&to, last)
                    };
                    (now, target)
                }
                _ => (target, target),
            });
        }
    }

    /// The transformation to draw at `partial` ticks past this tick:
    /// `TransformationInterpolator.get(progress)`, or `target` before the
    /// first tick.
    pub fn transformation(
        &self,
        target: DisplayTransformation,
        partial: f32,
    ) -> DisplayTransformation {
        match self.render {
            None => target,
            Some((from, to)) => {
                let p = self.progress(partial);
                if p >= 1.0 {
                    to
                } else {
                    from.slerp(&to, p)
                }
            }
        }
    }
}

impl DisplayState {
    /// M265 — the synced target transformation.
    pub fn target(&self) -> DisplayTransformation {
        DisplayTransformation {
            translation: self.translation,
            left_rotation: self.left_rotation,
            scale: self.scale,
            right_rotation: self.right_rotation,
        }
    }

    /// M265 — the transformation to draw at `partial` ticks.
    pub fn render_transformation(&self, partial: f32) -> DisplayTransformation {
        self.interp.transformation(self.target(), partial)
    }
}

impl Default for DisplayState {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            scale: [1.0; 3],
            left_rotation: [0.0, 0.0, 0.0, 1.0],
            right_rotation: [0.0, 0.0, 0.0, 1.0],
            billboard: 0,
            text: Vec::new(),
            line_width: 200,
            background: 0x4000_0000,
            text_opacity: 255,
            style: 0,
            item: None,
            item_context: 0,
            block: None,
            interp: DisplayInterp::default(),
            brightness: -1,
        }
    }
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MobState {
    /// `Mob.DATA_MOB_FLAGS_ID`. Bit 1 no-AI, bit 2 left-handed, bit 4
    /// aggressive.
    pub flags: u8,
    /// M277 — `Creeper.swell` and `oldSwell`, the client's own count toward
    /// `maxSwell` (30; the `Fuse` that sets it is not synced).
    pub swell: i32,
    pub old_swell: i32,
    /// M238 — `ArmorStand.DATA_CLIENT_FLAGS` (index 15 BYTE too, a different
    /// class): 0x01 small, 0x04 show arms, 0x08 no base plate, 0x10 marker.
    pub stand_flags: u8,
    /// M270 — `ArmorStand`'s six pose rotations (indices 16-21: head, body,
    /// left arm, right arm, left leg, right leg), degrees; `None` is
    /// vanilla's default for that part.
    pub stand_pose: [Option<[f32; 3]>; 6],
    /// M239 — `FireworkRocketEntity.DATA_ATTACHED_TO_TARGET`: the entity a
    /// rocket is attached to (an elytra boost), carried here so it is
    /// cleared with the entity like every other per-entity state.
    pub attached_to: Option<i32>,
    /// `Raider.isCelebrating()`.
    pub celebrating: bool,
    /// `SpellcasterIllager.DATA_SPELL_CASTING_ID`; `isCastingSpell()` is `> 0`.
    pub spell_casting: u8,
    /// `Pillager.isChargingCrossbow()` (index 17), or M389 the piglin's
    /// (`Piglin.DATA_IS_CHARGING_CROSSBOW`, index 18).
    pub charging_crossbow: bool,
    /// M389 — `Piglin.DATA_IS_DANCING` (index 19, BOOLEAN; `AbstractPiglin`
    /// owns 16, the piglin's baby flag is 17, charging 18).
    pub piglin_dancing: bool,
    /// M393 — a boat's `VehicleEntity.DATA_ID_HURT` (8, INT),
    /// `DATA_ID_HURTDIR` (9, INT) and `DATA_ID_DAMAGE` (10, FLOAT). The
    /// client counts the hurt and damage down each tick as the server does
    /// (`AbstractBoat.tick` runs on both sides).
    pub boat_hurt: i32,
    pub boat_hurt_dir: i32,
    pub boat_damage: f32,
    /// M393 — `AbstractBoat.DATA_ID_PADDLE_LEFT` / `_RIGHT` (11, 12), or the
    /// steered boat's own (`controlBoat`'s `setPaddleState`).
    pub boat_paddles: [bool; 2],
    /// M393 — `AbstractBoat.paddlePositions`, kept by the client: +π/8 a
    /// tick while `getPaddleState(side)` (paddling with a controlling
    /// passenger), else 0.
    pub boat_paddle_pos: [f32; 2],
    /// M401 — `LivingEntity.SLEEPING_POS_ID` (14, OPTIONAL_BLOCK_POS) for a
    /// remote entity: the bed block its sleeping pose lies in.
    pub sleeping_pos: Option<(i32, i32, i32)>,
    /// M396 — a sulfur cube's client-side `fuse` (`None` is vanilla's −1,
    /// not primed): set from `MAX_FUSE` (19, INT) on every sync
    /// (`onSyncedDataUpdated`), counted down a tick at a time
    /// (`tickFuse`'s `if (fuse > 0) fuse--`).
    pub sulfur_fuse: Option<i32>,
    /// M394 — `AbstractBoat.DATA_ID_BUBBLE_TIME` (13, INT), and the client's
    /// `bubbleMultiplier`, `bubbleAngle` and `bubbleAngleO`
    /// (`tickBubbleColumn`).
    pub boat_bubble_time: i32,
    pub boat_bubble_mult: f32,
    pub boat_bubble_angle: f32,
    pub boat_bubble_angle_o: f32,
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
    /// M285 — `Creeper.DATA_IS_POWERED` (index 17, BOOLEAN): a charged
    /// creeper. `Creeper` declares `DATA_SWELL_DIR` (16) first.
    pub creeper_powered: bool,
    /// M296 — `AbstractChestedHorse.DATA_ID_CHEST` (index 19, BOOLEAN).
    pub has_chest: bool,
    /// M365 — `Turtle.HAS_EGG` (index 18, BOOLEAN; `AgeableMob` owns 16 and
    /// 17) and `LAYING_EGG` (19).
    pub turtle_egg: bool,
    pub turtle_laying: bool,
    /// M387 — `Axolotl.DATA_PLAYING_DEAD` (index 19, BOOLEAN; `AgeableMob`
    /// owns 16 and 17, the variant is 18).
    pub axolotl_playing_dead: bool,
    /// M288 — `WitherBoss.DATA_ID_INV` (index 19, INT): the invulnerable
    /// ticks of a freshly built wither (0 otherwise). `WitherBoss` declares
    /// the three head targets (16-18) first.
    pub wither_invulnerable: i32,
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
    /// M342 — `Wolf.DATA_COLLAR_COLOR` (index 21, INT), a `DyeColor` id.
    /// `None` when the server sent none, which is vanilla's default
    /// (`DEFAULT_COLLAR_COLOR`, red): the server sends only non-default
    /// metadata, so the reader of this field supplies the default.
    pub collar: Option<i32>,
    /// M352 — `EnderMan.DATA_CARRY_STATE` (index 16, OPTIONAL_BLOCK_STATE):
    /// the block state an enderman carries; `None` for empty (the wire's 0).
    pub carried_block: Option<u32>,
    /// M353 — `SnowGolem.DATA_PUMPKIN_ID` (index 16, BYTE; 0x10 is the
    /// pumpkin). `None` when the server sent none: vanilla's default has it.
    pub snow_golem_flags: Option<u8>,
    /// M354 — `Bee.DATA_FLAGS_ID` (index 18, BYTE): 0x02 rolling, 0x04 has
    /// stung, 0x08 has nectar; 0 by default.
    pub bee_flags: Option<u8>,
    /// `TamableAnimal.DATA_FLAGS_ID` — index **18**, BYTE, bit 0x04 is
    /// `isTame()` and bit 0x01 `isInSittingPose()` (M64).
    ///
    /// The same slot *and the same serializer* as `Sheep.DATA_WOOL_ID`: both
    /// classes extend `Animal`, whose accessor count is zero, so both put
    /// their first byte at 18. Only the entity **kind** separates them — the
    /// M18 rule again, and the reason `apply_set_entity_data` gates this on
    /// the type being a cat or a wolf.
    pub tamable_flags: u8,
    /// `VillagerData` (serializer 18) of a villager or zombie villager: the
    /// raw `(villager_type id, villager_profession id, level)`. `None` until
    /// the server sends it, which is not vanilla's default (plains / none /
    /// 1): the layer draws that default only once it is told.
    pub villager_data: Option<(i32, i32, i32)>,
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

    /// M356 — `TamableAnimal.isInSittingPose()` — `(DATA_FLAGS_ID & 1) != 0`.
    pub fn is_in_sitting_pose(self) -> bool {
        self.tamable_flags & 1 != 0
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
        // A fresh entity at this id inherits nothing. The packet stream sends
        // `remove_entities` before reusing an id, but a dropped removal must
        // not leave the previous occupant's armour, anger clock, animation
        // timing or passenger links on its replacement: vanilla's
        // `ClientLevel.addEntity` runs `removeEntity(id, DISCARDED)` first, so
        // a re-add is a removal followed by an add. Every per-id side map is
        // therefore cleared here, by the same function `remove` uses.
        self.clear_side_state(id);
        self.map.insert(id, state);
    }

    pub fn remove(&mut self, id: i32) {
        self.map.remove(&id);
        self.clear_side_state(id);
    }

    /// Drop **every** per-entity side map's row for `id`.
    ///
    /// This is the one list of them, and it is exhaustive by construction:
    /// the destructuring below names every field of [`EntityTable`], so adding
    /// a map and forgetting to decide whether it is per-entity does not
    /// compile. `names` (profile UUID -> name, list membership rather than
    /// entity lifetime), the local player's id, the attachment table and the
    /// wavy-cape switch are the fields that are *not* per-entity, and are
    /// listed as such.
    fn clear_side_state(&mut self, id: i32) {
        // Every field, by name — see above. A per-entity one is cleared below.
        let EntityTable {
            map: _,
            names: _,
            custom_names,
            poses,
            gesture_states,
            sizes,
            babies,
            uses,
            deaths,
            attributes,
            shared_flags,
            custom_name_visible,
            silent,
            passengers: _,
            vehicle_of: _,
            anger_end_time,
            guardian_attack,
            local_player: _,
            attachments: _,
            model_customisation,
            item_stacks,
            item_frames,
            armor,
            head_profiles,
            stack_profiles,
            hand_profiles,
            avatar_profiles,
            body_armor,
            saddles,
            dances,
            events,
            swings: _,
            hands: _,
            main_arms: _,
            swing_effects: _,
            hurts,
            hurt_dirs,
            mob_state,
            wavy_capes,
            wavy_capes_enabled: _,
            minecarts,
            leash_data,
            projectile_power,
            displays,
            minecart_displays,
            primed,
        } = self;
        custom_names.remove(&id);
        poses.remove(&id);
        gesture_states.remove(&id);
        sizes.remove(&id);
        babies.remove(&id);
        events.remove(&id);
        dances.remove(&id);
        mob_state.remove(&id);
        hurts.remove(&id);
        // The direction outlives the *clock*, not the *entity*: vanilla's
        // `hurtDir` is a field on the Player object, and the object is gone.
        hurt_dirs.remove(&id);
        uses.remove(&id);
        deaths.remove(&id);
        item_stacks.remove(&id);
        item_frames.remove(&id);
        stack_profiles.remove(&id);
        hand_profiles.remove(&id);
        avatar_profiles.remove(&id);
        body_armor.remove(&id);
        saddles.remove(&id);
        attributes.remove(&id);
        shared_flags.remove(&id);
        custom_name_visible.remove(&id);
        silent.remove(&id);
        model_customisation.remove(&id);
        // A cape spine and a cart's schedule are clocks (M61, M77): a recycled
        // id must not inherit a chain or be dragged toward the previous
        // cart's rail.
        wavy_capes.remove(&id);
        minecarts.remove(&id);
        leash_data.remove(&id);
        projectile_power.remove(&id);
        displays.remove(&id);
        minecart_displays.remove(&id);
        primed.remove(&id);
        // The three the original `remove` forgot: worn armour (a reused id
        // would render the last occupant's chestplate), the bee's anger end
        // time and the guardian's beam target. Each also grew without bound
        // over a long session, one row per entity ever seen.
        armor.remove(&id);
        head_profiles.remove(&id);
        anger_end_time.remove(&id);
        guardian_attack.remove(&id);
        self.clear_riding(id);
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

    /// M169 — the SADDLE equipment slot changed.
    pub fn set_saddled(&mut self, id: i32, saddled: bool) {
        if let Some(e) = self.map.get_mut(&id) {
            e.saddled = saddled;
        }
    }

    /// `Mob.isSaddled()` (M169). An untracked entity is not saddled.
    pub fn saddled(&self, id: i32) -> bool {
        self.map.get(&id).is_some_and(|e| e.saddled)
    }

    /// `Camel.onSyncedDataUpdated` / `AbstractNautilus.onSyncedDataUpdated`
    /// for the DASH accessor (M169):
    ///
    /// ```java
    /// if (!this.firstTick && DASH.equals(accessor)) {
    ///    this.dashCooldown = this.dashCooldown == 0 ? 55 : this.dashCooldown;
    /// }
    /// ```
    /// (`Camel.java:645-647`; the nautilus's literal is 40.) Three things
    /// that invert: it fires on ANY DASH entry, true or false, because
    /// `assignValues` has no change guard; it does NOT restart a running
    /// cooldown; and the entity's spawn-time metadata arrives before its
    /// first tick and arms nothing.
    pub fn arm_dash_cooldown(&mut self, id: i32, ticks: i32) {
        if let Some(e) = self.map.get_mut(&id) {
            if e.ticked && e.dash_cooldown == 0 {
                e.dash_cooldown = ticks;
            }
        }
    }

    /// `AbstractHorse.DATA_ID_FLAGS` (index 17 BYTE) arrived: the tame, bred,
    /// eating, standing and open-mouth bits ([`crate::equine`]).
    pub fn set_equine_flags(&mut self, id: i32, flags: u8) {
        if let Some(e) = self.map.get_mut(&id) {
            e.equine.set_flags(flags);
        }
    }

    /// `AbstractHorse.onPlayerJump(amount)`'s rearing, run for the horse the
    /// local player steers (`isEffectiveAi()` is then true): see
    /// [`crate::equine::EquineAnim::on_player_jump`].
    pub fn equine_on_player_jump(&mut self, id: i32, jump_amount: i32, can_rear: bool) {
        if let Some(e) = self.map.get_mut(&id) {
            e.equine.on_player_jump(jump_amount, can_rear);
        }
    }

    /// The horse's rearing, eating and mouth clocks at partial tick `alpha`:
    /// `(getEatAnim, getStandAnim, getMouthAnim)`, all 0 for anything else.
    pub fn equine_anims(&self, id: i32, alpha: f32) -> (f32, f32, f32) {
        self.map.get(&id).map_or((0.0, 0.0, 0.0), |e| {
            (
                e.equine.eat_anim(alpha),
                e.equine.stand_anim(alpha),
                e.equine.mouth_anim(alpha),
            )
        })
    }

    /// `isStanding()` and `allowStandSliding`, for `getRiddenInput`.
    pub fn equine_standing(&self, id: i32) -> (bool, bool) {
        self.map.get(&id).map_or((false, false), |e| {
            (e.equine.is_standing(), e.equine.allow_stand_sliding())
        })
    }

    /// `getJumpCooldown()` for a camel or nautilus (M169); 0 otherwise.
    pub fn dash_cooldown(&self, id: i32) -> i32 {
        self.map.get(&id).map_or(0, |e| e.dash_cooldown)
    }

    /// `Camel.executeRidersJump`'s `dashCooldown = 55` (M225): the dash the
    /// local rider fired sets it outright, running or not.
    pub fn set_dash_cooldown(&mut self, id: i32, ticks: i32) {
        if let Some(e) = self.map.get_mut(&id) {
            e.dash_cooldown = ticks;
        }
    }

    /// `Camel.tickRidden`'s stand-up test, `isCamelSitting() &&
    /// !isInPoseTransition()` (M225): a sitting camel that has finished
    /// sitting down stands up when its rider pushes forward.
    pub fn camel_can_stand_up(&self, id: i32, game_time: i64) -> bool {
        self.map.get(&id).is_some_and(|e| {
            e.last_pose_change_tick < 0
                && game_time.wrapping_sub(e.last_pose_change_tick.wrapping_abs()) >= 40
        })
    }

    /// `Camel.LAST_POSE_CHANGE_TICK` arrived (M169).
    pub fn set_last_pose_change_tick(&mut self, id: i32, tick: i64) {
        if let Some(e) = self.map.get_mut(&id) {
            e.last_pose_change_tick = tick;
        }
    }

    /// `Camel.refuseToMove()` — `isCamelSitting() || isInPoseTransition()`
    /// (M169): sitting is `LAST_POSE_CHANGE_TICK < 0`, and the transition
    /// is `getPoseTime() < (sitting ? 40 : 52)` with `getPoseTime =
    /// gameTime - abs(lastPoseChangeTick)` (`Camel.java:267-269`,
    /// `:572-583`, `:630-632`). The `!refuseToMove()` half of the camel's
    /// `canJump()`.
    pub fn camel_refuse_to_move(&self, id: i32, game_time: i64) -> bool {
        let Some(e) = self.map.get(&id) else {
            return false;
        };
        let sitting = e.last_pose_change_tick < 0;
        let pose_time = game_time.wrapping_sub(e.last_pose_change_tick.wrapping_abs());
        let in_transition = pose_time < if sitting { 40 } else { 52 };
        sitting || in_transition
    }

    /// `!Entity.firstTick` (M169), for tests.
    pub fn has_ticked(&self, id: i32) -> bool {
        self.map.get(&id).is_some_and(|e| e.ticked)
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

    /// Apply `Entity.DATA_SILENT` (metadata index 4, BOOLEAN) — M138a.
    ///
    /// `false` is applied as eagerly as `true` for the same reason index 3's
    /// is: the server toggles it both ways, and a latch would leave an entity
    /// permanently muted after one `/data merge`.
    pub fn set_silent(&mut self, id: i32, silent: bool) {
        if silent {
            self.silent.insert(id);
        } else {
            self.silent.remove(&id);
        }
    }

    /// `Entity.isSilent()` (`Entity.java:1512-1514`), which gates `playSound`.
    ///
    /// An entity nothing has been said about is **audible**, which is exact
    /// rather than a fallback: `Entity.java:322` seeds the flag `false`.
    pub fn is_silent(&self, id: i32) -> bool {
        self.silent.contains(&id)
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

    /// `Guardian.DATA_ID_ATTACK_TARGET` (index 17 INT), and the client-side
    /// counter its arrival resets (M141g).
    ///
    /// **`clientSideAttackTime` is reset on EVERY arrival of the accessor,
    /// not on a change** — `SynchedEntityData.assignValues` calls
    /// `onSyncedDataUpdated` once per entry in the packet with no comparison
    /// (M141e's finding, second application). So a server re-sending the same
    /// target restarts the beam's wind-up, which is audible: the guardian's
    /// volume is the scale *squared*, so a reset drops it to silence and it
    /// climbs again.
    pub fn set_guardian_attack_target(&mut self, id: i32, target: i32) {
        self.guardian_attack.insert(
            id,
            GuardianAttack {
                target,
                client_side_attack_time: 0,
            },
        );
    }

    /// `Guardian.hasActiveAttackTarget()` — `DATA_ID_ATTACK_TARGET != 0`.
    ///
    /// Zero is "no target", not "entity 0": the accessor stores an entity id
    /// and vanilla reserves 0 for absent, which is why this is an inequality
    /// rather than an `Option`.
    ///
    /// **The tick calls the same predicate**, rather than repeating `!= 0`.
    /// It had its own copy and this accessor had no caller at all, so a
    /// mutation to it changed nothing — the battery reported it as a survivor,
    /// which is what a dead accessor looks like from the outside.
    pub fn guardian_has_attack_target(&self, id: i32) -> bool {
        self.guardian_attack
            .get(&id)
            .is_some_and(GuardianAttack::has_target)
    }

    /// `Guardian.getAttackAnimationScale(0.0F)` —
    /// `clientSideAttackTime / getAttackDuration()`.
    ///
    /// The duration is the caller's, because it is **per species**: 80 for a
    /// guardian and 60 for an elder (`Guardian.java:109`,
    /// `ElderGuardian.java:40`), so the same counter yields a different scale
    /// and the elder's beam ramps a third faster.
    pub fn guardian_attack_scale(&self, id: i32, attack_duration: i32) -> f32 {
        self.guardian_attack.get(&id).map_or(0.0, |g| {
            g.client_side_attack_time as f32 / attack_duration as f32
        })
    }

    /// `Bee.DATA_ANGER_END_TIME` (M141f) — a synced **deadline**, not a flag.
    ///
    /// Stored raw and compared against the world clock at read time, because
    /// `NeutralMob.isAngry()` is `endTime > 0 && endTime - gameTime > 0` and
    /// the second half changes every tick without any packet. Storing a
    /// boolean here would freeze a bee's anger at whatever it was when the
    /// last metadata arrived.
    pub fn set_anger_end_time(&mut self, id: i32, end_time: i64) {
        self.anger_end_time.insert(id, end_time);
    }

    /// `Bee.getPersistentAngerEndTime()`.
    ///
    /// `None` for an entity that has never sent one. That is **not** the same
    /// as `Some(-1)` to any caller that cares, though both are "not angry":
    /// `Bee` seeds the accessor to -1 (`Bee.java:165`), so an absent entry and
    /// a default one agree, and a caller may treat them alike.
    pub fn anger_end_time(&self, id: i32) -> Option<i64> {
        self.anger_end_time.get(&id).copied()
    }

    /// `Guardian.aiStep`'s client branch (M141g):
    ///
    /// ```java
    /// if (this.hasActiveAttackTarget()) {
    ///    if (this.clientSideAttackTime < this.getAttackDuration()) {
    ///       this.clientSideAttackTime++;
    ///    }
    ///    ...
    /// }
    /// ```
    ///
    /// **It runs only while there is a target and it never counts down.** A
    /// guardian that loses its target holds its counter where it was; what
    /// zeroes it is the *metadata* arriving, not the target going away — so a
    /// beam that stops and restarts against the same target resumes from where
    /// it left off unless the server re-sends the accessor.
    ///
    /// The cap is the species' duration, and the table does not know the
    /// species — so it stores the raw counter and caps at the **larger** of
    /// the two (80). An elder's scale is then computed with 60 at read time,
    /// which can exceed 1.0 for 20 ticks. That is a **stated divergence**:
    /// vanilla caps at the elder's own 60. It is here rather than hidden
    /// because fixing it wants the type id in the table, which is a bigger
    /// change than the milestone earns, and because the consequence is bounded
    /// — an elder's beam saturates its volume early rather than misbehaving.
    fn tick_guardian_attacks(&mut self) {
        for g in self.guardian_attack.values_mut() {
            if g.has_target() && g.client_side_attack_time < GUARDIAN_MAX_ATTACK_DURATION {
                g.client_side_attack_time += 1;
            }
        }
    }

    /// `Sniffer.canPlayDiggingSound()` — `Sniffer.java:139-141` (M141g).
    ///
    /// **Two states keep the sound alive, not one**: `DIGGING` (5) *or*
    /// `SEARCHING` (4), so a sniffer that has found something keeps digging
    /// audibly while it searches. Reading only DIGGING cuts the sound at the
    /// moment it matters most.
    ///
    /// The state is `gesture_state`, which the same rig reads — and which
    /// M141g1 had to move from index 17 to **18** before any of it could
    /// arrive from a real server.
    pub fn sniffer_digging(&self, id: i32) -> bool {
        matches!(self.gesture_state(id), SNIFFER_SEARCHING | SNIFFER_DIGGING)
    }

    /// Tell the table which entity id is the local player (M141d).
    ///
    /// The local player is **not** in this table — its state lives in
    /// `rewo_world::physics` — so the id is here only to answer
    /// `Entity.isLocalClientAuthoritative()` for a vehicle it is riding, which
    /// is the one case where a remote entity's velocity does *not* decay.
    /// Leaving it unset makes every entity non-authoritative, which is the
    /// right answer for every entity the local player is not riding.
    pub fn set_local_player(&mut self, id: i32) {
        self.local_player = Some(id);
    }

    /// `entity.lerpMotion(movement)` — `handleSetEntityMotion`'s whole body
    /// (M141d).
    ///
    /// In 26.2 `Entity.lerpMotion` is a bare `setDeltaMovement`, so this is a
    /// **replace**, not a blend and not an add. `AbstractMinecart` overrides
    /// it, but the override runs `behavior.lerpMotion` →
    /// `MinecartBehavior.setDeltaMovement` → `minecart.setDeltaMovement`, so
    /// the stored value is identical; only `OldMinecartBehavior`'s extra
    /// `targetDeltaMovement` cache differs and nothing in the sound path reads
    /// it.
    ///
    /// **A non-finite movement is ignored and the old value survives** —
    /// `Entity.setDeltaMovement` is guarded by `deltaMovement.isFinite()`, so
    /// this is a silent keep rather than a write of NaN.
    ///
    /// `living` and `player` are class facts the caller answers from
    /// `rewo_data::entity_types::EntityClasses`; see [`EntityState`]'s fields.
    pub fn lerp_motion(&mut self, id: i32, movement: [f64; 3], living: bool, player: bool) {
        let Some(e) = self.map.get_mut(&id) else {
            // `handleSetEntityMotion` is `if (entity != null)`, so an unknown
            // id is inert rather than an error.
            return;
        };
        e.motion_living = living;
        e.motion_player = player;
        if movement.iter().all(|c| c.is_finite()) {
            e.delta_movement = movement;
        }
    }

    /// M362 — make `id` a projectile the client flies (see
    /// [`EntityState::set_flight`]).
    pub fn set_flight(&mut self, id: i32, flight: Flight) {
        if let Some(e) = self.map.get_mut(&id) {
            e.set_flight(flight);
        }
    }

    /// M362 — `AbstractArrow.IN_GROUND` from the server; only an arrow's
    /// flight reads it (index 10 BOOLEAN is another class's field elsewhere).
    pub fn set_in_ground(&mut self, id: i32, in_ground: bool) {
        if let Some(e) = self.map.get_mut(&id) {
            if e.flight.is_some_and(|f| f.arrow) {
                e.in_ground = in_ground;
            }
        }
    }

    /// Mark `id` a camel: its walk animation follows `Camel.
    /// updateWalkAnimation` (see [`EntityState::update_walk`]).
    pub fn set_camel_walk(&mut self, id: i32) {
        if let Some(e) = self.map.get_mut(&id) {
            e.camel_walk = true;
        }
    }

    /// The walk animation of the mount the local player steers, after the
    /// session moved it (see [`EntityState::update_walk_after_move`]), and
    /// its look (`tickRidden`'s `setRot`).
    pub fn steered_mount_moved(&mut self, id: i32, yaw: f32, pitch: f32) {
        if let Some(e) = self.map.get_mut(&id) {
            e.update_walk_after_move();
            e.set_ridden_look(yaw, pitch);
        }
    }

    /// `AbstractArrow.ID_FLAGS` (index 8 BYTE; bit 1 is `FLAG_CRIT`); only an
    /// arrow's flight reads it.
    pub fn set_arrow_flags(&mut self, id: i32, flags: u8) {
        if let Some(e) = self.map.get_mut(&id) {
            if e.flight.is_some_and(|f| f.arrow) {
                e.arrow_crit = flags & 1 != 0;
            }
        }
    }

    /// M362 — one client tick of every projectile's flight; the crit trails
    /// of the critical arrows that flew this tick.
    pub fn step_flights(&mut self, solid: &dyn Fn([f64; 3]) -> bool) -> Vec<CritTrail> {
        self.map
            .values_mut()
            .filter_map(|e| e.step_flight(solid))
            .collect()
    }

    /// `entity.getDeltaMovement()`.
    ///
    /// `None` for an entity this table has never seen. Callers that want a
    /// speed should use [`Self::horizontal_speed`] / [`Self::speed`] /
    /// [`Self::speed_sqr`], which name the three `Vec3` reductions the sound
    /// ramps use so no call site does the arithmetic itself.
    pub fn delta_movement(&self, id: i32) -> Option<[f64; 3]> {
        self.map.get(&id).map(|e| e.delta_movement)
    }

    /// `getDeltaMovement().horizontalDistance()` — `sqrt(x² + z²)`.
    pub fn horizontal_speed(&self, id: i32) -> f64 {
        self.delta_movement(id)
            .map_or(0.0, |v| (v[0] * v[0] + v[2] * v[2]).sqrt())
    }

    /// `getDeltaMovement().length()` — `sqrt(x² + y² + z²)`.
    pub fn speed(&self, id: i32) -> f64 {
        self.speed_sqr(id).sqrt()
    }

    /// `getDeltaMovement().lengthSqr()` — **not** rooted.
    pub fn speed_sqr(&self, id: i32) -> f64 {
        self.delta_movement(id)
            .map_or(0.0, |v| v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
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

    /// M310 — the whole `DATA_PLAYER_MODE_CUSTOMISATION` mask (`Avatar`'s
    /// synched default 0 until the server sends one).
    pub fn model_customisation(&self, id: i32) -> u8 {
        self.model_customisation.get(&id).copied().unwrap_or(0)
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

    // -- item frames (M194) -------------------------------------------------

    /// `HangingEntity.DATA_DIRECTION` (slot 8, DIRECTION) for a frame.
    pub fn set_frame_facing(&mut self, id: i32, facing: u8) {
        self.item_frames.entry(id).or_default().facing = Some(facing);
    }

    /// `ItemFrame.DATA_ITEM` (slot 9, ITEM_STACK); `None` is an empty frame.
    pub fn set_frame_item(&mut self, id: i32, item: Option<FrameItem>) {
        self.item_frames.entry(id).or_default().item = item;
    }

    /// `ItemFrame.DATA_ROTATION` (slot 10, INT), kept `% 8` as `setRotation`
    /// does.
    pub fn set_frame_rotation(&mut self, id: i32, rotation: i32) {
        self.item_frames.entry(id).or_default().rotation = rotation.rem_euclid(8) as u8;
    }

    /// A frame's facing, rotation and item. The facing is the synced slot if
    /// one arrived, else the spawn packet's data (`recreateFromPacket`), which
    /// is what a frame whose facing equals the slot's default carries alone
    /// (the server sends only non-default metadata).
    pub fn item_frame(&self, id: i32) -> Option<(u8, u8, Option<FrameItem>)> {
        let e = self.map.get(&id)?;
        let st = self.item_frames.get(&id).cloned().unwrap_or_default();
        let facing = st.facing.unwrap_or_else(|| {
            u8::try_from(e.spawn_data)
                .ok()
                .filter(|d| *d < 6)
                .unwrap_or(0)
        });
        Some((facing, st.rotation, st.item))
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

    /// M273 — the worn head's profile skin (`None` clears it).
    pub fn set_head_profile(&mut self, id: i32, textures: Option<String>) {
        match textures {
            Some(t) => {
                self.head_profiles.insert(id, t);
            }
            None => {
                self.head_profiles.remove(&id);
            }
        }
    }

    /// M309 — a mannequin's synched profile.
    pub fn set_avatar_profile(&mut self, id: i32, profile: AvatarProfile) {
        self.avatar_profiles.insert(id, profile);
    }

    /// M309 — the profile mannequin `id` was last sent, if any (none is the
    /// empty default profile, which the server never sends).
    pub fn avatar_profile(&self, id: i32) -> Option<&AvatarProfile> {
        self.avatar_profiles.get(&id)
    }

    /// M289 — a dropped or framed stack's player-head skin (`None` clears).
    pub fn set_stack_profile(&mut self, id: i32, textures: Option<String>) {
        match textures {
            Some(t) => {
                self.stack_profiles.insert(id, t);
            }
            None => {
                self.stack_profiles.remove(&id);
            }
        }
    }

    /// M289 — the `textures` property of the dropped or framed stack `id`
    /// carries.
    pub fn stack_profile(&self, id: i32) -> Option<&str> {
        self.stack_profiles.get(&id).map(String::as_str)
    }

    /// M295 — the saddle slot's piece (`None` clears).
    pub fn set_saddle_piece(&mut self, id: i32, piece: Option<WornPiece>) {
        match piece {
            Some(p) => {
                self.saddles.insert(id, p);
            }
            None => {
                self.saddles.remove(&id);
            }
        }
    }

    /// M295 — what `id` wears in its saddle slot.
    pub fn saddle_piece(&self, id: i32) -> Option<WornPiece> {
        self.saddles.get(&id).copied()
    }

    /// M294 — the body slot's piece (`None` clears).
    pub fn set_body_armor(&mut self, id: i32, piece: Option<WornPiece>) {
        match piece {
            Some(p) => {
                self.body_armor.insert(id, p);
            }
            None => {
                self.body_armor.remove(&id);
            }
        }
    }

    /// M294 — what `id` wears in its body slot.
    pub fn body_armor(&self, id: i32) -> Option<WornPiece> {
        self.body_armor.get(&id).copied()
    }

    /// M290 — the skin of a player head held in `hand` (`None` clears).
    pub fn set_hand_profile(&mut self, id: i32, hand: InteractionHand, textures: Option<String>) {
        let slot = self.hand_profiles.entry(id).or_default();
        slot[hand_slot(hand)] = textures;
        if slot.iter().all(Option::is_none) {
            self.hand_profiles.remove(&id);
        }
    }

    /// M290 — the `textures` property of the player head `id` holds in
    /// `hand`.
    pub fn hand_profile(&self, id: i32, hand: InteractionHand) -> Option<&str> {
        self.hand_profiles.get(&id)?[hand_slot(hand)].as_deref()
    }

    /// M290 — every held head's skin, to request.
    pub fn hand_profiles(&self) -> impl Iterator<Item = &str> {
        self.hand_profiles
            .values()
            .flatten()
            .flatten()
            .map(String::as_str)
    }

    /// M289 — every dropped or framed head's skin, to request.
    pub fn stack_profiles(&self) -> impl Iterator<Item = &str> {
        self.stack_profiles.values().map(String::as_str)
    }

    /// M273 — the `textures` property of the player head `id` wears.
    pub fn head_profile(&self, id: i32) -> Option<&str> {
        self.head_profiles.get(&id).map(String::as_str)
    }

    /// M273 — every worn head's profile skin.
    pub fn head_profiles(&self) -> impl Iterator<Item = &str> {
        self.head_profiles.values().map(String::as_str)
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

    /// M239 — a firework rocket's attachment (index 9). Kind-gated by the
    /// caller.
    pub fn set_attached_to(&mut self, id: i32, target: Option<i32>) {
        self.mob_state.entry(id).or_default().attached_to = target;
    }

    /// The entity this rocket is attached to, if any.
    pub fn attached_to(&self, id: i32) -> Option<i32> {
        self.mob_state.get(&id).and_then(|s| s.attached_to)
    }

    /// M269 — primed TNT's synced state, created empty on first write.
    pub fn primed_mut(&mut self, id: i32) -> &mut PrimedState {
        self.primed.entry(id).or_default()
    }

    /// M269 — primed TNT's state (all defaults when none came).
    pub fn primed(&self, id: i32) -> PrimedState {
        self.primed.get(&id).copied().unwrap_or_default()
    }

    /// M277 — `Creeper.tick`'s swell for every creeper (`creeper`, its type
    /// id): `swell += swellDir` (`DATA_SWELL_DIR`, index 16 INT, default -1,
    /// kept in `sizes`), clamped to `0..=30`.
    pub fn tick_creepers(&mut self, creeper: i32) {
        let ids: Vec<i32> = self
            .map
            .iter()
            .filter(|(_, e)| e.type_id == creeper)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let dir = self.sizes.get(&id).copied().unwrap_or(-1);
            let m = self.mob_state.entry(id).or_default();
            m.old_swell = m.swell;
            m.swell = (m.swell + dir).clamp(0, CREEPER_MAX_SWELL);
        }
    }

    /// M277 — `Creeper.getSwelling(a)`: `lerp(a, oldSwell, swell) /
    /// (maxSwell - 2)`.
    pub fn creeper_swelling(&self, id: i32, alpha: f32) -> f32 {
        let m = self.mob_state.get(&id).copied().unwrap_or_default();
        (m.old_swell as f32 + (m.swell - m.old_swell) as f32 * alpha)
            / (CREEPER_MAX_SWELL - 2) as f32
    }

    /// M269 — `PrimedTnt.tick`'s `setFuse(getFuse() - 1)`, which the client
    /// runs too; the server's next value overwrites it.
    pub fn tick_fuses(&mut self) {
        for p in self.primed.values_mut() {
            if let Some(f) = p.fuse.as_mut() {
                *f -= 1;
            }
        }
    }

    /// M261 — the minecart's synced display, created empty on first write.
    pub fn minecart_display_mut(&mut self, id: i32) -> &mut MinecartDisplay {
        self.minecart_displays.entry(id).or_default()
    }

    /// M261 — the minecart's synced display (all defaults when none came).
    pub fn minecart_display(&self, id: i32) -> MinecartDisplay {
        self.minecart_displays.get(&id).copied().unwrap_or_default()
    }

    /// M255 — the display entity's state, created at vanilla's defaults on
    /// first write.
    pub fn display_mut(&mut self, id: i32) -> &mut DisplayState {
        self.displays.entry(id).or_default()
    }

    /// M265 — advance every display's interpolation one client tick.
    pub fn tick_displays(&mut self) {
        for d in self.displays.values_mut() {
            let target = d.target();
            d.interp.tick(target);
        }
    }

    /// M255 — the display entity's state, if any metadata has arrived.
    pub fn display(&self, id: i32) -> Option<&DisplayState> {
        self.displays.get(&id)
    }

    /// M238 — `ArmorStand.DATA_CLIENT_FLAGS` (index 15, BYTE). Kind-gated by
    /// the caller: a `Mob`'s flags share the index.
    pub fn set_armor_stand_flags(&mut self, id: i32, flags: u8) {
        self.mob_state.entry(id).or_default().stand_flags = flags;
    }

    /// The armour stand's client flags, 0 (vanilla's `define` default: big,
    /// no arms, base plate) when none were sent.
    /// M270 — one of the armour stand's six poses (`part` 0..6).
    pub fn set_armor_stand_pose(&mut self, id: i32, part: usize, rot: [f32; 3]) {
        if let Some(slot) = self
            .mob_state
            .entry(id)
            .or_default()
            .stand_pose
            .get_mut(part)
        {
            *slot = Some(rot);
        }
    }

    /// M270 — the armour stand's six poses, `None` where vanilla's default
    /// stands.
    pub fn armor_stand_pose(&self, id: i32) -> [Option<[f32; 3]>; 6] {
        self.mob_state.get(&id).map_or([None; 6], |s| s.stand_pose)
    }

    pub fn armor_stand_flags(&self, id: i32) -> u8 {
        self.mob_state.get(&id).map_or(0, |s| s.stand_flags)
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

    /// A villager's or zombie villager's `VillagerData`, as the raw wire ids
    /// (see [`MobState::villager_data`]).
    pub fn set_villager_data(&mut self, id: i32, data: (i32, i32, i32)) {
        self.mob_state.entry(id).or_default().villager_data = Some(data);
    }

    /// The `VillagerData` the server sent, `None` if it sent none.
    pub fn villager_data(&self, id: i32) -> Option<(i32, i32, i32)> {
        self.mob_state.get(&id).and_then(|s| s.villager_data)
    }

    /// M342 — a wolf's collar colour (`DATA_COLLAR_COLOR`). Kind-gated by the
    /// caller: the llama's variant is the same index and serializer.
    pub fn set_collar(&mut self, id: i32, color: i32) {
        self.mob_state.entry(id).or_default().collar = Some(color);
    }

    /// M342 — the collar colour the server sent, `None` if it sent none.
    pub fn collar(&self, id: i32) -> Option<i32> {
        self.mob_state.get(&id).and_then(|s| s.collar)
    }

    /// M352 — an enderman's carried block (`DATA_CARRY_STATE`); the wire's
    /// empty (0) clears it. Kind-gated by the caller.
    pub fn set_carried_block(&mut self, id: i32, state: Option<u32>) {
        self.mob_state.entry(id).or_default().carried_block = state;
    }

    /// M352 — the block state an enderman carries, if any.
    pub fn carried_block(&self, id: i32) -> Option<u32> {
        self.mob_state.get(&id).and_then(|s| s.carried_block)
    }

    /// M354 — a bee's `DATA_FLAGS_ID` byte. Kind-gated by the caller.
    pub fn set_bee_flags(&mut self, id: i32, flags: u8) {
        self.mob_state.entry(id).or_default().bee_flags = Some(flags);
    }

    /// M354 — the bee's flags, 0 (vanilla's default) when none were sent.
    pub fn bee_flags(&self, id: i32) -> u8 {
        self.mob_state
            .get(&id)
            .and_then(|s| s.bee_flags)
            .unwrap_or(0)
    }

    /// M353 — a snow golem's `DATA_PUMPKIN_ID` byte. Kind-gated by the caller.
    pub fn set_snow_golem_flags(&mut self, id: i32, flags: u8) {
        self.mob_state.entry(id).or_default().snow_golem_flags = Some(flags);
    }

    /// M353 — `SnowGolem.hasPumpkin()`: the 0x10 bit, set by default
    /// (`defineSynchedData` defines `PUMPKIN_FLAG`), so a golem the server
    /// sent nothing for wears it.
    pub fn has_pumpkin(&self, id: i32) -> bool {
        self.mob_state
            .get(&id)
            .and_then(|s| s.snow_golem_flags)
            .is_none_or(|b| b & 0x10 != 0)
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

    /// M356 — a tamable mob's sitting pose (`DATA_FLAGS_ID` bit 0x01).
    pub fn is_in_sitting_pose(&self, id: i32) -> bool {
        self.mob_state
            .get(&id)
            .is_some_and(|s| s.is_in_sitting_pose())
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

    /// M288 — `WitherBoss.DATA_ID_INV`. Kind-gated by the caller.
    pub fn set_wither_invulnerable(&mut self, id: i32, ticks: i32) {
        self.mob_state.entry(id).or_default().wither_invulnerable = ticks;
    }

    /// M288 — `WitherBoss.getInvulnerableTicks()`; the synched default is 0.
    pub fn wither_invulnerable(&self, id: i32) -> i32 {
        self.mob_state.get(&id).map_or(0, |s| s.wither_invulnerable)
    }

    /// M296 — `AbstractChestedHorse.DATA_ID_CHEST`. Kind-gated by the
    /// caller.
    pub fn set_has_chest(&mut self, id: i32, chest: bool) {
        self.mob_state.entry(id).or_default().has_chest = chest;
    }

    /// M365 — `Turtle.HAS_EGG`. Kind-gated by the caller.
    pub fn set_turtle_egg(&mut self, id: i32, egg: bool) {
        self.mob_state.entry(id).or_default().turtle_egg = egg;
    }

    /// M365 — `Turtle.LAYING_EGG`. Kind-gated by the caller.
    pub fn set_turtle_laying(&mut self, id: i32, laying: bool) {
        self.mob_state.entry(id).or_default().turtle_laying = laying;
    }

    /// M393 — a boat's hurt metadata (`DATA_ID_HURT`, `_HURTDIR`,
    /// `_DAMAGE`); each `None` leaves its field. Kind-gated by the caller.
    pub fn set_boat_hurt(
        &mut self,
        id: i32,
        hurt: Option<i32>,
        dir: Option<i32>,
        damage: Option<f32>,
    ) {
        let st = self.mob_state.entry(id).or_default();
        if let Some(v) = hurt {
            st.boat_hurt = v;
        }
        if let Some(v) = dir {
            st.boat_hurt_dir = v;
        }
        if let Some(v) = damage {
            st.boat_damage = v;
        }
    }

    /// M393 — a boat's paddle flags (`DATA_ID_PADDLE_LEFT` / `_RIGHT`, or
    /// the steered boat's own); each `None` leaves its side.
    pub fn set_boat_paddles(&mut self, id: i32, paddles: [Option<bool>; 2]) {
        let st = self.mob_state.entry(id).or_default();
        for (dst, v) in st.boat_paddles.iter_mut().zip(paddles) {
            if let Some(v) = v {
                *dst = v;
            }
        }
    }

    /// M401 — a living entity's sleeping position (`None` clears).
    pub fn set_sleeping_pos(&mut self, id: i32, pos: Option<(i32, i32, i32)>) {
        self.mob_state.entry(id).or_default().sleeping_pos = pos;
    }

    /// M396 — a sulfur cube's `MAX_FUSE`: `setFuse(maxFuse)`, −1 (any
    /// negative) unprimed. Kind-gated by the caller.
    pub fn set_sulfur_fuse(&mut self, id: i32, max_fuse: i32) {
        self.mob_state.entry(id).or_default().sulfur_fuse = (max_fuse >= 0).then_some(max_fuse);
    }

    /// M396 — `SulfurCubeRenderer`'s `fuseRemainingTicks`: `isPrimed() ?
    /// fuse − a + 1 : 0`.
    pub fn sulfur_fuse_remaining(&self, id: i32, alpha: f32) -> f32 {
        self.mob_state
            .get(&id)
            .and_then(|st| st.sulfur_fuse)
            .map_or(0.0, |f| f as f32 - alpha + 1.0)
    }

    /// M394 — a boat's `DATA_ID_BUBBLE_TIME`. Kind-gated by the caller.
    pub fn set_boat_bubble_time(&mut self, id: i32, time: i32) {
        self.mob_state.entry(id).or_default().boat_bubble_time = time;
    }

    /// M394 — `AbstractBoat.getBubbleAngle(a)`: `lerp(a, bubbleAngleO,
    /// bubbleAngle)`, degrees.
    pub fn boat_bubble_angle(&self, id: i32, alpha: f32) -> f32 {
        self.mob_state.get(&id).map_or(0.0, |st| {
            st.boat_bubble_angle_o + alpha * (st.boat_bubble_angle - st.boat_bubble_angle_o)
        })
    }

    /// M393 — `AbstractBoat.tick`'s client half: `if (hurtTime > 0)
    /// hurtTime--`, `if (damage > 0) damage -= 1`, and the paddle positions
    /// (+π/8 while `getPaddleState`: the flag and a controlling passenger).
    /// M394 — and `tickBubbleColumn`'s client arm: the multiplier +0.05 a
    /// tick while `bubbleTime > 0`, else −0.1, clamped to 0..1, and
    /// `bubbleAngle = 10·sin(0.5·tickCount)·multiplier`.
    fn tick_boats(&mut self) {
        use std::f32::consts::PI;
        for (id, st) in self.mob_state.iter_mut() {
            if st.boat_hurt > 0 {
                st.boat_hurt -= 1;
            }
            if st.boat_damage > 0.0 {
                st.boat_damage -= 1.0;
            }
            // M396 — `SulfurCube.tickFuse`.
            if let Some(f) = st.sulfur_fuse.as_mut() {
                if *f > 0 {
                    *f -= 1;
                }
            }
            if let Some(e) = self.map.get(id) {
                st.boat_bubble_mult = (st.boat_bubble_mult
                    + if st.boat_bubble_time > 0 { 0.05 } else { -0.1 })
                .clamp(0.0, 1.0);
                st.boat_bubble_angle_o = st.boat_bubble_angle;
                st.boat_bubble_angle =
                    10.0 * (0.5 * e.tick_count as f64).sin() as f32 * st.boat_bubble_mult;
            }
            let controlled = self.passengers.get(id).is_some_and(|p| !p.is_empty());
            for i in 0..2 {
                if st.boat_paddles[i] && controlled {
                    st.boat_paddle_pos[i] += PI / 8.0;
                } else {
                    st.boat_paddle_pos[i] = 0.0;
                }
            }
        }
    }

    /// M393 — `AbstractBoat.getRowingTime(side, a)`: `getPaddleState(side) ?
    /// clampedLerp(a, pos − π/8, pos) : 0`.
    pub fn boat_rowing_time(&self, id: i32, side: usize, alpha: f32) -> f32 {
        use std::f32::consts::PI;
        let Some(st) = self.mob_state.get(&id) else {
            return 0.0;
        };
        let controlled = self.passengers.get(&id).is_some_and(|p| !p.is_empty());
        if !(st.boat_paddles[side] && controlled) {
            return 0.0;
        }
        let pos = st.boat_paddle_pos[side];
        let a = alpha.clamp(0.0, 1.0);
        pos - PI / 8.0 + a * (PI / 8.0)
    }

    /// M389 — `Piglin.DATA_IS_DANCING`. Kind-gated by the caller.
    pub fn set_piglin_dancing(&mut self, id: i32, dancing: bool) {
        self.mob_state.entry(id).or_default().piglin_dancing = dancing;
    }

    /// M387 — `Axolotl.DATA_PLAYING_DEAD`. Kind-gated by the caller.
    pub fn set_axolotl_playing_dead(&mut self, id: i32, dead: bool) {
        self.mob_state.entry(id).or_default().axolotl_playing_dead = dead;
    }

    /// M387 — `isPlayingDead()`; the synched default is `false`.
    pub fn axolotl_playing_dead(&self, id: i32) -> bool {
        self.mob_state
            .get(&id)
            .is_some_and(|s| s.axolotl_playing_dead)
    }

    /// M365 — `(hasEgg(), isLayingEgg())`; both default `false`.
    pub fn turtle_flags(&self, id: i32) -> (bool, bool) {
        self.mob_state
            .get(&id)
            .map_or((false, false), |s| (s.turtle_egg, s.turtle_laying))
    }

    /// M296 — `hasChest()`; the synched default is `false`.
    pub fn has_chest(&self, id: i32) -> bool {
        self.mob_state.get(&id).is_some_and(|s| s.has_chest)
    }

    /// M285 — `Creeper.DATA_IS_POWERED`. Kind-gated by the caller.
    pub fn set_creeper_powered(&mut self, id: i32, powered: bool) {
        self.mob_state.entry(id).or_default().creeper_powered = powered;
    }

    /// M285 — `Creeper.isPowered()`; the synched default is `false`.
    pub fn creeper_powered(&self, id: i32) -> bool {
        self.mob_state.get(&id).is_some_and(|s| s.creeper_powered)
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
        let local = self.local_player;
        for (id, e) in self.map.iter_mut() {
            // `LivingEntity.isFallFlying()` is `getSharedFlag(7)` — the cape's
            // `fallFlyingScale` needs the *count*, which only the client keeps.
            let fall_flying = self.shared_flags.get(id).copied().unwrap_or(0) & (1 << 7) != 0;
            // `Entity.canSimulateMovement()` — see `EntityState::tick_motion`.
            // `getControllingPassenger()` is overridden per class, but every
            // override that matters here (minecart, boat, horse, happy ghast)
            // returns the FIRST passenger, so the first entry is the answer.
            let authoritative = local.is_some_and(|p| {
                self.passengers
                    .get(id)
                    .and_then(|riders| riders.first())
                    .copied()
                    == Some(p)
            });
            // M394 — `Level.tickNonPassenger`: `tickCount++` then `tick()`.
            e.tick_count += 1;
            e.tick(fall_flying, authoritative);
            // M395 — the cube mobs' squish (their `tick` brackets
            // `super.tick()`; a remote entity's move there changes nothing).
            e.tick_squish();
            // M400 — `isVisuallySwimming()`: the SWIMMING pose (3), or
            // FALL_FLYING (1) while not fall flying.
            let pose = self.poses.get(id).copied().unwrap_or(0);
            e.tick_swim(pose == 3 || (pose == 1 && !fall_flying));
            // `AbstractHorse.tick`'s stand / eat / mouth clocks.
            e.equine.tick();
            // `Camel.updateWalkAnimation`: only `Pose.STANDING` (0) walks.
            e.camel_still = pose != 0;
            // M169 — `Camel.tick` / `AbstractNautilus.tick`: `if (dashCooldown
            // > 0) dashCooldown--`, and `Entity.baseTick`'s `firstTick = false`.
            if e.dash_cooldown > 0 {
                e.dash_cooldown -= 1;
            }
            e.ticked = true;
        }
        for d in self.dances.values_mut() {
            d.tick();
        }
        self.tick_guardian_attacks();
        self.tick_wavy_capes();
        self.tick_uses();
        self.tick_boats();
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

    /// The installed attachment table, if any. Read by M83's locator bar for
    /// a type's declared `sized(width, height)` — the only place outside
    /// riding that needs an entity's bounding box.
    pub fn attachments(&self) -> Option<&rewo_data::entity_attachments::Attachments> {
        self.attachments.as_deref()
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

    /// M228 — `Entity.positionRider` for a rider who is NOT in the table
    /// (the local player): the world position a rider of `rider_type`, yaw
    /// `rider_yaw`, `index`-th on `vehicle_id`'s roster is snapped to. A
    /// player's age scale is 1. `None` when the vehicle or either type is
    /// unknown (see [`crate::riding::rider_position`]).
    pub fn seat_of(
        &self,
        vehicle_id: i32,
        rider_type: i32,
        rider_yaw: f32,
        index: usize,
    ) -> Option<[f64; 3]> {
        let att = self.attachments.as_ref()?;
        let v = self.vehicle_inputs(vehicle_id)?;
        let r = crate::riding::RiderInputs {
            type_id: rider_type,
            yaw: rider_yaw,
            scale: 1.0,
            index,
        };
        crate::riding::rider_position(att, &v, &r)
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
            stand_anim_o: e.equine.stand_anim_o(),
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
            let delta = [cloak[0] - pos[0], cloak[1] - pos[1], cloak[2] - pos[2]];
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

    /// `getOnlinePlayerNames()` (M114) — every name `player_info_update` has
    /// reported and `player_info_remove` has not taken back. Unordered, like
    /// vanilla's; `Suggestions.create` sorts whatever reaches it.
    pub fn all_names(&self) -> impl Iterator<Item = &str> {
        self.names.values().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M393 — `AbstractBoat.tick` on the client: the hurt and damage count
    /// down a tick at a time; a paddle rows (+π/8 a tick) only while its
    /// flag is set AND the boat has a controlling passenger, else it rests
    /// at 0; `getRowingTime` lerps the last π/8.
    #[test]
    fn a_boats_hurt_counts_down_and_its_paddles_row_with_a_rider() {
        use std::f32::consts::PI;
        let mut t = EntityTable::default();
        t.add(5, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_boat_hurt(5, Some(10), Some(-1), Some(40.0));
        t.set_boat_paddles(5, [Some(true), None]);
        t.tick_lerp();
        let st = t.mob_state(5);
        assert_eq!((st.boat_hurt, st.boat_damage), (9, 39.0));
        assert_eq!(st.boat_paddle_pos, [0.0; 2], "no rider: no rowing");
        assert_eq!(t.boat_rowing_time(5, 0, 0.5), 0.0);
        t.set_passengers(5, vec![6]);
        t.tick_lerp();
        t.tick_lerp();
        assert_eq!(t.mob_state(5).boat_paddle_pos, [2.0 * PI / 8.0, 0.0]);
        assert!((t.boat_rowing_time(5, 0, 0.5) - 1.5 * PI / 8.0).abs() < 1e-6);
        assert_eq!(t.boat_rowing_time(5, 1, 0.5), 0.0);
        t.set_boat_paddles(5, [Some(false), None]);
        t.tick_lerp();
        assert_eq!(t.mob_state(5).boat_paddle_pos[0], 0.0);
        for _ in 0..20 {
            t.tick_lerp();
        }
        assert_eq!(t.mob_state(5).boat_hurt, 0);
    }

    /// M396 — a sulfur cube's fuse: unprimed (−1) reads 0 remaining; a
    /// synced `MAX_FUSE` primes it and the client counts it down to 0,
    /// where it holds; `fuse − a + 1` is what the renderer sees.
    #[test]
    fn a_sulfur_cubes_fuse_counts_down_on_the_client() {
        let mut t = EntityTable::default();
        t.add(8, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_sulfur_fuse(8, -1);
        assert_eq!(t.sulfur_fuse_remaining(8, 0.5), 0.0);
        t.set_sulfur_fuse(8, 30);
        t.tick_lerp();
        t.tick_lerp();
        assert_eq!(t.mob_state(8).sulfur_fuse, Some(28));
        assert_eq!(t.sulfur_fuse_remaining(8, 0.5), 28.5);
        for _ in 0..40 {
            t.tick_lerp();
        }
        assert_eq!(t.mob_state(8).sulfur_fuse, Some(0));
        assert_eq!(t.sulfur_fuse_remaining(8, 0.25), 0.75);
    }

    /// M400 — the swim amount climbs 0.09 a tick to 1 in the SWIMMING pose
    /// and falls back out of it; FALL_FLYING counts only while not gliding.
    #[test]
    fn a_swimming_entitys_swim_amount_eases() {
        let mut t = EntityTable::default();
        t.add(9, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_pose(9, 3);
        t.tick_lerp();
        t.tick_lerp();
        let e = t.get(9).unwrap();
        assert!((e.swim_amount - 0.18).abs() < 1e-6);
        assert!((e.swim_at(0.5) - 0.135).abs() < 1e-6);
        for _ in 0..20 {
            t.tick_lerp();
        }
        assert_eq!(t.get(9).unwrap().swim_amount, 1.0);
        t.set_pose(9, 0);
        for _ in 0..12 {
            t.tick_lerp();
        }
        assert_eq!(t.get(9).unwrap().swim_amount, 0.0);
        t.set_pose(9, 1);
        t.tick_lerp();
        assert!(
            t.get(9).unwrap().swim_amount > 0.0,
            "a crawl in FALL_FLYING"
        );
    }

    /// M395 — `AbstractCubeMob`'s squish: landing aims it at −0.5 and it
    /// eases half the gap a tick while the aim decays by 0.6; leaving the
    /// ground aims at 1.0; `squish_at` lerps the last tick.
    #[test]
    fn a_cube_mob_squishes_on_landing_and_stretches_on_leaving() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.on_ground = true;
        e.tick_squish();
        assert_eq!((e.squish, e.target_squish), (0.0, -0.3));
        e.tick_squish();
        assert_eq!(e.squish, -0.15);
        assert!((e.squish_at(0.5) - (-0.075)).abs() < 1e-6);
        e.on_ground = false;
        e.tick_squish();
        assert!((e.target_squish - 0.6).abs() < 1e-6, "{}", e.target_squish);
        for _ in 0..40 {
            e.tick_squish();
        }
        assert!(e.squish.abs() < 1e-3 && e.target_squish.abs() < 1e-6);
    }

    /// M394 — `tickBubbleColumn` on the client: over a bubble column the
    /// multiplier climbs 0.05 a tick to 1, the angle is
    /// `10·sin(0.5·tickCount)·multiplier` on the entity's own tick count,
    /// and off it the multiplier falls 0.1 a tick to 0.
    #[test]
    fn a_boat_over_a_bubble_column_tilts_on_its_tick_count() {
        let mut t = EntityTable::default();
        t.add(7, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_boat_bubble_time(7, 60);
        for _ in 0..4 {
            t.tick_lerp();
        }
        let st = t.mob_state(7);
        assert!((st.boat_bubble_mult - 0.2).abs() < 1e-6);
        assert_eq!(t.get(7).unwrap().tick_count, 4);
        let want = 10.0 * (0.5f64 * 4.0).sin() as f32 * 0.2;
        assert!((st.boat_bubble_angle - want).abs() < 1e-6);
        let prev = 10.0 * (0.5f64 * 3.0).sin() as f32 * 0.15;
        assert!((t.boat_bubble_angle(7, 0.5) - (prev + want) / 2.0).abs() < 1e-6);
        for _ in 0..30 {
            t.tick_lerp();
        }
        assert_eq!(t.mob_state(7).boat_bubble_mult, 1.0);
        t.set_boat_bubble_time(7, 0);
        for _ in 0..11 {
            t.tick_lerp();
        }
        assert_eq!(t.mob_state(7).boat_bubble_mult, 0.0);
        assert_eq!(t.mob_state(7).boat_bubble_angle, 0.0);
    }

    #[test]
    fn three_step_lerp_converges_exactly() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.set_target(3.0, 0.0, 0.0);
        e.tick(false, false); // (3-0)/3 → 1
        assert_eq!(e.render_pos(1.0)[0], 1.0);
        assert_eq!(e.render_pos(0.5)[0], 0.5, "partial tick blends prev→cur");
        e.tick(false, false); // (3-1)/2 → 2
        e.tick(false, false); // (3-2)/1 → 3 exact
        assert_eq!(e.render_pos(1.0)[0], 3.0);
        e.tick(false, false); // no steps left — stays put
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
            e.tick(true, false);
        }
        assert_eq!(e.fall_fly_ticks(), 3);
        e.tick(false, false);
        assert_eq!(e.fall_fly_ticks(), 0, "reset, not decayed");
    }

    #[test]
    fn deltas_accumulate_on_the_target_not_the_render_pos() {
        let mut e = EntityState::new(0, 0, 10.0, 0.0, 0.0, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0); // mid-lerp — target must not lose the first
        assert_eq!(e.x, 11.0);
        for _ in 0..3 {
            e.tick(false, false);
        }
        assert_eq!(e.render_pos(1.0)[0], 11.0);
    }

    #[test]
    fn still_entity_has_no_limb_swing_but_walking_builds_it_up() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..10 {
            e.tick(false, false); // no target set → cur never moves
        }
        let (_, amt_still) = e.limb();
        assert!(amt_still < 1e-6, "still entity: amount {amt_still}");
        // Now walk ~0.2 blk/tick and let the smoother ramp.
        for _ in 0..20 {
            e.nudge(0.2, 0.0, 0.0);
            e.tick(false, false);
        }
        let (swing, amt) = e.limb();
        assert!(amt > 0.5, "sustained walk drives amount up: {amt}");
        assert!(swing > 0.0, "phase advanced: {swing}");
    }

    /// A mount the local player steers moves after its own tick, so its
    /// walk animation reads the move the session made (`calculateEntityAnimation`
    /// at the end of `LivingEntity.tick`): `target = min(0.1 * 4, 1) = 0.4`,
    /// eased by 0.4 to 0.16, and the phase advances by that speed. Before the
    /// fix the limbs of a ridden horse never moved.
    #[test]
    fn a_steered_mount_walks_by_the_move_the_session_made() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.tick(false, true);
        e.set_simulated([0.0, 0.0, 0.1], 0.0);
        assert_eq!(e.limb(), (0.0, 0.0), "the entity's own tick sees no move");
        e.update_walk_after_move();
        let (swing, amount) = e.limb();
        assert!((amount - 0.16).abs() < 1e-6 && (swing - 0.16).abs() < 1e-6);
        // A second tick builds on it (the displacement is from the tick's start).
        e.tick(false, true);
        e.set_simulated([0.0, 0.0, 0.2], 0.0);
        e.update_walk_after_move();
        assert!((e.limb().1 - (0.16 + (0.4 - 0.16) * 0.4)).abs() < 1e-6);
    }

    /// `Camel.updateWalkAnimation`: `min(distance * 6, 1)` eased by 0.2, and
    /// 0 while the pose is not STANDING.
    #[test]
    fn a_camel_walks_at_six_times_the_distance_and_a_fifth_the_ease() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.camel_walk = true;
        e.nudge(0.0, 0.0, 0.1);
        for _ in 0..3 {
            e.tick(false, false);
        }
        // 0.1 over the three lerp ticks, a third each: 0.0333 * 6 = 0.2,
        // eased by 0.2 three times from 0.
        let (_, amount) = e.limb();
        let want = (0..3).fold(0.0f32, |a, _| a + (0.2 - a) * 0.2);
        assert!((amount - want).abs() < 1e-4, "{amount} vs {want}");
        // Sitting: the target is 0 and the speed decays.
        e.camel_still = true;
        e.nudge(0.0, 0.0, 0.3);
        e.tick(false, false);
        assert!(e.limb().1 < amount);
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
        assert!(
            (prog - 5.0 / 15.0).abs() < 1e-6,
            "progress @alpha=1 = 5/15: {prog}"
        );
        // Partial-tick interpolation blends spinning_ticks0=4 → spinning_ticks=5.
        let half = t.allay_dance_render(1, 0.5).unwrap().1;
        assert!(
            (half - 4.5 / 15.0).abs() < 1e-6,
            "progress @alpha=0.5 = 4.5/15: {half}"
        );
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
        assert!(
            (prog - 1.0 / 15.0).abs() < 1e-6,
            "one tick after restart: 1/15, got {prog}"
        );
    }

    #[test]
    fn dance_state_clears_on_removal_and_readd() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        t.tick_lerp();
        assert!(t.allay_dance_render(1, 1.0).is_some());
        t.remove(1);
        assert_eq!(
            t.allay_dance_render(1, 1.0),
            None,
            "removal clears the dance"
        );
        // A reused id (dropped despawn, new occupant) must not inherit the clock.
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        t.tick_lerp();
        let (_, prog) = t.allay_dance_render(1, 1.0).unwrap();
        assert!(
            (prog - 1.0 / 15.0).abs() < 1e-6,
            "fresh occupant restarts at 1/15, got {prog}"
        );
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
            charged: None,
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
            Some((
                true,
                -1,
                3.0 / 6.0,
                2.0 / 6.0,
                Some(InteractionHand::OffHand)
            ))
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
        assert_eq!(
            t.swing_debug(1).unwrap().1,
            -1,
            "frozen at the accept value"
        );
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
        assert_eq!(
            t.main_arm(1),
            HumanoidArm::Right,
            "clearing bit 2 restores it"
        );
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
        assert_eq!(
            t.hurt_state(1),
            HurtState::default(),
            "a reused id must not inherit it"
        );
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
            charged: None,
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

    // ---- M141d: `getDeltaMovement()` for a remote entity ------------------

    fn living(id: i32) -> EntityTable {
        let mut t = EntityTable::default();
        t.add(id, EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0));
        t
    }

    /// `lerpMotion` is a **replace**, and the three reductions are the ones
    /// the ramps ask for.
    #[test]
    fn a_motion_packet_replaces_the_velocity() {
        let mut t = living(1);
        t.lerp_motion(1, [3.0, 4.0, 12.0], true, false);
        assert_eq!(t.delta_movement(1), Some([3.0, 4.0, 12.0]));
        assert_eq!(t.speed_sqr(1), 9.0 + 16.0 + 144.0);
        assert_eq!(t.speed(1), 13.0);
        // horizontal drops the Y term, which is what makes a falling minecart
        // quiet: sqrt(9 + 144).
        assert!((t.horizontal_speed(1) - 153.0_f64.sqrt()).abs() < 1e-12);

        // …and a second packet replaces rather than accumulating.
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        assert_eq!(t.delta_movement(1), Some([1.0, 0.0, 0.0]));
    }

    /// **A non-finite movement is ignored and the old value survives** —
    /// `Entity.setDeltaMovement` is guarded by `isFinite()`, so this is a
    /// silent keep. Storing the NaN would poison every later `sqrt` and the
    /// symptom would surface as a silent sound, not as an error.
    #[test]
    fn a_non_finite_movement_keeps_the_previous_velocity() {
        let mut t = living(1);
        t.lerp_motion(1, [0.5, 0.0, 0.0], true, false);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            t.lerp_motion(1, [bad, 0.0, 0.0], true, false);
            assert_eq!(t.delta_movement(1), Some([0.5, 0.0, 0.0]), "{bad}");
        }
        // An unknown id is inert rather than an error — `handleSetEntityMotion`
        // is `if (entity != null)`.
        t.lerp_motion(999, [1.0, 1.0, 1.0], true, false);
        assert_eq!(t.delta_movement(999), None);
        assert_eq!(t.horizontal_speed(999), 0.0);
    }

    /// **A living entity's velocity decays by 0.98 a tick** once it has
    /// stopped interpolating — `aiStep`'s `else if (!canSimulateMovement())`.
    #[test]
    fn a_living_entitys_velocity_decays_but_a_minecarts_does_not() {
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        t.tick_lerp();
        assert!(
            (t.delta_movement(1).unwrap()[0] - 0.98).abs() < 1e-12,
            "{:?}",
            t.delta_movement(1)
        );
        t.tick_lerp();
        assert!((t.delta_movement(1).unwrap()[0] - 0.9604).abs() < 1e-12);

        // A minecart is not a LivingEntity, so `aiStep` never runs for it and
        // both minecart behaviours' client branches touch position only. Its
        // velocity is frozen until the next packet.
        let mut t = living(2);
        t.lerp_motion(2, [1.0, 0.0, 0.0], false, false);
        for _ in 0..100 {
            t.tick_lerp();
        }
        assert_eq!(t.delta_movement(2), Some([1.0, 0.0, 0.0]), "frozen");
    }

    /// **The decay is the `else` of the interpolation branch**, so an entity
    /// still catching up to a synced position does not decay at all.
    ///
    /// This is the shape a parallel implementation gets wrong: running the
    /// decay beside the lerp rather than opposite it makes a briskly-updated
    /// entity — one that is interpolating almost every tick — fade anyway.
    #[test]
    fn an_interpolating_entity_does_not_decay() {
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        // A fresh target arms the 3-tick lerp.
        t.get_mut(1).unwrap().set_target(10.0, 64.0, 0.0);
        for _ in 0..3 {
            t.tick_lerp();
            assert_eq!(t.delta_movement(1), Some([1.0, 0.0, 0.0]), "held");
        }
        // The steps are spent; now it falls.
        t.tick_lerp();
        assert!((t.delta_movement(1).unwrap()[0] - 0.98).abs() < 1e-12);
    }

    /// **The deadband has two forms and they disagree at (0.0025, 0.0025)** —
    /// per-axis zeroes both, joint keeps both, because the magnitude is
    /// 0.00354. `physics.rs` records the same divergence from the player's
    /// side; this is the non-player half.
    #[test]
    fn the_deadband_is_per_axis_for_a_mob_and_joint_for_a_player() {
        // A mob: each axis below 0.003 is zeroed on its own.
        let mut t = living(1);
        t.lerp_motion(1, [0.0025, 0.0, 0.0025], true, false);
        t.tick_lerp();
        assert_eq!(t.delta_movement(1), Some([0.0, 0.0, 0.0]), "per-axis");

        // A player: the same vector survives, because 0.0025² + 0.0025² is
        // 1.25e-5 and the threshold is 9e-6.
        let mut t = living(2);
        t.lerp_motion(2, [0.0025, 0.0, 0.0025], true, true);
        t.tick_lerp();
        let v = t.delta_movement(2).unwrap();
        assert!(v[0] > 0.0 && v[2] > 0.0, "joint rule keeps it: {v:?}");

        // Y is per-axis for both — and the witness is TWO-SIDED, because a
        // one-sided one cannot see the threshold move. `0.002` is below 0.003
        // and below any larger wrong value, so on its own it passes against a
        // deadband ten times too wide; `0.01` is the value that separates
        // them. The battery found exactly that.
        let mut t = living(3);
        t.lerp_motion(3, [1.0, 0.002, 0.0], true, true);
        t.tick_lerp();
        assert_eq!(t.delta_movement(3).unwrap()[1], 0.0, "below the threshold");

        let mut t = living(4);
        t.lerp_motion(4, [1.0, 0.01, 0.0], true, true);
        t.tick_lerp();
        assert!(
            t.delta_movement(4).unwrap()[1] > 0.0,
            "0.01 decays to 0.0098, which is ABOVE 0.003 and must survive"
        );
    }

    /// The decay terminates *at* zero rather than approaching it, because the
    /// deadband catches the tail — so a bee's buzz reaches silence rather than
    /// becoming quieter forever.
    #[test]
    fn the_decay_reaches_exactly_zero() {
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        let mut ticks = 0;
        while t.horizontal_speed(1) > 0.0 {
            t.tick_lerp();
            ticks += 1;
            assert!(ticks < 1000, "the decay never landed on zero");
        }
        // 0.98^n < 0.003 → n > ln(0.003)/ln(0.98) ≈ 287.7, so 288.
        assert_eq!(ticks, 288, "took {ticks} ticks");
    }

    /// **A vehicle the local player rides is locally authoritative, so its
    /// velocity does NOT decay** — `isLocalClientAuthoritative()` is inherited
    /// from the controlling passenger and `Player`'s is `isLocalPlayer()`.
    #[test]
    fn a_vehicle_the_local_player_rides_does_not_decay() {
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        t.set_local_player(7);
        t.set_passengers(1, vec![7]);
        for _ in 0..10 {
            t.tick_lerp();
        }
        assert_eq!(t.delta_movement(1), Some([1.0, 0.0, 0.0]), "authoritative");

        // A vehicle someone ELSE rides is not: authority comes from the local
        // player specifically, not from having a rider.
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        t.set_local_player(7);
        t.set_passengers(1, vec![8]);
        t.tick_lerp();
        assert!((t.delta_movement(1).unwrap()[0] - 0.98).abs() < 1e-12);

        // …and only the CONTROLLING (first) passenger confers it.
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], true, false);
        t.set_local_player(7);
        t.set_passengers(1, vec![8, 7]);
        t.tick_lerp();
        assert!(
            (t.delta_movement(1).unwrap()[0] - 0.98).abs() < 1e-12,
            "a back-seat local player is not the controller"
        );
    }

    /// With no registry the class facts are `false`, which turns the decay
    /// **off** rather than on — a held velocity rather than one that fades.
    #[test]
    fn without_the_class_facts_the_velocity_holds() {
        let mut t = living(1);
        t.lerp_motion(1, [1.0, 0.0, 0.0], false, false);
        for _ in 0..50 {
            t.tick_lerp();
        }
        assert_eq!(t.delta_movement(1), Some([1.0, 0.0, 0.0]));
    }

    #[test]
    fn walking_alone_never_reaches_the_clamp() {
        // The clamp added for the hurt kick must be a no-op for movement: the
        // target is already `min(1, dist*4)` and the lerp cannot overshoot.
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for i in 0..200 {
            e.set_target(i as f64 * 0.5, 0.0, 0.0);
            e.tick(false, false);
            assert!(e.limb().1 <= 1.0);
        }
    }
}

#[cfg(test)]
mod jump_vehicle_tests {
    use super::*;

    fn table_with(id: i32) -> EntityTable {
        let mut t = EntityTable::default();
        t.add(id, EntityState::new(0, 1, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    }

    /// `onSyncedDataUpdated(DASH)`: gated on `!firstTick`, arms only from
    /// zero, never restarts; `tick()` counts it down.
    #[test]
    fn the_dash_cooldown_arms_after_the_first_tick_from_zero_only() {
        let mut t = table_with(9);
        t.arm_dash_cooldown(9, 55);
        assert_eq!(
            t.dash_cooldown(9),
            0,
            "the spawn-time DASH entry arrives before the first tick"
        );
        t.tick_lerp();
        assert!(t.has_ticked(9));
        t.arm_dash_cooldown(9, 55);
        assert_eq!(t.dash_cooldown(9), 55);
        for _ in 0..10 {
            t.tick_lerp();
        }
        assert_eq!(t.dash_cooldown(9), 45);
        t.arm_dash_cooldown(9, 55);
        assert_eq!(
            t.dash_cooldown(9),
            45,
            "`dashCooldown == 0 ? 55 : dashCooldown` does not restart"
        );
        for _ in 0..50 {
            t.tick_lerp();
        }
        assert_eq!(t.dash_cooldown(9), 0, "never negative");
        t.arm_dash_cooldown(9, 40);
        assert_eq!(t.dash_cooldown(9), 40, "the nautilus literal");
        assert_eq!(t.dash_cooldown(8), 0, "an untracked entity has no cooldown");
    }

    /// `Camel.refuseToMove()` — sitting is a NEGATIVE pose-change tick, and
    /// the transition window is 40 ticks when sitting and 52 when standing,
    /// measured from `|lastPoseChangeTick|`.
    #[test]
    fn the_camel_refuses_while_sitting_or_changing_pose() {
        let mut t = table_with(9);
        assert!(
            !t.camel_refuse_to_move(9, 1000),
            "default 0: standing, long settled"
        );
        t.set_last_pose_change_tick(9, -900);
        assert!(
            t.camel_refuse_to_move(9, 1000),
            "negative = sitting, always refuses"
        );
        t.set_last_pose_change_tick(9, 900);
        assert!(
            t.camel_refuse_to_move(9, 951),
            "standing up: 51 < 52 is still the transition"
        );
        assert!(!t.camel_refuse_to_move(9, 952), "52 is not");
        t.set_last_pose_change_tick(9, -900);
        assert!(
            t.camel_refuse_to_move(9, 939) && t.camel_refuse_to_move(9, 940),
            "sitting refuses regardless of the 40"
        );
        assert!(
            !t.camel_refuse_to_move(8, 1000),
            "an untracked entity refuses nothing"
        );
        // Saddled defaults to false and follows the setter.
        assert!(!t.saddled(9));
        t.set_saddled(9, true);
        assert!(t.saddled(9));
        assert!(!t.saddled(8));
    }
}

/// The per-id side maps: a removed or re-added entity leaves nothing behind.
#[cfg(test)]
mod side_state_tests {
    /// M266 — an entity with a lerp length of 0 (a display) snaps to a new
    /// target, last tick's position too, so no frame slides; a length of 5
    /// moves a fifth of the way the first tick, as the default 3 moves a
    /// third.
    #[test]
    fn a_zero_lerp_length_snaps_and_a_longer_one_glides() {
        let mut e = EntityState::new(0, 1, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.set_lerp_len(0);
        e.set_target(10.0, 0.0, 0.0);
        assert_eq!(e.render_pos(0.0), [10.0, 0.0, 0.0]);
        assert_eq!(e.render_pos(0.5), [10.0, 0.0, 0.0]);
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 1, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.add(2, EntityState::new(0, 1, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.get_mut(1).unwrap().set_lerp_len(5);
        t.get_mut(1).unwrap().set_target(10.0, 0.0, 0.0);
        t.get_mut(2).unwrap().set_target(9.0, 0.0, 0.0);
        t.tick_lerp();
        assert!((t.get(1).unwrap().render_pos(1.0)[0] - 2.0).abs() < 1e-9);
        assert!((t.get(2).unwrap().render_pos(1.0)[0] - 3.0).abs() < 1e-9);
    }

    /// M265 — a display's transformation follows `Display.tick`: the first
    /// tick builds a fresh state; an update with a 10-tick duration moves
    /// from where it was over ten ticks (1.1 at a twentieth, 2.0 at half);
    /// a new target half-way starts from the point the last frame drew
    /// (`lastProgress` 0.7 under the OLD start: 2.4); without a duration an
    /// update snaps.
    #[test]
    fn display_transformation_interpolates_like_vanillas_tick() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let scale = |d: &DisplayState, p: f32| d.render_transformation(p).scale[0];
        let mut d = DisplayState::default();
        d.interp.pending_render = true;
        d.interp.tick(d.target());
        assert!(close(scale(&d, 0.5), 1.0));
        d.scale = [3.0; 3];
        d.interp.delay = 0;
        d.interp.pending_start = true;
        d.interp.duration_synced = 10;
        d.interp.pending_duration = true;
        d.interp.pending_render = true;
        d.interp.tick(d.target());
        assert!(close(scale(&d, 0.0), 1.0));
        assert!(close(scale(&d, 0.5), 1.1), "{}", scale(&d, 0.5));
        for _ in 0..5 {
            d.interp.tick(d.target());
        }
        assert!(close(scale(&d, 0.0), 2.0), "{}", scale(&d, 0.0));
        d.interp.tick(d.target());
        d.scale = [1.0; 3];
        d.interp.pending_start = true;
        d.interp.pending_render = true;
        d.interp.tick(d.target());
        assert!(close(scale(&d, 0.0), 2.4), "{}", scale(&d, 0.0));
        for _ in 0..10 {
            d.interp.tick(d.target());
        }
        assert!(close(scale(&d, 0.0), 1.0));
        d.interp.duration_synced = 0;
        d.interp.pending_duration = true;
        d.scale = [5.0; 3];
        d.interp.pending_render = true;
        d.interp.tick(d.target());
        assert!(close(scale(&d, 0.0), 5.0));
    }

    /// M265 — the rotations slerp: half-way from identity to a quarter turn
    /// about Y is an eighth; a quaternion and its negation (one rotation)
    /// interpolate to themselves, the shorter way round.
    #[test]
    fn display_rotations_slerp_the_short_way() {
        let s = std::f32::consts::FRAC_PI_4.sin();
        let id = DisplayTransformation::default();
        let quarter = DisplayTransformation {
            left_rotation: [0.0, s, 0.0, s],
            ..id
        };
        let half = id.slerp(&quarter, 0.5).left_rotation;
        let e = (std::f32::consts::PI / 8.0).sin_cos();
        assert!(
            (half[1] - e.0).abs() < 1e-5 && (half[3] - e.1).abs() < 1e-5,
            "{half:?}"
        );
        let neg = DisplayTransformation {
            left_rotation: [0.0, -s, 0.0, -s],
            ..id
        };
        let mid = quarter.slerp(&neg, 0.3).left_rotation;
        assert!(
            (mid[1] - s).abs() < 1e-5 && (mid[3] - s).abs() < 1e-5,
            "{mid:?}"
        );
    }

    use super::*;

    /// How many per-entity side maps hold a row for `id`.
    ///
    /// Exhaustive over [`EntityTable`]'s fields (no `..`), so a map added
    /// later must be classified here as well as in `clear_side_state` — the
    /// counting half cannot silently miss it.
    fn side_rows(t: &EntityTable, id: i32) -> usize {
        let EntityTable {
            map: _,
            names: _,
            custom_names,
            poses,
            gesture_states,
            sizes,
            babies,
            uses,
            deaths,
            attributes,
            shared_flags,
            custom_name_visible,
            silent,
            passengers,
            vehicle_of,
            anger_end_time,
            guardian_attack,
            local_player: _,
            attachments: _,
            model_customisation,
            item_stacks,
            item_frames,
            armor,
            head_profiles,
            stack_profiles,
            hand_profiles,
            avatar_profiles,
            body_armor,
            saddles,
            dances,
            events,
            swings,
            hands,
            main_arms,
            swing_effects,
            hurts,
            hurt_dirs,
            mob_state,
            wavy_capes,
            wavy_capes_enabled: _,
            minecarts,
            leash_data,
            projectile_power,
            displays,
            minecart_displays,
            primed,
        } = t;
        [
            custom_names.contains_key(&id),
            poses.contains_key(&id),
            gesture_states.contains_key(&id),
            sizes.contains_key(&id),
            babies.contains(&id),
            uses.contains_key(&id),
            deaths.contains_key(&id),
            attributes.contains_key(&id),
            shared_flags.contains_key(&id),
            custom_name_visible.contains(&id),
            silent.contains(&id),
            passengers.contains_key(&id),
            vehicle_of.contains_key(&id),
            anger_end_time.contains_key(&id),
            guardian_attack.contains_key(&id),
            model_customisation.contains_key(&id),
            item_stacks.contains_key(&id),
            armor.contains_key(&id),
            head_profiles.contains_key(&id),
            stack_profiles.contains_key(&id),
            hand_profiles.contains_key(&id),
            avatar_profiles.contains_key(&id),
            body_armor.contains_key(&id),
            saddles.contains_key(&id),
            dances.contains_key(&id),
            events.contains_key(&id),
            swings.contains_key(&id),
            hands.contains_key(&id),
            main_arms.contains_key(&id),
            swing_effects.contains_key(&id),
            hurts.contains_key(&id),
            hurt_dirs.contains_key(&id),
            mob_state.contains_key(&id),
            wavy_capes.contains_key(&id),
            minecarts.contains_key(&id),
            leash_data.contains_key(&id),
            projectile_power.contains_key(&id),
            displays.contains_key(&id),
            minecart_displays.contains_key(&id),
            primed.contains_key(&id),
            item_frames.contains_key(&id),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }

    fn spawned(t: &mut EntityTable, id: i32) {
        t.add(id, EntityState::new(id as u128, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
    }

    /// Give `id` a row in as many per-entity maps as the public setters and
    /// the table's own fields allow, **including the three `remove` used to
    /// forget**: armour, the bee's anger clock and the guardian's beam.
    fn dress(t: &mut EntityTable, id: i32) {
        t.set_frame_rotation(id, 3);
        t.set_armor(
            id,
            1,
            Some(WornPiece {
                item: 7,
                dye: Some(0x00ff_0000),
                trim: None,
                foil: true,
                wear: None,
            }),
        );
        t.set_head_profile(id, Some("e30=".into()));
        t.set_anger_end_time(id, 12_345);
        t.set_guardian_attack_target(id, 99);
        t.set_custom_name(id, Some("Jeb".into()));
        t.set_pose(id, 3);
        t.set_passengers(id, vec![id + 1000]);
        t.silent.insert(id);
        t.babies.insert(id);
        t.sizes.insert(id, 4);
        t.shared_flags.insert(id, 1);
        t.leash_data.insert(id, 0);
        t.projectile_power.insert(id, 1.5);
        t.displays.entry(id).or_default();
        t.minecart_displays.insert(id, MinecartDisplay::default());
        t.primed.insert(id, PrimedState::default());
        t.item_stacks.insert(id, (1, 1, false));
        t.stack_profiles.insert(id, "e30=".to_string());
        t.hand_profiles.insert(id, [Some("e30=".to_string()), None]);
        t.avatar_profiles.insert(id, AvatarProfile::default());
        t.body_armor.insert(
            id,
            WornPiece {
                item: 1,
                dye: None,
                trim: None,
                foil: false,
                wear: None,
            },
        );
        t.saddles.insert(
            id,
            WornPiece {
                item: 2,
                dye: None,
                trim: None,
                foil: false,
                wear: None,
            },
        );
        t.hurts.insert(id, HurtState::default());
        t.hurt_dirs.insert(id, 45.0);
        t.model_customisation.insert(id, 0x7f);
        t.gesture_states.insert(id, 2);
        t.custom_name_visible.insert(id);
        t.mob_state.entry(id).or_default();
        t.attributes.entry(id).or_default();
        t.uses.entry(id).or_default();
        t.main_arms.insert(id, HumanoidArm::Left);
        t.minecarts.entry(id).or_default();
    }

    /// M277 — `Creeper.tick`'s swell: it counts down from nothing (the
    /// default `swellDir` is -1), up by one a tick once the server sets the
    /// direction to 1, and stops at 30; `getSwelling` lerps and divides by 28.
    #[test]
    fn a_creepers_swell_counts_to_thirty_and_lerps() {
        let mut t = EntityTable::default();
        t.add(4, EntityState::new(1, 7, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.tick_creepers(7);
        assert_eq!(t.creeper_swelling(4, 1.0), 0.0);
        t.set_size(4, 1);
        for _ in 0..10 {
            t.tick_creepers(7);
        }
        assert!((t.creeper_swelling(4, 1.0) - 10.0 / 28.0).abs() < 1e-6);
        assert!((t.creeper_swelling(4, 0.5) - 9.5 / 28.0).abs() < 1e-6);
        for _ in 0..40 {
            t.tick_creepers(7);
        }
        assert!((t.creeper_swelling(4, 0.3) - 30.0 / 28.0).abs() < 1e-6);
        // Another type does not swell.
        t.add(5, EntityState::new(2, 8, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_size(5, 1);
        t.tick_creepers(7);
        assert_eq!(t.creeper_swelling(5, 1.0), 0.0);
    }

    /// M194 — a frame's facing falls back to the spawn packet's data until
    /// slot 8 arrives; rotation is kept `% 8`; an empty frame holds nothing.
    #[test]
    fn an_item_frame_reads_its_facing_from_spawn_data_then_metadata() {
        let mut t = EntityTable::default();
        let mut e = EntityState::new(1, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        e.spawn_data = 3; // south
        t.add(9, e);
        assert_eq!(t.item_frame(9), Some((3, 0, None)));
        t.set_frame_facing(9, 4);
        t.set_frame_rotation(9, 11);
        let map = FrameItem {
            item_id: 900,
            map_id: Some(7),
            foil: false,
        };
        t.set_frame_item(9, Some(map));
        assert_eq!(t.item_frame(9), Some((4, 3, Some(map))));
        t.set_frame_item(9, None);
        assert_eq!(t.item_frame(9).unwrap().2, None);
        // Out-of-range spawn data reads as down (0), not a panic.
        let mut bad = EntityState::new(2, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        bad.spawn_data = 99;
        t.add(10, bad);
        assert_eq!(t.item_frame(10).unwrap().0, 0);
        assert_eq!(t.item_frame(11), None, "no entity");
    }

    /// **`remove` leaves no row in any per-entity map.**
    ///
    /// MUTATION: dropping `armor.remove(&id)`, `anger_end_time.remove(&id)` or
    /// `guardian_attack.remove(&id)` from `clear_side_state` — the three the
    /// pre-fix `remove` never cleared — leaves a row behind, and `side_rows`
    /// reports it.
    #[test]
    fn a_removed_entity_leaves_no_row_in_any_side_map() {
        let mut t = EntityTable::default();
        spawned(&mut t, 5);
        dress(&mut t, 5);
        assert!(
            side_rows(&t, 5) >= 18,
            "the fixture must populate the maps, or this test proves nothing: {}",
            side_rows(&t, 5)
        );
        assert!(t.armor(5)[1].is_some());
        assert_eq!(t.anger_end_time(5), Some(12_345));
        assert!(t.guardian_has_attack_target(5));

        t.remove(5);
        assert_eq!(side_rows(&t, 5), 0);
        assert_eq!(t.armor(5), [None; 4]);
        assert_eq!(t.anger_end_time(5), None);
        assert!(!t.guardian_has_attack_target(5));
    }

    /// **A reused id inherits nothing**, with no `remove` in between — the
    /// dropped-removal case `add` exists for. The old occupant's chestplate is
    /// the visible symptom; the anger clock and the guardian's beam would have
    /// followed it.
    ///
    /// MUTATION: `add` back to clearing only events, dances, capes, carts and
    /// swings.
    #[test]
    fn a_reused_id_does_not_inherit_the_previous_occupants_state() {
        let mut t = EntityTable::default();
        spawned(&mut t, 8);
        dress(&mut t, 8);
        // The server reuses id 8 for a new entity with no removal in between.
        spawned(&mut t, 8);
        assert_eq!(side_rows(&t, 8), 0, "every per-entity row is gone");
        assert_eq!(t.armor(8), [None; 4], "no inherited armour");
        assert_eq!(t.anger_end_time(8), None, "no inherited anger");
        assert!(!t.guardian_has_attack_target(8), "no inherited beam");
        assert_eq!(t.custom_name(8), None);
        assert!(t.get(8).is_some(), "and the entity itself is there");
    }

    /// A villager's profession is the entity's alone: stored per id, and a
    /// reused id starts with none (its outfit would otherwise follow the id).
    #[test]
    fn a_villagers_data_is_per_entity_and_does_not_survive_the_id() {
        let mut t = EntityTable::default();
        spawned(&mut t, 4);
        spawned(&mut t, 6);
        assert_eq!(t.villager_data(4), None, "nothing sent yet");
        t.set_villager_data(4, (2, 5, 3));
        assert_eq!(t.villager_data(4), Some((2, 5, 3)));
        assert_eq!(t.villager_data(6), None, "a neighbour is unaffected");
        t.set_villager_data(4, (0, 9, 1));
        assert_eq!(t.villager_data(4), Some((0, 9, 1)), "the latest wins");
        spawned(&mut t, 4);
        assert_eq!(t.villager_data(4), None, "a re-add inherits nothing");
        t.set_villager_data(4, (1, 1, 1));
        t.remove(4);
        assert_eq!(t.villager_data(4), None);
        assert_eq!(side_rows(&t, 4), 0);
    }

    /// Churn does not grow the side maps: a thousand ids spawned, dressed and
    /// removed leave every map empty (the unbounded growth the audit named).
    #[test]
    fn churning_a_thousand_entities_leaves_every_side_map_empty() {
        let mut t = EntityTable::default();
        for id in 0..1000 {
            spawned(&mut t, id);
            dress(&mut t, id);
            t.remove(id);
        }
        let leaked: usize = (0..1000).map(|id| side_rows(&t, id)).sum();
        assert_eq!(leaked, 0);
        assert!(t.armor.is_empty() && t.anger_end_time.is_empty() && t.guardian_attack.is_empty());
    }

    /// The re-add of one entity does not touch another's rows.
    #[test]
    fn clearing_one_id_leaves_its_neighbours_alone() {
        let mut t = EntityTable::default();
        for id in [1, 2, 3] {
            spawned(&mut t, id);
            dress(&mut t, id);
        }
        let before = side_rows(&t, 3);
        t.remove(2);
        spawned(&mut t, 1);
        assert_eq!(side_rows(&t, 3), before);
        assert!(t.armor(3)[1].is_some());
    }
}

#[cfg(test)]
mod snow_golem_tests {
    use super::*;

    /// M353 — a snow golem wears its pumpkin by default; the 0x10 bit of
    /// `DATA_PUMPKIN_ID` takes it off and puts it back.
    #[test]
    fn a_snow_golem_wears_its_pumpkin_until_sheared() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 7, 0.0, 0.0, 0.0, 0.0, 0.0));
        assert!(t.has_pumpkin(1));
        t.set_snow_golem_flags(1, 0x00);
        assert!(!t.has_pumpkin(1));
        t.set_snow_golem_flags(1, 0x10);
        assert!(t.has_pumpkin(1));
    }
}

#[cfg(test)]
mod render_rotation_tests {
    use super::*;

    /// M361 — an entity's drawn rotation is last tick's blended into this
    /// tick's: the yaw the short way round across ±180°, the pitch straight,
    /// the head yaw like the body's; a tick rolls this tick's into last.
    #[test]
    fn an_entity_turns_every_frame_between_ticks() {
        assert_eq!(rot_lerp(0.5, 170.0, -170.0), 180.0);
        assert_eq!(rot_lerp(0.25, -170.0, 170.0), -175.0);
        assert_eq!(rot_lerp(2.0, 0.0, 90.0), 90.0);
        let mut e = EntityState::new(0, 1, 0.0, 0.0, 0.0, 10.0, 0.0);
        e.set_rot(50.0, 20.0);
        e.set_head_yaw(30.0);
        assert_eq!(e.render_yaw(0.5), 30.0);
        assert_eq!(e.render_pitch(0.5), 10.0);
        assert_eq!(e.render_head_yaw(0.5), 20.0);
        e.tick(false, false);
        assert_eq!(e.render_yaw(0.0), 50.0);
        assert_eq!(e.render_head_yaw(0.3), 30.0);
    }
}

#[cfg(test)]
mod flight_tests {
    use super::*;

    /// M362 — an arrow flies on the client: it moves by its velocity, turns
    /// toward it, loses 1% to drag and 0.05 to gravity each tick; a block in
    /// its path stops it just short; the server's `IN_GROUND` ends a flight;
    /// a non-projectile does not fly.
    #[test]
    fn an_arrow_flies_between_server_updates() {
        let none = |_: [f64; 3]| false;
        let mut a = EntityState::new(0, 1, 0.0, 70.0, 0.0, 0.0, 0.0);
        a.set_flight(ARROW_FLIGHT);
        a.set_spawn_motion([0.0, 0.0, 3.0]);
        a.step_flight(&none);
        assert_eq!(a.render_pos(1.0), [0.0, 70.0, 3.0]);
        let v = a.delta_movement;
        assert!((v[2] - 2.97).abs() < 1e-9 && (v[1] + 0.05).abs() < 1e-9);
        // The yaw toward +z is 0; the pitch turns down a fifth of the way.
        a.step_flight(&none);
        assert!(a.cur[2] > 5.9 && a.cur[1] < 70.0);
        assert!(a.pitch < 0.0);
        // A wall at z >= 8: the next step would cross it, so it stops short.
        let wall = |p: [f64; 3]| p[2] >= 8.0;
        a.step_flight(&wall);
        assert!(a.cur[2] < 8.0 && a.in_ground && a.delta_movement == [0.0; 3]);
        let still = a.cur;
        a.step_flight(&none);
        assert_eq!(a.cur, still);
        // A plain entity never flies; a server position snaps a projectile.
        let mut z = EntityState::new(0, 2, 0.0, 70.0, 0.0, 0.0, 0.0);
        z.set_spawn_motion([1.0, 0.0, 0.0]);
        z.step_flight(&none);
        assert_eq!(z.render_pos(1.0), [0.0, 70.0, 0.0]);
        let mut b = EntityState::new(0, 1, 0.0, 70.0, 0.0, 0.0, 0.0);
        b.set_flight(THROWN_FLIGHT);
        b.set_target(5.0, 71.0, 5.0);
        assert_eq!(b.render_pos(0.0), [5.0, 71.0, 5.0]);
        assert_eq!(flight_of("minecraft:trident"), Some(ARROW_FLIGHT));
        assert_eq!(flight_of("minecraft:zombie"), None);
        // Unwound to within 180 degrees first, as vanilla's: -190 toward -170.
        assert!((lerp_rotation(170.0, -170.0) + 186.0).abs() < 1e-4);
    }

    /// A critical arrow in flight reports `AbstractArrow.tick`'s four crit
    /// particles from where it was, along the movement it is about to make
    /// (the decompile's `origin + movement * i / 4`, velocity `-movement +
    /// (0, 0.2, 0)`); a plain arrow, an arrow in the ground and a thrown
    /// item report none.
    #[test]
    fn a_critical_arrow_trails_crit_particles() {
        let none = |_: [f64; 3]| false;
        let mut t = EntityTable::default();
        for id in 1..=3 {
            t.add(id, EntityState::new(0, id, 0.0, 70.0, 0.0, 0.0, 0.0));
            t.get_mut(id).unwrap().set_spawn_motion([0.0, 1.0, 3.0]);
        }
        t.set_flight(1, ARROW_FLIGHT);
        t.set_flight(2, ARROW_FLIGHT);
        t.set_flight(3, THROWN_FLIGHT);
        t.set_arrow_flags(1, 1);
        t.set_arrow_flags(3, 1);
        let trails = t.step_flights(&none);
        assert_eq!(trails.len(), 1);
        let p = trails[0].particles();
        assert_eq!(p[0].0, [0.0, 70.0, 0.0]);
        assert_eq!(p[2].0, [0.0, 70.5, 1.5]);
        assert_eq!(p[3].0, [0.0, 70.75, 2.25]);
        assert_eq!(p[1].1, [-0.0, -0.8, -3.0]);
        // In the ground: none.
        t.get_mut(1).unwrap().in_ground = true;
        assert!(t.step_flights(&none).is_empty());
    }

    /// M362 — the server's `IN_GROUND` stops an arrow's flight, and only an
    /// arrow's.
    #[test]
    fn in_ground_reaches_only_an_arrow() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 1, 0.0, 70.0, 0.0, 0.0, 0.0));
        t.add(2, EntityState::new(0, 2, 0.0, 70.0, 0.0, 0.0, 0.0));
        t.set_flight(1, ARROW_FLIGHT);
        t.set_flight(2, THROWN_FLIGHT);
        t.set_in_ground(1, true);
        t.set_in_ground(2, true);
        assert!(t.get(1).unwrap().in_ground);
        assert!(!t.get(2).unwrap().in_ground);
    }
}

#[cfg(test)]
mod turtle_flag_tests {
    use super::*;

    /// M365 — the turtle's two egg flags are kept apart and default off; a
    /// re-add forgets them.
    #[test]
    fn a_turtles_egg_flags_are_kept() {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 1, 0.0, 64.0, 0.0, 0.0, 0.0));
        assert_eq!(t.turtle_flags(1), (false, false));
        t.set_turtle_egg(1, true);
        assert_eq!(t.turtle_flags(1), (true, false));
        t.set_turtle_laying(1, true);
        t.set_turtle_egg(1, false);
        assert_eq!(t.turtle_flags(1), (false, true));
        t.add(1, EntityState::new(0, 1, 0.0, 64.0, 0.0, 0.0, 0.0));
        assert_eq!(t.turtle_flags(1), (false, false));
    }
}
