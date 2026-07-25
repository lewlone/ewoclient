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

/// Vanilla's interpolation step count for tracked entities.
const LERP_STEPS: u32 = 3;

/// `net.minecraft.world.InteractionHand`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InteractionHand {
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
}

impl EntityEvent {
    /// Dense count for the fixed-size per-entity store.
    pub const COUNT: usize = 3;

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

    fn tick(&mut self) {
        let before = self.cur;
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

    /// Walk-cycle phase + amplitude for the model's limb swing.
    pub fn limb(&self) -> (f32, f32) {
        (self.limb_swing, self.limb_amount)
    }
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
        self.clear_swing(id);
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

    // -- combat swings (M19) ------------------------------------------------

    /// Set one hand's item — `ClientboundSetEquipmentPacket`'s MAINHAND /
    /// OFFHAND slots. Armour slots never reach here: they change no swing
    /// input. [`HandItem::Unknown`] is a first-class value, not an absence.
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
    pub fn main_arm(&self, id: i32) -> HumanoidArm {
        self.main_arms.get(&id).copied().unwrap_or_default()
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
        for e in self.map.values_mut() {
            e.tick();
        }
        for d in self.dances.values_mut() {
            d.tick();
        }
        self.tick_swings();
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
        e.tick(); // (3-0)/3 → 1
        assert_eq!(e.render_pos(1.0)[0], 1.0);
        assert_eq!(e.render_pos(0.5)[0], 0.5, "partial tick blends prev→cur");
        e.tick(); // (3-1)/2 → 2
        e.tick(); // (3-2)/1 → 3 exact
        assert_eq!(e.render_pos(1.0)[0], 3.0);
        e.tick(); // no steps left — stays put
        assert_eq!(e.render_pos(0.0)[0], 3.0);
    }

    #[test]
    fn deltas_accumulate_on_the_target_not_the_render_pos() {
        let mut e = EntityState::new(0, 0, 10.0, 0.0, 0.0, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0);
        e.nudge(0.5, 0.0, 0.0); // mid-lerp — target must not lose the first
        assert_eq!(e.x, 11.0);
        for _ in 0..3 {
            e.tick();
        }
        assert_eq!(e.render_pos(1.0)[0], 11.0);
    }

    #[test]
    fn still_entity_has_no_limb_swing_but_walking_builds_it_up() {
        let mut e = EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..10 {
            e.tick(); // no target set → cur never moves
        }
        let (_, amt_still) = e.limb();
        assert!(amt_still < 1e-6, "still entity: amount {amt_still}");
        // Now walk ~0.2 blk/tick and let the smoother ramp.
        for _ in 0..20 {
            e.nudge(0.2, 0.0, 0.0);
            e.tick();
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
